# 07 Overlay

## Goal
A transparent, always-on-top, click-through window that shows the current subtitle line, driven by `Cue`s.

## Current state
**Order:** finish the open items here only after [11](../11-models-and-setup/README.md) (settings window, model setup) lands. Manual runs for 07 come after that, not now.

First slice landed. Feature `overlay` builds [src/overlay.rs](../../src/overlay.rs) (eframe, thin); render logic is [src/subtitles.rs](../../src/subtitles.rs) (`SubtitleState`, always compiled, unit-tested). `main` fans each cue out on a `broadcast` channel after printing it. Run: `cargo run --features overlay -- --overlay`, config under `[overlay]`. Tray menu (Pause/Resume, Quit) landed in code, manual run pending. Edit mode and multi-monitor are open.

### Toolkit result
`eframe` picked; Tauri not prototyped (a webview for one text line). Fill in from manual runs:

| Requirement | macOS | Windows |
| --- | --- | --- |
| Transparent | not yet run | not tested |
| Click-through | not yet run | not tested |
| Always on top, windowed video | not yet run | not tested |
| Over native fullscreen | expected to fail: winit cannot set fullScreenAuxiliary, `unsafe` is forbidden | not tested |

## Todo
- [ ] **Decide toolkit.** Prototype `eframe` and Tauri against three requirements: transparency, click-through, always on top over fullscreen video, on macOS and Windows. Record the result here and pick one. Accept: a short table of what worked.
- [x] **Subscriber boundary.** Overlay consumes `tokio::sync::broadcast::Receiver<Cue>` (or watch). Change `main` to fan out one cue stream to terminal and overlay. Accept: terminal printing still works with overlay off.
- [x] **Render model as a pure struct.** `SubtitleState::apply(Cue)`: partial replaces the line for that id, final replaces the partial, old lines expire after N seconds, max 2 lines. Accept: unit tests; depends on the id fix in [05](../05-pipeline/README.md).
- [ ] **Window.** Bottom-centre, width 80% of the screen, configurable font size, outline or background box for contrast. No decorations, no taskbar entry.
- [ ] **Click-through.** Mouse events pass to the app below (done, always on). Toggle edit mode with a hotkey to drag and resize. Persist position.
- [ ] **Multi-monitor.** Place on the monitor with the focused window, or a configured one.
- [ ] **Partial styling.** Partials dimmer or italic, final snaps to full style without layout jump. Accept: manual check; state struct exposes `is_partial`.
- [x] **Feature gating.** `cargo build` without the feature compiles and tests with no GUI deps. CI builds the feature once on macOS.
- [ ] **Tray or hotkey.** Pause/resume and quit. Accept: works with the window click-through. Tray done in code (`tray-icon`, `SubtitleState::set_paused` unit-tested); pending manual run on macOS to confirm the menu works while click-through. Hotkey skipped; pause only hides subtitles, the pipeline keeps running.

## Tests
`SubtitleState` fully unit-tested. Window code is a manual run; keep it thin.

## Out of scope
Translation display. Themes beyond font size and colours.
