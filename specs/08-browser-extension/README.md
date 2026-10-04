# 08 Browser extension

## Goal
A browser extension draws the subtitle line over the page. It does not run any model. It reads cues from the local binary over `127.0.0.1`.

## Current state
Nothing. `Cue` already derives `Serialize` in [src/pipeline.rs](../../src/pipeline.rs).

## Todo
- [ ] **Local server in the library.** `src/server.rs` behind feature `server`: WebSocket (`tokio-tungstenite`) on `127.0.0.1:<port>` pushing each `Cue` as a JSON text frame. Bind loopback only. Accept: test connects a client, sends cues through a broadcast channel, reads them back.
- [ ] **Origin and auth.** Reject connections without an allowed `Origin` (extension origin) or a shared token from config. Browsers let any page open `ws://127.0.0.1`; a malicious site must not read subtitles. Accept: test rejects a bad origin.
- [ ] **Wire protocol doc.** `PROTOCOL.md` in this folder: message shape (`kind`, `id`, `text`, timing fields from [05](../05-pipeline/README.md)), a `hello` frame with `version`, and the rule that unknown fields are ignored. Accept: snapshot test of serialized `Cue` guards the doc.
- [ ] **Replay on connect.** Send the last final so a reconnect shows something. Bounded buffer of 1.
- [ ] **Extension skeleton.** Manifest V3, content script injecting a shadow-DOM overlay, background worker holding the socket (reconnect with backoff). Lives in `extension/` at repo root, separate from the Cargo tree.
- [ ] **Overlay rendering.** Same rules as `SubtitleState` in [07](../07-overlay/README.md): partial replaced by final, expiry, two lines. Share the spec, not the code.
- [ ] **Fullscreen video.** Attach inside the fullscreen element when present. Accept: works on a YouTube fullscreen player, manual check.
- [ ] **Settings popup.** Port, token, font size, on/off per site.

## Tests
Server tested in Rust with a real loopback socket (no audio, no models). Extension logic tested with the browser's test runner of choice; keep out of the Cargo CI job.

## Out of scope
Publishing to extension stores. Reading the page's own captions.
