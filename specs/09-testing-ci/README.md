# 09 Testing and CI

## Goal
Everything that can be checked without a microphone, models, or a GUI is checked in CI. Performance claims have numbers.

## Current state
[.github/workflows/ci.yml](../../.github/workflows/ci.yml) runs fmt, clippy, test. Tests: unit tests in `vad.rs` and `cleanup.rs`, [tests/segments.rs](../../tests/segments.rs). No fixtures, no benches.

## Todo
- [ ] **Shared test helpers.** `tests/common/mod.rs`: `FakeSource`, `FakeStt`, `FakeCleaner`, synthetic speech generator (tone bursts with silence). Accept: used by at least vad, pipeline, and server tests.
- [ ] **WAV fixtures.** Small (under 200 KB total) 16 kHz clips in `tests/fixtures/`: one sentence, two sentences with a pause, noise only, music-like tone. Document the source and licence in a `README` beside them.
- [ ] **Golden cue sequences.** Pipeline integration test feeds a fixture through the real VAD with fake STT and asserts the `Vec<Cue>` (ids, kinds, order). Accept: fails if ids regress.
- [ ] **Property tests.** `proptest` on the segmenter: no sample lost or duplicated across `Ended` events; speech never exceeds the max by more than one frame.
- [ ] **Benches.** `benches/vad.rs` and `benches/resample.rs` with `criterion`: ns per second of audio. Accept: numbers in this folder's `RESULTS.md`; the audio path must stay far under real time.
- [ ] **Manual latency script.** `examples/latency.rs` runs a fixture through real `whisper-cli` / `llama-cli` and prints per-stage times. Manual, never in CI.
- [ ] **CI matrix.** Linux, macOS, Windows for fmt/clippy/test. Linux needs `libasound2-dev`. Accept: `cpal` compiles on all three.
- [ ] **Feature builds.** Jobs for `--features overlay`, `server`, `silero`, `whisper-rs` as they land (compile and clippy only).
- [ ] **Caching and MSRV.** `Swatinem/rust-cache`; pin MSRV in `Cargo.toml` and test it once.
- [ ] **Supply chain.** `cargo deny` or `cargo audit` job. Accept: weekly schedule plus on PR.
- [ ] **Coverage guard.** Optional `cargo llvm-cov` report on the pure modules; no hard gate.

## Tests
This folder is the tests. Keep helper code small; do not build a framework.

## Out of scope
Hardware-in-the-loop capture tests.
