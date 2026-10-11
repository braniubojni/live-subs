//! One validated `Config`, loaded once. This is the only module that reads
//! environment variables.
//!
//! Layers, later wins: defaults, `live-subs.toml`, env, CLI flags. Each layer is a
//! TOML table; they are deep-merged and deserialized once.

use crate::vad::VadConfig;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toml::{Table, Value};

const FILE_NAME: &str = "live-subs.toml";
/// One input frame at 48 kHz. `silence_ms` below this can never close an utterance.
const FRAME_MS: u32 = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub whisper_bin: PathBuf,
    pub whisper_model: PathBuf,
    pub llama_bin: PathBuf,
    pub llama_model: PathBuf,
    /// Set: one warm llama-server. Unset: `llama_bin` per line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llama_server: Option<PathBuf>,
    /// Exact input device name. Unset: follow the default output device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// whisper `-l` value. `auto` detects.
    pub language: String,
    /// `false`: raw transcript, no cleanup model.
    pub cleanup: bool,
    pub cleanup_timeout_ms: u64,
    pub partial_every_ms: u64,
    /// Pass the previous final line to whisper as `--prompt`.
    pub prompt_carry: bool,
    /// Cue channel capacity.
    pub channel_size: usize,
    pub vad: VadConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            whisper_bin: "whisper-cli".into(),
            whisper_model: "models/ggml-tiny.bin".into(),
            llama_bin: "llama-completion".into(),
            llama_model: "models/cleanup-q4.gguf".into(),
            llama_server: None,
            device: None,
            language: "auto".into(),
            cleanup: true,
            cleanup_timeout_ms: 700,
            partial_every_ms: 900,
            prompt_carry: false,
            channel_size: 32,
            vad: VadConfig::default(),
        }
    }
}

/// Command line. Every flag overrides the same key from file and env.
#[derive(Debug, Default, Parser)]
#[command(version, about = "Local live subtitles from system audio")]
pub struct Cli {
    /// Print input and output devices, then exit.
    #[arg(long)]
    pub list_devices: bool,
    /// Exact input device name (see --list-devices). Key: device.
    #[arg(long)]
    pub device: Option<String>,
    /// whisper language code, or `auto`. Key: language.
    #[arg(long)]
    pub language: Option<String>,
    /// Skip the cleanup model; show the raw transcript. Key: cleanup = false.
    #[arg(long)]
    pub no_cleanup: bool,
    /// Config file. Default: ./live-subs.toml, then the user config dir.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Print the effective config as TOML, then exit.
    #[arg(long)]
    pub print_config: bool,
}

impl Cli {
    fn layer(&self) -> Table {
        let mut t = Table::new();
        if let Some(d) = &self.device {
            t.insert("device".into(), d.clone().into());
        }
        if let Some(l) = &self.language {
            t.insert("language".into(), l.clone().into());
        }
        if self.no_cleanup {
            t.insert("cleanup".into(), false.into());
        }
        t
    }
}

/// Legacy env names, kept working.
const ENV_KEYS: [(&str, &str); 6] = [
    ("WHISPER_BIN", "whisper_bin"),
    ("WHISPER_MODEL", "whisper_model"),
    ("LLAMA_BIN", "llama_bin"),
    ("LLAMA_MODEL", "llama_model"),
    ("LLAMA_SERVER", "llama_server"),
    ("SUBS_DEVICE", "device"),
];

fn env_layer(get: impl Fn(&str) -> Option<String>) -> Table {
    ENV_KEYS
        .iter()
        .filter_map(|(var, key)| get(var).map(|v| ((*key).to_string(), Value::String(v))))
        .collect()
}

