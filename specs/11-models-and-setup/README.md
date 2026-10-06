# 11 Models and setup

## Goal
A user installs one binary, opens a small window, and gets subtitles. The app ships the whisper and llama engines inside the binary, recommends models that fit the machine, and downloads the chosen ones. No cmake, no `curl`, no `.env`. All models are free and run locally.

## Current state
Nothing ships. The README tells the user to build `whisper-cli` and `llama-completion` with cmake, `curl` two model files into `models/`, and set `WHISPER_*` / `LLAMA_*` env vars. [src/stt.rs](../../src/stt.rs) and [src/cleanup.rs](../../src/cleanup.rs) shell out to those binaries. There is no GUI, no hardware probe, no model catalogue.

This spec supersedes the packaging items "Remove external CLIs" and "`live-subs setup`" in [10](../10-packaging/README.md), and builds on [03](../03-stt/README.md), [04](../04-cleanup/README.md), [06](../06-config/README.md) and the toolkit decision in [07](../07-overlay/README.md). Do the manual per-OS runs in [01](../01-capture/README.md) after the first-run flow here lands, not before.

## Design
- **Engines in the binary.** `whisper-rs` and `llama-cpp-2` compile whisper.cpp and llama.cpp statically. Metal on macOS, CUDA/Vulkan feature flags elsewhere, CPU always. They sit behind the existing `Transcriber` and `Cleaner` traits; the CLI backends stay as an opt-in fallback.
- **Model catalogue.** `models.toml` embedded in the binary. Per entry: id, kind (`stt` or `cleanup`), display name, URL, size, sha256, licence, minimum RAM, languages, expected speed class.
- **Hardware matcher.** A pure function `recommend(&Hardware, &Catalogue, Kind) -> Vec<Ranked>`. `Hardware` is RAM, free disk, CPU cores, GPU kind (Metal, CUDA, Vulkan, none). Rule: the largest model whose RAM need fits within about 60% of total RAM, and whose speed class is real-time on this hardware. Each result carries a one-line reason.
- **Bring your own.** A model picker with autocomplete over the catalogue, plus a file browse button for a local `.bin` / `.gguf`, plus a Hugging Face search box (`https://huggingface.co/api/models?search=<q>&filter=gguf`) with a Download button. A user-picked model is validated before use (magic bytes, loadable), not trusted.
- **GUI.** One small settings window using the toolkit chosen in 07. Three controls per model kind: dropdown with the recommendation first, browse, download with progress and cancel. Plus device picker (from `--list-devices`), language, start/stop. Everything it sets is written to the config in 06.
- **First run.** No config means the window opens with the recommended models pre-selected. One click downloads both and starts capture.

## Todo
- [ ] **Decide engine crates.** Prototype `whisper-rs` and `llama-cpp-2` static builds on macOS, Windows, Linux. Record build time, binary size, and Metal/CUDA/Vulkan status in this file. Accept: a short table, and a pick.
- [ ] **`WhisperRs` backend.** Implements `Transcriber`, model loaded once, no temp WAV. Accept: same pipeline tests pass with a fake and with the real engine behind a model-file-exists skip.
- [ ] **In-process llama `Cleaner`.** Model loaded once, same 700 ms cap and 2x-length guard as today. Accept: guard tests unchanged.
- [ ] **Catalogue.** `models.toml` plus parser in `src/models/catalogue.rs`. At least: whisper tiny, base, small, medium (multilingual and `.en`) and two small cleanup GGUFs. Accept: parse test; every entry has sha256, size, licence.
- [ ] **Hardware probe.** `Hardware::detect()` using `sysinfo` plus a GPU check per OS. Accept: detect is a thin shim; everything downstream takes a `Hardware` value.
- [ ] **Matcher.** `recommend` as described. Accept: table test over fake hardware (8 GB no GPU, 16 GB Apple Silicon, 4 GB, 64 GB CUDA) giving expected top pick and reason, never a model above the RAM budget.
- [ ] **Downloader.** Resumable, progress callback, sha256 verify, cancel, atomic rename into the OS data dir. Accept: tested against a local HTTP responder: resume after a cut, corrupt file re-fetched, cancel leaves no partial file.
- [ ] **Hugging Face browse.** Search by text, list GGUF/bin files with sizes, pick one, download through the same downloader. Accept: parser tested on a recorded JSON response; network is behind a trait so tests never go online.
- [ ] **Local model import.** File browse, then validate (magic bytes, size sane, loads). Accept: bad file gives one error naming the problem.
- [ ] **Settings state as a pure struct.** `SettingsState::apply(Event)` for select, download progress, error, start/stop; the window only renders it. Accept: unit tests, no GUI deps.
- [ ] **Settings window.** Model dropdowns with autocomplete, browse, download, device, language. Accept: manual run; thin code only.
- [ ] **First-run flow.** No config opens the window with recommendations preselected; one click downloads and starts. Accept: manual run on a clean user on each OS.
- [ ] **Config hook.** Selections persist through the `Config` layer in 06. Accept: restart keeps the choice; missing model file re-opens the window with a clear message.
- [ ] **Licences.** Show each model's licence before download. Accept: shown in the window and in `models.toml`.

## Tests
Matcher, catalogue parsing, downloader against a local responder, Hugging Face response parsing, and `SettingsState` are pure and run in CI. Real engine loads and the window are manual runs.

## Out of scope
Paid or API-key services. Auto-update of the app. Model fine-tuning. Cloud inference.
