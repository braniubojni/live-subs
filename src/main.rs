//! Live subtitles binary.
//!
//! Audio callback never transcribes. It only pushes f32 samples into a ring buffer.
//! A worker resamples to 16 kHz mono, cuts on silence, and emits cues.
//! Whisper and the cleanup model run on their own tasks so a slow token
//! cannot stall capture.

use anyhow::Result;
use clap::Parser;
use live_subs::cleanup::Cleaner;
use live_subs::config::{Cli, Config};
use live_subs::pipeline::Cue;
use live_subs::{audio, cleanup, pipeline, stt};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

fn main() -> Result<()> {
    // Optional .env in the working directory. Real env vars win over it.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    if cli.list_devices {
        return audio::list_devices();
    }
    let cfg = Config::load(&cli)?;
    if cli.print_config {
        print!("{}", toml::to_string_pretty(&cfg)?);
        return Ok(());
    }
    cfg.check_files()?;
    if cfg.overlay.enabled && !cfg!(feature = "overlay") {
        anyhow::bail!("overlay.enabled needs a build with `--features overlay`");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "live_subs=info".into()),
        )
        .init();

    // eframe must own the main thread on macOS, so the runtime is built by hand.
    let rt = tokio::runtime::Runtime::new()?;
    let cancel = CancellationToken::new();
    // Every subscriber (terminal, overlay, later the browser server) reads this.
    let (cues, _) = tokio::sync::broadcast::channel::<Cue>(cfg.channel_size);

    #[cfg(feature = "overlay")]
    if cfg.overlay.enabled {
        let cfg_run = cfg.clone();
        // `Capture` is not Send, so `run` gets its own thread.
        let (h, c, cues2) = (rt.handle().clone(), cancel.clone(), cues.clone());
        let task = std::thread::spawn(move || h.block_on(run(cfg_run, cues2, c)));
        live_subs::overlay::run(cfg.overlay, cues.subscribe(), rt.handle().clone(), cancel)?;
        return task
            .join()
            .map_err(|_| anyhow::anyhow!("pipeline thread panicked"))?;
    }
    rt.block_on(run(cfg, cues, cancel))
}

async fn run(
    cfg: Config,
    cues: tokio::sync::broadcast::Sender<Cue>,
    cancel: CancellationToken,
) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Cue>(cfg.channel_size);
    let (capture, audio_rx) = audio::start_loopback(cfg.device.as_deref())?;
    info!(device = %capture.device_name, rate = capture.sample_rate, "capture started");

    let timeout = Duration::from_millis(cfg.cleanup_timeout_ms);
    let cleaner: Box<dyn Cleaner> = match (cfg.cleanup, &cfg.llama_server) {
        (false, _) => Box::new(cleanup::Passthrough),
        // One warm llama-server.
        (true, Some(server)) => Box::new(cleanup::LlamaServer::spawn(
            server,
            &cfg.llama_model,
            timeout,
        )?),
        // llama-completion per line.
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

    // Terminal prints each cue and fans it out on `cues`.
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
        let _ = cues.send(cue.clone()); // no subscribers is fine
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
