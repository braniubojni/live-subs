# 12 Settings UI

## Goal
The settings window is a React + MUI form. The user can pick a speech model, a cleanup model, a device, and a language, then download and start, without the egui screen. After this lands, try the app through this window.

Do this after [11](../11-models-and-setup/README.md). The egui window proved the flow on macOS (window opened, both models downloaded, capture started). It is not the UI to keep. Park the manual runs in [00](../00-local/README.md) until this window exists, then run them here.

## Current state
The settings screen is [src/setup.rs](../../src/setup.rs) (`eframe` / `egui`, feature `overlay`). It renders [src/settings.rs](../../src/settings.rs) and does not decide anything: `SettingsState::apply(Event)` covers select, progress, download, error, start, stop, language, prompt carry, device, and cancel. The language filter is `filter_languages`. Downloads, catalogue, and Hugging Face search stay in [src/models/](../../src/models/). The window writes `whisper_model`, `llama_model`, `language`, `device`, `prompt_carry`, and `dev` through `Config::save`.

The subtitle window is a different screen: [src/overlay.rs](../../src/overlay.rs). It stays `eframe`. Spec [07](../07-overlay/README.md) needs transparency, click-through, and always-on-top. A webview is the wrong tool for that.

There is no `ui/` app and no webview. `unsafe_code` is forbidden in this crate.

## Design
- **Shell.** [Tauri](https://tauri.app/) 2, one webview, settings only. The Rust process still owns the catalogue, the downloader, the file check, and `Config::save`. The webview only renders.
- **State crosses as JSON.** Rust sends a snapshot: `SettingsState`, the catalogue rows for both kinds (recommendation first), device names, and the latest Hugging Face hits. The page sends back an `Event`. Rust calls `SettingsState::apply`. No second state machine in TypeScript.
- **Frontend.** `ui/` at the repo root, separate from the Cargo tree. React, MUI, Vite. MUI widgets, not a hand-built list: Autocomplete for models and language, Select for device, LinearProgress for a download, Button for browse, download, cancel, start, and stop.
- **Same behavior as the egui window.** Licence is visible on the selected model before Download. Start downloads a missing file, then starts capture. A missing model keeps the window open and names the model. Closing without a start exits non-zero. `--dev` lines stay the ones in `format_line`.
- **Overlay unchanged.** Feature `overlay` still builds the subtitle window with `eframe`. The settings webview does not draw cues.

## Todo
- [ ] **Tauri shell.** Settings window is a Tauri webview. The subtitle window is still `eframe`. Accept: a feature flag; `cargo build` without it does not compile the webview or the `ui/` bundle.
- [ ] **JSON boundary.** Serde types for the snapshot and for `Event`. Accept: round-trip test, no webview, no browser.
- [ ] **`ui/` app.** React + MUI + Vite. Accept: `npm run build` writes a static bundle. No Rust in that folder.
- [ ] **Form.** Speech and Cleanup autocomplete (recommendation first), licence on the selected row, Browse, Download, progress, Cancel, Hugging Face search, Device, Language (`Auto` first; `en` and `English` both hit English), "Use the previous line as a hint", Start, Stop, the error string, and the delete actions from [13](../13-delete-models/README.md). Accept: manual run of this window.
- [ ] **Behavior carried over.** Start downloads a missing model and then starts. A missing file keeps the window open and names the model. Close without Start exits non-zero. Selections still go through `Config::save`. Accept: the same cases the egui window already covered, checked on this window.
- [ ] **`--dev` lines.** Opening the window, choosing a model, download progress, and capture started still print `format_line`. Accept: the existing formatter tests stay green.
- [ ] **Remove the egui settings screen.** [src/setup.rs](../../src/setup.rs) no longer paints the form. Accept: one settings window, the MUI one. The overlay still builds.

## Tests
The JSON round-trip and the existing `SettingsState` / `filter_languages` / `format_line` tests run in CI with no browser and no microphone. The MUI window is a manual run. Do not shell out to `whisper-cli` or `llama-cli`.

## Out of scope
The subtitle overlay ([07](../07-overlay/README.md)). The browser extension ([08](../08-browser-extension/README.md)). Changing what `SettingsState::apply` decides. Restyling cues. Fixing Hugging Face rows that already arrive as `0` bytes or `unknown`: the form shows the size and licence Rust parsed; it does not invent them.
