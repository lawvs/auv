import type { Invoke, ModelEndpoint } from './agent'

import { Buffer } from 'node:buffer'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { AgentRunError, complete, modelApiFrom, parseDecision, runAgent, toInvocation } from './agent'

/** AUV stub: `display.capture` returns a real file, input commands succeed. */
async function stubInvoke(): Promise<{ commands: string[], invoke: Invoke }> {
  const image = join(await mkdtemp(join(tmpdir(), 'osworld-agent-')), 'capture.webp')
  await writeFile(image, 'webp-bytes')
  const commands: string[] = []
  return {
    commands,
    invoke: async (command) => {
      commands.push(command)
      return { artifacts: command === 'display.capture' ? [{ file_path: image }] : [], run_id: `run-${commands.length}` }
    },
  }
}

/** Captures every model request and answers from `replies` in order. */
function stubModel(replies: unknown[]): Array<{ body: Record<string, any>, url: string }> {
  const requests: Array<{ body: Record<string, any>, url: string }> = []
  vi.stubGlobal('fetch', vi.fn(async (url: string, init: RequestInit) => {
    requests.push({ body: JSON.parse(String(init.body)), url })
    return Response.json(replies[requests.length - 1])
  }))
  return requests
}

const endpoint = (api: ModelEndpoint['api']): ModelEndpoint => ({ api, apiKey: 'k', baseURL: 'http://proxy.test/v1/', model: 'm' })

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('runAgent model API', () => {
  it('sends Responses requests and carries earlier replies as assistant output_text', async () => {
    const typeReply = '{"action":{"type":"type","text":"hi"}}'
    const requests = stubModel([
      // Reasoning items precede the message; only output_text is the reply.
      { output: [{ type: 'reasoning' }, { content: [{ text: typeReply, type: 'output_text' }], type: 'message' }] },
      { output: [{ content: [{ text: '{"action":{"type":"done"}}', type: 'output_text' }], type: 'message' }] },
    ])
    const { commands, invoke } = await stubInvoke()

    const result = await runAgent('do it', endpoint('responses'), invoke, { maxSteps: 3, signal: AbortSignal.timeout(10_000) })

    expect(result.lastAction).toBe('DONE')
    expect(commands).toEqual(['display.capture', 'input.typeText', 'display.capture'])
    expect(requests.map(request => request.url)).toEqual(['http://proxy.test/v1/responses', 'http://proxy.test/v1/responses'])
    const second = requests[1].body
    // ROOT CAUSE:
    //
    // If tool choice was left open, the Codex backend ran image_generation
    // between answers, several rounds per request (44ee5668 step 1 replayed:
    // 29-173 s; with tool_choice none: 4 s, one message).
    expect(second).toMatchObject({ model: 'm', store: false, tool_choice: 'none' })
    expect(second.instructions).toContain('Ubuntu GNOME desktop')
    expect(second.input).toEqual([
      { content: [{ text: 'Task: do it', type: 'input_text' }], role: 'user' },
      { content: [{ text: typeReply, type: 'output_text' }], role: 'assistant' },
      {
        content: [
          { text: 'Step 2 of 3. Current screenshot:', type: 'input_text' },
          { image_url: `data:image/webp;base64,${Buffer.from('webp-bytes').toString('base64')}`, type: 'input_image' },
        ],
        role: 'user',
      },
    ])
  })

  // ROOT CAUSE:
  //
  // If the model concatenated several actions, only the first ran, but the
  // whole reply went into history. The model then planned from a state that
  // never existed and ran out of steps.
  //
  // The fix keeps only the executed object and says the rest were ignored.
  it('records only the executed action when a reply holds several', async () => {
    const first = '{"thought":"open","action":{"type":"keys","keys":["ctrl","l"]}}'
    const requests = stubModel([
      { output: [{ content: [{ text: `${first}{"action":{"type":"type","text":"x"}}`, type: 'output_text' }], type: 'message' }] },
      { output: [{ content: [{ text: '{"action":{"type":"done"}}', type: 'output_text' }], type: 'message' }] },
    ])
    const { commands, invoke } = await stubInvoke()

    await runAgent('do it', endpoint('responses'), invoke, { maxSteps: 3, signal: AbortSignal.timeout(10_000) })

    expect(commands).toEqual(['display.capture', 'input.keys', 'display.capture'])
    const input = requests[1].body.input
    expect(input[1]).toEqual({ content: [{ text: first, type: 'output_text' }], role: 'assistant' })
    expect(input[2].content[0].text).toBe('Only the first action of your previous reply was executed; the others were ignored. Reply with exactly one action. Step 2 of 3. Current screenshot:')
  })

  // ROOT CAUSE:
  //
  // If a model request failed mid-episode (chrome/47543840: HTTP 400
  // `invalid_prompt` from moderation), runAgent threw and its earlier steps
  // were lost, so result.json could not show what the model had seen.
  //
  // The fix throws AgentRunError carrying the partial result.
  it('keeps earlier steps when a later model request fails', async () => {
    let calls = 0
    vi.stubGlobal('fetch', vi.fn(async () => {
      calls += 1
      return calls === 1
        ? Response.json({ output: [{ content: [{ text: '{"action":{"type":"type","text":"hi"}}', type: 'output_text' }], type: 'message' }] })
        : Response.json({ error: { code: 'invalid_prompt' } }, { status: 400 })
    }))
    const { invoke } = await stubInvoke()

    const failure = await runAgent('do it', endpoint('responses'), invoke, { maxSteps: 3, signal: AbortSignal.timeout(10_000) }).catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(AgentRunError)
    expect((failure as AgentRunError).message).toBe('Agent stopped after 1 steps')
    expect((failure as AgentRunError).partial.steps.map(step => step.action.type)).toEqual(['type'])
    expect(String((failure as AgentRunError).cause)).toContain('HTTP 400')
  })

  it('keeps the Chat Completions shape when OSWORLD_OPENAI_API=chat', async () => {
    const requests = stubModel([{ choices: [{ message: { content: '{"action":{"type":"fail"}}' } }] }])
    const { invoke } = await stubInvoke()

    const result = await runAgent('do it', endpoint('chat'), invoke, { maxSteps: 1, signal: AbortSignal.timeout(10_000) })

    expect(result.lastAction).toBe('FAIL')
    expect(requests[0].url).toBe('http://proxy.test/v1/chat/completions')
    const messages = requests[0].body.messages
    expect(messages[0].role).toBe('system')
    expect(messages.at(-1).content[1]).toEqual({ image_url: { url: `data:image/webp;base64,${Buffer.from('webp-bytes').toString('base64')}` }, type: 'image_url' })
  })
})