fn merge(base: &mut Table, over: Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(Value::Table(b)), Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

fn parse_file(text: &str, path: &Path) -> Result<Table> {
    text.parse::<Table>()
        .with_context(|| format!("{}", path.display()))
}

fn user_config_dir() -> Option<PathBuf> {
    let var = |k| std::env::var_os(k).map(PathBuf::from);
    var("XDG_CONFIG_HOME")
        .or_else(|| var("APPDATA"))
        .or_else(|| {
            let home = var("HOME")?;
            Some(if cfg!(target_os = "macos") {
                home.join("Library/Application Support")
            } else {
                home.join(".config")
            })
        })
        .map(|d| d.join("live-subs"))
}

impl Config {
    /// Merge layers over the defaults and validate the values. No filesystem access.
    pub fn from_layers(layers: impl IntoIterator<Item = Table>) -> Result<Self> {
        let mut t = Table::try_from(Config::default())?;
        for l in layers {
            merge(&mut t, l);
        }
        let cfg: Config = t.try_into().map_err(|e| anyhow!("config: {e}"))?;
        cfg.check_values()?;
        Ok(cfg)
    }

    /// Defaults, config file, env, `cli`. Call `check_files` before running.
    pub fn load(cli: &Cli) -> Result<Self> {
        let file = match &cli.config {
            Some(p) => Some(p.clone()),
            None => [
                Some(PathBuf::from(FILE_NAME)),
                user_config_dir().map(|d| d.join(FILE_NAME)),
            ]
            .into_iter()
            .flatten()
            .find(|p| p.is_file()),
        };
        let file_layer = match file {
            Some(p) => parse_file(
                &std::fs::read_to_string(&p).with_context(|| format!("{}", p.display()))?,
                &p,
            )?,
            None => Table::new(),
        };
        let env = env_layer(|k| std::env::var(k).ok());
        Self::from_layers([file_layer, env, cli.layer()])
    }

    pub fn check_values(&self) -> Result<()> {
        let v = &self.vad;
        let checks: [(&str, bool, &str); 11] = [
            (
                "language",
                !self.language.trim().is_empty(),
                "must not be empty",
            ),
            (
                "cleanup_timeout_ms",
                self.cleanup_timeout_ms > 0,
                "must be > 0",
            ),
            ("partial_every_ms", self.partial_every_ms > 0, "must be > 0"),
            ("channel_size", self.channel_size > 0, "must be > 0"),
            (
                "vad.open",
                v.open > 0.0 && v.open <= 1.0,
                "must be in (0, 1]",
            ),
            (
                "vad.close",
                v.close > 0.0 && v.close <= v.open,
                "must be in (0, vad.open]",
            ),
            ("vad.noise_ratio", v.noise_ratio >= 1.0, "must be >= 1"),
            (
                "vad.silence_ms",
                v.silence_ms >= FRAME_MS,
                "must be at least one 20 ms frame",
            ),
            ("vad.min_speech_ms", v.min_speech_ms > 0, "must be > 0"),
            (
                "vad.max_speech_ms",
                v.max_speech_ms >= v.min_speech_ms,
                "must be >= vad.min_speech_ms",
            ),
            (
                "vad.preroll_ms",
                v.preroll_ms <= v.max_speech_ms,
                "must be <= vad.max_speech_ms",
            ),
        ];
        match checks.iter().find(|(_, ok, _)| !ok) {
            Some((key, _, why)) => bail!("config key `{key}` {why}"),
            None => Ok(()),
        }
    }

    pub fn check_files(&self) -> Result<()> {
        // In-process whisper does not shell out, so the CLI binary is unused.
        #[cfg(not(feature = "whisper-rs"))]
        let mut need = vec![
            ("whisper_bin", &self.whisper_bin, true),
            ("whisper_model", &self.whisper_model, false),
        ];
        #[cfg(feature = "whisper-rs")]
        let mut need = vec![("whisper_model", &self.whisper_model, false)];
        if self.cleanup {
            need.push(("llama_model", &self.llama_model, false));
            match &self.llama_server {
                Some(s) => need.push(("llama_server", s, true)),
                None => need.push(("llama_bin", &self.llama_bin, true)),
            }
        }
        for (key, path, is_bin) in need {
            if !(path.is_file() || is_bin && on_path(path)) {
                bail!("config key `{key}`: file not found: {}", path.display());
            }
        }
        Ok(())
    }
}

/// A bare program name that resolves through `PATH`.
fn on_path(name: &Path) -> bool {
    name.components().count() == 1
        && std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> Table {
        s.parse().unwrap()
    }

    #[test]
    fn defaults_alone_are_valid() {
        assert_eq!(Config::from_layers([]).unwrap(), Config::default());
    }

    #[test]
    fn later_layer_wins_and_vad_merges_deep() {
        let file =
            table("language = \"en\"\ndevice = \"file\"\n[vad]\nsilence_ms = 300\nopen = 0.02");
        let env = env_layer(|k| (k == "SUBS_DEVICE").then(|| "env".to_string()));
        let cli = Cli {
            language: Some("de".into()),
            no_cleanup: true,
            ..Cli::default()
        }
        .layer();
        let c = Config::from_layers([file, env, cli]).unwrap();
        assert_eq!(c.language, "de"); // cli beats file
        assert_eq!(c.device.as_deref(), Some("env")); // env beats file
        assert!(!c.cleanup);
        assert_eq!(c.vad.silence_ms, 300); // file beats default
        assert_eq!(c.vad.open, 0.02);
        assert_eq!(c.vad.close, VadConfig::default().close); // untouched keys keep defaults
    }

    #[test]
    fn legacy_env_names_map_to_keys() {
        let env = env_layer(|k| Some(format!("/x/{k}")));
        let c = Config::from_layers([env]).unwrap();
        assert_eq!(c.whisper_bin, PathBuf::from("/x/WHISPER_BIN"));
        assert_eq!(c.whisper_model, PathBuf::from("/x/WHISPER_MODEL"));
        assert_eq!(c.llama_bin, PathBuf::from("/x/LLAMA_BIN"));
        assert_eq!(c.llama_model, PathBuf::from("/x/LLAMA_MODEL"));
        assert_eq!(c.llama_server, Some(PathBuf::from("/x/LLAMA_SERVER")));
        assert_eq!(c.device.as_deref(), Some("/x/SUBS_DEVICE"));
    }

    #[test]
    fn bad_configs_name_the_key() {
        let cases = [
            ("language = \" \"", "language"),
            ("channel_size = 0", "channel_size"),
            ("partial_every_ms = 0", "partial_every_ms"),
            ("cleanup_timeout_ms = 0", "cleanup_timeout_ms"),
            ("[vad]\nopen = 1.5", "vad.open"),
            ("[vad]\nclose = 0.5", "vad.close"),
            ("[vad]\nnoise_ratio = 0.5", "vad.noise_ratio"),
            ("[vad]\nsilence_ms = 5", "vad.silence_ms"),
            ("[vad]\nmax_speech_ms = 100", "vad.max_speech_ms"),
            ("[vad]\npreroll_ms = 9000", "vad.preroll_ms"),
            ("bogus = 1", "bogus"),
            ("channel_size = \"x\"", "channel_size"),
        ];
        for (toml, key) in cases {
            let err = Config::from_layers([table(toml)]).unwrap_err().to_string();
            assert!(err.contains(key), "{toml:?} -> {err}");
        }
    }

    #[test]
    fn missing_files_name_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("whisper");
        let model = dir.path().join("m.bin");
        std::fs::write(&bin, "").unwrap();
        let mut c = Config {
            whisper_bin: bin,
            whisper_model: model.clone(),
            cleanup: false,
            ..Config::default()
        };
        let err = c.check_files().unwrap_err().to_string();
        assert!(err.contains("whisper_model"), "{err}");
        assert!(err.contains(&model.display().to_string()), "{err}");
        std::fs::write(&model, "").unwrap();
        c.check_files().unwrap();
        c.cleanup = true; // llama files are checked only when cleanup is on
        let err = c.check_files().unwrap_err().to_string();
        assert!(err.contains("llama_model"), "{err}");
    }

    #[test]
    fn config_file_parses_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, "language = \"fr\"\n").unwrap();
        let t = parse_file(&std::fs::read_to_string(&p).unwrap(), &p).unwrap();
        assert_eq!(Config::from_layers([t]).unwrap().language, "fr");
        assert!(parse_file("language = ", &p).is_err());
    }
}
