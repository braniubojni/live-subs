use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        let prompt = prompt(raw);
        let out = tokio::time::timeout(
            Duration::from_millis(700),
            tokio::process::Command::new(&self.bin)
                .arg("-m")
                .arg(&self.model)
                .args([
                    "-p",
                    &prompt,
                    "-n",
                    "64",
                    "--temp",
                    "0",
                    "-c",
                    "512",
                    "--no-display-prompt",
                    // llama-completion honours this. New llama-cli ignores it and hangs
                    // at ">" (llama.cpp#27214).
                    "-no-cnv",
                    "-ngl",
                    "99",
                ])
                // Dropping the future (timeout, shutdown) must not orphan the child.
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| anyhow!("cleanup timed out"))??;
        if !out.status.success() {
            // A missing model must not drop the subtitle. Return the raw line.
            tracing::warn!(
                "llama-completion failed, keeping raw: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return Ok(raw.to_string());
        }
        Ok(first_line(&String::from_utf8_lossy(&out.stdout), raw))
    }
}

fn prompt(raw: &str) -> String {
    format!(
        "<|im_start|>system\n{SYSTEM}<|im_end|>\n<|im_start|>user\n{raw}<|im_end|>\n<|im_start|>assistant\n"
    )
}

/// First non-empty line of model output, run through `accept_cleanup`.
fn first_line(out: &str, raw: &str) -> String {
    let cleaned = out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(raw);
    accept_cleanup(raw, cleaned)
}

/// Keeps one `llama-server` warm so the model loads once, not per line.
pub struct LlamaServer {
    addr: SocketAddr,
    // Held so `kill_on_drop` stops the server when the cleaner is dropped.
    _child: Option<tokio::process::Child>,
}

impl LlamaServer {
    pub fn spawn(bin: &Path, model: &Path) -> Result<Self> {
        // Grab a free port, release it, hand it to the server.
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))?
            .local_addr()?
            .port();
        // ponytail: no readiness wait; /completion answers 503 while the model loads and
        // that returns raw. Restart on death is the "Health and restart" box.
        let child = tokio::process::Command::new(bin)
            .arg("-m")
            .arg(model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-c", "512", "-ngl", "99"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self {
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            _child: Some(child),
        })
    }

    async fn complete(&self, raw: &str) -> Result<String> {
        let body = serde_json::json!({
            "prompt": prompt(raw),
            "n_predict": 64,
            "temperature": 0,
            "cache_prompt": true,
        })
        .to_string();
        let mut stream = tokio::net::TcpStream::connect(self.addr).await?;
        // HTTP/1.0: the server closes after the reply and never chunks it.
        let req = format!(
            "POST /completion HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        stream.write_all(req.as_bytes()).await?;
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await?;
        let split = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| anyhow!("malformed HTTP response"))?;
        let head = String::from_utf8_lossy(&resp[..split]);
        let status = head.split_whitespace().nth(1).unwrap_or("");
        if status != "200" {
            return Err(anyhow!("llama-server status {status}"));
        }
        let json: serde_json::Value = serde_json::from_slice(&resp[split + 4..])?;
        json["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("no content in response"))
    }
}

#[async_trait::async_trait]
impl Cleaner for LlamaServer {
    async fn clean(&self, raw: &str) -> Result<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(String::new());
        }
        match tokio::time::timeout(Duration::from_millis(700), self.complete(raw)).await {
            Ok(Ok(out)) => Ok(first_line(&out, raw)),
            Ok(Err(err)) => {
                tracing::warn!("llama-server failed, keeping raw: {err}");
                Ok(raw.to_string())
            }
            Err(_) => {
                tracing::warn!("llama-server timed out, keeping raw");
                Ok(raw.to_string())
            }
        }
    }
}

/// Keep the source line when the model adds nothing, or roughly doubles it.
/// A cleanup pass that invents a paragraph is worse than the raw subtitle.
pub fn accept_cleanup(raw: &str, cleaned: &str) -> String {
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.chars().count() > raw.chars().count() * 2 {
        raw.to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{accept_cleanup, Cleaner, LlamaServer, SYSTEM};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-shot fake llama-server. Returns its address and the request body it saw.
    async fn fake(
        response: &'static str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let (head_end, len) = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    break (i + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            sock.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(buf[head_end..head_end + len].to_vec()).unwrap()
        });
        (addr, task)
    }

    fn server(addr: std::net::SocketAddr) -> LlamaServer {
        LlamaServer { addr, _child: None }
    }

    #[tokio::test]
    async fn server_sends_expected_request() {
        let (addr, seen) = fake(
            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"content\":\"Ship it Friday.\"}",
        )
        .await;
        let out = server(addr).clean("um ship it friday").await.unwrap();
        assert_eq!(out, "Ship it Friday.");
        let body: serde_json::Value = serde_json::from_str(&seen.await.unwrap()).unwrap();
        assert_eq!(body["n_predict"], 64);
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["cache_prompt"], true);
        let prompt = body["prompt"].as_str().unwrap();
        assert!(prompt.contains(SYSTEM));
        assert!(prompt.contains("um ship it friday"));
    }

    #[tokio::test]
    async fn server_bad_response_returns_raw() {
        for resp in [
            "HTTP/1.0 500 Internal Server Error\r\n\r\n{\"content\":\"Nope.\"}",
            "HTTP/1.0 200 OK\r\n\r\nnot json",
            "HTTP/1.0 200 OK\r\n\r\n{\"other\":1}",
        ] {
            let (addr, _seen) = fake(resp).await;
            let out = server(addr).clean("um ship it friday").await.unwrap();
            assert_eq!(out, "um ship it friday", "response: {resp}");
        }
    }

    #[tokio::test]
    async fn server_unreachable_returns_raw() {
        // Bind then drop to get a port nothing listens on.
        let addr = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let out = server(addr).clean("ship it friday").await.unwrap();
        assert_eq!(out, "ship it friday");
    }

    #[test]
    fn keeps_a_punctuated_line() {
        assert_eq!(
            accept_cleanup("um ship it friday", "Ship it Friday."),
            "Ship it Friday."
        );
    }

    #[test]
    fn rejects_hallucinated_expansion() {
        let raw = "ship it friday";
        let invented = "We should ship it on Friday and email the whole company about the launch.";
        assert_eq!(accept_cleanup(raw, invented), raw);
    }
}
