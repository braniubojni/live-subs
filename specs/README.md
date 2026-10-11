# Specs

Todo specs for work not yet done. One folder per area, in build order. Each folder has a `README.md` with the same shape:

- **Goal**: what the area must do when finished.
- **Current state**: what the code does today, with file refs. Update this in the PR that lands a box.
- **Todo**: checkbox list. Each item has an acceptance line. Tick the box in the PR that lands it.
- **Tests**: what proves it, runnable without a microphone.
- **Out of scope**: what not to do here.

Constraints from the root [README](../README.md) apply to every spec: no `unsafe`, audio callback never allocates or calls a model, no test shells out to `whisper-cli` / `llama-cli`, CI (`fmt`, `clippy -D warnings`, `test`) stays green.

## Order

| # | Area | Why it sits here |
| --- | --- | --- |
| 11 | [models-and-setup](11-models-and-setup/README.md) | **Build this next.** Settings window: the user picks speech and cleanup models, a language, and starts capture. No model path in the environment. Folder number stays 11; the row is first because the window is the next thing to write. |
| 00 | [local](00-local/README.md) | After 11. Confirm missing-model, logs, language lock, and latency through that window and `--dev`. |
| 01 | [capture](01-capture/README.md) | Rate plumbing and callback safety. Everything downstream depends on correct samples. Per-OS manual runs wait until the first-run flow in 11 exists. |
| 02 | [vad](02-vad/README.md) | Cut quality decides subtitle quality. |
| 03 | [stt](03-stt/README.md) | Correctness bugs in the CLI path, then in-process whisper. |
| 04 | [cleanup](04-cleanup/README.md) | Latency cap is unreachable with `llama-cli` cold starts. |
| 05 | [pipeline](05-pipeline/README.md) | Cue id and ordering bugs. Split into testable stages. |
| 06 | [config](06-config/README.md) | One config struct replaces scattered env reads. |
| 07 | [overlay](07-overlay/README.md) | Subtitle window. First slice landed. Finish edit mode and multi-monitor after 11. |
| 08 | [browser-extension](08-browser-extension/README.md) | Third subscriber over localhost. |
| 09 | [testing-ci](09-testing-ci/README.md) | Fixtures, benches, CI matrix. |
| 10 | [packaging](10-packaging/README.md) | Release builds. Model download lives in 11. |

## Build flow

1. Write 11 first: engines, catalogue, matcher, downloader, settings window, language control, `--dev` logs, first-run. The user picks models in that window.
2. Then run [00](00-local/README.md) against that window. Then the rest of 07 (edit mode, multi-monitor). Prompt carry in the window comes after the settings window works.
3. Other manual runs come after the feature they check is implemented. Do not ask for a manual check, or suggest one as the next step, before that.

## Reporting remaining work

When asked "what spec is remaining", "what is left" or similar, and before asking the user for any manual check, list the remaining work first:

1. Run `grep -c '^- \[ \]' specs/*/README.md` (open) and `grep -c '^- \[x\]' specs/*/README.md` (done). Checkboxes are the source of truth; do not answer from memory.
2. Report per spec: open / total, then the open item names. Mark which are code still to write and which are only a manual run.
3. Ask for a manual check only when all requested code is done (see Build flow), and say which boxes it would tick.

A box that landed in code but still needs a manual run stays unticked, with the pending run named in the spec.

## Known bugs found while writing these

Fixed in #2, do not re-open:

- The audio callback owns the ring producer by move. No `Mutex`. (01)

Fixed in #1, do not re-open:

- `SUBS_RATE` is gone. The pipeline takes the capture rate. (01, 05)
- Partial and final share an utterance id. (05)
- A late partial is dropped once the utterance has ended. The check-then-send window is documented in `pipeline.rs`. (05)

Fixed with 03 slice (unique temp WAV, kill on timeout), do not re-open:

- Temp WAV name is `subs-<pid>-<seq>.wav`, removed on every path. (03)
- `whisper-cli` is killed when the STT timeout drops its future. (03)

Fixed with 04 slice (non-interactive llama, kill on timeout), do not re-open:

- The cleanup binary is `llama-completion -no-cnv`, so it exits after the answer instead of waiting at `>`. (04)
- The `llama` child is killed when the cleanup timeout drops its future. (04)
