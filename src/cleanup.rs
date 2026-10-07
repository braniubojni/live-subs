use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

const SYSTEM: &str = "\
You clean one speech-to-text line. Remove filler words and stutters. \
Fix capitalization and punctuation. Do not add, translate, or rephrase. \
Output only the cleaned line.";

#[async_trait::async_trait]
pub trait Cleaner: Send + Sync {
    async fn clean(&self, raw: &str) -> Result<String>;
}

#[async_trait::async_trait]
impl Cleaner for Box<dyn Cleaner> {
    async fn clean(&self, raw: &str) -> Result<String> {
        (**self).clean(raw).await
    }
}

/// `--no-cleanup`: the raw transcript is the subtitle.
pub struct Passthrough;

#[async_trait::async_trait]
impl Cleaner for Passthrough {
    async fn clean(&self, raw: &str) -> Result<String> {
        Ok(raw.trim().to_string())
    }
}

pub struct LlamaCli {
    pub bin: PathBuf,
    pub model: PathBuf,
    pub timeout: Duration,
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
            self.timeout,
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

/// Restart pacing. `grace` is how long a fresh child may refuse connections (model load)
/// before a failed `clean` is allowed to declare it dead; a child that outlives it resets
/// the backoff.
#[derive(Clone, Copy)]
struct Backoff {
    min: Duration,
    max: Duration,
    grace: Duration,
}

const BACKOFF: Backoff = Backoff {
    min: Duration::from_millis(500),
    max: Duration::from_secs(30),
    grace: Duration::from_secs(30),
};

/// Keeps one `llama-server` warm so the model loads once, not per line. A background task
/// owns the child and restarts it with backoff; while it is down `clean` returns raw
/// without touching the network.
pub struct LlamaServer {
    /// `Some` while a child is believed up. Never held across an await.
    addr: Arc<Mutex<Option<SocketAddr>>>,
    /// `clean` pokes this when nothing is listening.
    down: Arc<Notify>,
    timeout: Duration,
    // Aborted on drop, which drops the child future and so kills the server.
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

const STDERR_TAIL: usize = 20;

/// Last `cap` lines of `r`, each cut to 300 chars. Memory stays bounded.
/// ponytail: a single endless line still grows `next_line`'s buffer; llama.cpp logs real lines.
async fn tail_lines<R: tokio::io::AsyncRead + Unpin>(r: R, cap: usize) -> VecDeque<String> {
    let mut tail = VecDeque::with_capacity(cap);
    let mut lines = tokio::io::BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tail.len() == cap {
            tail.pop_front();
        }
        tail.push_back(line.chars().take(300).collect());
    }
    tail
}

/// Grab a free port and release it. The server binds it later, so another process can win
/// the race; the supervisor then sees the child exit and picks a new port.
fn free_port() -> Result<u16> {
    let mut last = None;
    for _ in 0..5 {
        match std::net::TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => return Ok(l.local_addr()?.port()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.map(Into::into).unwrap_or_else(|| anyhow!("no port")))
}

async fn supervise<F, Fut>(
    spawn: F,
    first: (u16, Fut),
    addr: Arc<Mutex<Option<SocketAddr>>>,
    down: Arc<Notify>,
    backoff: Backoff,
) where
    F: Fn(u16) -> Result<Fut>,
    Fut: std::future::Future<Output = ()>,
{
    let mut next = Some(first);
    let mut delay = backoff.min;
    loop {
        let started = Instant::now();
        let launched = match next.take() {
            Some(l) => Some(l),
            None => free_port()
                .and_then(|p| spawn(p).map(|f| (p, f)))
                .map_err(|e| tracing::warn!("llama-server restart failed: {e}"))
                .ok(),
        };
        if let Some((port, exited)) = launched {
            *addr.lock().unwrap() = Some(SocketAddr::from(([127, 0, 0, 1], port)));
            let mut exited = Box::pin(exited);
            loop {
                tokio::select! {
                    _ = &mut exited => break,
                    _ = down.notified() => {
                        if started.elapsed() >= backoff.grace {
                            break;
                        }
                    }
                }
            }
            // Leaving scope drops `exited`, which kills a child that is still running.
            *addr.lock().unwrap() = None;
            tracing::warn!("llama-server down, keeping raw until restart");
            if started.elapsed() >= backoff.grace {
                delay = backoff.min;
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(backoff.max);
    }
}

impl LlamaServer {
    /// Starts the first child now, so a bad binary path fails fast. Later failures retry.
    /// Must run inside a tokio runtime.
    pub fn spawn(bin: &Path, model: &Path, timeout: Duration) -> Result<Self> {
        let (bin, model) = (bin.to_path_buf(), model.to_path_buf());
        // ponytail: no readiness wait; /completion answers 503 while the model loads and
        // that returns raw.
        let mut srv = Self::start(
            move |port| {
                let mut child = tokio::process::Command::new(&bin)
                    .arg("-m")
                    .arg(&model)
                    .args(["--host", "127.0.0.1", "--port", &port.to_string()])
                    .args(["-c", "512", "-ngl", "99"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()?;
                let stderr = child.stderr.take();
                Ok(async move {
                    // stderr closes when the child dies; keep only its last lines.
                    let tail = match stderr {
                        Some(r) => tail_lines(r, STDERR_TAIL).await,
                        None => VecDeque::new(),
                    };
                    let status = child.wait().await;
                    tracing::warn!(
                        "llama-server exited ({status:?}), stderr tail:\n{}",
                        Vec::from(tail).join("\n")
                    );
                })
            },
            BACKOFF,
        )?;
        srv.timeout = timeout;
        Ok(srv)
    }

    /// `spawn` returns a future that resolves when the child exits and kills it when dropped.
    fn start<F, Fut>(spawn: F, backoff: Backoff) -> Result<Self>
    where
        F: Fn(u16) -> Result<Fut> + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let port = free_port()?;
        let first = (port, spawn(port)?);
        let addr = Arc::new(Mutex::new(Some(SocketAddr::from(([127, 0, 0, 1], port)))));
        let down = Arc::new(Notify::new());
        let task = tokio::spawn(supervise(spawn, first, addr.clone(), down.clone(), backoff));
        Ok(Self {
            addr,
            down,
            timeout: Duration::from_millis(700),
            task: Some(task),
        })
    }

    async fn complete(&self, addr: SocketAddr, raw: &str) -> Result<String> {
        let body = serde_json::json!({
            "prompt": prompt(raw),
            "n_predict": 64,
            "temperature": 0,
            "cache_prompt": true,
        })
        .to_string();
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        // HTTP/1.0: the server closes after the reply and never chunks it.
        let req = format!(
            "POST /completion HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            addr,
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
        let Some(addr) = *self.addr.lock().unwrap() else {
            return Ok(raw.to_string());
        };
        match tokio::time::timeout(self.timeout, self.complete(addr, raw)).await {
            Ok(Ok(out)) => Ok(first_line(&out, raw)),
            Ok(Err(err)) => {
                let refused = err
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused);
                if refused {
                    self.down.notify_one();
                }
                tracing::warn!("llama-server failed, keeping raw: {err}");
                Ok(raw.to_string())
            }
            Err(_) => {
                // Accepted but never answered: hung. The grace rule decides if it dies.
                self.down.notify_one();
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
    use super::{accept_cleanup, Backoff, Cleaner, LlamaServer, SYSTEM};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

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
        LlamaServer {
            addr: Arc::new(Mutex::new(Some(addr))),
            down: Arc::new(Notify::new()),
            timeout: Duration::from_millis(700),
            task: None,
        }
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

    /// Point the client at a fake listener. Yields first so the supervisor has already
    /// published its own address and cannot overwrite ours.
    async fn point_at(srv: &LlamaServer, addr: std::net::SocketAddr) {
        tokio::task::yield_now().await;
        *srv.addr.lock().unwrap() = Some(addr);
    }

    fn quick(grace_ms: u64) -> Backoff {
        Backoff {
            min: Duration::from_millis(100),
            max: Duration::from_millis(200),
            grace: Duration::from_millis(grace_ms),
        }
    }

    async fn until(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn dead_child_returns_raw_at_once_then_restarts() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        // Fake child exits immediately, like llama-server losing the port race.
        let srv = LlamaServer::start(
            move |_port| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(async {})
            },
            quick(0),
        )
        .unwrap();
        until("child down", || srv.addr.lock().unwrap().is_none()).await;
        let t = Instant::now();
        assert_eq!(srv.clean("ship it friday").await.unwrap(), "ship it friday");
        assert!(t.elapsed() < Duration::from_millis(50));
        until("restart", || spawns.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test]
    async fn refused_connect_restarts_child() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        // Fake child stays "alive" but nothing listens on its port.
        let srv = LlamaServer::start(
            move |_port| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(std::future::pending::<()>())
            },
            quick(0),
        )
        .unwrap();
        assert_eq!(srv.clean("ship it friday").await.unwrap(), "ship it friday");
        until("restart", || spawns.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test]
    async fn refused_connect_inside_grace_keeps_child() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        let srv = LlamaServer::start(
            move |_port| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(std::future::pending::<()>())
            },
            quick(60_000),
        )
        .unwrap();
        assert_eq!(srv.clean("ship it friday").await.unwrap(), "ship it friday");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert!(srv.addr.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn timeout_on_live_address_restarts_child() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        let srv = LlamaServer::start(
            move |_port| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(std::future::pending::<()>())
            },
            quick(0),
        )
        .unwrap();
        // Accepts, reads nothing, never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        point_at(&srv, listener.local_addr().unwrap()).await;
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            std::future::pending::<()>().await;
        });
        assert_eq!(srv.clean("ship it friday").await.unwrap(), "ship it friday");
        until("restart", || spawns.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test]
    async fn loading_503_never_restarts_child() {
        let spawns = Arc::new(AtomicUsize::new(0));
        let n = spawns.clone();
        // Grace 0: a restart would be allowed, so only a missing notify keeps the child.
        let srv = LlamaServer::start(
            move |_port| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(std::future::pending::<()>())
            },
            quick(0),
        )
        .unwrap();
        let (addr, _seen) = fake("HTTP/1.0 503 Service Unavailable\r\n\r\nLoading model").await;
        point_at(&srv, addr).await;
        assert_eq!(srv.clean("ship it friday").await.unwrap(), "ship it friday");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(*srv.addr.lock().unwrap(), Some(addr));
    }

    #[tokio::test]
    async fn stderr_tail_keeps_last_lines() {
        let tail = super::tail_lines(&b"a\nb\nc\nd\n"[..], 2).await;
        assert_eq!(Vec::from(tail), ["c", "d"]);
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
