import type { AgentResult, AgentTelemetry, AgentTools, ModelEndpoint } from '../agent'

import process from 'node:process'

import { runAgent as runBaselineAgent } from './baseline/runAgent'
import { observationPolicyFrom, runAgent as runPiAgent } from './pi/runAgent'

/**
 * Which model -> tool loop drives an episode.
 *
 * - `pi`: `@earendil-works/pi-agent-core` with harness observations after
 *   actions and a multi-screenshot window (see `./pi/runAgent.ts`).
 * - `baseline`: the frozen hand-written loop the 2026-10-12/13 runs used,
 *   kept to sanity-check scores and harness changes against
 *   (see `./baseline/runAgent.ts`).
 */
export type AgentLoop = 'baseline' | 'pi'

/** Reads `OSWORLD_AGENT`; unset or empty selects `pi`. */
export function agentLoopFrom(value: string | undefined): AgentLoop {
  if (value == null || value === '' || value === 'pi') {
    return 'pi'
  }
  if (value === 'baseline') {
    return value
  }
  throw new Error(`OSWORLD_AGENT must be "pi" or "baseline", got ${JSON.stringify(value)}.`)
}

/** Runs `loop` for one episode; both loops return the same {@link AgentResult}. */
export async function runAgent(
  loop: AgentLoop,
  instruction: string,
  endpoint: ModelEndpoint,
  tools: AgentTools,
  options: { maxSteps: number, signal: AbortSignal, telemetry?: AgentTelemetry },
): Promise<AgentResult> {
  if (loop === 'baseline') {
    return await runBaselineAgent(instruction, endpoint, tools, options)
  }
  return await runPiAgent(instruction, endpoint, tools, { ...options, observation: observationPolicyFrom(process.env) })
}