describe('complete', () => {
  // ROOT CAUSE:
  //
  // If cliproxyapi loses its upstream stream, it answers HTTP 408
  // ("stream closed before response.completed"). Only 429 and 5xx were
  // retried, so one dropped stream failed the whole OSWorld episode
  // (chrome/3720f614 in the 2026-10-11 test_nogdrive run).
  //
  // The fix retries 408 like the other transient proxy failures.
  it('retries a 408 request timeout from the proxy', async () => {
    vi.useFakeTimers()
    let calls = 0
    vi.stubGlobal('fetch', vi.fn(async () => {
      calls += 1
      return calls === 1
        ? Response.json({ error: { message: 'stream closed before response.completed' } }, { status: 408 })
        : Response.json({ output: [{ content: [{ text: 'ok', type: 'output_text' }], type: 'message' }] })
    }))
    const reply = complete(endpoint('responses'), { turns: [{ role: 'user', text: 'hi' }] }, new AbortController().signal)
    await vi.runAllTimersAsync()
    await expect(reply).resolves.toBe('ok')
    expect(calls).toBe(2)
    vi.useRealTimers()
  })
})

describe('modelApiFrom', () => {
  it('defaults to the Responses API', () => {
    expect(modelApiFrom(undefined)).toBe('responses')
    expect(modelApiFrom('')).toBe('responses')
    expect(modelApiFrom('chat')).toBe('chat')
  })

  it('rejects an unknown API instead of silently falling back', () => {
    expect(() => modelApiFrom('completions')).toThrow(/responses" or "chat/)
  })
})

describe('parseDecision', () => {
  // ROOT CAUSE:
  //
  // If the model appended planned turns as concatenated JSON objects, the
  // reply failed to parse and the step became `wait`.
  //
  // Before the fix, the parser sliced from the first `{` to the last `}`.
  // The fix keeps only the first balanced object as the current decision.
  it('uses the first object when the model concatenates several turns', () => {
    const reply = '{"thought":"a {brace}","action":{"type":"keys","keys":["F2"]}}{"action":{"type":"wait"}}'
    expect(parseDecision(reply)).toEqual({
      action: { keys: ['F2'], type: 'keys' },
      text: '{"thought":"a {brace}","action":{"type":"keys","keys":["F2"]}}',
      truncated: true,
    })
  })

  it('treats a reply without an action as wait', () => {
    expect(parseDecision('I cannot see the screen.')).toEqual({ action: { type: 'wait' }, text: 'I cannot see the screen.', truncated: false })
  })
})

describe('toInvocation', () => {
  it('places positionals after -- so typed text cannot become a flag', () => {
    expect(toInvocation({ text: '-rf', type: 'type' })).toEqual(['input.typeText', ['--', '-rf']])
  })
})
