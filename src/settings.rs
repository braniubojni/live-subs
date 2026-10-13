//! Settings window state, language autocomplete, and `--dev` lines.
//! The window renders this; it does not decide it.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whisper language codes and English names. `Auto` is added by [`filter_languages`].
const LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh", "Chinese"),
    ("de", "German"),
    ("es", "Spanish"),
    ("ru", "Russian"),
    ("ko", "Korean"),
    ("fr", "French"),
    ("ja", "Japanese"),
    ("pt", "Portuguese"),
    ("tr", "Turkish"),
    ("pl", "Polish"),
    ("ca", "Catalan"),
    ("nl", "Dutch"),
    ("ar", "Arabic"),
    ("sv", "Swedish"),
    ("it", "Italian"),
    ("id", "Indonesian"),
    ("hi", "Hindi"),
    ("fi", "Finnish"),
    ("vi", "Vietnamese"),
    ("he", "Hebrew"),
    ("uk", "Ukrainian"),
    ("el", "Greek"),
    ("ms", "Malay"),
    ("cs", "Czech"),
    ("ro", "Romanian"),
    ("da", "Danish"),
    ("hu", "Hungarian"),
    ("ta", "Tamil"),
    ("no", "Norwegian"),
    ("th", "Thai"),
    ("ur", "Urdu"),
    ("hr", "Croatian"),
    ("bg", "Bulgarian"),
    ("lt", "Lithuanian"),
    ("la", "Latin"),
    ("mi", "Maori"),
    ("ml", "Malayalam"),
    ("cy", "Welsh"),
    ("sk", "Slovak"),
    ("te", "Telugu"),
    ("fa", "Persian"),
    ("lv", "Latvian"),
    ("bn", "Bengali"),
    ("sr", "Serbian"),
    ("az", "Azerbaijani"),
    ("sl", "Slovenian"),
    ("kn", "Kannada"),
    ("et", "Estonian"),
    ("mk", "Macedonian"),
    ("br", "Breton"),
    ("eu", "Basque"),
    ("is", "Icelandic"),
    ("hy", "Armenian"),
    ("ne", "Nepali"),
    ("mn", "Mongolian"),
    ("bs", "Bosnian"),
    ("kk", "Kazakh"),
    ("sq", "Albanian"),
    ("sw", "Swahili"),
    ("gl", "Galician"),
    ("mr", "Marathi"),
    ("pa", "Punjabi"),
    ("si", "Sinhala"),
    ("km", "Khmer"),
    ("sn", "Shona"),
    ("yo", "Yoruba"),
    ("so", "Somali"),
    ("af", "Afrikaans"),
    ("oc", "Occitan"),
    ("ka", "Georgian"),
    ("be", "Belarusian"),
    ("tg", "Tajik"),
    ("sd", "Sindhi"),
    ("gu", "Gujarati"),
    ("am", "Amharic"),
    ("yi", "Yiddish"),
    ("lo", "Lao"),
    ("uz", "Uzbek"),
    ("fo", "Faroese"),
    ("ht", "Haitian Creole"),
    ("ps", "Pashto"),
    ("tk", "Turkmen"),
    ("nn", "Nynorsk"),
    ("mt", "Maltese"),
    ("sa", "Sanskrit"),
    ("lb", "Luxembourgish"),
    ("my", "Myanmar"),
    ("bo", "Tibetan"),
    ("tl", "Tagalog"),
    ("mg", "Malagasy"),
    ("as", "Assamese"),
    ("tt", "Tatar"),
    ("haw", "Hawaiian"),
    ("ln", "Lingala"),
    ("ha", "Hausa"),
    ("ba", "Bashkir"),
    ("jw", "Javanese"),
    ("su", "Sundanese"),
    ("yue", "Cantonese"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
}

/// `Auto` first on an empty query. `en`, `eng`, and `English` all hit English.
pub fn filter_languages(query: &str) -> Vec<Lang> {
    let q = query.trim().to_ascii_lowercase();
    let mut rows = Vec::new();
    if q.is_empty() || "auto".starts_with(&q) {
        rows.push(Lang {
            code: "auto",
            name: "Auto",
        });
    }
    for &(code, name) in LANGS {
        if q.is_empty() || code.starts_with(&q) || name.to_ascii_lowercase().starts_with(&q) {
            rows.push(Lang { code, name });
        }
    }
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Stt,
    Cleanup,
}

