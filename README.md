# live-subs

Local live subtitles. System audio goes in, a subtitle line comes out. Speech-to-text is [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Cleanup is a tiny [llama.cpp](https://github.com/ggml-org/llama.cpp) model. Neither runs on the audio thread.

This is a v0 starter. The terminal is the subtitle surface. An overlay and a browser extension subscribe to the same `Cue` stream later.

## Pipeline

```
cpal callback --ringbuf--> resample 16 kHz --> energy VAD
                                                 | silence ~500 ms, or 8 s max
                                                 v
                                          whisper-cli (partial every 0.9 s)
                                                 |
                                                 v
                                          llama-cli, 700 ms cap, temp 0
                                                 |
                                                 v
                                          Cue { partial | final }
```

Partials are the raw Whisper line. The final cue replaces that line after cleanup. Cleanup never blocks the next utterance. If `llama-cli` is missing, slow, or returns a line more than twice as long as the source, the raw line stays.

## Requirements

- Rust 1.85+
- A built `whisper-cli` from whisper.cpp
- A built `llama-cli` from llama.cpp
- macOS 14.6+ for output-device loopback, Windows WASAPI loopback, or a Linux monitor source

## Setup

```bash
git clone git@github.com:braniubojni/live-subs.git
cd live-subs

mkdir -p models
# whisper tiny, multilingual, ~75 MB
curl -L -o models/ggml-tiny.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin

# transcript cleanup, ~400 MB Q4. A fine-tune beats a generic chat model.
curl -L -o models/cleanup-q4.gguf \
  https://huggingface.co/superwhisper/s1-mini-GGUF/resolve/main/s1-mini-q4_k_m.gguf
```

Build the CLIs once:

```bash
# whisper.cpp
git clone https://github.com/ggml-org/whisper.cpp
cmake -S whisper.cpp -B whisper.cpp/build
cmake --build whisper.cpp/build -j --target whisper-cli

# llama.cpp
git clone https://github.com/ggml-org/llama.cpp
cmake -S llama.cpp -B llama.cpp/build
cmake --build llama.cpp/build -j --target llama-cli
```

## Run

```bash
export WHISPER_BIN=$PWD/whisper.cpp/build/bin/whisper-cli
export WHISPER_MODEL=$PWD/models/ggml-tiny.bin
export LLAMA_BIN=$PWD/llama.cpp/build/bin/llama-cli
export LLAMA_MODEL=$PWD/models/cleanup-q4.gguf
export SUBS_RATE=48000   # use the rate printed on startup

cargo run --release
```

macOS asks for system-audio recording permission. Grant it to the terminal you launch from. On Linux, list monitor sources with `pactl list sources short` and set `SUBS_DEVICE` to that name.

`ggml-tiny` is the right first model. Move to `ggml-base` only if names and numbers are wrong. Do not put a 7B chat model on the cleanup step. It will miss the 700 ms cap.

## Layout

| File | Job |
| --- | --- |
| `src/audio.rs` | Loopback capture. Callback only downmixes and pushes `f32`. |
| `src/vad.rs` | RMS gate. Swap for Silero later. The segmenter API stays. |
| `src/stt.rs` | `whisper-cli` behind a `Transcriber` trait. |
| `src/cleanup.rs` | `llama-cli`, temp 0, 512 context, 64 tokens. |
| `src/pipeline.rs` | Resample, cut, emit cues. Capture never awaits a model. |

## Next

- In-process `whisper-rs` once the CLI path feels right. Same `Transcriber` trait.
- Silero VAD instead of the RMS gate.
- Click-through overlay (`eframe` or Tauri) subscribed to `Cue`.
- Browser extension as a third subscriber on `127.0.0.1`. The extension draws the line. It does not run either model.
