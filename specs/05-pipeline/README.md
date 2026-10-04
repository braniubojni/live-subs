# 05 Pipeline

## Goal
Resample, cut, transcribe, clean, emit `Cue`s with stable ids and strict per-utterance ordering. Testable end to end with fakes and no audio device.

## Current state
[src/pipeline.rs](../../src/pipeline.rs): one `worker` loop does drain, resample, VAD, partial scheduling, and final scheduling inline. Detached `tokio::spawn` tasks send cues.

## Bugs to fix first
- **Id mismatch.** Partial uses `cue_id = id` (current utterance index), the `Ended` path does `id += 1` then uses it. The partial for utterance 3 carries id 2... then the final carries id 3. Consumers cannot match partial to final. Assign the id when speech starts, use it for partials and the final.
- **Late partial overwrites final.** Partial and final tasks race. A slow partial can send after the final. Track per-utterance state; drop partials once the utterance ended (generation check before send).
- **Dropped on full channel.** `tx.send(..).await` in detached tasks can stall; `let _ =` hides closed channel. Decide policy: partials use `try_send` (droppable), finals use `send`.
- **Resampler error kills the loop.** `?` inside `loop` ends the pipeline with one log line. Reset and continue on a bad frame; stop only on fatal.

## Todo
- [ ] **Fix the four bugs above**, each with a failing test first.
- [ ] **Rate from capture** (see [01](../01-capture/README.md)). Remove `SUBS_RATE`.
- [ ] **Split `worker`.** Pure stages: `Resample`, `Vad`, `PartialScheduler` (decides when to fire, given elapsed and speech length), `Finalizer`. Accept: `worker` is a short loop wiring stages; scheduler is unit-tested with a fake clock.
- [ ] **Injectable clock.** `Instant::now()` behind a small trait or `tokio::time::pause` in tests. Accept: no real sleeps in pipeline tests.
- [ ] **Input as a trait.** `AudioSource { fn drain(&mut self, out:&mut Vec<f32>) }` so tests feed synthetic PCM. Accept: integration test runs speech/silence/speech through fake STT and fake cleaner and asserts the exact cue sequence.
- [ ] **Backpressure limits.** Cap in-flight finals (e.g. 3). If STT is slower than speech, drop oldest queued final's cleanup, never capture. Log lag. Accept: test with a slow fake STT.
- [ ] **Ordering of finals.** Finals emitted in utterance order even if utterance 4 finishes before 3 (reorder buffer keyed by id). Accept: test with inverted delays.
- [ ] **Cue schema.** Add `start_ms`, `end_ms` (relative to run start), `lang` when known, `raw` text on finals (for debugging). Bump nothing silently: add fields, document in [08](../08-browser-extension/README.md). Accept: serde snapshot test.
- [ ] **Allocation in the loop.** `pending.drain(..960).collect()` allocates per frame. Reuse a frame buffer. Accept: no `collect` in the hot loop.
- [ ] **Graceful shutdown.** Ctrl-C stops capture, flushes the current utterance as a final, then exits. Accept: manual check plus a test using a cancellation token.
- [ ] **Metrics.** Per-stage latency (vad end to STT done, STT to cleanup done) via `tracing` spans. Accept: one log line per final with both numbers.

## Tests
Everything via fakes in `tests/common/`: `FakeSource`, `FakeStt`, `FakeCleaner`, paused tokio time. Assertions on ordered `Vec<Cue>`.

## Out of scope
Multi-stream mixing. Persisting transcripts.
