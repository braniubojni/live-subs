# 03 STT

## Goal
Low-latency, accurate transcription behind the `Transcriber` trait, first via `whisper-cli`, then in-process.

## Current state
[src/stt.rs](../../src/stt.rs): writes a 16-bit WAV to the temp dir, runs `whisper-cli -nt -np -l auto -t 4`, filters stdout lines. Partials re-transcribe the whole growing utterance every 0.9 s.

## Todo
- [ ] **Unique temp file.** Name by `pid + atomic counter`, not millis. Use `tempfile` or `O_EXCL`. Always delete, including on error and cancel (guard struct with `Drop`). Accept: 1000 concurrent calls in a test produce 1000 distinct paths.
- [ ] **Kill on timeout.** `Command::kill_on_drop(true)`. `with_timeout` must not leave an orphan `whisper-cli`. Accept: manual check with a `sleep` stub binary.
- [ ] **Non-UTF-8 paths.** `to_str().unwrap_or("")` silently passes an empty arg. Use `.arg(&path)` with `OsStr`. Accept: no `unwrap_or("")` in `stt.rs`.
- [ ] **Output parsing as a pure fn.** `parse_whisper_stdout(&str) -> String`. Strip log lines, bracketed tags (`[BLANK_AUDIO]`, `[MUSIC]`, `(applause)`). Accept: table test of real captured outputs.
- [ ] **Hallucination filter.** Drop known silence hallucinations ("Thank you.", "Thanks for watching", repeated token loops). Accept: pure fn with table test; list lives in one const.
- [ ] **Language.** Replace `-l auto` with config (see 06). Auto-detect costs time per call; allow fixing the language, and lock it after the first confident detection.
- [ ] **Initial prompt carry-over.** Pass the previous final line as `--prompt` for continuity and name spelling. Accept: flag wired, off by default, switchable in config.
- [ ] **Partial cost.** Partials re-run the full utterance. Options, pick one and measure: (a) cap the partial window to the last 4 s, (b) skip a partial when audio grew less than 300 ms. Accept: bench number in `benches/` (see 09).
- [ ] **`WhisperRs` implementation.** Behind feature `whisper-rs`. Load the model once, reuse state, run on a blocking thread (`spawn_blocking`) so the runtime stays free. Same trait. Accept: cold-start gone; per-utterance latency lower than CLI on the same fixture, recorded in the PR.
- [ ] **Trait stays stable.** If streaming partials need a different shape, add a second method with a default impl rather than changing `transcribe`.
- [ ] **Model check at startup.** Fail fast with a clear message if the bin or model path is missing, instead of silent empty cues. Accept: `live-subs` exits non-zero with the path it looked for.

## Tests
Parsing and hallucination filter are pure and fully tested. A fake `Transcriber` (returns canned text, optional delay) lives in `tests/common/` for pipeline tests (05). No test runs `whisper-cli`.

## Out of scope
Fine-tuning. Translation. Word-level timestamps (revisit if the overlay needs karaoke highlighting).
