import { readFile } from 'node:fs/promises'

/** One model decision; coordinates use a 0..1000 grid over the screenshot. */
export type AgentAction
  = | { button?: 'left' | 'middle' | 'right', count?: number, type: 'click', x: number, y: number }
    | { dy: number, type: 'scroll', x: number, y: number }
    | { keys: string[], type: 'keys' }
    | { text: string, type: 'type' }
    | { type: 'done' }
    | { type: 'drag', x1: number, x2: number, y1: number, y2: number }
    | { type: 'fail' }
    | { type: 'wait' }

export interface AgentResult {
  /** `DONE` or `FAIL` when the model ended the episode, as OSWorld expects. */
  lastAction?: 'DONE' | 'FAIL'
  runIds: string[]
  /** Each model decision with its raw reply, kept as debugging evidence. */
  steps: Array<{ action: AgentAction, reply: string, runId?: string }>
}

/** The executed part of one model reply. */
export interface Decision {
  action: AgentAction
  /** The first JSON object of the reply, or the whole reply when none parsed. */
  text: string
  /** The reply held more actions after the first; they were not executed. */
  truncated: boolean
}

/** Executes one AUV invoke command and returns its parsed JSON result. */
export type Invoke = (command: string, args: string[]) => Promise<{ artifacts: Array<{ file_path: string }>, run_id: string }>

/**
 * OpenAI-compatible wire protocol: `responses` (`POST /responses`, default) or
 * `chat` (`POST /chat/completions`) for endpoints without the Responses API.
 */
export type ModelApi = 'chat' | 'responses'

/** OpenAI-compatible model endpoint used to choose the next AUV action. */
export interface ModelEndpoint {
  api: ModelApi
  apiKey: string
  baseURL: string
  model: string
}

/**
 * One model request independent of the wire protocol. The last user turn may
 * carry one screenshot as a data URL.
 */
export interface Prompt {
  instructions?: string
  turns: Array<{ image?: string, role: 'assistant' | 'user', text: string }>
}

/**
 * A failure inside {@link runAgent} that keeps the steps taken before it, so
 * an episode error (for example a model request rejected by moderation) can
 * still be reviewed.
 */
export class AgentRunError extends Error {
  constructor(readonly partial: AgentResult, cause: unknown) {
    super(`Agent stopped after ${String(partial.steps.length)} steps`, { cause })
    this.name = 'AgentRunError'
  }
}

// NOTICE: One model attempt may not take longer than this. Without it, a
// stalled proxy stream held a case until Vieval's case timeout (p50 6 s,
// p99 314 s, max 832 s in the 2026-10-11 test_nogdrive run). A timed-out
// attempt is retried like a network error.
const modelAttemptTimeoutMs = 180_000

const systemPrompt = `You operate an Ubuntu GNOME desktop to complete the user's task.
Each turn you receive the current screenshot. Reply with exactly one JSON object and nothing else:
{"thought": "<one short sentence>", "action": <action>}
Actions (x and y are integers on a 0..1000 grid over the screenshot; 0,0 is top-left):
{"type":"click","x":500,"y":500,"button":"left","count":1}
{"type":"drag","x1":100,"y1":100,"x2":300,"y2":100}
{"type":"type","text":"hello"}            types into the focused control
{"type":"keys","keys":["ctrl","s"]}         one key combination, modifiers first
{"type":"scroll","x":500,"y":500,"dy":300}  positive dy scrolls down, in pixels
{"type":"wait"}
{"type":"done"}  the task is complete
{"type":"fail"}  the task is impossible
The user password is "password".`

/**
 * Requests one model reply, retrying transient proxy failures.
 *
 * Network errors, attempts over three minutes, 408, 429, and 5xx are retried
 * at most three times with a short linear backoff; other HTTP errors fail the case immediately. cliproxyapi
 * answers 408 when its upstream stream closes before `response.completed`.
 *
 * Use when:
 * - the agent loop needs the next decision
 * - `doctor` probes that the configured endpoint answers the same request shape
 *
 * Returns:
 * - the reply text, or an empty string when the model returned no text
 */
