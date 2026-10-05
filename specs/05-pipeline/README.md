# 05 Pipeline

## Goal
Resample, cut, transcribe, clean, emit `Cue`s with stable ids and strict per-utterance ordering. Testable end to end with fakes and no audio device.

## Current state
[src/pipeline.rs](../../src/pipeline.rs) is split into four stages that `worker` wires in a short loop: drain an `AudioSource`, then per 960-sample frame `Resample` -> `Vad` -> `PartialScheduler` / `Finalizer`.

- `Resample`: device rate to 16 kHz. On error it resets and returns `None`; the frame is skipped.
- `Vad`: thin wrapper over `Segmenter`.
- `PartialScheduler`: decides when a partial may start (900 ms since the last partial or final, more than 0.5 s of speech, none already running). It owns the busy flag and reads time only through the `Clock` trait. It spawns nothing.
- `Finalizer`: owns the utterance id and the `ended` counter, and spawns the detached partial and final tasks that send cues. It also keeps the list of finals still eligible for cleanup (`MAX_LIVE_FINALS` = 3). When a fourth utterance ends, the oldest eligible final is told to skip cleanup and sends its raw STT line as the final, with a `warn` log of the shed id. Capture never waits on this.

`AudioSource` is a one-method trait (`drain`, non-blocking append of ready samples). `AudioRx` implements it; `run` and `worker` take any `AudioSource`, so tests feed synthetic PCM through a fake.

`Clock` is a one-method trait. `TokioClock` uses `tokio::time::Instant`, so it follows `tokio::time::pause`. `run` always uses `TokioClock`; only the scheduler unit tests inject a fake.

Landed in #1:

- `run` takes `in_rate` from capture. No `SUBS_RATE`.
- Id is assigned for the utterance and reused for its partial and final, then incremented.
- `ended` drops a partial whose utterance has already closed. Check-then-send is not atomic; noted in the source.
- Partials use `try_send`. Finals use `send`. A closed channel is logged.
- A resample error resets the resampler and skips the frame. That path has no test.

Fakes (including `FakeSource` and `CountingStt`) live inline in [tests/pipeline.rs](../../tests/pipeline.rs), not in `tests/common/`.

## Bugs to fix first
Fixed in #1. Kept here so the tests stay the regression net.

- **Id mismatch.** Partial and final used different ids. Id is now taken, then incremented on `Ended`.
- **Late partial overwrites final.** Partial task checks `ended` before send.
- **Dropped on full channel.** Partials `try_send`, finals `send`.
- **Resampler error kills the loop.** Error resets and continues. Still untested.

## Todo
- [x] **Fix the four bugs above**, each with a failing test first. (Tests in `tests/pipeline.rs`; the channel-policy test passed before the fix and guards regression. Resampler-reset path has no test.)
- [x] **Rate from capture** (see [01](../01-capture/README.md)). Remove `SUBS_RATE`.
- [x] **Split `worker`.** Pure stages: `Resample`, `Vad`, `PartialScheduler` (decides when to fire, given elapsed and speech length), `Finalizer`. Accept: `worker` is a short loop wiring stages; scheduler is unit-tested with a fake clock.
- [x] **Injectable clock.** `Instant::now()` behind a small trait or `tokio::time::pause` in tests. Accept: no real sleeps in pipeline tests. (Scheduler tests use a fake `Clock`; integration tests use `#[tokio::test(start_paused = true)]`, which needs tokio `test-util` as a dev-dependency.)
- [x] **Input as a trait.** `AudioSource { fn drain(&mut self, out:&mut Vec<f32>) }` so tests feed synthetic PCM. Accept: integration test runs speech/silence/speech through fake STT and fake cleaner and asserts the exact cue sequence. (`run` and `worker` are generic over `AudioSource`; `AudioRx` implements it. Test: `speech_silence_speech_yields_exact_cues`.)
- [x] **Backpressure limits.** Cap in-flight finals (e.g. 3). If STT is slower than speech, drop oldest queued final's cleanup, never capture. Log lag. Accept: test with a slow fake STT. (Cap is 3 finals eligible for cleanup. The 4th ender sheds the oldest: its final is the raw STT line, and a `warn` names the shed id. Shed tasks still finish STT, so STT tasks are not themselves capped. Test: `slow_stt_sheds_oldest_cleanups_and_never_blocks_capture`.)
- [ ] **Ordering of finals.** Finals emitted in utterance order even if utterance 4 finishes before 3 (reorder buffer keyed by id). Accept: test with inverted delays.
- [ ] **Cue schema.** Add `start_ms`, `end_ms` (relative to run start), `lang` when known, `raw` text on finals (for debugging). Bump nothing silently: add fields, document in [08](../08-browser-extension/README.md). Accept: serde snapshot test.
- [ ] **Allocation in the loop.** `pending.drain(..960).collect()` allocates per frame. Reuse a frame buffer. Accept: no `collect` in the hot loop.
- [ ] **Graceful shutdown.** Ctrl-C stops capture, flushes the current utterance as a final, then exits. Accept: manual check plus a test using a cancellation token.
- [ ] **Metrics.** Per-stage latency (vad end to STT done, STT to cleanup done) via `tracing` spans. Accept: one log line per final with both numbers.
- [ ] **Resampler reset test.** Feed a frame that makes `process` fail, assert the worker is still running and later frames still emit. Accept: one test, no device.

## Tests
Today: four `PartialScheduler` unit tests in `src/pipeline.rs` with a fake clock, and five tests in `tests/pipeline.rs`: three with inline `FakeStt` / `FakeCleaner` and a pre-filled ring, one with `FakeSource` that asserts the exact cue sequence for speech, silence, speech, and one with a slow STT that asserts the cleanup cap and that capture drains fully. Integration tests run on paused tokio time, so no real sleeps. Whisper and llama are never run.

Still to add: move the fakes into `tests/common/` once a second test file needs them.

## Out of scope
Multi-stream mixing. Persisting transcripts.
