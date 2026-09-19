# Yeet on Terminal-Bench 2.0

This adapter benchmarks Yeet's actual agent coordinator and tool loop through Harbor. It deliberately does **not** use `yeet run`, because that command is a raw model-call path rather than the autonomous Yeet agent runtime.

## Isolation model

Terminal-Bench does **not** use the normal Docker daemon or the user's existing Docker containers.

`run.sh` creates and uses a dedicated Apple container machine named `yeet-terminalbench`. The machine is based on `dockerd:latest`, has its own root filesystem and Docker store, and is created with `home-mount=none`. Harbor itself runs inside that machine and connects only to its local `/var/run/docker.sock`.

The host repository is not mounted into the benchmark machine. `setup-apple-machine.sh` streams only the benchmark adapter and prebuilt Yeet bundles into `/opt/yeet-benchmark`. After a run, `jobs/` is streamed back into the host Yeet repository. Provider API keys are forwarded only when they are explicitly present in the environment.

Unused task images and Docker build cache are pruned inside the dedicated machine during long runs and again at shutdown. `fstrim` is also run there so its sparse disk releases unused host blocks. None of those cleanup operations target the normal Docker machine.

## Prerequisites

- Apple `container` CLI
- The local `dockerd:latest` Apple machine image
- A provider API key supported by Harbor and Yeet, for example `OPENAI_API_KEY`, for scored runs

Harbor 0.23.0 is installed inside the dedicated machine automatically. Host Harbor and host Docker are not required by `run.sh`.

The adapter intentionally does not copy Yeet's host credential store or browser/OAuth sessions into benchmark tasks. Export a benchmark credential explicitly instead.

## Setup / verify the isolated machine

```bash
./benchmarks/terminalbench/setup-apple-machine.sh
./benchmarks/terminalbench/enable_amd64.sh
```

By default the machine has 4 CPUs and 8 GiB RAM. These can be changed before initial creation with:

```bash
YEET_TERMINALBENCH_CPUS=6 \
YEET_TERMINALBENCH_MEMORY=12G \
./benchmarks/terminalbench/setup-apple-machine.sh
```

The machine name can be overridden with `YEET_TERMINALBENCH_MACHINE`.

Some Terminal-Bench 2.0 task images are amd64-only. `enable_amd64.sh` registers qemu/binfmt **inside the dedicated machine only** and verifies an amd64 Alpine container there.

## Build the task-container bundles

From the Yeet repository root:

```bash
./benchmarks/terminalbench/build_bundle.sh arm64
./benchmarks/terminalbench/build_bundle.sh amd64
```

With no architecture argument, the wrapper selects the host architecture. Bundle builds are also performed inside `yeet-terminalbench`; `build_bundle.sh` does not build through the normal Docker daemon.

The generated bundles are copied back to `benchmarks/terminalbench/dist/` and are intentionally git-ignored. Each bundle contains the current local Yeet build, compiled TypeScript runtime, skills, and Node.js. The Yeet executable is built against musl so benchmark task images do not need a matching glibc.

The Harbor adapter detects the architecture from inside each task container and chooses `dist/linux-arm64` or `dist/linux-amd64` automatically.

## Dry-run validation

```bash
./benchmarks/terminalbench/run.sh \
  --model openai/gpt-5.6-sol \
  --ak reasoning=high \
  --dry-run \
  --n-tasks 1 \
  --yes
```

This starts Harbor inside the dedicated Apple machine but does not execute a task.

## One-task smoke run

```bash
export OPENAI_API_KEY='...'

./benchmarks/terminalbench/run.sh \
  --model openai/gpt-5.6-sol \
  --ak reasoning=high \
  --n-tasks 1 \
  --n-concurrent 1
```

`run.sh` pins the dataset to `terminal-bench@2.0` and supplies the custom Yeet agent import path. All remaining arguments are passed directly to Harbor running in the isolated machine.

## Full run

```bash
./benchmarks/terminalbench/run.sh \
  --model openai/gpt-5.6-sol \
  --ak reasoning=high \
  --n-concurrent 4 \
  --job-name yeet-gpt-5.6-sol-tb2
```

The wrapper checks host free space before allowing the dedicated sparse disk to grow. The default safety floor is 20 GiB; override it deliberately with `YEET_HARBOR_MIN_FREE_GB`.

For reproducible comparisons, hold model, reasoning level, concurrency, task set, Harbor version, retry policy, and provider endpoint constant across agents.

## Benchmark behavior

Harbor creates the task environment in the dedicated Docker daemon and invokes `YeetAgent`. The adapter does not run a blanket `apt-get install` during startup; the Yeet bundle is self-contained and only falls back to installing `tar` if a task image does not provide it. This is especially important for amd64 task images running under emulation on Apple Silicon.

The adapter uploads the matching Linux bundle, writes the benchmark instruction to a file, and then runs:

```text
yeet agent --benchmark --json --model ... --reasoning ... < instruction.txt
```

The instruction is sent through stdin rather than embedded in the process argument list. `--benchmark` disables Goal continuation and enables unattended shell approval inside the already-isolated Harbor task container. Normal interactive Yeet sandbox behavior is unchanged.

Yeet writes its final JSON output to Harbor's agent log directory as `yeet.jsonl`. The task verifier, not Yeet's prose response, determines the Terminal-Bench reward.

`--ak timeout_seconds=N` adds an optional Yeet-internal timeout. Its default is `0`, which leaves timeout enforcement to Harbor.

## Local headless-agent check

The same underlying Yeet command can still be exercised outside Harbor:

```bash
printf '%s\n' 'Inspect this workspace and report what it contains.' | \
  yeet agent --benchmark --model openai/gpt-5.6-sol --reasoning high --json
```

This path uses the same `Backend` / `AgentCoordinator` as Yeet's TUI rather than the raw `yeet run` model call.
