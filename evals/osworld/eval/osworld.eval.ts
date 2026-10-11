import process from 'node:process'

import { caseOf, describeTask, expect } from 'vieval'
import { modelFromRun, openaiFromRunContext } from 'vieval/plugins/chat-models'

import { modelApiFrom } from '../src/agent'
import { loadCases } from '../src/cases'
import { runEpisode } from '../src/episode'

// Wall-clock budget for one episode: Pod boot, setup, every agent step, and
// evaluation. Raise it together with OSWORLD_MAX_STEPS.
const caseTimeoutMs = Number(process.env.OSWORLD_CASE_TIMEOUT_MINUTES ?? 60) * 60 * 1000

// NOTICE: One Vieval task holds every case because Vieval applies its case
// concurrency cap within a task and runs separate task definitions in this
// module one after another. The OSWorld domain is the case-name prefix.
describeTask('osworld', () => {
  for (const item of loadCases()) {
    caseOf(`${item.domain}/${item.task.id}`, async (context) => {
      const { apiKey = '', baseURL = 'https://api.openai.com/v1', model } = openaiFromRunContext(modelFromRun(context, { axis: 'model' }))
      const repeat = Number(context.task.matrix.run.repeat ?? 1)
      context.metric('osworld.domain', item.domain)
      context.metric('osworld.task.file', item.file)
      context.metric('osworld.evaluator.func', JSON.stringify(item.task.evaluator.func))
      const api = modelApiFrom(process.env.OSWORLD_OPENAI_API)
      context.metric('osworld.model', model)
      context.metric('osworld.model.api', api)
      context.metric('osworld.repeat', repeat)

      const result = await runEpisode(item, { api, apiKey, baseURL, model }, { repeat, signal: context.signal })
      context.metric('osworld.pod', result.podName)
      context.metric('osworld.evidence', result.evidenceDirectory)
      context.metric('osworld.agent.steps', result.agent.steps.length)
      context.metric('osworld.agent.last_action', result.agent.lastAction ?? 'none')
      context.metric('auv.run.ids', result.agent.runIds)
      context.metric('auv.linux_binary.sha256', result.auvLinuxSha256)
      context.score(result.score, 'exact')
      expect(result.score).toBe(1)
    }, { input: item, timeout: caseTimeoutMs })
  }
}, {
  description: 'OSWorld tasks in disposable KVM Pods, driven through AUV and scored by the upstream evaluator.',
})
