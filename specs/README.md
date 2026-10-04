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
| 01 | [capture](01-capture/README.md) | Rate plumbing and callback safety. Everything downstream depends on correct samples. |
| 02 | [vad](02-vad/README.md) | Cut quality decides subtitle quality. |
| 03 | [stt](03-stt/README.md) | Correctness bugs in the CLI path, then in-process whisper. |
| 04 | [cleanup](04-cleanup/README.md) | Latency cap is unreachable with `llama-cli` cold starts. |
| 05 | [pipeline](05-pipeline/README.md) | Cue id and ordering bugs. Split into testable stages. |
| 06 | [config](06-config/README.md) | One config struct replaces scattered env reads. |
| 07 | [overlay](07-overlay/README.md) | First real UI. Subscribes to `Cue`. |
| 08 | [browser-extension](08-browser-extension/README.md) | Third subscriber over localhost. |
| 09 | [testing-ci](09-testing-ci/README.md) | Fixtures, benches, CI matrix. |
| 10 | [packaging](10-packaging/README.md) | Release builds and model download. |

## Known bugs found while writing these

Still open. Each is detailed in its folder.

1. `llama-cli` may enter interactive conversation mode and hang. The timeout drops the future but does not kill the child. (04)
2. Audio callback takes a `Mutex`. (01)

Fixed in #1, do not re-open:

- `SUBS_RATE` is gone. The pipeline takes the capture rate. (01, 05)
- Partial and final share an utterance id. (05)
- A late partial is dropped once the utterance has ended. The check-then-send window is documented in `pipeline.rs`. (05)

Fixed with 03 slice (unique temp WAV, kill on timeout), do not re-open:

- Temp WAV name is `subs-<pid>-<seq>.wav`, removed on every path. (03)
- `whisper-cli` is killed when the STT timeout drops its future. (03)
