import type { AgentTools, FunctionCall, ModelEndpoint } from './agent'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { AgentRunError, complete, modelApiFrom, requestFor, runAgent } from './agent'

const endpoint = (api: ModelEndpoint['api']): ModelEndpoint => ({ api, apiKey: 'k', baseURL: 'http://proxy.test/v1/', model: 'm' })

const invokeTool = { description: 'Invoke one AUV command.', name: 'invoke', parameters: { properties: { command_id: { type: 'string' } }, type: 'object' } }

/** Captures every model request and answers from `replies` in order. */
function stubModel(replies: unknown[]): Array<{ body: Record<string, any>, url: string }> {
  const requests: Array<{ body: Record<string, any>, url: string }> = []
  vi.stubGlobal('fetch', vi.fn(async (url: string, init: RequestInit) => {
    requests.push({ body: JSON.parse(String(init.body)), url })
    return Response.json(replies[requests.length - 1])
  }))
  return requests
}

/** Tool stub: every call returns text plus one image, and is recorded. */
function stubTools(): { calls: FunctionCall[], tools: AgentTools } {
  const calls: FunctionCall[] = []
  return {
    calls,
    tools: {
      call: async (call) => {
        calls.push(call)
        return { images: [`data:image/webp;base64,shot-${calls.length}`], runId: `run-${calls.length}`, text: `{"status":"completed","n":${calls.length}}` }
      },
      definitions: [invokeTool],
    },
  }
}

const call = (callId: string, name: string, args: unknown = {}) => ({ arguments: JSON.stringify(args), call_id: callId, name, type: 'function_call' })

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('runAgent with Responses tools', () => {
  it('executes the chosen AUV call and returns its output with the image', async () => {
    const requests = stubModel([
      // Reasoning items precede the call; only function_call items are actions.
      { output: [{ type: 'reasoning' }, call('c1', 'invoke', { command_id: 'display.capture' })] },
      { output: [call('c2', 'done')] },
    ])
    const { calls, tools } = stubTools()

    const result = await runAgent('do it', endpoint('responses'), tools, { maxSteps: 3, signal: AbortSignal.timeout(10_000) })

    expect(result.lastAction).toBe('DONE')
    expect(result.runIds).toEqual(['run-1'])
    expect(calls.map(item => item.arguments)).toEqual(['{"command_id":"display.capture"}'])
    const first = requests[0].body
    expect(first).toMatchObject({ model: 'm', parallel_tool_calls: false, store: false, tool_choice: 'auto' })
    expect(first.tools.map((tool: { name: string }) => tool.name)).toEqual(['invoke', 'done', 'fail'])
    expect(requests[1].body.input.slice(1)).toEqual([
      { arguments: '{"command_id":"display.capture"}', call_id: 'c1', name: 'invoke', type: 'function_call' },
      {
        call_id: 'c1',
        output: [
          { text: '{"status":"completed","n":1}', type: 'input_text' },
          { image_url: 'data:image/webp;base64,shot-1', type: 'input_image' },
        ],
        type: 'function_call_output',
      },
    ])
  })

  it('ends with FAIL when the model calls fail', async () => {
    stubModel([{ output: [call('c1', 'fail')] }])
    const { calls, tools } = stubTools()

    const result = await runAgent('impossible', endpoint('responses'), tools, { maxSteps: 3, signal: AbortSignal.timeout(10_000) })

    expect(result.lastAction).toBe('FAIL')
    expect(calls).toEqual([])
  })

  it('keeps only the latest screenshot in history', async () => {
    const requests = stubModel([
      { output: [call('c1', 'invoke')] },
      { output: [call('c2', 'invoke')] },
      { output: [call('c3', 'done')] },
    ])
    const { tools } = stubTools()

    await runAgent('do it', endpoint('responses'), tools, { maxSteps: 5, signal: AbortSignal.timeout(10_000) })

    const outputs = requests[2].body.input.filter((item: { type?: string }) => item.type === 'function_call_output')
    expect(outputs[0].output).toBe('{"status":"completed","n":1}\n[Earlier screenshot omitted.]')
    expect(outputs[1].output[1]).toEqual({ image_url: 'data:image/webp;base64,shot-2', type: 'input_image' })
  })

  it('asks for a tool call when the model only answers in text', async () => {
    const requests = stubModel([
      { output: [{ content: [{ text: 'I will look first.', type: 'output_text' }], type: 'message' }] },
      { output: [call('c1', 'done')] },
    ])
    const { tools } = stubTools()

    const result = await runAgent('do it', endpoint('responses'), tools, { maxSteps: 3, signal: AbortSignal.timeout(10_000) })

    expect(result.steps[0]).toEqual({ message: 'I will look first.' })
    expect(requests[1].body.input.at(-1)).toEqual({ content: [{ text: 'Call a tool to act on the computer, or call done or fail.', type: 'input_text' }], role: 'user' })
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
        ? Response.json({ output: [call('c1', 'invoke')] })
        : Response.json({ error: { code: 'invalid_prompt' } }, { status: 400 })
    }))
    const { tools } = stubTools()

    const failure = await runAgent('do it', endpoint('responses'), tools, { maxSteps: 3, signal: AbortSignal.timeout(10_000) }).catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(AgentRunError)
    expect((failure as AgentRunError).message).toBe('Agent stopped after 1 steps')
    expect((failure as AgentRunError).partial.steps.map(step => step.tool)).toEqual(['invoke'])
    expect(String((failure as AgentRunError).cause)).toContain('HTTP 400')
  })
})

