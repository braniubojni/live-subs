# 02 VAD

## Goal
Cut audio into utterances that start on the first word and end on a natural pause. Few false triggers on music and noise.

## Current state
[src/vad.rs](../../src/vad.rs): RMS gate, fixed threshold 0.015, 500 ms silence, 250 ms min speech, 8 s hard cut. `Segmenter::push` returns `Speaking` / `Ended`. A blip shorter than `min_speech_ms` is cleared when silence passes the limit, so it cannot merge into the next burst. `voiced_samples` tracks speech separately from the silence tail. `preroll_ms` (200) of audio before the gate opens is kept and prepended. Sizes are `usize` throughout.

## Todo
- [x] **Pre-roll.** Keep the last ~200 ms of audio before the gate opens and prepend it. Accept: a test where the first 100 ms is below threshold still contains it in `Ended.speech`.
- [x] **Short blip really dropped.** A blip below `min_speech_ms` used to stay in `speech` with `in_speech` true, so the next burst merged with it. Silence past the limit without enough voiced samples now resets state. Accept: test with blip, 1 s silence, real speech yields one utterance without the blip.
- [ ] **Hangover and hysteresis.** Open threshold above close threshold, so quiet word endings do not flap. Accept: config has `open` and `close`; test with an amplitude ramp.
- [ ] **Adaptive threshold.** Track a noise floor (slow EMA over non-voiced frames) and gate at floor times a ratio, with the fixed value as a minimum. Accept: constant 0.01 hum does not trigger; speech at 0.05 does.
- [ ] **Cut at the quietest point.** When the 8 s cap hits, search the last 1 s for the lowest-RMS 20 ms window and cut there; carry the remainder into the next utterance. Accept: test where the cut lands in a planted gap.
- [x] **Sample-rate-aware frames.** `samples()` and `silence_samples: u32` use mixed widths. Use `usize` throughout. Accept: no `as u32` in `vad.rs`.
- [ ] **`Vad` trait.** Extract `trait Vad { fn push(&mut self, frame:&[f32]) -> Option<VadEvent>; fn speech(&self)->&[f32]; }` so Silero slots in. Pipeline takes `impl Vad`. Accept: pipeline test uses a fake Vad.
- [ ] **Silero VAD.** Behind a cargo feature `silero`, via `ort` or a pure-Rust ONNX runtime. Model file in `models/`. Frame size 512 at 16 kHz. Accept: same trait, same tests pass with both; a recorded fixture (see 09) shows fewer false triggers.
- [ ] **Config from [06](../06-config/README.md).** Thresholds read from config, not `Default` only.

## Tests
Synthetic signals (constant, ramp, gap) as in the existing tests. Add one table-driven test over `VadConfig` edge values. Silero test gated on the model file existing (skip, not fail, when absent).

## Out of scope
Speaker diarization. Language detection.
