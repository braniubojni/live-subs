# live-subs

Local live subtitles. System audio goes in, a subtitle line comes out. Speech-to-text is [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Cleanup is a tiny [llama.cpp](https://github.com/ggml-org/llama.cpp) model. Neither runs on the audio thread.

The binary is a thin subscriber. The library crate is what tests and the later overlay import.

## Package layout

Follow the [Cargo package layout](https://doc.rust-lang.org/cargo/guide/project-layout.html). Do not invent a second tree.

```
.
├── Cargo.toml
├── Cargo.lock
├── rustfmt.toml
├── rust-toolchain.toml
├── src/
│   ├── lib.rs            # library, public API
│   ├── main.rs           # binary only
│   ├── audio.rs
│   ├── vad.rs
│   ├── stt.rs
│   ├── cleanup.rs
│   └── pipeline.rs
├── tests/                # integration tests, library crate only
│   └── segments.rs
├── examples/             # add when a snippet is worth running alone
├── benches/              # add when a hot path needs a number
└── .github/workflows/ci.yml
```

`src/lib.rs` owns the modules. `src/main.rs` does not declare them again. Integration tests live in `tests/` and depend on `live_subs`, never on the binary. Extra executables go in `src/bin/`. Names of binaries, tests, and examples are kebab-case. Module names are snake_case.

## Pipeline

```
cpal callback --ringbuf--> resample 16 kHz --> energy VAD
                                                 | silence ~500 ms, or 8 s max
                                                 v
                                          whisper-cli (partial every 0.9 s)
                                                 |
                                                 v
                                          llama-completion, 700 ms cap, temp 0
                                                 |
                                                 v
                                          Cue { partial | final }
```

Partials are the raw Whisper line. The final cue replaces that line after cleanup. Cleanup never blocks the next utterance. If `llama-completion` is missing, slow, or returns a line more than twice as long as the source, the raw line stays.

## Requirements

- Rust stable, with `rustfmt` and `clippy` (`rust-toolchain.toml` installs both)
- A built `whisper-cli` from whisper.cpp
- A built `llama-completion` from llama.cpp (pinned to `b11395`)
- macOS 14.6+ for output-device loopback, Windows WASAPI loopback, or a Linux monitor source

## Setup

```bash
git clone git@github.com:braniubojni/live-subs.git
cd live-subs

mkdir -p models
curl -L -o models/ggml-tiny.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
curl -L -o models/cleanup-q4.gguf \
  https://huggingface.co/superwhisper/s1-mini-GGUF/resolve/main/s1-mini-q4_k_m.gguf
```

Build the CLIs once:

```bash
git clone https://github.com/ggml-org/whisper.cpp
cmake -S whisper.cpp -B whisper.cpp/build
cmake --build whisper.cpp/build -j --target whisper-cli

git clone https://github.com/ggml-org/llama.cpp
git -C llama.cpp checkout 7f2dd88b0ac393357ae6a9e1992185a48c20b13b   # b11395
cmake -S llama.cpp -B llama.cpp/build
cmake --build llama.cpp/build -j --target llama-completion --target llama-server
```

## Run

```bash
cp live-subs.example.toml live-subs.toml   # edit paths if yours differ
cargo run --release
```

Config precedence, later wins: defaults, `live-subs.toml` (current directory, then the user config dir; or `--config <path>`), env vars, CLI flags. Env names `WHISPER_BIN`, `WHISPER_MODEL`, `LLAMA_BIN`, `LLAMA_MODEL`, `LLAMA_SERVER`, `SUBS_DEVICE` still work, also from a `.env` file. `--print-config` shows the effective result; see `--help` for flags.

Set `llama_server` to the `llama-server` binary to keep the cleanup model loaded between lines. Unset, each line spawns `llama-completion` and reloads the model.

macOS asks for system-audio recording permission. Grant it to the terminal you launch from.

Device selection: `cargo run -- --list-devices` prints `in`/`out`, a `*` on the default, name, rate and channels. `device` (or `--device`, or `SUBS_DEVICE`) is an exact name from that list, input or output. Unset, the default output device is used.

- macOS: the default output device works as is. To pick another, set `device` to an `out` name.
- Windows: the WASAPI output device name (`out`).
- Linux: the `.monitor` source name. `--list-devices` shows it, or use `pactl list sources short`.

`ggml-tiny` is the right first model. Move to `ggml-base` only if names and numbers are wrong. Do not put a 7B chat model on the cleanup step. It will miss the 700 ms cap.

## Quality bar

CI on `main` and on every pull request runs three commands. All three must pass. A change that does not pass locally will not pass there.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --features overlay,silero -- -D warnings
cargo test --all-targets --features overlay,silero
```

`--features whisper-rs` compiles whisper.cpp into the binary and is not part of this job.

Format before committing: `cargo fmt --all`. `rustfmt.toml` is the only style config. Do not hand-format around it.

Lint is Clippy with warnings denied, plus the crate lints in `Cargo.toml`: `unsafe_code` is forbidden, `dbg!` / `todo!` / `unimplemented!` are denied. No `unsafe` in this crate. The audio callback stays allocation-free and must not call a model.

Tests are required for behavior you can run without a microphone:

- Unit tests sit next to the code, behind `#[cfg(test)]`.
- Integration tests sit in `tests/` and use the public library API.
- A bug fix needs a test that failed before the fix.
- A new cut rule, cleanup rule, or cue field needs a test.
- Device capture is not a CI test. There is no audio device on the runner.

`cargo test` is the check. Do not add a test that shells out to `whisper-cli` or `llama-cli`. Those belong in a manual run.

## Layout of the code

| File | Job |
| --- | --- |
| `src/lib.rs` | Crate root. Modules live here so tests can use them. |
| `src/main.rs` | Binary. Starts capture and prints cues. |
| `src/audio.rs` | Loopback capture. Callback only downmixes and pushes `f32`. |
| `src/vad.rs` | RMS gate. Swap for Silero later. The segmenter API stays. |
| `src/stt.rs` | `whisper-cli` behind a `Transcriber` trait. |
| `src/cleanup.rs` | `LlamaCli` (`llama-completion -no-cnv` per line) and `LlamaServer` (warm `llama-server`, `/completion`), temp 0, 512 context, 64 tokens. |
| `src/pipeline.rs` | Resample, cut, emit cues. Capture never awaits a model. |
| `tests/segments.rs` | Integration coverage of the VAD cut and the cleanup guard. |

## Next

- In-process `whisper-rs` once the CLI path feels right. Same `Transcriber` trait.
- Silero VAD instead of the RMS gate.
- Click-through overlay (`eframe` or Tauri) subscribed to `Cue`.
- Browser extension as a third subscriber on `127.0.0.1`. The extension draws the line. It does not run either model.
