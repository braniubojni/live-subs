# 06 Config

## Goal
One validated `Config` struct, loaded once, passed down. No `std::env::var` calls outside the loader.

## Current state
Done. [src/config.rs](../../src/config.rs) holds `Config`, the layers (defaults, `live-subs.toml`, env, CLI via `clap`), and validation. It is the only module that reads env vars. [`live-subs.example.toml`](../../live-subs.example.toml) documents every key. `.env` and the legacy env names stay as the env layer.

## Todo
- [x] **`Config` struct** in `src/config.rs` (add module in `lib.rs`): paths, device, language, VAD params, partial interval, cleanup timeout, channel size. `serde::Deserialize`, `Default` equal to today's constants. Accept: grep for `env::var` finds only `config.rs`.
- [x] **Layering.** defaults, then `live-subs.toml` (cwd, then XDG/OS config dir), then env, then CLI flags. Later wins. Accept: test of precedence with an in-memory source.
- [x] **CLI flags.** `clap` derive: `--list-devices`, `--device`, `--language`, `--no-cleanup`, `--config <path>`, `--print-config`. Accept: `--help` documents every key.
- [x] **Validation.** Missing files, `silence_ms < frame`, thresholds out of range produce one error naming the key. Accept: table test of bad configs.
- [x] **Env var names kept.** Existing `WHISPER_BIN` etc. keep working. Accept: test.
- [x] **Docs.** README Run section shrinks to the config file. An example `live-subs.example.toml` committed. `.env.example` stays only if env remains a supported layer.

## Tests
Pure parsing and merging. No filesystem except `tempfile`.

## Out of scope
Hot reload (revisit with the overlay settings panel).
