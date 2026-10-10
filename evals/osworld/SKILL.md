---
name: osworld-vieval
description: Run, extend, and debug the OSWorld-on-Kubernetes Vieval eval in evals/osworld. Use when asked to run OSWorld tasks through AUV, add OSWorld domains or models, repeat runs, summarize scores, or diagnose a failed OSWorld episode or model endpoint.
---

# OSWorld x Vieval operator skill

Read `VIEVAL.md` for the architecture. This file is the checklist. Run every
command from `evals/osworld/` unless noted.

## 0. Environment (once per shell)

```bash
export OSWORLD_AUV_LINUX_BINARY=/abs/auv-linux-x86_64   # same commit as macOS
export OSWORLD_AUV_MACOS_BINARY=/abs/auv-macos-arm64
export OSWORLD_KUBECONFIG=$HOME/.kube/config.d/ihome.conf
export OSWORLD_SOURCE=/abs/OSWorld                       # pinned checkout; default .cache/OSWorld
export OSWORLD_OPENAI_BASE_URL=http://127.0.0.1:8317/v1  # cliproxyapi
export OSWORLD_MODEL=gpt-5.5
```

- [ ] `scripts/sync-osworld "$OSWORLD_SOURCE"` ran, and `tasks/REVISION` matches the checkout HEAD.
- [ ] `pixi install -e osworld` ran at the repository root.

## 1. Preflight: never skip

- [ ] `pnpm doctor` prints `N/N checks passed`. On failure, run the printed
      `reproduce:` command and fix the cause before you start a run.
- [ ] `kubectl -n benchmark-osworld-auv get pods` shows no stale `osworld-*`
      Pods. Delete stale Pods first, because each Pod holds 8 GiB on the KVM node.

## 2. Select and run

- [ ] `OSWORLD_TASK_IDS=… pnpm -s cases:list` shows the expected cases.
- [ ] Start with one task and `OSWORLD_REPEAT=1`. Increase the repeat count
      and `OSWORLD_CONCURRENCY` only after that task passes.
- [ ] Run `pnpm -s eval:run > run.log 2>&1`. One episode takes about 3 to 10
      minutes. Do not interrupt it while a Pod is being deleted.

## 3. Record results

- [ ] Run `pnpm -s results:summarize`, then read `results/summary.md`.
- [ ] Every attempt has a row. An `error` row is a harness failure, not a
      model failure. Read its message before you retry.
- [ ] Commit `results/summary.{json,md}` and `results/doctor.json` only when
      they describe a run that you want to cite.

## 4. Triage a failed episode

Open `.vieval/episodes/<task>/<time>-<model>-r<n>/`:

| Symptom | Look at | Usual cause |
| --- | --- | --- |
| `status: error`, kubectl wait timeout | `kubectl describe pod` | Node is full, or the PVC or KVM label is missing |
| setup API never ready | run.log `port-forward` | QEMU still booting, or the guest failed to start |
| `OSWorld setup failed` | `bridge-setup.log` | Upstream setup step (download, Chrome CDP) |
| AUV pairing or invoke failed | run.log `tool=auv` | Linux and macOS binaries differ, or the guest daemon crashed (`/tmp/auv.log` in the guest) |
| `Chat completion failed with HTTP …` | `pnpm doctor` | Proxy down, model name wrong, quota 429, OAuth expired |
| `status: evaluated`, score 0 | `result.json` `agent.steps`, `auv-runs/` captures | The model did the wrong thing; this is a real result |

## 5. Extend

- **Another model:** append it to `OSWORLD_MODEL`, comma-separated. Each model
  is one run-matrix row, and the summary groups by model.
- **Another endpoint:** change `OSWORLD_OPENAI_BASE_URL` and
  `OSWORLD_OPENAI_API_KEY`. For Codex over ACP, put an ACP-to-OpenAI adapter
  in front of it, for example alint `model-adapter-acp`.
- **Another domain or task:** no code change. Filter with `OSWORLD_DOMAINS` or
  `OSWORLD_TASK_IDS`. To update the upstream revision, change `revision` in
  `scripts/sync-osworld`, re-sync, and run `pnpm doctor` again.
- **Another agent:** replace `runAgent` in `src/agent.ts`. Keep the contract:
  the agent gets the instruction and an AUV `invoke` function, and returns its
  run IDs and `DONE`/`FAIL`. Send every GUI action through AUV, never through
  `pyautogui`.
- Keep the agent small and keep the evaluators upstream. Do not port Python
  metrics to TypeScript.

## 6. Do not

- Do not use `lh workspace use`, and do not change the cluster scheduling labels.
- Do not run `OSWORLD_CONCURRENCY` above the number of free 8 GiB slots on the node.
- Do not present a single live pass as support for all domains. Cite the summary rows.
