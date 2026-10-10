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

/** OpenAI-compatible chat endpoint used to choose the next AUV action. */
export interface ChatEndpoint {
  apiKey: string
  baseURL: string
  model: string
}

/** Executes one AUV invoke command and returns its parsed JSON result. */
export type Invoke = (command: string, args: string[]) => Promise<{ artifacts: Array<{ file_path: string }>, run_id: string }>

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
 * Extracts the first complete JSON object from a reply; anything else becomes
 * `wait`. Models sometimes append several planned turns as concatenated
 * objects, so only the first balanced object is the current decision.
 */
export function parseAction(reply: string): AgentAction {
  let depth = 0
  let inString = false
  const begin = reply.indexOf('{')
  if (begin < 0) {
    return { type: 'wait' }
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
      try {
        const parsed = JSON.parse(reply.slice(begin, index + 1)) as { action?: AgentAction }
        return typeof parsed.action?.type === 'string' ? parsed.action : { type: 'wait' }
      }
      catch {
        break
      }
    }
  }
  return { type: 'wait' }
}

/**
 * Runs a bounded screenshot -> model -> AUV action loop.
 *
 * Call stack:
 *
 * {@link runEpisode}
 *   -> {@link runAgent}
 *     -> `display.capture` (AUV)
 *     -> `POST {baseURL}/chat/completions`
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
  endpoint: ChatEndpoint,
  invoke: Invoke,
  options: { maxSteps: number, signal: AbortSignal },
): Promise<AgentResult> {
  const result: AgentResult = { runIds: [], steps: [] }
  const history: Array<{ content: string, role: 'assistant' }> = []

  for (let step = 0; step < options.maxSteps; step += 1) {
    const capture = await invoke('display.capture', [])
    result.runIds.push(capture.run_id)
    const image = capture.artifacts[0]?.file_path
    if (image == null) {
      throw new Error(`AUV capture ${capture.run_id} returned no image artifact.`)
    }

    const reply = await complete(endpoint, [
      { content: systemPrompt, role: 'system' },
      { content: `Task: ${instruction}`, role: 'user' },
      // Only the latest screenshot is sent; earlier turns keep their text.
      ...history,
      {
        content: [
          { text: `Step ${step + 1} of ${options.maxSteps}. Current screenshot:`, type: 'text' },
          { image_url: { url: `data:image/webp;base64,${(await readFile(image)).toString('base64')}` }, type: 'image_url' },
        ],
        role: 'user',
      },
    ], options.signal)
    history.push({ content: reply, role: 'assistant' })

    const action = parseAction(reply)
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

/**
 * Requests one chat completion, retrying transient proxy failures.
 *
 * Network errors, 429, and 5xx are retried at most three times with a short
 * linear backoff; other HTTP errors fail the case immediately.
 */
async function complete(endpoint: ChatEndpoint, messages: unknown[], signal: AbortSignal): Promise<string> {
  for (let attempt = 1; ; attempt += 1) {
    const response = await fetch(`${endpoint.baseURL.replace(/\/$/, '')}/chat/completions`, {
      body: JSON.stringify({ messages, model: endpoint.model }),
      headers: { 'authorization': `Bearer ${endpoint.apiKey}`, 'content-type': 'application/json' },
      method: 'POST',
      signal,
    }).catch((error: unknown) => {
      if (attempt >= 3 || signal.aborted) {
        throw error
      }
      return undefined
    })
    if (response?.ok === true) {
      const body = await response.json() as { choices: Array<{ message: { content: string } }> }
      return body.choices[0]?.message.content ?? ''
    }
    if (response != null && ((response.status < 500 && response.status !== 429) || attempt >= 3)) {
      throw new Error(`Chat completion failed with HTTP ${response.status}: ${await response.text()}`)
    }
    await new Promise(resolve => setTimeout(resolve, attempt * 5_000))
  }
}
