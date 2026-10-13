//! Resumable download into the OS data dir. Hash, then rename. Cancel deletes the partial.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// `~/Library/Application Support/live-subs/models` on macOS, XDG data or `%APPDATA%` elsewhere.
pub fn model_dir() -> Option<PathBuf> {
    let var = |k| std::env::var_os(k).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    };
    base.map(|d| d.join("live-subs").join("models"))
}

pub fn url_file_name(url: &str) -> &str {
    url.split('?')
        .next()
        .and_then(|u| u.rsplit('/').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("model")
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

enum ReadEnd {
    Done,
    Short,
    Cancelled,
}

/// Download `url` to `dest`. Verifies `sha256`, resumes a partial, and leaves nothing behind on cancel.
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha256: &str,
    mut progress: impl FnMut(u64, u64),
    cancel: &CancellationToken,
) -> Result<()> {
    if sha256.is_empty() {
        bail!("no sha256");
    }
    let sha256 = sha256.to_ascii_lowercase();
    if dest.is_file() && hash_file(dest).unwrap_or_default() == sha256 {
        return Ok(());
    }
    if dest.is_file() {
        fs::remove_file(dest)?;
    }
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let partial = partial_path(dest);
    for _ in 0..2 {
        if cancel.is_cancelled() {
            let _ = fs::remove_file(&partial);
            bail!("download cancelled");
        }
        match download(client, url, &partial, &mut progress, cancel).await? {
            ReadEnd::Cancelled => {
                let _ = fs::remove_file(&partial);
                bail!("download cancelled");
            }
            ReadEnd::Short => bail!("download interrupted"),
            ReadEnd::Done => {}
        }
        let got = hash_file(&partial).context("hash")?;
        if got == sha256 {
            fs::rename(&partial, dest)?;
            return Ok(());
        }
        let _ = fs::remove_file(&partial);
    }
    bail!("sha256 mismatch for {}", dest.display())
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    partial: &Path,
    progress: &mut impl FnMut(u64, u64),
    cancel: &CancellationToken,
) -> Result<ReadEnd> {
    let mut cuts = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(ReadEnd::Cancelled);
        }
        if cuts > 8 {
            bail!("download interrupted");
        }
        let have = file_len(partial);
        let mut req = client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await.map_err(|e| anyhow!("download: {e}"))?;
        if !resp.status().is_success() {
            bail!("download {url}: {}", resp.status());
        }
        if have > 0 && resp.status() == reqwest::StatusCode::OK {
            let _ = fs::remove_file(partial);
            continue;
        }
        let total = content_total(&resp, have);
        let append = have > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(partial)?;
        let mut got = if append { have } else { 0 };
        let end = read_body(resp, &mut file, &mut got, total, progress, cancel).await?;
        file.sync_all()?;
        match end {
            ReadEnd::Short => {
                cuts += 1;
                continue;
            }
            other => return Ok(other),
        }
    }
}

fn content_total(resp: &reqwest::Response, have: u64) -> u64 {
    if let Some(total) = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split('/').nth(1))
        .and_then(|s| s.trim().parse().ok())
    {
        return total;
    }
    let len = resp.content_length().unwrap_or(0);
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        have + len
    } else {
        len
    }
}

async fn read_body(
    mut resp: reqwest::Response,
    file: &mut File,
    got: &mut u64,
    total: u64,
    progress: &mut impl FnMut(u64, u64),
    cancel: &CancellationToken,
) -> Result<ReadEnd> {
    loop {
        if cancel.is_cancelled() {
            return Ok(ReadEnd::Cancelled);
        }
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                file.write_all(&chunk)?;
                *got += chunk.len() as u64;
                progress(*got, total);
                if cancel.is_cancelled() {
                    return Ok(ReadEnd::Cancelled);
                }
            }
            Ok(None) => {
                return Ok(if total > 0 && *got < total {
                    ReadEnd::Short
                } else {
                    ReadEnd::Done
                });
            }
            Err(_) if total > 0 && *got < total => return Ok(ReadEnd::Short),
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap()
    }

    async fn read_headers(sock: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    async fn write_response(
        sock: &mut tokio::net::TcpStream,
        status: &str,
        extra: &str,
        body: &[u8],
    ) {
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        );
        sock.write_all(head.as_bytes()).await.unwrap();
        sock.write_all(body).await.unwrap();
    }

    #[tokio::test]
    async fn resumes_after_a_cut() {
        let body = vec![7u8; 80];
        let sum = sha(&body);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body2 = body.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let req = read_headers(&mut sock).await;
            assert!(!req.to_ascii_lowercase().contains("range:"), "{req}");
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body2.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(&body2[..30]).await.unwrap();
            drop(sock);

            let (mut sock, _) = listener.accept().await.unwrap();
            let req = read_headers(&mut sock).await;
            assert!(
                req.to_ascii_lowercase().contains("range: bytes=30-"),
                "{req}"
            );
            let rest = &body2[30..];
            let extra = format!(
                "Content-Range: bytes 30-{}/{}\r\n",
                body2.len() - 1,
                body2.len()
            );
            write_response(&mut sock, "206 Partial Content", &extra, rest).await;
        });

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        fetch(
            &client(),
            &format!("http://{addr}/m.bin"),
            &dest,
            &sum,
            |_, _| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert!(!partial_path(&dest).exists());
    }

    #[tokio::test]
    async fn corrupt_file_is_refetched() {
        let good = b"good-model-bytes".to_vec();
        let bad = b"bad-model-bytes!".to_vec();
        let sum = sha(&good);
        let good_body = good.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut sock).await;
            write_response(&mut sock, "200 OK", "", &bad).await;
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut sock).await;
            write_response(&mut sock, "200 OK", "", &good_body).await;
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        fetch(
            &client(),
            &format!("http://{addr}/m.gguf"),
            &dest,
            &sum,
            |_, _| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), good);
    }

    #[tokio::test]
    async fn cancel_leaves_no_partial() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_headers(&mut sock).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(&[1u8]).await.unwrap();
            std::future::pending::<()>().await;
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let err = fetch(
            &client(),
            &format!("http://{addr}/m.bin"),
            &dest,
            &"ab".repeat(32),
            move |got, _| {
                if got > 0 {
                    c2.cancel();
                }
            },
            &cancel,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("cancelled"), "{err}");
        assert!(!dest.exists());
        assert!(!partial_path(&dest).exists());
    }
}
