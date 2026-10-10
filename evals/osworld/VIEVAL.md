# OSWorld on Kubernetes with Vieval

Vieval schedules, repeats, scores, and reports OSWorld Ubuntu tasks. Each
case runs this pipeline:

```text
fresh KVM Pod over the read-only OSWorld base image
  -> guest AUV daemon (Linux x86_64) paired with a host AUV client (macOS)
  -> upstream OSWorld setup            (bridge/osworld_bridge.py setup)
  -> model loop: AUV capture -> chat completion -> one typed AUV input command
  -> upstream OSWorld evaluator score  (bridge/osworld_bridge.py evaluate)
  -> Vieval score + metrics, result.json, Pod deleted
```

Vieval owns the denominator, the model and repeat matrix, and concurrency. AUV
owns every GUI action. The pinned upstream Python code owns setup and scoring
semantics, so no evaluator is re-implemented in TypeScript.

## Layout

| Path | Owner |
| --- | --- |
| `tasks/<domain>/<id>.json` | Upstream task JSON, byte-for-byte, from `scripts/sync-osworld` |
| `tasks/test_*.json`, `tasks/REVISION` | Upstream task-set indexes (the "cases" per domain) and the pinned revision |
| `vieval.config.ts` | OpenAI-compatible models (`ChatModels`), `model` x `repeat` run matrix, concurrency |
| `eval/osworld.eval.ts` | One Vieval task; one case per task id, named `domain/id` |
| `src/cases.ts` | Loads one case per task id, filtered by environment |
| `src/episode.ts` | Pod, port-forward, AUV install/pairing, bridge calls, `result.json`, cleanup |
| `src/agent.ts` | Minimal screenshot -> model -> AUV action loop (not a planner) |
| `src/doctor.ts` | Prerequisite checks with reproduce commands -> `results/doctor.json` |
| `src/summarize.ts` | Aggregates `result.json` files -> `results/summary.{json,md}` |
| `bridge/` | Python package (Pixi env `osworld`) wrapping upstream `DesktopEnv` setup/evaluate |
| `results/` | Checked-in doctor output and result summary from the latest recorded run |
| `results/runs/<date>-<commit>/` | Cited run record: environment, commands, chain evidence, raw logs, per-episode results, failure samples |
| `SKILL.md` | Operator runbook and checklist for agents continuing this eval |

All process execution (`kubectl`, `auv`, `pixi`) goes through `tinyexec` argv
arrays in `src/executionLog.ts`; no shell strings are built on the host.

## Configure once

```bash
./evals/osworld/scripts/sync-osworld          # pinned checkout + tasks/
pixi install -e osworld                        # evaluator dependencies
kubectl apply -f evals/osworld/kubernetes/kvm-node-feature-rule.yaml
kubectl apply -f evals/osworld/kubernetes/base-image-pvc.yaml
kubectl label node <validated-node> auv.moeru.ai/osworld-ready=true
```

Populate `System.qcow2` in the base PVC through a one-time importer and verify
it against the `auv.moeru.ai/system-qcow2-sha256` annotation. Episodes mount it
read-only and write only to their own qcow2 overlay.

Copy `.env.example` into the shell. The AUV binaries must come from the same
source commit; the Linux build must run on the guest's glibc (Ubuntu 22.04).

### Model endpoint

The model is any OpenAI-compatible `/chat/completions` endpoint that accepts
image input. Vieval's built-in `openai` inference executor carries its
`baseURL`, `apiKey`, and model name; `src/agent.ts` calls it directly.

| Variable | Default | Meaning |
| --- | --- | --- |
| `OSWORLD_OPENAI_BASE_URL` | `http://127.0.0.1:8317/v1` | Local CLIProxyAPI (Codex OAuth accounts) |
| `OSWORLD_OPENAI_API_KEY` | `no-key` placeholder | Needed only for keyed endpoints |
| `OSWORLD_MODEL` | `gpt-5.5` | Comma-separated; each model is one run-matrix row |
| `OSWORLD_MAX_STEPS` | `15` | Model turns per episode |

