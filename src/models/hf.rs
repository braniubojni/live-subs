//! Hugging Face model search. Tests parse a recorded response and never go online.

use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub repo: String,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub licence: String,
    pub url: String,
}

#[async_trait::async_trait]
pub trait ModelSearch: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<RemoteFile>>;
}

pub struct HfHub {
    client: reqwest::Client,
}

impl HfHub {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl ModelSearch for HfHub {
    async fn search(&self, query: &str) -> Result<Vec<RemoteFile>> {
        let url = format!(
            "https://huggingface.co/api/models?search={}&filter=gguf&limit=5",
            encode_component(query)
        );
        let body = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("huggingface search: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("huggingface search: {e}"))?;
        let models: Vec<HfModel> =
            serde_json::from_str(&body).map_err(|e| anyhow!("huggingface search: {e}"))?;
        let mut files = Vec::new();
        for model in models {
            if model.siblings.is_empty() {
                if let Some(id) = model.id.clone() {
                    let detail = self
                        .client
                        .get(format!(
                            "https://huggingface.co/api/models/{}",
                            encode_path(&id)
                        ))
                        .send()
                        .await
                        .map_err(|e| anyhow!("huggingface model: {e}"))?
                        .text()
                        .await
                        .map_err(|e| anyhow!("huggingface model: {e}"))?;
                    files.extend(parse_models(&detail)?);
                }
            } else {
                files.extend(files_of(model));
            }
        }
        Ok(files)
    }
}

/// Array of models, or one model object. Keeps `.gguf` and `.bin` only.
pub fn parse_models(body: &str) -> Result<Vec<RemoteFile>> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    if value.is_array() {
        let models: Vec<HfModel> = serde_json::from_value(value)?;
        Ok(models.into_iter().flat_map(files_of).collect())
    } else {
        let model: HfModel = serde_json::from_value(value)?;
        Ok(files_of(model))
    }
}

#[derive(Debug, Deserialize)]
struct HfModel {
    id: Option<String>,
    license: Option<String>,
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Debug, Deserialize)]
struct Sibling {
    rfilename: String,
    size: Option<u64>,
    lfs: Option<Lfs>,
}

#[derive(Debug, Deserialize)]
struct Lfs {
    oid: Option<String>,
    sha256: Option<String>,
    size: Option<u64>,
}

fn files_of(model: HfModel) -> Vec<RemoteFile> {
    let repo = model.id.unwrap_or_default();
    let licence = model.license.unwrap_or_else(|| "unknown".into());
    model
        .siblings
        .into_iter()
        .filter(|s| {
            let n = s.rfilename.to_ascii_lowercase();
            n.ends_with(".gguf") || n.ends_with(".bin")
        })
        .map(|s| {
            let sha256 = s
                .lfs
                .as_ref()
                .and_then(|l| l.oid.as_deref().or(l.sha256.as_deref()))
                .unwrap_or("")
                .trim_start_matches("sha256:")
                .to_string();
            let size = s.size.or(s.lfs.as_ref().and_then(|l| l.size)).unwrap_or(0);
            let url = format!(
                "https://huggingface.co/{}/resolve/main/{}",
                encode_path(&repo),
                encode_component(&s.rfilename)
            );
            RemoteFile {
                repo: repo.clone(),
                filename: s.rfilename,
                size,
                sha256,
                licence: licence.clone(),
                url,
            }
        })
        .collect()
}

fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn encode_path(s: &str) -> String {
    s.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {
        "id": "org/name",
        "license": "apache-2.0",
        "siblings": [
          {"rfilename": "model.Q4.gguf", "size": 20, "lfs": {"oid": "sha256:abc123"}},
          {"rfilename": "tiny.bin", "size": 8, "lfs": {"sha256": "def456"}},
          {"rfilename": "README.md", "size": 3}
        ]
      }
    ]"#;

    #[test]
    fn parser_lists_gguf_and_bin_with_sizes() {
        let files = parse_models(FIXTURE).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "model.Q4.gguf");
        assert_eq!(files[0].size, 20);
        assert_eq!(files[0].sha256, "abc123");
        assert_eq!(files[0].licence, "apache-2.0");
        assert_eq!(
            files[0].url,
            "https://huggingface.co/org/name/resolve/main/model.Q4.gguf"
        );
        assert_eq!(files[1].filename, "tiny.bin");
        assert_eq!(files[1].sha256, "def456");
        assert!(parse_models("{").is_err());
    }
}
