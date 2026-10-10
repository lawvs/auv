import type { Invoke, ModelEndpoint } from './agent'

import { Buffer } from 'node:buffer'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { complete, modelApiFrom, parseAction, runAgent, toInvocation } from './agent'

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
    expect(second).toMatchObject({ model: 'm', store: false })
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

describe('parseAction', () => {
  // ROOT CAUSE:
  //
  // If the model appended planned turns as concatenated JSON objects, the
  // reply failed to parse and the step became `wait`.
  //
  // Before the fix, the parser sliced from the first `{` to the last `}`.
  // The fix keeps only the first balanced object as the current decision.
  it('uses the first object when the model concatenates several turns', () => {
    const reply = '{"thought":"a {brace}","action":{"type":"keys","keys":["F2"]}}{"action":{"type":"wait"}}'
    expect(parseAction(reply)).toEqual({ keys: ['F2'], type: 'keys' })
  })

  it('treats a reply without an action as wait', () => {
    expect(parseAction('I cannot see the screen.')).toEqual({ type: 'wait' })
  })
})

describe('toInvocation', () => {
  it('places positionals after -- so typed text cannot become a flag', () => {
    expect(toInvocation({ text: '-rf', type: 'type' })).toEqual(['input.typeText', ['--', '-rf']])
  })
})