export async function complete(endpoint: ModelEndpoint, prompt: Prompt, signal: AbortSignal): Promise<string> {
  const request = requestFor(endpoint, prompt)
  for (let attempt = 1; ; attempt += 1) {
    const outcome = await fetch(request.url, {
      body: request.body,
      headers: { 'authorization': `Bearer ${endpoint.apiKey}`, 'content-type': 'application/json' },
      method: 'POST',
      signal: AbortSignal.any([signal, AbortSignal.timeout(modelAttemptTimeoutMs)]),
    }).then(async response => response.ok
      ? { reply: replyText(endpoint.api, await response.json()) }
      : { response }).catch((error: unknown) => {
      if (attempt >= 3 || signal.aborted) {
        throw error
      }
      return undefined
    })
    if (outcome?.reply !== undefined) {
      return outcome.reply
    }
    const response = outcome?.response
    if (response != null && (!isTransientStatus(response.status) || attempt >= 3)) {
      throw new Error(`Model request to ${request.url} failed with HTTP ${response.status}: ${await response.text()}`)
    }
    await new Promise(resolve => setTimeout(resolve, attempt * 5_000))
  }
}

/** Reads `OSWORLD_OPENAI_API`; unset or empty selects the Responses API. */
export function modelApiFrom(value: string | undefined): ModelApi {
  if (value == null || value === '' || value === 'responses') {
    return 'responses'
  }
  if (value === 'chat') {
    return 'chat'
  }
  throw new Error(`OSWORLD_OPENAI_API must be "responses" or "chat", got ${JSON.stringify(value)}.`)
}

/**
 * Extracts the first complete JSON object from a reply; anything else becomes
 * `wait`. Models sometimes append several planned turns as concatenated
 * objects, so only the first balanced object is the current decision.
 */
export function parseDecision(reply: string): Decision {
  const fallback: Decision = { action: { type: 'wait' }, text: reply, truncated: false }
  let depth = 0
  let inString = false
  const begin = reply.indexOf('{')
  if (begin < 0) {
    return fallback
  }
  for (let index = begin; index < reply.length; index += 1) {
    const char = reply[index]
    if (inString) {
      if (char === '\\') {
        index += 1
      }
      else if (char === '"') {
        inString = false
      }
      continue
    }
    if (char === '"') {
      inString = true
    }
    else if (char === '{') {
      depth += 1
    }
    else if (char === '}' && --depth === 0) {
      const text = reply.slice(begin, index + 1)
      try {
        const parsed = JSON.parse(text) as { action?: AgentAction }
        return typeof parsed.action?.type === 'string'
          ? { action: parsed.action, text, truncated: reply.includes('{', index + 1) }
          : fallback
      }
      catch {
        break
      }
    }
  }
  return fallback
}

/**
 * Renders a {@link Prompt} into one wire request. Exported so `doctor` can
 * print the exact body it sent as a reproduce command.
 */
export function requestFor(endpoint: ModelEndpoint, prompt: Prompt): { body: string, url: string } {
  const baseURL = endpoint.baseURL.replace(/\/$/, '')
  if (endpoint.api === 'chat') {
    const messages = [
      ...(prompt.instructions == null ? [] : [{ content: prompt.instructions, role: 'system' }]),
      ...prompt.turns.map(turn => ({
        content: turn.image == null
          ? turn.text
          : [{ text: turn.text, type: 'text' }, { image_url: { url: turn.image }, type: 'image_url' }],
        role: turn.role,
      })),
    ]
    return { body: JSON.stringify({ messages, model: endpoint.model }), url: `${baseURL}/chat/completions` }
  }
  const input = prompt.turns.map(turn => ({
    content: turn.role === 'assistant'
      ? [{ text: turn.text, type: 'output_text' }]
      : [{ text: turn.text, type: 'input_text' }, ...(turn.image == null ? [] : [{ image_url: turn.image, type: 'input_image' }])],
    role: turn.role,
  }))
  // NOTICE: `store: false` keeps every step a self-contained request (history
  // is resent, no `previous_response_id`), so a step can be replayed from the
  // episode evidence alone and no conversation state lives at the provider.
  return {
    body: JSON.stringify({ input, instructions: prompt.instructions, model: endpoint.model, store: false }),
    url: `${baseURL}/responses`,
  }
}

/**
 * Runs a bounded screenshot -> model -> AUV action loop.
 *
 * Call stack:
 *
 * {@link runEpisode}
 *   -> {@link runAgent}
 *     -> `display.capture` (AUV)
 *     -> {@link complete} (`POST {baseURL}/responses` or `/chat/completions`)
 *     -> one typed `input.*` command (AUV)
 *
 * Use when:
 * - a Vieval case needs a real model to drive AUV inside an OSWorld VM
 *
 * Expects:
 * - `invoke` to target a paired guest device
 *
 * Returns:
 * - the AUV Run IDs, model actions, and the terminal action for the evaluator
 */
