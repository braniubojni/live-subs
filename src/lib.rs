//! Library crate. The binary in `src/main.rs` is a thin subscriber.
//! Integration tests in `tests/` use this crate, not the binary.

pub mod audio;
pub mod cleanup;
pub mod config;
pub mod models;
#[cfg(feature = "overlay")]
pub mod overlay;
pub mod pipeline;
pub mod session;
pub mod settings;
pub mod setup;
#[cfg(feature = "silero")]
pub mod silero;
pub mod stt;
pub mod subtitles;
pub mod vad;
