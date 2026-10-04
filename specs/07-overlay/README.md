# 07 Overlay

## Goal
A transparent, always-on-top, click-through window that shows the current subtitle line, driven by `Cue`s.

## Current state
Cargo feature `overlay` with an optional `eframe` dependency exists. No code. Binary prints to stderr in [src/main.rs](../../src/main.rs).

## Todo
- [ ] **Decide toolkit.** Prototype `eframe` and Tauri against three requirements: transparency, click-through, always on top over fullscreen video, on macOS and Windows. Record the result here and pick one. Accept: a short table of what worked.
- [ ] **Subscriber boundary.** Overlay consumes `tokio::sync::broadcast::Receiver<Cue>` (or watch). Change `main` to fan out one cue stream to terminal and overlay. Accept: terminal printing still works with overlay off.
- [ ] **Render model as a pure struct.** `SubtitleState::apply(Cue)`: partial replaces the line for that id, final replaces the partial, old lines expire after N seconds, max 2 lines. Accept: unit tests; depends on the id fix in [05](../05-pipeline/README.md).
- [ ] **Window.** Bottom-centre, width 80% of the screen, configurable font size, outline or background box for contrast. No decorations, no taskbar entry.
- [ ] **Click-through.** Mouse events pass to the app below. Toggle edit mode with a hotkey to drag and resize. Persist position.
- [ ] **Multi-monitor.** Place on the monitor with the focused window, or a configured one.
- [ ] **Partial styling.** Partials dimmer or italic, final snaps to full style without layout jump. Accept: manual check; state struct exposes `is_partial`.
- [ ] **Feature gating.** `cargo build` without the feature compiles and tests with no GUI deps. CI builds the feature once on macOS.
- [ ] **Tray or hotkey.** Pause/resume and quit. Accept: works with the window click-through.

## Tests
`SubtitleState` fully unit-tested. Window code is a manual run; keep it thin.

## Out of scope
Translation display. Themes beyond font size and colours.
