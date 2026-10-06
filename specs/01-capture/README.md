# 01 Capture

## Goal
Mono f32 samples at a known rate reach the pipeline from system audio on macOS, Windows, and Linux. The callback never blocks, allocates, or loses the rate.

## Current state
[src/audio.rs](../../src/audio.rs) opens the default output device as an input stream. The cpal callback owns the `HeapRb` producer by move, with no lock. It downmixes into a 512-frame stack buffer and does `push_slice`; samples that do not fit are added to an `AtomicU64` shared with `AudioRx`. `AudioRx` owns the consumer, `drain(&mut self)` logs a warning at most once per second when the drop count grows, and `AudioRx::dropped()` returns the total. `Capture` exposes `sample_rate`. [src/main.rs](../../src/main.rs) passes that rate into `pipeline::run`. There is no `SUBS_RATE`.

`i16` and `u16` scale by 2^15, so `u16` 32768 is silence. `I32` scales by 2^31 and `F64` casts to `f32`. Other `SampleFormat`s still error.

`SUBS_DEVICE` still matches input devices only.

## Todo
- [x] **Pass the rate through.** `pipeline::run` takes `sample_rate: u32` (or `Capture` hands over an `AudioRx` that knows its rate). Delete the `SUBS_RATE` read in [src/pipeline.rs](../../src/pipeline.rs) and the README export line. Accept: wrong-rate garbling is impossible without editing code.
- [x] **Remove the mutex from the callback.** Own the producer inside the closure (`move`), own the consumer in `AudioRx` without a lock (`&mut self` drain, or a single reader task). Accept: no `Mutex` in `audio.rs`; no `expect` on lock in the callback.
- [x] **Drop counter.** `try_push` failures increment an `AtomicU64`. Expose `AudioRx::dropped()`. Log once per second when it grows. Accept: overrun is visible.
- [x] **Bulk push.** Downmix into a fixed stack/preallocated buffer and `push_slice` instead of per-sample `try_push`. Accept: no per-sample call; callback stays allocation-free.
- [x] **Sample conversion.** Verify `I16` / `U16` mapping (`u16` midpoint is 32768, current math is off by half an LSB). Add `I32`, `F64` arms or document why unsupported. Accept: unit-tested pure conversion fns.
- [ ] **Device selection.** `SUBS_DEVICE` matches input devices only; on macOS the loopback device is an output device. Search outputs too. Add `--list-devices` that prints name, rate, channels. Accept: documented per-OS selection that works.
- [ ] **Stream error recovery.** On the cpal error callback, signal the pipeline (channel or flag). Reopen the default device on device change (headphones unplugged). Accept: capture resumes within 2 s of a default-device change, manual run.
- [ ] **Permission failure message.** macOS without permission yields silence, not an error. Detect N seconds of exact zeros after start and log the fix (System Settings path). Accept: one actionable warning.
- [ ] **Linux monitor source.** Auto-pick the `.monitor` of the default sink when `SUBS_DEVICE` unset. Accept: works on PipeWire and Pulse without config.

## Tests
- Pure fns: downmix, `i16`/`u16` to `f32`, ring-full counting. Put them in `audio.rs` `#[cfg(test)]`, no device.
- Device open is a manual run, not CI.

## Out of scope
Per-application capture. Echo cancellation. Microphone input.
