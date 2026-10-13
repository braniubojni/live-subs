use anyhow::{anyhow, Result};
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Last 4 s at 16 kHz. Partials transcribe this tail; finals keep the utterance.
pub const PARTIAL_WINDOW_SAMPLES: usize = 16_000 * 4;

/// Samples a partial may send. Shorter input is returned whole.
pub fn partial_window(speech: &[f32]) -> &[f32] {
    let start = speech.len().saturating_sub(PARTIAL_WINDOW_SAMPLES);
    &speech[start..]
}

/// Lock `auto` once whisper reports a language at least this confident.
const LANG_LOCK_MIN: f32 = 0.8;

/// Known silence hallucinations, compared after normalization. One list.
const HALLUCINATIONS: &[&str] = &[
    "thank you",
    "thanks for watching",
    "thank you for watching",
    "thanks for listening",
    "please subscribe",
    "please subscribe to the channel",
    "subtitles by the amara.org community",
];

/// 16 kHz mono f32 in, one utterance out.
///
/// `transcribe` stays the method fakes implement. Prompt carry and the locked
/// language are extra methods with defaults, so the signature does not move.
#[async_trait::async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String>;

    /// `prompt` is the previous final line. The default ignores it.
    async fn transcribe_with_prompt(&self, pcm16k: &[f32], prompt: Option<&str>) -> Result<String> {
        let _ = prompt;
        self.transcribe(pcm16k).await
    }

    /// Locked or configured language. `None` while still on `auto`.
    fn language(&self) -> Option<String> {
        None
    }
}

pub struct WhisperCli {
    pub bin: PathBuf,
    pub model: PathBuf,
    /// whisper `-l` value from config. `auto` may lock after a confident detect.
    pub language: String,
    /// Set once `auto` detects a language with probability >= [`LANG_LOCK_MIN`].
    locked: Mutex<Option<String>>,
}

