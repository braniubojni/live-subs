//! Capture plus the pipeline. The binary and the settings window both call this.

use crate::cleanup::Cleaner;
use crate::config::Config;
use crate::pipeline::{Cue, CueKind};
use crate::settings::{emit, DevEvent};
use crate::{audio, cleanup, pipeline, stt};
use anyhow::Result;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn run(
    cfg: Config,
    cues: tokio::sync::broadcast::Sender<Cue>,
    cancel: CancellationToken,
) -> Result<()> {
    crate::settings::set_dev(cfg.dev);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Cue>(cfg.channel_size);
    let (capture, audio_rx) = audio::start_loopback(cfg.device.as_deref())?;
    info!(device = %capture.device_name, rate = capture.sample_rate, "capture started");
    emit(&DevEvent::CaptureStarted);

    let timeout = Duration::from_millis(cfg.cleanup_timeout_ms);
    #[cfg(feature = "llama")]
    let cleaner: Box<dyn Cleaner> = if cfg.cleanup {
        Box::new(cleanup::LlamaInProc::load(&cfg.llama_model, timeout)?)
    } else {
        Box::new(cleanup::Passthrough)
    };
    #[cfg(not(feature = "llama"))]
    let cleaner: Box<dyn Cleaner> = match (cfg.cleanup, &cfg.llama_server) {
        (false, _) => Box::new(cleanup::Passthrough),
        (true, Some(server)) => Box::new(cleanup::LlamaServer::spawn(
            server,
            &cfg.llama_model,
            timeout,
        )?),
        (true, None) => Box::new(cleanup::LlamaCli {
            bin: cfg.llama_bin.clone(),
            model: cfg.llama_model.clone(),
            timeout,
        }),
    };
    #[cfg(feature = "whisper-rs")]
    let stt = stt::WhisperRs::load(&cfg.whisper_model, cfg.language.clone())?;
    #[cfg(not(feature = "whisper-rs"))]
    let stt = stt::WhisperCli::new(&cfg.whisper_bin, &cfg.whisper_model, cfg.language.clone());
    tokio::spawn(pipeline::run_with(
        audio_rx,
        capture.sample_rate,
        stt,
        cleaner,
        tx,
        cancel.clone(),
        cfg.vad,
        Duration::from_millis(cfg.partial_every_ms),
        cfg.prompt_carry,
    ));

    let dev = cfg.dev;
    let prompt_carry = cfg.prompt_carry;
    let mut prev: Option<String> = None;
    let mut capture = Some(capture);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
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
        if dev {
            let hint = if prompt_carry { prev.as_deref() } else { None };
            emit(&DevEvent::Cue {
                kind: match cue.kind {
                    CueKind::Partial => "partial",
                    CueKind::Final => "final",
                },
                id: cue.id,
                lang: cue.lang.as_deref().unwrap_or(""),
                text: &cue.text,
                hint,
            });
        }
        if matches!(cue.kind, CueKind::Final) {
            prev = Some(cue.text.clone());
        }
        let _ = cues.send(cue.clone());
        match cue.kind {
            CueKind::Partial => eprint!("\r\x1b[2K{}", cue.text),
            CueKind::Final => eprintln!("\r\x1b[2K{}", cue.text),
        }
    }
    drop(capture);
    Ok(())
}