describe('requestFor', () => {
  // ROOT CAUSE:
  //
  // If a request declared no tools and left tool choice open, the Codex
  // backend ran its injected image_generation tool between answers, several
  // rounds per request (44ee5668 step 1 replayed: 29-173 s; with tool_choice
  // none: 4 s, one message).
  it('disables tools for a text-only Responses request', () => {
    const request = requestFor(endpoint('responses'), { items: [{ role: 'user', text: 'Reply with OK.', type: 'message' }] })
    expect(JSON.parse(request.body)).toMatchObject({ store: false, tool_choice: 'none' })
  })

  it('maps tool calls onto Chat Completions, with images in a following user turn', () => {
    const request = requestFor(endpoint('chat'), {
      instructions: 'system',
      items: [
        { role: 'user', text: 'Task: do it', type: 'message' },
        { arguments: '{}', callId: 'c1', name: 'invoke', type: 'function_call' },
        { callId: 'c1', images: ['data:image/webp;base64,x'], text: 'ok', type: 'function_call_output' },
      ],
      tools: [invokeTool],
    })

    expect(request.url).toBe('http://proxy.test/v1/chat/completions')
    const body = JSON.parse(request.body)
    expect(body.tools).toEqual([{ function: invokeTool, type: 'function' }])
    expect(body.messages).toEqual([
      { content: 'system', role: 'system' },
      { content: 'Task: do it', role: 'user' },
      { content: null, role: 'assistant', tool_calls: [{ function: { arguments: '{}', name: 'invoke' }, id: 'c1', type: 'function' }] },
      { content: 'ok', role: 'tool', tool_call_id: 'c1' },
      { content: [{ text: 'Images from c1:', type: 'text' }, { image_url: { url: 'data:image/webp;base64,x' }, type: 'image_url' }], role: 'user' },
    ])
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
    const reply = complete(endpoint('responses'), { items: [{ role: 'user', text: 'hi', type: 'message' }] }, new AbortController().signal)
    await vi.runAllTimersAsync()
    await expect(reply).resolves.toEqual({ calls: [], text: 'ok' })
    expect(calls).toBe(2)
  })

  it('reads Chat Completions tool calls', async () => {
    stubModel([{ choices: [{ message: { content: null, tool_calls: [{ function: { arguments: '{"a":1}', name: 'invoke' }, id: 'c9' }] } }] }])
    const reply = await complete(endpoint('chat'), { items: [], tools: [invokeTool] }, AbortSignal.timeout(10_000))
    expect(reply).toEqual({ calls: [{ arguments: '{"a":1}', callId: 'c9', name: 'invoke' }], text: '' })
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
