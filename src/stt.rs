use anyhow::{anyhow, Result};
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 16 kHz mono f32 in, one utterance out.
/// v0 shells out to whisper.cpp so you do not compile whisper into the binary
/// until the overlay and the queue are proven. Swap this for `whisper-rs`
/// without touching the pipeline.
#[async_trait::async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String>;
}

pub struct WhisperCli {
    pub bin: PathBuf,
    pub model: PathBuf,
    /// whisper `-l` value; `auto` detects.
    pub language: String,
}

#[async_trait::async_trait]
impl Transcriber for WhisperCli {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String> {
        // Guard first: a partial write, an error, or a cancelled future all
        // still delete the file.
        let wav = TempWav(wav_path(&std::env::temp_dir()));
        write_wav(&wav.0, pcm16k, 16_000)?;

        // whisper-cli prints the text. `-nt` strips timestamps so the subtitle
        // line is the whole stdout after we trim the log noise.
        let out = tokio::process::Command::new(&self.bin)
            .arg("-m")
            .arg(&self.model)
            .arg("-f")
            .arg(&wav.0)
            .args(["-l", &self.language, "-nt", "-np", "-t", "4"])
            // Dropping the future (timeout, shutdown) must not orphan the child.
            .kill_on_drop(true)
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow!(
                "whisper-cli failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("whisper_"))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(text)
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
}