impl SlotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSlot {
    pub id: String,
    pub path: String,
    pub licence: String,
    pub present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub got: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsState {
    pub stt: Option<ModelSlot>,
    pub cleanup: Option<ModelSlot>,
    /// `auto` or a whisper code. Stored as `Config.language`.
    pub language: String,
    pub prompt_carry: bool,
    pub device: Option<String>,
    pub running: bool,
    pub error: Option<String>,
    pub stt_progress: Option<Progress>,
    pub cleanup_progress: Option<Progress>,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            stt: None,
            cleanup: None,
            language: "auto".into(),
            prompt_carry: false,
            device: None,
            running: false,
            error: None,
            stt_progress: None,
            cleanup_progress: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Select {
        kind: SlotKind,
        id: String,
        path: String,
        licence: String,
        present: bool,
    },
    Progress {
        kind: SlotKind,
        got: u64,
        total: u64,
    },
    Downloaded(SlotKind),
    Error(String),
    Start,
    Stop,
    Language(String),
    PromptCarry(bool),
    Device(Option<String>),
    Cancel(SlotKind),
}

impl SettingsState {
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Select {
                kind,
                id,
                path,
                licence,
                present,
            } => {
                if !present {
                    self.error = Some(format!("{id}: file not found"));
                }
                let slot = ModelSlot {
                    id,
                    path,
                    licence,
                    present,
                };
                match kind {
                    SlotKind::Stt => self.stt = Some(slot),
                    SlotKind::Cleanup => self.cleanup = Some(slot),
                }
            }
            Event::Progress { kind, got, total } => {
                let p = Some(Progress { got, total });
                match kind {
                    SlotKind::Stt => self.stt_progress = p,
                    SlotKind::Cleanup => self.cleanup_progress = p,
                }
            }
            Event::Downloaded(kind) => {
                let slot = match kind {
                    SlotKind::Stt => &mut self.stt,
                    SlotKind::Cleanup => &mut self.cleanup,
                };
                if let Some(s) = slot {
                    s.present = true;
                }
                match kind {
                    SlotKind::Stt => self.stt_progress = None,
                    SlotKind::Cleanup => self.cleanup_progress = None,
                }
                self.error = None;
            }
            Event::Error(msg) => {
                self.error = Some(msg);
                self.running = false;
            }
            Event::Start => match self.block_reason() {
                Some(msg) => {
                    self.error = Some(msg);
                    self.running = false;
                }
                None => {
                    self.error = None;
                    self.running = true;
                }
            },
            Event::Stop => self.running = false,
            Event::Language(code) => self.language = code,
            Event::PromptCarry(on) => self.prompt_carry = on,
            Event::Device(name) => self.device = name,
            Event::Cancel(kind) => match kind {
                SlotKind::Stt => self.stt_progress = None,
                SlotKind::Cleanup => self.cleanup_progress = None,
            },
        }
    }

    /// Why Start must refuse. Names the model id when the file is absent.
    pub fn block_reason(&self) -> Option<String> {
        match &self.stt {
            None => Some("no speech model".into()),
            Some(s) if !s.present => Some(format!("{}: file not found", s.id)),
            _ => match &self.cleanup {
                None => Some("no cleanup model".into()),
                Some(s) if !s.present => Some(format!("{}: file not found", s.id)),
                _ => None,
            },
        }
    }
}

static DEV: AtomicBool = AtomicBool::new(false);

pub fn set_dev(on: bool) {
    DEV.store(on, Ordering::Relaxed);
}

pub fn dev_enabled() -> bool {
    DEV.load(Ordering::Relaxed)
}

/// One `--dev` step. Lifetimes so a cue line does not allocate the text twice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DevEvent<'a> {
    WindowOpened,
    ModelChosen {
        kind: &'a str,
        id: &'a str,
    },
    Download {
        id: &'a str,
        got: u64,
        total: u64,
    },
    CaptureStarted,
    Cue {
        kind: &'a str,
        id: u64,
        lang: &'a str,
        text: &'a str,
        hint: Option<&'a str>,
    },
    LanguageLock {
        code: &'a str,
        probability: f32,
    },
}

