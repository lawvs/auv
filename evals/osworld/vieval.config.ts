import process from 'node:process'

import { defineConfig } from 'vieval'
import { chatModelFrom, ChatModels } from 'vieval/plugins/chat-models'

// NOTICE: Vieval CLI concurrency flags only lower configured caps, so the
// cap itself comes from the environment. Every case owns its Pod and local
// ports; the limit is KVM node capacity.
const parallel = Number(process.env.OSWORLD_CONCURRENCY ?? 1)

// Comma-separated model names served by one OpenAI-compatible endpoint
// (default: local cliproxyapi). Each becomes one run-matrix row.
const models = (process.env.OSWORLD_MODEL ?? 'gpt-5.5').split(',').map(model => model.trim()).filter(Boolean)

// Independent repetitions of every (case, model) pair. Each repetition is a
// separate run-matrix row, so it gets its own fresh Pod and its own score.
const repeat = Array.from({ length: Math.max(1, Number(process.env.OSWORLD_REPEAT ?? 1)) }, (_, index) => index + 1)

export default defineConfig({
  concurrency: {
    workspace: 1,
  },
  plugins: [
    ChatModels({
      models: models.map(model => chatModelFrom({
        // NOTICE: Vieval rejects an empty `apiKey`, but a local cliproxyapi
        // with `api-keys: []` accepts any bearer token. The placeholder only
        // satisfies the config check; set the variable for keyed endpoints.
        apiKey: () => process.env.OSWORLD_OPENAI_API_KEY || 'no-key',
        baseURL: () => process.env.OSWORLD_OPENAI_BASE_URL ?? 'http://127.0.0.1:8317/v1',
        inferenceExecutor: 'openai',
        model,
      })),
    }),
  ],
  projects: [
    {
      concurrency: {
        case: parallel,
        project: 1,
        task: parallel,
      },
      include: ['eval/*.eval.ts'],
      name: 'osworld-kubernetes',
      root: '.',
      runMatrix: {
        extend: { model: models, repeat },
      },
    },
  ],
})
