# 05 Pipeline

## Goal
Resample, cut, transcribe, clean, emit `Cue`s with stable ids and strict per-utterance ordering. Testable end to end with fakes and no audio device.

## Current state
[src/pipeline.rs](../../src/pipeline.rs) is still one `worker` loop: drain, resample, VAD, partial scheduling, final scheduling. Detached `tokio::spawn` tasks send cues.

Landed in #1:

- `run` takes `in_rate` from capture. No `SUBS_RATE`.
- Id is assigned for the utterance and reused for its partial and final, then incremented.
- `ended` drops a partial whose utterance has already closed. Check-then-send is not atomic; noted in the source.
- Partials use `try_send`. Finals use `send`. A closed channel is logged.
- A resample error resets the resampler and skips the frame. That path has no test.

Fakes live inline in [tests/pipeline.rs](../../tests/pipeline.rs), not in `tests/common/`. The clock is `Instant::now()`.

## Bugs to fix first
Fixed in #1. Kept here so the tests stay the regression net.

- **Id mismatch.** Partial and final used different ids. Id is now taken, then incremented on `Ended`.
- **Late partial overwrites final.** Partial task checks `ended` before send.
- **Dropped on full channel.** Partials `try_send`, finals `send`.
- **Resampler error kills the loop.** Error resets and continues. Still untested.

## Todo
- [x] **Fix the four bugs above**, each with a failing test first. (Tests in `tests/pipeline.rs`; the channel-policy test passed before the fix and guards regression. Resampler-reset path has no test.)
- [x] **Rate from capture** (see [01](../01-capture/README.md)). Remove `SUBS_RATE`.
- [ ] **Split `worker`.** Pure stages: `Resample`, `Vad`, `PartialScheduler` (decides when to fire, given elapsed and speech length), `Finalizer`. Accept: `worker` is a short loop wiring stages; scheduler is unit-tested with a fake clock.
- [ ] **Injectable clock.** `Instant::now()` behind a small trait or `tokio::time::pause` in tests. Accept: no real sleeps in pipeline tests.
- [ ] **Input as a trait.** `AudioSource { fn drain(&mut self, out:&mut Vec<f32>) }` so tests feed synthetic PCM. Accept: integration test runs speech/silence/speech through fake STT and fake cleaner and asserts the exact cue sequence.
- [ ] **Backpressure limits.** Cap in-flight finals (e.g. 3). If STT is slower than speech, drop oldest queued final's cleanup, never capture. Log lag. Accept: test with a slow fake STT.
- [ ] **Ordering of finals.** Finals emitted in utterance order even if utterance 4 finishes before 3 (reorder buffer keyed by id). Accept: test with inverted delays.
- [ ] **Cue schema.** Add `start_ms`, `end_ms` (relative to run start), `lang` when known, `raw` text on finals (for debugging). Bump nothing silently: add fields, document in [08](../08-browser-extension/README.md). Accept: serde snapshot test.
- [ ] **Allocation in the loop.** `pending.drain(..960).collect()` allocates per frame. Reuse a frame buffer. Accept: no `collect` in the hot loop.
- [ ] **Graceful shutdown.** Ctrl-C stops capture, flushes the current utterance as a final, then exits. Accept: manual check plus a test using a cancellation token.
- [ ] **Metrics.** Per-stage latency (vad end to STT done, STT to cleanup done) via `tracing` spans. Accept: one log line per final with both numbers.
- [ ] **Resampler reset test.** Feed a frame that makes `process` fail, assert the worker is still running and later frames still emit. Accept: one test, no device.

## Tests
Today: three tests in `tests/pipeline.rs` with inline `FakeStt` / `FakeCleaner` and a pre-filled ring. They use real sleeps.

Still to add, once the stages split: `tests/common/` with `FakeSource`, `FakeStt`, `FakeCleaner`, paused tokio time. Assertions on an ordered `Vec<Cue>`.

## Out of scope
Multi-stream mixing. Persisting transcripts.
