# 10 Packaging

## Goal
A user installs one thing, runs one command, and gets subtitles. No manual cmake builds, no hand-set env vars.

## Current state
README setup clones whisper.cpp and llama.cpp, builds them with cmake, downloads models with `curl`, then exports four env vars. `models/.gitkeep` holds the model dir.

## Todo
- [ ] **`live-subs setup` command.** Downloads models into the OS data dir with progress and SHA-256 verification; resumes partial downloads. Accept: second run is a no-op; corrupt file is re-fetched.
- [ ] **Model manifest.** `models.toml` embedded in the binary: name, URL, size, sha256, licence. One source for README and code.
- [ ] **Remove external CLIs.** After `WhisperRs` ([03](../03-stt/README.md)) and `LlamaServer` or in-process llama ([04](../04-cleanup/README.md)) are stable, make the CLI-backed paths optional. Accept: a fresh machine needs only the binary and the downloaded models.
- [ ] **Release builds.** GitHub Actions on tag: macOS (arm64, x86_64), Windows, Linux. Strip, LTO, `codegen-units = 1` in a `[profile.release]`. Accept: artifacts attached to the release.
- [ ] **macOS signing and permissions.** Bundle with an `Info.plist` containing audio-capture usage text so the permission prompt names the app, not the terminal. Notarize. Accept: manual first-run check on a clean user.
- [ ] **Windows and Linux notes.** WASAPI loopback device naming; PipeWire/Pulse monitor setup. Accept: short per-OS section in the README, verified on a real machine.
- [ ] **Install paths.** `cargo install live-subs` works for the core binary. Homebrew tap after the first release.
- [ ] **Versioning and changelog.** `CHANGELOG.md`, semver, the cue wire protocol version tracked separately ([08](../08-browser-extension/README.md)).
- [ ] **Uninstall.** `live-subs setup --remove` deletes downloaded models and config after confirmation.

## Tests
Manifest parsing and checksum verification are pure. Downloader tested against a local HTTP responder.

## Out of scope
Auto-update. Installers (MSI, pkg) until there is demand.
