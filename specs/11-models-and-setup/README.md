# 11 Models and setup

## Goal
A user installs one binary, opens a small window, and gets subtitles. The app ships the whisper and llama engines inside the binary, recommends models that fit the machine, and downloads the chosen ones. No cmake, no `curl`, no `.env`. All models are free and run locally.

## Current state
The settings window is [src/setup.rs](../../src/setup.rs) (`--features overlay`). It only renders [src/settings.rs](../../src/settings.rs): `SettingsState`, the language filter, and the `--dev` line formatter. Downloads, Hugging Face parsing, and the ggml/GGUF magic check are [src/models/download.rs](../../src/models/download.rs), [src/models/hf.rs](../../src/models/hf.rs), and [src/models/validate.rs](../../src/models/validate.rs). The window writes `whisper_model`, `llama_model`, `language`, `device`, `prompt_carry`, and `dev` through `Config::save`. No config file, or a missing model file, opens the window instead of capture. Closing it without a start exits non-zero.

`LlamaInProc` in [src/cleanup.rs](../../src/cleanup.rs) (feature `llama`, `llama-cpp-2` 0.1.159, Metal on macOS) loads the cleanup model once and keeps the 700 ms cap and the 2× length guard. `LlamaCli` / `LlamaServer` stay when that feature is off. `WhisperRs` is unchanged. The root README still documents the CLI setup. The macOS engine row is filled in below; Windows and Linux are not built. The window, first-run, and licence line still need a manual run.

This spec supersedes the packaging items "Remove external CLIs" and "`live-subs setup`" in [10](../10-packaging/README.md), and builds on [03](../03-stt/README.md), [04](../04-cleanup/README.md), [06](../06-config/README.md) and the toolkit decision in [07](../07-overlay/README.md). Do the manual per-OS runs in [01](../01-capture/README.md) after the first-run flow here lands, not before.

## Engine pick

Linked `whisper-rs` 0.16 (`metal`) and `llama-cpp-2` 0.1.159 (`metal`) into one release binary on this Mac (arm64, Apple clang 21, cmake 4.4). They link. `supports_gpu_offload()` is true and the binary links Metal.framework. No CUDA or Vulkan symbols in that build.

| | macOS arm64 | Windows | Linux |
| --- | --- | --- | --- |
| Build | 33 s wall, 181 s CPU, release | not built | not built |
| Size | probe 5.9 MB after link-time strip; static libs whisper 3.9 MB + llama 19 MB | not built | not built |
| GPU | Metal on | `cuda` / `vulkan` flags exist, not built | same |

Pick: those two crates, static. Metal on macOS. `cuda` and `vulkan` feature flags elsewhere. CPU when no GPU feature is on. The checkbox stays open until the Windows and Linux rows are filled in.

## Design
- **Engines in the binary.** `whisper-rs` and `llama-cpp-2` compile whisper.cpp and llama.cpp statically. Metal on macOS, CUDA/Vulkan feature flags elsewhere, CPU always. They sit behind the existing `Transcriber` and `Cleaner` traits; the CLI backends stay as an opt-in fallback.
- **Model catalogue.** `models.toml` embedded in the binary. Per entry: id, kind (`stt` or `cleanup`), display name, URL, size, sha256, licence, minimum RAM, languages, expected speed class.
- **Hardware matcher.** A pure function `recommend(&Hardware, &Catalogue, Kind) -> Vec<Ranked>`. `Hardware` is RAM, free disk, CPU cores, GPU kind (Metal, CUDA, Vulkan, none). Rule: the largest model whose RAM need fits within about 60% of total RAM, and whose speed class is real-time on this hardware. Each result carries a one-line reason.
- **Bring your own.** A model picker with autocomplete over the catalogue, plus a file browse button for a local `.bin` / `.gguf`, plus a Hugging Face search box (`https://huggingface.co/api/models?search=<q>&filter=gguf`) with a Download button. A user-picked model is validated before use (magic bytes, loadable), not trusted.
- **GUI.** One small settings window using the toolkit chosen in 07 (`eframe`). Three controls per model kind: dropdown with the recommendation first, browse, download with progress and cancel. Plus device picker (from `--list-devices`), the language control below, start/stop. Everything it sets is written to the config in 06. The user does not set `WHISPER_MODEL` or `LLAMA_MODEL`.
- **Language control.** A field with autocomplete over whisper's language list (code plus English name: `en` English, `ru` Russian, …) and a first row `Auto`. Typing `en` or `eng` filters the list. `Auto` is the default. A picked code is stored as `language` and is never overwritten by detection. `Auto` lets the first utterance whose detection probability is at least 0.8 lock that code for later utterances; the field then shows the locked code, with a way to return to `Auto`. The filter is a pure function, unit-tested. The window only renders it.
- **`--dev` logs.** `live-subs --dev` (config key `dev`, default false) prints one line per step to stderr: settings window opened, model chosen (kind and id), download progress, capture started, each cue (`kind`, `id`, `lang`, text), and a language lock (`code`, probability). Default runs stay quiet. This is how a later local check sees that the window is doing work.
- **First run.** No config means the window opens with the recommended models pre-selected. One click downloads both and starts capture.
- **Missing model.** A chosen model whose file is absent keeps the window open, names the model, and does not start capture. Closing the window without a model exits non-zero.

