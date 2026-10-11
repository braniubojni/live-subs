# 00 Local check

## Goal
After the settings window in [11](../11-models-and-setup/README.md) exists, confirm the speech path on a real machine. The user picks models and language in that window. `--dev` is how these runs are observed. Tick each box in the PR after that run. Record the latency numbers in this file.

## Current state
Do not run these yet. 11 is the next build. CI never starts a model. Unit tests cover parsing, the hallucination list, the 0.8 lock rule, prompt plumbing, and the 4 s partial window without a model. See [03](../03-stt/README.md).

## Todo
- [ ] **Missing model.** In the settings window, leave the speech model empty or point it at a file that is not there. Accept: the window stays open, the message names the model, capture does not start. Closing without a model exits non-zero.
- [ ] **`--dev` shows work.** Run `live-subs --dev`, pick a model, start. Accept: stderr has the window-opened line, the model line, `capture started`, and one line per cue. A log line or tag that `parse_whisper_stdout` in `src/stt.rs` does not cover gets added to that table test.
- [ ] **Language lock.** In the language field choose `Auto`. Two utterances of clear speech. Accept: `--dev` shows the first utterance as `auto`, then a lock line with probability ≥ 0.8, and the next utterance uses that code. Pick `English` and accept that detection does not replace it.
- [ ] **Prompt carry.** After the checkbox in 11. Accept: with the box on, the second utterance's `--dev` line includes the previous final text. With the box off, no hint.
- [ ] **In-process model loads once.** A catalogue model is on disk. One utterance through the window produces a line. Accept: `--dev` shows a single load line for the whole process, including a second utterance.
- [ ] **Latency.** Same short utterance through the window. Accept: write the per-utterance `stt_ms` here. The load line is once at start, not inside each utterance's time.

## Tests
None in CI. These need the settings window, a microphone or a fixture, and a downloaded model. Do not add a test that shells out to `whisper-cli`.

## Out of scope
Device selection and capture recovery. Those stay in [01](../01-capture/MANUAL.md). Building the window stays in [11](../11-models-and-setup/README.md).