pub fn format_line(event: &DevEvent) -> String {
    match *event {
        DevEvent::WindowOpened => "settings window opened".into(),
        DevEvent::ModelChosen { kind, id } => format!("model chosen kind={kind} id={id}"),
        DevEvent::Download { id, got, total } => format!("download id={id} {got}/{total}"),
        DevEvent::CaptureStarted => "capture started".into(),
        DevEvent::Cue {
            kind,
            id,
            lang,
            text,
            hint: None,
        } => format!("cue kind={kind} id={id} lang={lang} text={text}"),
        DevEvent::Cue {
            kind,
            id,
            lang,
            text,
            hint: Some(hint),
        } => format!("cue kind={kind} id={id} lang={lang} text={text} hint={hint}"),
        DevEvent::LanguageLock { code, probability } => {
            format!("language lock code={code} p={probability:.2}")
        }
    }
}

/// Print [`format_line`] when `--dev` is on. No-op otherwise.
pub fn emit(event: &DevEvent) {
    if dev_enabled() {
        eprintln!("{}", format_line(event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn language_filter_matches_code_prefix_and_english_name() {
        let empty = filter_languages("");
        assert_eq!(empty[0].code, "auto");
        assert!(empty.iter().any(|l| l.code == "en"));

        let en = filter_languages("en");
        assert!(en.iter().any(|l| l.code == "en" && l.name == "English"));
        assert!(!en.iter().any(|l| l.code == "ru"));

        assert!(filter_languages("eng").iter().any(|l| l.code == "en"));
        assert!(filter_languages("English").iter().any(|l| l.code == "en"));
        assert!(filter_languages("ru").iter().any(|l| l.name == "Russian"));
    }

    #[test]
    fn chosen_language_round_trips_through_config() {
        let mut state = SettingsState::default();
        state.apply(Event::Language("en".into()));
        assert_eq!(state.language, "en");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live-subs.toml");
        let cfg = Config {
            language: state.language.clone(),
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let table = text.parse().unwrap();
        let loaded = Config::from_layers([table]).unwrap();
        assert_eq!(loaded.language, "en");
    }

    #[test]
    fn missing_model_blocks_start_and_names_the_id() {
        let mut s = SettingsState::default();
        s.apply(Event::Select {
            kind: SlotKind::Stt,
            id: "ggml-tiny".into(),
            path: "models/ggml-tiny.bin".into(),
            licence: "MIT".into(),
            present: false,
        });
        s.apply(Event::Select {
            kind: SlotKind::Cleanup,
            id: "s1-mini-q4".into(),
            path: "models/s1.gguf".into(),
            licence: "Apache-2.0".into(),
            present: true,
        });
        assert!(s.error.as_deref().unwrap().contains("ggml-tiny"));
        s.apply(Event::Start);
        assert!(!s.running);
        assert!(s.error.as_deref().unwrap().contains("ggml-tiny"));
        s.apply(Event::Downloaded(SlotKind::Stt));
        s.apply(Event::Start);
        assert!(s.running);
        assert!(s.error.is_none());
    }

    #[test]
    fn prompt_carry_and_progress_are_events() {
        let mut s = SettingsState::default();
        s.apply(Event::PromptCarry(true));
        assert!(s.prompt_carry);
        s.apply(Event::Progress {
            kind: SlotKind::Stt,
            got: 10,
            total: 100,
        });
        assert_eq!(
            s.stt_progress,
            Some(Progress {
                got: 10,
                total: 100
            })
        );
        s.apply(Event::Stop);
        assert!(!s.running);
    }

    #[test]
    fn dev_lines_name_the_step() {
        let chosen = format_line(&DevEvent::ModelChosen {
            kind: "stt",
            id: "ggml-tiny",
        });
        assert_eq!(chosen, "model chosen kind=stt id=ggml-tiny");
        let cue = format_line(&DevEvent::Cue {
            kind: "final",
            id: 2,
            lang: "en",
            text: "hello",
            hint: Some("prior"),
        });
        assert!(cue.contains("hint=prior"));
        assert!(!format_line(&DevEvent::Cue {
            kind: "final",
            id: 1,
            lang: "en",
            text: "hello",
            hint: None,
        })
        .contains("hint="));
        let lock = format_line(&DevEvent::LanguageLock {
            code: "ru",
            probability: 0.91,
        });
        assert_eq!(lock, "language lock code=ru p=0.91");
        assert_eq!(
            format_line(&DevEvent::WindowOpened),
            "settings window opened"
        );
        assert_eq!(format_line(&DevEvent::CaptureStarted), "capture started");
        set_dev(false);
        emit(&DevEvent::WindowOpened);
    }
}