impl WhisperCli {
    pub fn new(
        bin: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        language: impl Into<String>,
    ) -> Self {
        Self {
            bin: bin.into(),
            model: model.into(),
            language: language.into(),
            locked: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Transcriber for WhisperCli {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String> {
        self.transcribe_with_prompt(pcm16k, None).await
    }

    async fn transcribe_with_prompt(&self, pcm16k: &[f32], prompt: Option<&str>) -> Result<String> {
        // Guard first: a partial write, an error, or a cancelled future all
        // still delete the file.
        let wav = TempWav(wav_path(&std::env::temp_dir()));
        write_wav(&wav.0, pcm16k, 16_000)?;

        let lang = effective_language(&self.language, &self.locked.lock().unwrap());
        // whisper-cli prints the text. `-nt` strips timestamps so the subtitle
        // line is the whole stdout after we trim the log noise.
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.arg("-m")
            .arg(&self.model)
            .arg("-f")
            .arg(&wav.0)
            .args(["-l", &lang, "-nt", "-np", "-t", "4"]);
        if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
            cmd.arg("--prompt").arg(prompt);
        }
        // Dropping the future (timeout, shutdown) must not orphan the child.
        let out = cmd.kill_on_drop(true).output().await?;
        if !out.status.success() {
            return Err(anyhow!(
                "whisper-cli failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        consider_lock(
            &self.language,
            &mut self.locked.lock().unwrap(),
            parse_detected_language(&stderr),
        );
        let text = parse_whisper_stdout(&String::from_utf8_lossy(&out.stdout));
        Ok(filter_hallucination(&text))
    }

    fn language(&self) -> Option<String> {
        reported_language(&self.language, &self.locked.lock().unwrap())
    }
}

/// Drop `whisper_*` log lines and non-speech tags. Tags alone yield an empty line.
pub fn parse_whisper_stdout(stdout: &str) -> String {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("whisper_"))
        .map(strip_tags)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_tags(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' || c == '(' {
            let end = if c == '[' { ']' } else { ')' };
            for d in chars.by_ref() {
                if d == end {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Empty when `text` is a known silence hallucination or a repeated-token loop.
pub fn filter_hallucination(text: &str) -> String {
    let tokens = normalize_tokens(text);
    if tokens.is_empty() {
        return String::new();
    }
    let line = tokens.join(" ");
    if HALLUCINATIONS.contains(&line.as_str()) || is_token_loop(&tokens) {
        String::new()
    } else {
        text.to_string()
    }
}

fn normalize_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// Same token three times in a row, or a phrase of two or more tokens repeated
/// so it fills the line.
fn is_token_loop(tokens: &[String]) -> bool {
    let mut run = 1;
    for pair in tokens.windows(2) {
        if pair[0] == pair[1] {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 1;
        }
    }
    for len in 2..=tokens.len() / 2 {
        if !tokens.len().is_multiple_of(len) {
            continue;
        }
        let phrase = &tokens[..len];
        if tokens.chunks(len).all(|chunk| chunk == phrase) {
            return true;
        }
    }
    false
}

/// `auto-detected language: en (p = 0.98)` from whisper.cpp stderr.
pub fn parse_detected_language(stderr: &str) -> Option<(String, f32)> {
    for line in stderr.lines() {
        let Some(rest) = line.split("auto-detected language:").nth(1) else {
            continue;
        };
        let rest = rest.trim();
        let Some(code) = rest.split_whitespace().next() else {
            continue;
        };
        let code = code.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if code.is_empty() {
            continue;
        }
        let Some(p) = rest.split("p =").nth(1) else {
            continue;
        };
        let Ok(p) = p.trim().trim_end_matches(')').trim().parse::<f32>() else {
            continue;
        };
        return Some((code.to_string(), p));
    }
    None
}

fn consider_lock(configured: &str, locked: &mut Option<String>, detected: Option<(String, f32)>) {
    if configured != "auto" || locked.is_some() {
        return;
    }
    if let Some((code, p)) = detected {
        if p >= LANG_LOCK_MIN {
            crate::settings::emit(&crate::settings::DevEvent::LanguageLock {
                code: &code,
                probability: p,
            });
            *locked = Some(code);
        }
    }
}

fn effective_language(configured: &str, locked: &Option<String>) -> String {
    if configured == "auto" {
        locked.clone().unwrap_or_else(|| "auto".into())
    } else {
        configured.to_string()
    }
}

fn reported_language(configured: &str, locked: &Option<String>) -> Option<String> {
    if configured == "auto" {
        locked.clone()
    } else if configured.is_empty() {
        None
    } else {
        Some(configured.to_string())
    }
}

static WAV_SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique per process (pid) and per call (counter).
fn wav_path(dir: &Path) -> PathBuf {
    let seq = WAV_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("subs-{}-{seq}.wav", std::process::id()))
}

/// Deletes the file on drop: success, error, and cancel.
struct TempWav(PathBuf);

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_wav(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // create_new = O_EXCL: never overwrite a file we did not make.
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut w = hound::WavWriter::new(BufWriter::new(file), spec)?;
    for s in samples {
        let clipped = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        w.write_sample(clipped)?;
    }
    w.finalize()?;
    Ok(())
}

/// In-process whisper.cpp. The model is loaded once in [`WhisperRs::load`];
/// each call reuses that state on a blocking thread.
#[cfg(feature = "whisper-rs")]
mod whisper_inproc {
    use super::{
        consider_lock, effective_language, filter_hallucination, parse_whisper_stdout,
        reported_language, LANG_LOCK_MIN,
    };
    use anyhow::{anyhow, Result};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use whisper_rs::{
        get_lang_str, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
        WhisperState,
    };

    struct Inner {
        state: Mutex<WhisperState>,
        language: String,
        locked: Mutex<Option<String>>,
    }

    pub struct WhisperRs {
        inner: Arc<Inner>,
    }

    impl WhisperRs {
        /// Load `model` once. Later `transcribe` calls do not reload it.
        pub fn load(model: impl AsRef<Path>, language: impl Into<String>) -> Result<Self> {
            let model = model.as_ref();
            let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
                .map_err(|e| anyhow!("whisper-rs load {}: {e}", model.display()))?;
            let state = ctx
                .create_state()
                .map_err(|e| anyhow!("whisper-rs state: {e}"))?;
            // `state` holds its own `Arc` of the model, so `ctx` can drop.
            drop(ctx);
            Ok(Self {
                inner: Arc::new(Inner {
                    state: Mutex::new(state),
                    language: language.into(),
                    locked: Mutex::new(None),
                }),
            })
        }
    }

    #[async_trait::async_trait]
    impl super::Transcriber for WhisperRs {
        async fn transcribe(&self, pcm16k: &[f32]) -> Result<String> {
            self.transcribe_with_prompt(pcm16k, None).await
        }

        async fn transcribe_with_prompt(
            &self,
            pcm16k: &[f32],
            prompt: Option<&str>,
        ) -> Result<String> {
            let inner = Arc::clone(&self.inner);
            let pcm = pcm16k.to_vec();
            let prompt = prompt.map(str::to_string);
            tokio::task::spawn_blocking(move || infer(&inner, &pcm, prompt.as_deref()))
                .await
                .map_err(|e| anyhow!("whisper-rs task failed: {e}"))?
        }

        fn language(&self) -> Option<String> {
            reported_language(&self.inner.language, &self.inner.locked.lock().unwrap())
        }
    }

    fn infer(inner: &Inner, pcm: &[f32], prompt: Option<&str>) -> Result<String> {
        let mut state = inner.state.lock().unwrap();
        // Confidence is only available from lang_detect, not from full().
        // The first auto call pays for it; a lock skips it afterwards.
        if inner.language == "auto"
            && inner.locked.lock().unwrap().is_none()
            && state.pcm_to_mel(pcm, 4).is_ok()
        {
            if let Ok((id, probs)) = state.lang_detect(0, 4) {
                let p = usize::try_from(id)
                    .ok()
                    .and_then(|i| probs.get(i).copied())
                    .unwrap_or(0.0);
                if p >= LANG_LOCK_MIN {
                    if let Some(code) = lang_code(id) {
                        consider_lock(
                            &inner.language,
                            &mut inner.locked.lock().unwrap(),
                            Some((code, p)),
                        );
                    }
                }
            }
        }
        let lang = effective_language(&inner.language, &inner.locked.lock().unwrap());
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        // Reused state must not keep the previous utterance unless asked.
        params.set_no_context(prompt.is_none());
        params.set_language(Some(&lang));
        if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
            params.set_initial_prompt(prompt);
        }
        state
            .full(params, pcm)
            .map_err(|e| anyhow!("whisper-rs: {e}"))?;
        let mut parts = Vec::new();
        for segment in state.as_iter() {
            let line = segment.to_string();
            if !line.trim().is_empty() {
                parts.push(line);
            }
        }
        let text = parse_whisper_stdout(&parts.join(" "));
        Ok(filter_hallucination(&text))
    }

    fn lang_code(id: i32) -> Option<String> {
        let name = get_lang_str(id)?;
        (!name.is_empty()).then(|| name.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::stt::Transcriber;

        #[test]
        fn load_skips_without_a_model_file() {
            let path = Path::new("models/ggml-tiny.bin");
            if !path.is_file() {
                eprintln!("skip: {} missing", path.display());
                return;
            }
            let engine = WhisperRs::load(path, "en").unwrap();
            assert_eq!(engine.language().as_deref(), Some("en"));
        }
    }
}

#[cfg(feature = "whisper-rs")]
pub use whisper_inproc::WhisperRs;

/// Cancellation drops `fut`; callees that spawn children must use `kill_on_drop`.
pub async fn with_timeout<T>(
    fut: impl std::future::Future<Output = Result<T>>,
    limit: Duration,
) -> Result<T> {
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => v,
        Err(_) => Err(anyhow!("stt timed out")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn wav_paths_are_unique_under_concurrency() {
        let dir = std::env::temp_dir();
        let paths: HashSet<PathBuf> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..1000).map(|_| s.spawn(|| wav_path(&dir))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(paths.len(), 1000);
    }

    #[test]
    fn temp_wav_is_removed_on_drop() {
        let wav = TempWav(wav_path(&std::env::temp_dir()));
        write_wav(&wav.0, &[0.0, 0.5, -0.5], 16_000).unwrap();
        let path = wav.0.clone();
        assert!(path.exists());
        drop(wav);
        assert!(!path.exists());
    }

    #[test]
    fn parse_whisper_stdout_strips_logs_and_tags() {
        let cases = [
            (
                "whisper_init_from_file: loading model\nHello there",
                "Hello there",
            ),
            ("[BLANK_AUDIO]", ""),
            ("Hello [MUSIC] there", "Hello there"),
            ("(applause)", ""),
            ("  \nwhisper_print_timings: load = 10 ms\n", ""),
            ("[00:00:00.000 --> 00:00:01.000]  Hi", "Hi"),
            ("Hello\nworld", "Hello world"),
            ("See you (laughter) soon", "See you soon"),
        ];
        for (stdout, want) in cases {
            assert_eq!(parse_whisper_stdout(stdout), want, "{stdout:?}");
        }
    }

    #[test]
    fn filter_drops_hallucinations_and_loops() {
        let cases = [
            ("Thank you.", ""),
            ("Thanks for watching!", ""),
            ("Thanks for watching. Thanks for watching.", ""),
            ("the the the", ""),
            ("I went home.", "I went home."),
            ("bye bye", "bye bye"),
            ("Thank you for the ride", "Thank you for the ride"),
        ];
        for (text, want) in cases {
            assert_eq!(filter_hallucination(text), want, "{text:?}");
        }
    }

    #[test]
    fn language_locks_only_when_auto_and_confident() {
        let stderr = "whisper_full_with_state: auto-detected language: en (p = 0.97)\n";
        let detected = parse_detected_language(stderr);
        assert_eq!(detected.as_ref().map(|(c, _)| c.as_str()), Some("en"));
        let p = detected.as_ref().unwrap().1;
        assert!((p - 0.97).abs() < 1e-6);

        let mut locked = None;
        consider_lock("auto", &mut locked, detected.clone());
        assert_eq!(locked.as_deref(), Some("en"));
        assert_eq!(effective_language("auto", &locked), "en");
        assert_eq!(reported_language("auto", &locked).as_deref(), Some("en"));

        let mut weak = None;
        consider_lock(
            "auto",
            &mut weak,
            parse_detected_language("auto-detected language: de (p = 0.40)"),
        );
        assert!(weak.is_none());
        assert_eq!(effective_language("auto", &weak), "auto");

        let mut fixed = None;
        consider_lock("fr", &mut fixed, detected);
        assert!(fixed.is_none());
        assert_eq!(effective_language("fr", &fixed), "fr");
        assert_eq!(reported_language("fr", &None).as_deref(), Some("fr"));
        assert!(parse_detected_language("nothing here").is_none());
    }

    #[test]
    fn partial_window_keeps_the_last_four_seconds() {
        let short = vec![0.1; 100];
        assert_eq!(partial_window(&short).len(), 100);
        let long = vec![0.2; PARTIAL_WINDOW_SAMPLES + 50];
        let tail = partial_window(&long);
        assert_eq!(tail.len(), PARTIAL_WINDOW_SAMPLES);
        assert_eq!(tail[0], long[50]);
    }
}