Other routes need no code change, only a different base URL:

- **OpenAI or OpenRouter:** set the base URL and API key.
- **Codex through ACP:** run an ACP-to-chat-completions adapter such as
  `alint-dev/alint` `packages/model-adapter-acp` and point the base URL at it.
  This was not needed here because CLIProxyAPI already serves Codex models.

## Diagnose

```bash
pnpm -F @auv-js/eval-osworld doctor
```

The doctor checks the binaries and digests, `kubectl` permissions, the
OSWorld-ready KVM node, the base PVC, the Python bridge import, `GET /models`,
and a text and an image chat completion for every configured model. Each
failure prints a paste-able `reproduce:` command. Model failures also print
the CLIProxyAPI config, account, log, and service locations. The JSON goes to
`results/doctor.json`.

## Run

```bash
pnpm -F @auv-js/eval-osworld cases:list                  # no cluster access
OSWORLD_TASK_IDS=e0df059f-28a6-4169-924f-b9623e7184cc \
OSWORLD_REPEAT=3 pnpm -F @auv-js/eval-osworld eval:run   # one task, 3 attempts
OSWORLD_DOMAINS=os OSWORLD_MODEL=gpt-5.5,gpt-5.6-terra \
OSWORLD_CONCURRENCY=2 pnpm -F @auv-js/eval-osworld eval:run
pnpm -F @auv-js/eval-osworld results:summarize           # results/summary.{json,md}
```

Selection:

- `OSWORLD_TASK_IDS`, `OSWORLD_DOMAINS`: comma-separated filters.
- `OSWORLD_TEST_SET`: `test_nogdrive` (default, 361 tasks), `test_all` (369;
  Google Drive tasks need operator credentials), or `test_infeasible`.
- `OSWORLD_REPEAT`: independent attempts per (task, model); each attempt is a
  separate run-matrix row with its own fresh Pod.
- `OSWORLD_CONCURRENCY`: case and task cap. Each case allocates its own local
  ports and Pod name, so the limit is KVM node capacity.
- `OSWORLD_KEEP_POD=true`: keep the Pod for debugging; you then own cleanup.

## Results and scoring

The score is the upstream evaluator's value in `0..1`. Vieval records it with
`context.score(score, 'exact')` and asserts `score === 1` for pass/fail. Vieval
reports go to `.vieval/reports/`.

Every episode writes `.vieval/episodes/<task>/<time>-<model>-r<repeat>/result.json`.
Errors write it too, so the denominator stays honest:

```jsonc
{
  "domain": "os",
  "model": "gpt-5.5",
  "podName": "osworld-e0df059f-1a2b3c",
  "repeat": 1,
  "startedAt": "2026-10-11T00:00:00.000Z",
  "taskId": "e0df059f-28a6-4169-924f-b9623e7184cc",
  "status": "evaluated", // or "error" with an "error" message
  "score": 1, // absent on error
  "agent": { "lastAction": "DONE", "runIds": [], "steps": [] },
  "auvLinuxSha256": "…",
  "finishedAt": "…"
}
```

The same directory holds the AUV run store (captures, input records), and
`bridge-setup.log` / `bridge-evaluate.log`. Each bridge log ends with the
verdict JSON that the bridge printed (`--- stdout (exit N) ---`). `results:summarize` groups records
by task and model. It reports attempts, errors, scores, mean (an error counts
as 0), and pass@k.

## Limitations

- **Evidence level:** the checked-in `results/summary.md` names the tasks
  that ran live. On 2026-10-11, 9 of 11 episodes on 6 tasks scored 1:
  5 `os` tasks and 1 `vs_code` task. See
  `results/runs/2026-10-11-pr258-a2c8ce64/`. Other domains are
  discoverable and pass through the same code path, but they are not proven
  until a summary row exists. Office, GIMP, and multi-app tasks are the
  hardest for this small agent.
