# 03 STT

## Goal
Low-latency, accurate transcription behind the `Transcriber` trait, first via `whisper-cli`, then in-process.

## Current state
[src/stt.rs](../../src/stt.rs): writes a 16-bit WAV named `subs-<pid>-<seq>.wav` in the temp dir (atomic counter, created with `create_new`). A `TempWav` guard deletes it on success, error, and cancel. Runs `whisper-cli -nt -np -t 4` with `kill_on_drop(true)`, so a timeout kills the child. `-l` is the config language (`auto` by default). After stderr reports `auto-detected language: … (p = …)` at 0.8 or higher, later calls use that code. A configured language is never overwritten. Model and WAV paths are passed as `OsStr`. Stdout goes through `parse_whisper_stdout` (drops `whisper_*` lines and `[…]` / `(…)` tags) and `filter_hallucination` (one const list, plus repeated-token loops).

`prompt_carry` (default false, [live-subs.example.toml](../../live-subs.example.toml)) passes the previous emitted final as `--prompt`. `transcribe` is unchanged; `transcribe_with_prompt` and `language` have default impls.

Partials send only the last 4 s (`partial_window`, 64_000 samples). Finals still get the whole utterance. Copying that tail is about half the copy of an 8 s buffer: criterion in [benches/partial.rs](../../benches/partial.rs), 20 samples, 2 s measurement, `copy_full_8s` 9.97 µs, `copy_tail_4s` 4.72 µs.

`--features whisper-rs` builds `WhisperRs`: one `WhisperContext` at `load`, state reused, `spawn_blocking` for `full`. The default CI job does not compile it (whisper.cpp). Real `whisper-cli` output, the language lock, `--prompt`, and the in-process latency comparison need a local binary and model. Those boxes are [00](../00-local/README.md).

Missing bin or model: `Config::check_files` in [src/main.rs](../../src/main.rs) returns `file not found: <path>` before capture, and the process exits non-zero. With `whisper-rs`, the CLI binary is not required.

## Todo
- [x] **Unique temp file.** Name by `pid + atomic counter`, not millis. Use `tempfile` or `O_EXCL`. Always delete, including on error and cancel (guard struct with `Drop`). Accept: 1000 concurrent calls in a test produce 1000 distinct paths.
- [x] **Kill on timeout.** `Command::kill_on_drop(true)`. `with_timeout` must not leave an orphan `whisper-cli`. Accept: manual check with a `sleep` stub binary.
- [x] **Non-UTF-8 paths.** `to_str().unwrap_or("")` silently passes an empty arg. Use `.arg(&path)` with `OsStr`. Accept: no `unwrap_or("")` in `stt.rs`.
- [x] **Output parsing as a pure fn.** `parse_whisper_stdout(&str) -> String`. Strip log lines, bracketed tags (`[BLANK_AUDIO]`, `[MUSIC]`, `(applause)`). Accept: table test of real captured outputs.
- [x] **Hallucination filter.** Drop known silence hallucinations ("Thank you.", "Thanks for watching", repeated token loops). Accept: pure fn with table test; list lives in one const.
- [x] **Language.** Replace `-l auto` with config (see 06). Auto-detect costs time per call; allow fixing the language, and lock it after the first confident detection. Lock threshold is 0.8. `Cue.lang` is set from `Transcriber::language` once a code is known.
- [x] **Initial prompt carry-over.** Pass the previous final line as `--prompt` for continuity and name spelling. Accept: flag wired, off by default, switchable in config. Key: `prompt_carry`.
- [x] **Partial cost.** Partials re-run the full utterance. Options, pick one and measure: (a) cap the partial window to the last 4 s, (b) skip a partial when audio grew less than 300 ms. Picked (a): a 900 ms cadence already grows by more than 300 ms, and (b) would still transcribe up to 8 s. Numbers above.
- [x] **`WhisperRs` implementation.** Behind feature `whisper-rs`. Load the model once, reuse state, run on a blocking thread (`spawn_blocking`) so the runtime stays free. Same trait. Accept: cold-start gone; per-utterance latency lower than CLI on the same fixture, recorded in the PR. Cold start is the single `load`. The load test skips when the model is absent. Timing against `whisper-cli` is the open latency box in [00](../00-local/README.md).
- [x] **Trait stays stable.** If streaming partials need a different shape, add a second method with a default impl rather than changing `transcribe`. Added `transcribe_with_prompt` and `language`, both with defaults. `transcribe(&[f32])` is unchanged.
- [x] **Model check at startup.** Fail fast with a clear message if the bin or model path is missing, instead of silent empty cues. Accept: `live-subs` exits non-zero with the path it looked for. `check_files` already did this; the missing-file test now asserts the path is in the error.

## Tests
Parsing, the hallucination filter, the language lock, and the partial window are unit-tested in `src/stt.rs` and `src/pipeline.rs`. No test runs `whisper-cli`. The `whisper-rs` load test skips when `models/ggml-tiny.bin` is absent.

## Out of scope
Fine-tuning. Translation. Word-level timestamps (revisit if the overlay needs karaoke highlighting).
