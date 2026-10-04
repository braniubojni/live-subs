//! Live subtitles binary.
//!
//! Audio callback never transcribes. It only pushes f32 samples into a ring buffer.
//! A worker resamples to 16 kHz mono, cuts on silence, and emits cues.
//! Whisper and the cleanup model run on their own tasks so a slow token
//! cannot stall capture.

use anyhow::Result;
use live_subs::pipeline::Cue;
use live_subs::{audio, cleanup, pipeline, stt};
use std::path::PathBuf;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "live_subs=info".into()),
        )
        .init();

    let whisper_bin = std::env::var("WHISPER_BIN").unwrap_or_else(|_| "whisper-cli".into());
    let whisper_model =
        std::env::var("WHISPER_MODEL").unwrap_or_else(|_| "models/ggml-tiny.bin".into());
    let llama_bin = std::env::var("LLAMA_BIN").unwrap_or_else(|_| "llama-cli".into());
    let llama_model =
        std::env::var("LLAMA_MODEL").unwrap_or_else(|_| "models/cleanup-q4.gguf".into());

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Cue>(32);

    let capture = audio::start_loopback()?;
    info!(device = %capture.device_name, rate = capture.sample_rate, "capture started");

    let stt = stt::WhisperCli {
        bin: PathBuf::from(whisper_bin),
        model: PathBuf::from(whisper_model),
    };
    let cleaner = cleanup::LlamaCli {
        bin: PathBuf::from(llama_bin),
        model: PathBuf::from(llama_model),
    };

    tokio::spawn(pipeline::run(
        capture.audio_rx,
        capture.sample_rate,
        stt,
        cleaner,
        tx,
    ));

    // v0 surface: terminal. The same Cue stream is what an eframe/Tauri overlay
    // and the later browser extension subscribe to.
    while let Some(cue) = rx.recv().await {
        match cue.kind {
            pipeline::CueKind::Partial => {
                eprint!("\r\x1b[2K{}", cue.text);
            }
            pipeline::CueKind::Final => {
                eprintln!("\r\x1b[2K{}", cue.text);
            }
        }
    }
    Ok(())
}
