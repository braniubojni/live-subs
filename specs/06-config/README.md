# 06 Config

## Goal
One validated `Config` struct, loaded once, passed down. No `std::env::var` calls outside the loader.

## Current state
Env vars read in three places: [src/main.rs](../../src/main.rs) (`WHISPER_*`, `LLAMA_*`), [src/audio.rs](../../src/audio.rs) (`SUBS_DEVICE`), [src/pipeline.rs](../../src/pipeline.rs) (`SUBS_RATE`). Constants hard-coded in vad, stt, cleanup, pipeline.

## Todo
- [ ] **`Config` struct** in `src/config.rs` (add module in `lib.rs`): paths, device, language, VAD params, partial interval, cleanup timeout, channel size. `serde::Deserialize`, `Default` equal to today's constants. Accept: grep for `env::var` finds only `config.rs`.
- [ ] **Layering.** defaults, then `live-subs.toml` (cwd, then XDG/OS config dir), then env, then CLI flags. Later wins. Accept: test of precedence with an in-memory source.
- [ ] **CLI flags.** `clap` derive: `--list-devices`, `--device`, `--language`, `--no-cleanup`, `--config <path>`, `--print-config`. Accept: `--help` documents every key.
- [ ] **Validation.** Missing files, `silence_ms < frame`, thresholds out of range produce one error naming the key. Accept: table test of bad configs.
- [ ] **Env var names kept.** Existing `WHISPER_BIN` etc. keep working. Accept: test.
- [ ] **Docs.** README Run section shrinks to the config file. An example `live-subs.example.toml` committed.

## Tests
Pure parsing and merging. No filesystem except `tempfile`.

## Out of scope
Hot reload (revisit with the overlay settings panel).
