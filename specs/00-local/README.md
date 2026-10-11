# 00 Local check

## Goal
Confirm the 03 behavior that this environment could not run: a real `whisper-cli`, a model file, and an in-process build. Tick each box in the PR after a local run. Record the latency numbers in this file.

## Current state
CI never starts `whisper-cli` or `llama-cli`. The agent environment had neither binary nor `models/ggml-tiny.bin`, so the checks below are open. Unit tests cover parsing, the hallucination list, the 0.8 lock rule, prompt plumbing, and the 4 s partial window without a model. See [03](../03-stt/README.md).

## Todo
- [ ] **Missing model exits.** Point `whisper_model` at a path that is not a file. Accept: `live-subs` exits non-zero and the message contains that path.
- [ ] **Real stdout.** Run `whisper-cli -nt -np` on a short clip. If its stdout has a log line or tag the table in `src/stt.rs` does not cover, add that line to the table test. Accept: the box is ticked only after comparing a real capture, or the test already matches it.
- [ ] **Language lock.** `language = "auto"`. Two utterances of clear speech. Accept: the first `whisper-cli` gets `-l auto`; once stderr shows `p` ≥ 0.8, the next call gets `-l <code>` instead. A wrapper `whisper_bin` that appends its argv to a file is enough to see this.
- [ ] **Prompt carry.** `prompt_carry = true`. Accept: the second utterance's argv contains `--prompt` and the previous final line. Same wrapper as above. With the flag false, `--prompt` is absent.
- [ ] **`whisper-rs` loads and transcribes.** `models/ggml-tiny.bin` present. `cargo test --features whisper-rs --lib load_skips_without_a_model_file` does not print `skip:`. Then one utterance through `cargo run --release --features whisper-rs` produces a line. Accept: the model is not loaded again for the second utterance (no second load log). If `cc` is clang and the build cannot find `<array>`, set `CXX=g++`.
- [ ] **Latency.** Same short fixture through `whisper-cli` and through `WhisperRs`. Accept: write both per-utterance times here. In-process is lower, and the in-process cold start is once at process start, not per utterance.

## Tests
None in CI. These need a microphone or a fixture plus the local binaries. Do not add a test that shells out to `whisper-cli`.

## Out of scope
Device selection and capture recovery. Those stay in [01](../01-capture/MANUAL.md). Model download and the settings window stay in [11](../11-models-and-setup/README.md).
