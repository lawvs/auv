# Run record: 2026-10-11, PR 258 rebased onto main `6d09fbc9`

One Vieval batch at the rebased branch head. It uses the Responses API agent
default (`358f1684`) and AUV binaries rebuilt from `22be28be`. Each episode
used one fresh KVM Pod. Both episodes reached the upstream evaluator.

**Result:** 2 of 2 episodes scored 1.

| Task | Instruction (short) | Steps | Score |
| --- | --- | ---: | ---: |
| os/e0df059f | Rename `todo_list_Jan_1` to `todo_list_Jan_2` | 9 | 1 |
| os/28cc3b7e | Set the system volume to the maximum | 5 | 1 |

`e0df059f-final.jpg` is the last guest screen capture of the e0df059f
episode. The model renamed the folder in the file manager with F2.

## Environment

| Item | Value |
| --- | --- |
| Source | `codex/pr-258-rebased` at `22be28be`, rebased onto `origin/main` `6d09fbc9` (#332). The range-diff shows all 14 PR commits as identical patches. |
| Guest AUV | `auv-linux-x86_64` from `scripts/build-auv-linux` at `22be28be`. sha256 `abd3a262851bce0156a2fdbf69aa89eaaad285551d1c9347a488ea3260b32764` |
| Host AUV | `auv-macos-arm64` from `cargo build --release` at `22be28be`. sha256 `d4777bc8e17844a458c236794186da1535f079a0d1ffe31ad4209b0efa79db3f` |
| Model | `gpt-5.5` through local CLIProxyAPI `http://127.0.0.1:8317/v1`, `OSWORLD_OPENAI_API=responses` |
| Cluster | `KUBECONFIG=~/.kube/config.d/ihome.conf`, namespace `benchmark-osworld-auv`, node `liet-gpu-1` |

## Commands

```sh
export OSWORLD_AUV_LINUX_BINARY=/tmp/auv-pr258-22be28be/auv-linux-x86_64
export OSWORLD_AUV_MACOS_BINARY=/tmp/auv-pr258-22be28be/auv-macos-arm64
export OSWORLD_KUBECONFIG=$HOME/.kube/config.d/ihome.conf
export OSWORLD_SOURCE=$HOME/.cache/auv-osworld/OSWorld
export OSWORLD_MODEL=gpt-5.5
pnpm -s doctor                       # doctor.txt: 12/12 checks passed
OSWORLD_TASK_IDS=e0df059f-28a6-4169-924f-b9623e7184cc,28cc3b7e-b194-4bc9-8353-d04c0f4d56d2 \
OSWORLD_REPEAT=1 OSWORLD_CONCURRENCY=2 pnpm -s eval:run   # vieval-run-5.log: cases 2 passed | 0 failed
pnpm -s results:summarize
```

## create-on-k8s path at the same head

Separately, `scripts/create-on-k8s --node node2 --base-pvc osworld-v1-base`
created a desktop Pod. `scripts/install-auv` then installed the same guest
binary. Two script bugs were found and fixed:

- `22be28be`: a host profile saved for a deleted VM was reused against a new
  VM and failed as `unauthorized`. The script now re-pairs when the guest has
  no pairing store.
- `58ebe5ce`: during the slow guest apt step, a host-side connection to the
  forwarded AUV port was reset and `kubectl port-forward` exited. The script
  now forwards the AUV port only after the guest daemon listens.

After the fixes, a reconnect after a guest daemon restart reused the same
`device_id` and credential fingerprint.
