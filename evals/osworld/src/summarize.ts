import process from 'node:process'

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'

import { packageDirectory } from './cases'

/** One `result.json` written by `runEpisode`, for evaluated and failed episodes alike. */
interface EpisodeRecord {
  agent?: { lastAction?: string, steps: unknown[] }
  auvLinuxSha256?: string
  domain: string
  error?: string
  evidence: string
  model: string
  repeat: number
  score?: number
  startedAt: string
  status: 'error' | 'evaluated'
  taskId: string
}

/**
 * Aggregates every episode record into `results/summary.json` and `.md`.
 *
 * Call stack:
 *
 * `pnpm results:summarize [episodesDir]`
 *   -> read `.vieval/episodes/<task>/<run>/result.json`
 *   -> group by task and model
 *   -> write `results/summary.{json,md}`
 *
 * Scoring:
 * - `mean` averages the upstream evaluator score over every attempt; errors count as 0
 * - `pass@k` is 1 when any attempt scored 1
 */
function main(): void {
  const episodes = process.argv[2] ?? join(packageDirectory, '.vieval', 'episodes')
  const records: EpisodeRecord[] = existsSync(episodes)
    ? readdirSync(episodes).flatMap(task => readdirSync(join(episodes, task))
        .map(run => join(episodes, task, run, 'result.json'))
        .filter(existsSync)
        .map(file => ({ ...JSON.parse(readFileSync(file, 'utf8')), evidence: relative(packageDirectory, join(file, '..')) }) as EpisodeRecord)).filter(record => typeof record.model === 'string')
    : []
  records.sort((left, right) => left.startedAt.localeCompare(right.startedAt))

  const groups = new Map<string, EpisodeRecord[]>()
  for (const record of records) {
    const key = `${record.domain}/${record.taskId}\t${record.model}`
    groups.set(key, [...(groups.get(key) ?? []), record])
  }
  const rows = [...groups.entries()].map(([key, attempts]) => {
    const [task, model] = key.split('\t')
    const scores = attempts.map(attempt => attempt.status === 'evaluated' ? Number(attempt.score) : 0)
    return {
      attempts: attempts.length,
      errors: attempts.filter(attempt => attempt.status === 'error').length,
      mean: scores.reduce((sum, score) => sum + score, 0) / scores.length,
      model,
      passAtK: scores.some(score => score >= 1) ? 1 : 0,
      scores,
      task,
    }
  })

  const passed = records.filter(record => record.status === 'evaluated' && Number(record.score) >= 1).length
  const errors = records.filter(record => record.status === 'error').length
  const binaries = [...new Set(records.map(record => record.auvLinuxSha256).filter(Boolean))]
  const output = join(packageDirectory, 'results')
  mkdirSync(output, { recursive: true })
  writeFileSync(join(output, 'summary.json'), `${JSON.stringify({ generatedAt: new Date().toISOString(), overall: { auvLinuxSha256: binaries, episodes: records.length, errors, passed }, records, rows }, null, 2)}\n`)
  writeFileSync(join(output, 'summary.md'), [
    '# OSWorld x Vieval results',
    '',
    `Generated ${new Date().toISOString()} from \`${relative(packageDirectory, episodes)}\`. Errors count as score 0.`,
    '',
    `**Overall:** ${passed}/${records.length} episodes scored 1 (${records.length > 0 ? (100 * passed / records.length).toFixed(1) : '0.0'}%), ${errors} harness errors. Guest AUV sha256: ${binaries.map(sha => `\`${sha}\``).join(', ') || '-'}.`,
    '',
    '| Task | Model | Attempts | Errors | Scores | Mean | pass@k |',
    '| --- | --- | ---: | ---: | --- | ---: | ---: |',
    ...rows.map(row => `| ${row.task} | ${row.model} | ${row.attempts} | ${row.errors} | ${row.scores.join(', ')} | ${row.mean.toFixed(2)} | ${row.passAtK} |`),
    '',
    '## Episodes',
    '',
    '| Started | Task | Model | Repeat | Status | Score | Steps | Last action | Evidence / error |',
    '| --- | --- | --- | ---: | --- | ---: | ---: | --- | --- |',
    ...records.map(record => `| ${record.startedAt} | ${record.taskId.slice(0, 8)} | ${record.model} | ${record.repeat} | ${record.status} | ${record.score ?? '-'} | ${record.agent?.steps.length ?? '-'} | ${record.agent?.lastAction ?? '-'} | ${record.error?.replaceAll('|', '\\|').replaceAll('\n', ' ').slice(0, 160) ?? record.evidence} |`),
    '',
  ].join('\n'))
  process.stdout.write(`${rows.length} task/model rows from ${records.length} episodes -> ${relative(process.cwd(), output)}/summary.{json,md}\n`)
}

main()
