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
use tokio_util::sync::CancellationToken;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Optional .env in the working directory. Real env vars win over it.
    dotenvy::dotenv().ok();

    if std::env::args().any(|a| a == "--list-devices") {
        return audio::list_devices();
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "live_subs=info".into()),
        )
        .init();

    let whisper_bin = std::env::var("WHISPER_BIN").unwrap_or_else(|_| "whisper-cli".into());
    let whisper_model =
        std::env::var("WHISPER_MODEL").unwrap_or_else(|_| "models/ggml-tiny.bin".into());
    let llama_bin = std::env::var("LLAMA_BIN").unwrap_or_else(|_| "llama-completion".into());
    let llama_model =
        std::env::var("LLAMA_MODEL").unwrap_or_else(|_| "models/cleanup-q4.gguf".into());

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Cue>(32);

    let cancel = CancellationToken::new();
    let (capture, audio_rx) = audio::start_loopback()?;
    info!(device = %capture.device_name, rate = capture.sample_rate, "capture started");

    let stt = stt::WhisperCli {
        bin: PathBuf::from(whisper_bin),
        model: PathBuf::from(whisper_model),
    };

    // LLAMA_SERVER set: one warm llama-server. Unset: llama-completion per line.
    if let Ok(server_bin) = std::env::var("LLAMA_SERVER") {
        let cleaner =
            cleanup::LlamaServer::spawn(&PathBuf::from(server_bin), &PathBuf::from(llama_model))?;
        tokio::spawn(pipeline::run(
            audio_rx,
            capture.sample_rate,
            stt,
            cleaner,
            tx,
            cancel.clone(),
        ));
    } else {
        let cleaner = cleanup::LlamaCli {
            bin: PathBuf::from(llama_bin),
            model: PathBuf::from(llama_model),
        };
        tokio::spawn(pipeline::run(
            audio_rx,
            capture.sample_rate,
            stt,
            cleaner,
            tx,
            cancel.clone(),
        ));
    }

    // v0 surface: terminal. The same Cue stream is what an eframe/Tauri overlay
    // and the later browser extension subscribe to.
    // Ctrl-C: cancel the pipeline and stop capture. It flushes any open
    // utterance as a final, then the channel closes and the loop ends. A second
    // Ctrl-C quits at once.
    // Once a second: reopen on stream error or default-device change.
    let mut capture = Some(capture);
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        let cue = tokio::select! {
            cue = rx.recv() => match cue {
                Some(cue) => cue,
                None => break,
            },
            _ = tokio::signal::ctrl_c() => {
                if cancel.is_cancelled() {
                    break;
                }
                cancel.cancel();
                capture = None;
                continue;
            }
            _ = tick.tick() => {
                if let Some(c) = capture.as_mut().filter(|c| c.needs_reopen()) {
                    match c.reopen() {
                        Ok(()) => info!(device = %c.device_name, rate = c.sample_rate, "capture resumed"),
                        Err(err) => tracing::warn!(?err, "capture reopen failed, retrying"),
                    }
                }
                continue;
            }
        };
        match cue.kind {
            pipeline::CueKind::Partial => {
                eprint!("\r\x1b[2K{}", cue.text);
            }
            pipeline::CueKind::Final => {
                eprintln!("\r\x1b[2K{}", cue.text);
            }
        }
    }
    drop(capture);
    Ok(())
}