## Todo
- [ ] **Decide engine crates.** macOS row is in the table above. Still open: the same probe on Windows and Linux.
- [x] **`WhisperRs` backend.** Implements `Transcriber`, model loaded once, no temp WAV. Accept: same pipeline tests pass with a fake and with the real engine behind a model-file-exists skip. Landed with [03](../03-stt/README.md).
- [x] **Catalogue.** `models.toml` plus parser in `src/models/catalogue.rs`. At least: whisper tiny, base, small, medium (multilingual and `.en`) and two small cleanup GGUFs. Accept: parse test; every entry has sha256, size, licence.
- [x] **Hardware probe.** `Hardware::detect()` using `sysinfo` plus a GPU check per OS. Accept: detect is a thin shim; everything downstream takes a `Hardware` value.
- [x] **Matcher.** `recommend` as described. Accept: table test over fake hardware (8 GB no GPU, 16 GB Apple Silicon, 4 GB, 64 GB CUDA) giving expected top pick and reason, never a model above the RAM budget.
- [x] **In-process llama `Cleaner`.** Model loaded once, same 700 ms cap and 2x-length guard as today. Accept: guard tests unchanged. Feature `llama`. Load test skips when the model file is missing.
- [x] **Downloader.** Resumable, progress callback, sha256 verify, cancel, atomic rename into the OS data dir. Accept: tested against a local HTTP responder: resume after a cut, corrupt file re-fetched, cancel leaves no partial file.
- [x] **Hugging Face browse.** Search by text, list GGUF/bin files with sizes, pick one, download through the same downloader. Accept: parser tested on a recorded JSON response; network is behind a trait so tests never go online.
- [x] **Local model import.** File browse, then validate (magic bytes, size sane, loads). Accept: bad file gives one error naming the problem. A real engine load stays a manual run.
- [x] **Settings state as a pure struct.** `SettingsState::apply(Event)` for select, download progress, error, start/stop; the window only renders it. Accept: unit tests, no GUI deps.
- [ ] **Settings window.** Model dropdowns with autocomplete, browse, download, device, language field. Accept: manual run; thin code only.
- [x] **Language field.** Autocomplete over the whisper language list plus `Auto`, as in Design. Accept: pure filter test (`en`, `English`, empty query returns `Auto` first). A chosen code round-trips through `Config.language`.
- [x] **`--dev`.** Flag and config key. Accept: with `--dev`, the lines in Design are emitted; without it, those lines are absent. Test the line formatter with a fake event, no window.
- [ ] **First-run flow.** No config opens the window with recommendations preselected; one click downloads and starts. Accept: manual run on a clean user on each OS.
- [x] **Config hook.** Selections persist through the `Config` layer in 06. Accept: restart keeps the choice; a missing model file keeps the window open and names the model.
- [ ] **Licences.** Show each model's licence before download. Accept: shown in the window and in `models.toml`.

## After the settings window

Prompt carry. Already implemented in the pipeline ([03](../03-stt/README.md), key `prompt_carry`, default false). The window checkbox writes that key.

Whisper transcribes one utterance at a time and forgets the previous line. Carry hands that previous final line back as a hint, so a name spelled in line one stays spelled that way in line two. The hint is not shown as a subtitle. The audio of the new utterance is still what gets transcribed.

- [x] **Prompt-carry checkbox.** Label: "Use the previous line as a hint". Writes `prompt_carry`. Accept: pure `SettingsState` event; restart keeps it.
- [ ] **Hint in `--dev`.** With the box on, the second utterance's `--dev` line includes the previous final text. With the box off, that line has no hint. Check this in [00](../00-local/README.md) after the window exists.

## Tests
Matcher, catalogue parsing, downloader against a local responder, Hugging Face response parsing, the language filter, the `--dev` line formatter, and `SettingsState` are pure and run in CI. Real engine loads and the window are manual runs.

## Out of scope
Paid or API-key services. Auto-update of the app. Model fine-tuning. Cloud inference.
