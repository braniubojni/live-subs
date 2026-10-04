use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
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
}

#[async_trait::async_trait]
impl Transcriber for WhisperCli {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String> {
        let dir = std::env::temp_dir();
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let wav = dir.join(format!("subs-{id}.wav"));
        write_wav(&wav, pcm16k, 16_000)?;

        // whisper-cli prints the text. `-nt` strips timestamps so the subtitle
        // line is the whole stdout after we trim the log noise.
        let out = tokio::process::Command::new(&self.bin)
            .args([
                "-m",
                self.model.to_str().unwrap_or(""),
                "-f",
                wav.to_str().unwrap_or(""),
                "-l",
                "auto",
                "-nt",
                "-np",
                "-t",
                "4",
            ])
            .output()
            .await?;
        let _ = std::fs::remove_file(&wav);
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

fn write_wav(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        let clipped = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        w.write_sample(clipped)?;
    }
    w.finalize()?;
    Ok(())
}

pub async fn with_timeout<T>(
    fut: impl std::future::Future<Output = Result<T>>,
    limit: Duration,
) -> Result<T> {
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => v,
        Err(_) => Err(anyhow!("stt timed out")),
    }
}
