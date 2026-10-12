import process from 'node:process'

import { caseOf, describeTask, expect } from 'vieval'
import { modelFromRun, openaiFromRunContext } from 'vieval/plugins/chat-models'

import { modelApiFrom } from '../src/agent'
import { loadCases } from '../src/cases'
import { runEpisode } from '../src/episode'
import { terminalUseOf } from '../src/terminalUse'

// Wall-clock budget for one episode: Pod boot, setup, every agent step, and
// evaluation. Raise it together with OSWORLD_MAX_STEPS.
const caseTimeoutMs = Number(process.env.OSWORLD_CASE_TIMEOUT_MINUTES ?? 60) * 60 * 1000

// NOTICE: One task holds every case: Vieval 0.2.0 caps cases per task only,
// and OSWORLD_CONCURRENCY must bound all VMs. Domain is the case prefix.
// TODO(osworld-domain-tasks): one task per domain once Vieval shares a case
// cap across a project's tasks (owner is adding it).
describeTask('osworld', () => {
  for (const item of loadCases()) {
    caseOf(`${item.domain}/${item.task.id}`, async (context) => {
      const { apiKey = '', baseURL = 'https://api.openai.com/v1', model } = openaiFromRunContext(modelFromRun(context, { axis: 'model' }))
      const repeat = Number(context.task.matrix.run.repeat ?? 1)
      context.recordMetric('osworld.domain', item.domain)
      context.recordMetric('osworld.task.file', item.file)
      context.recordMetric('osworld.evaluator.func', JSON.stringify(item.task.evaluator.func))
      const api = modelApiFrom(process.env.OSWORLD_OPENAI_API)
      context.recordMetric('osworld.model', model)
      context.recordMetric('osworld.model.api', api)
      context.recordMetric('osworld.repeat', repeat)

      const result = await runEpisode(item, { api, apiKey, baseURL, model }, { repeat, signal: context.signal })
      context.recordMetric('osworld.pod', result.podName)
      context.recordMetric('osworld.evidence', result.evidenceDirectory)
      context.recordMetric('osworld.agent.steps', result.agent.steps.length)
      context.recordMetric('osworld.agent.last_action', result.agent.lastAction ?? 'none')
      // GUI-only episodes are the ones that measure AUV driving applications;
      // see src/terminalUse.ts.
      const terminalUse = terminalUseOf(result.agent.steps)
      context.recordMetric('osworld.agent.terminal_steps', terminalUse.terminalSteps)
      context.recordMetric('osworld.agent.script_steps', terminalUse.scriptSteps)
      context.recordMetric('osworld.agent.gui_only', terminalUse.terminalSteps === 0)
      context.recordMetric('auv.run.ids', result.agent.runIds)
      context.recordMetric('auv.linux_binary.sha256', result.auvLinuxSha256)
      context.recordScore(result.score, 'exact')
      expect(result.score).toBe(1)
    }, { input: item, timeout: caseTimeoutMs })
  }
}, {
  description: 'OSWorld tasks in disposable KVM Pods, driven through AUV and scored by the upstream evaluator.',
})
