//! Live subtitles binary.
//!
//! Audio callback never transcribes. It only pushes f32 samples into a ring buffer.
//! A worker resamples to 16 kHz mono, cuts on silence, and emits cues.
//! Whisper and the cleanup model run on their own tasks so a slow token
//! cannot stall capture.

use anyhow::Result;
use clap::Parser;
#[cfg(feature = "overlay")]
use live_subs::config::stored_file;
use live_subs::config::{Cli, Config};
use live_subs::pipeline::Cue;
use live_subs::{audio, session, settings};
use tokio_util::sync::CancellationToken;

fn main() -> Result<()> {
    // Optional .env in the working directory. Real env vars win over it.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    if cli.list_devices {
        return audio::list_devices();
    }
    #[cfg_attr(not(feature = "overlay"), allow(unused_mut))]
    let mut cfg = Config::load(&cli)?;
    settings::set_dev(cfg.dev);
    if cli.print_config {
        print!("{}", toml::to_string_pretty(&cfg)?);
        return Ok(());
    }
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

    #[cfg(feature = "overlay")]
    if live_subs::setup::should_open(stored_file(&cli).is_some(), &cfg) {
        let has_file = stored_file(&cli).is_some();
        let save_to = live_subs::setup::save_target(cli.config.as_deref());
        let started = live_subs::setup::run(cfg, save_to, has_file, rt.handle().clone())?;
        if !started {
            anyhow::bail!("closed without a model");
        }
        cfg = Config::load(&cli)?;
        settings::set_dev(cfg.dev);
    }

    cfg.check_files()?;
    let cancel = CancellationToken::new();
    // Every subscriber (terminal, overlay, later the browser server) reads this.
    let (cues, _) = tokio::sync::broadcast::channel::<Cue>(cfg.channel_size);

    #[cfg(feature = "overlay")]
    if cfg.overlay.enabled {
        let cfg_run = cfg.clone();
        // `Capture` is not Send, so the session gets its own thread.
        let (h, c, cues2) = (rt.handle().clone(), cancel.clone(), cues.clone());
        let task = std::thread::spawn(move || h.block_on(session::run(cfg_run, cues2, c)));
        live_subs::overlay::run(cfg.overlay, cues.subscribe(), rt.handle().clone(), cancel)?;
        return task
            .join()
            .map_err(|_| anyhow::anyhow!("pipeline thread panicked"))?;
    }
    rt.block_on(session::run(cfg, cues, cancel))
}