export async function runAgent(
  instruction: string,
  endpoint: ModelEndpoint,
  invoke: Invoke,
  options: { maxSteps: number, signal: AbortSignal },
): Promise<AgentResult> {
  const result: AgentResult = { runIds: [], steps: [] }
  const history: Prompt['turns'] = []
  let ignoredNote = ''

  try {
    for (let step = 0; step < options.maxSteps; step += 1) {
      const capture = await invoke('display.capture', [])
      result.runIds.push(capture.run_id)
      const image = capture.artifacts[0]?.file_path
      if (image == null) {
        throw new Error(`AUV capture ${capture.run_id} returned no image artifact.`)
      }

      const reply = await complete(endpoint, {
        instructions: systemPrompt,
        turns: [
          { role: 'user', text: `Task: ${instruction}` },
          // Only the latest screenshot is sent; earlier turns keep their text.
          ...history,
          {
            image: `data:image/webp;base64,${(await readFile(image)).toString('base64')}`,
            role: 'user',
            text: `${ignoredNote}Step ${step + 1} of ${options.maxSteps}. Current screenshot:`,
          },
        ],
      }, options.signal)
      // History holds only what was executed. With the full reply, the model
      // assumed its extra concatenated actions had run and planned from a state
      // that never existed (8 of 10 early zero scores in test_nogdrive).
      const { action, text, truncated } = parseDecision(reply)
      history.push({ role: 'assistant', text })
      ignoredNote = truncated
        ? 'Only the first action of your previous reply was executed; the others were ignored. Reply with exactly one action. '
        : ''
      if (action.type === 'done' || action.type === 'fail') {
        result.steps.push({ action, reply })
        result.lastAction = action.type === 'done' ? 'DONE' : 'FAIL'
        return result
      }
      const invocation = toInvocation(action)
      if (invocation == null) {
        result.steps.push({ action, reply })
        await new Promise(resolve => setTimeout(resolve, 2_000))
        continue
      }
      const delivered = await invoke(invocation[0], invocation[1])
      result.runIds.push(delivered.run_id)
      result.steps.push({ action, reply, runId: delivered.run_id })
      // NOTICE: UI effects are asynchronous and AUV input delivery does not wait
      // for repaint. One second matches OSWorld's default inter-step pause order.
      await new Promise(resolve => setTimeout(resolve, 1_000))
    }
  }
  catch (error) {
    throw new AgentRunError(result, error)
  }
  return result
}

/**
 * Maps a model action to AUV invoke argv; `wait` has no invocation.
 *
 * Flags come first and positionals follow `--`, so typed text or a negative
 * scroll delta is never parsed as an option.
 */
export function toInvocation(action: AgentAction): [string, string[]] | undefined {
  // NOTICE: OSWorld guests run at a fixed 1920x1080 with scale 1. AUV's
  // `--normalized` needs a device-specific `display:` target, so the 0..1000
  // grid is projected onto that fixed screen here instead.
  const x = (value: number) => String(Math.round(Math.min(Math.max(value, 0), 1000) * 1.919))
  const y = (value: number) => String(Math.round(Math.min(Math.max(value, 0), 1000) * 1.079))
  switch (action.type) {
    case 'click':
      return ['input.clickPoint', ['--relative-to', 'screen', '--button', action.button ?? 'left', '--click-count', String(action.count ?? 1), '--', x(action.x), y(action.y)]]
    case 'drag':
      return ['input.drag', ['--relative-to', 'screen', '--', x(action.x1), y(action.y1), x(action.x2), y(action.y2)]]
    case 'keys':
      return ['input.keys', ['--', ...action.keys.map(key => key.toLowerCase())]]
    case 'scroll':
      return ['input.scrollPoint', ['--', x(action.x), y(action.y), '0', String(Math.round(action.dy))]]
    case 'type':
      return ['input.typeText', ['--', action.text]]
    default:
      return undefined
  }
}

function isTransientStatus(status: number): boolean {
  return status === 408 || status === 429 || status >= 500
}

/** Extracts the assistant text; Responses output may also hold reasoning items. */
function replyText(api: ModelApi, body: unknown): string {
  if (api === 'chat') {
    return (body as { choices?: Array<{ message?: { content?: string } }> }).choices?.[0]?.message?.content ?? ''
  }
  const output = (body as { output?: Array<{ content?: Array<{ text?: string, type: string }>, type: string }> }).output ?? []
  return output
    .filter(item => item.type === 'message')
    .flatMap(item => item.content ?? [])
    .filter(part => part.type === 'output_text')
    .map(part => part.text ?? '')
    .join('')
}
