use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::Duration;

const SYSTEM: &str = "\
You clean one speech-to-text line. Remove filler words and stutters. \
Fix capitalization and punctuation. Do not add, translate, or rephrase. \
Output only the cleaned line.";

#[async_trait::async_trait]
pub trait Cleaner: Send + Sync {
    async fn clean(&self, raw: &str) -> Result<String>;
}

pub struct LlamaCli {
    pub bin: PathBuf,
    pub model: PathBuf,
}

#[async_trait::async_trait]
impl Cleaner for LlamaCli {
    async fn clean(&self, raw: &str) -> Result<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(String::new());
        }
        // One sentence, tiny context, greedy. This is the whole latency budget.
        let prompt = format!(
            "<|im_start|>system\n{SYSTEM}<|im_end|>\n<|im_start|>user\n{raw}<|im_end|>\n<|im_start|>assistant\n"
        );
        let out = tokio::time::timeout(
            Duration::from_millis(700),
            tokio::process::Command::new(&self.bin).args([
                "-m",
                self.model.to_str().unwrap_or(""),
                "-p",
                &prompt,
                "-n",
                "64",
                "--temp",
                "0",
                "-c",
                "512",
                "--no-display-prompt",
                "-ngl",
                "99",
            ]).output(),
        )
        .await
        .map_err(|_| anyhow!("cleanup timed out"))??;
        if !out.status.success() {
            // A missing model must not drop the subtitle. Return the raw line.
            tracing::warn!(
                "llama-cli failed, keeping raw: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return Ok(raw.to_string());
        }
        let cleaned = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or(raw)
            .to_string();
        Ok(if cleaned.chars().count() > raw.chars().count() * 2 {
            // Hallucinated expansion. Keep the source line.
            raw.to_string()
        } else {
            cleaned
        })
    }
}