- **Agent:** no accessibility tree, no planner, and only the latest
  screenshot plus earlier text replies. This is intentional. To evaluate
  another agent, replace `src/agent.ts` and keep the episode contract
  (`instruction`, `invoke`, terminal `DONE`/`FAIL`). The recorded failures
  show the cost. In the VS Code theme task, the model opened Settings
  instead of the theme picker and split `Ctrl+K Ctrl+T` across two turns.
  In the GNOME favorites task, AUV opened the correct dock context menu, but
  the model clicked about 90 grid units to the right of "Remove from
  Favorites" five times.
- **Local evidence:** `results/summary.json` keeps every step and reply. The
  full `.vieval/episodes/` captures and Vieval reports are gitignored and
  exist only on the machine that ran them. A cited run copies its logs,
  per-episode results, and failure captures into `results/runs/`.
- **Proxy and Google Drive:** proxy tasks run without the upstream proxy.
  Sites with bot detection can fail regardless of the agent. `test_all`
  Drive tasks need credentials that are not provided.
- **Screen:** the guest is assumed to be 1920x1080 at scale 1, as upstream.
- **Cluster:** one validated KVM node (`liet-gpu-1`) currently carries
  `auv.moeru.ai/osworld-ready=true`. Each Pod requests 8 GiB of memory.
- **Model access:** CLIProxyAPI routes to Codex OAuth accounts. Quota or
  expired accounts show up as HTTP 429/401 in `doctor` and as `error` records.
  The agent retries 429/5xx three times before the case fails.

## Adapting a Python benchmark to Vieval: checklist

1. **Copy tasks, do not translate them.** Sync the upstream JSON at a pinned
   revision; keep the upstream index files as the denominator.
2. **Map grouping to Vieval.** One `describeTask`; each task id becomes
   `` caseOf(`${domain}/${id}`) ``. Vieval caps concurrency per task and runs task
   definitions in one module sequentially, so one task per domain serializes
   domains. Keep upstream ids in case names for joins.
3. **Filter with the environment, not code edits.** Vieval has no task/case
   CLI filter; read comma-separated ids/domains in the case loader.
4. **Use the run matrix for models and repeats.** `model` and `repeat` axes
   give each attempt its own row, score, and report entry.
5. **Translate orchestration, wrap semantics.** Node.js owns lifecycle (Pods,
   port-forwards, retries, cleanup) through `tinyexec` argv arrays. Setup and
   evaluator semantics stay in upstream Python behind a one-shot JSON
   stdin/stdout process. Only port an evaluator when it is small and exact.
6. **Construct, do not fork, the upstream environment.** Build the object
   with only the attributes setup/evaluate read and skip the provider; see
   `bridge/osworld_bridge.py`. Run it with the upstream checkout as `cwd`.
7. **Return failures as values.** Upstream `reset` swallows setup failure;
   call the underlying `setup` and check its boolean.
8. **Keep the agent's actions typed.** Route every GUI action through AUV
   commands; never fall back to the benchmark's `pyautogui` executor, so AUV
   Run records stay the evidence.
9. **Forward every port a getter needs** (OSWorld: setup API 5000, Chromium
   CDP 9222, VLC 8080) and keep the AUV daemon on a separate port.
10. **Isolate per case.** Unique Pod names and dynamically allocated local
    ports make `OSWORLD_CONCURRENCY` safe. Vieval CLI concurrency flags only
    lower configured caps, so raise the cap in config.
11. **Retain evidence.** Score via `context.score`, add metrics for the Pod,
    AUV Run IDs, agent steps, and binary digest, and write `result.json` on
    failure too.

gRPC is not needed for this shape: each bridge call is one short request and
one scalar score. Consider it only for a long-lived Python service with many
typed calls per case.
