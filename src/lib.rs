//! Library crate. The binary in `src/main.rs` is a thin subscriber.
//! Integration tests in `tests/` use this crate, not the binary.

pub mod audio;
pub mod cleanup;
pub mod pipeline;
#[cfg(feature = "silero")]
pub mod silero;
pub mod stt;
pub mod vad;
