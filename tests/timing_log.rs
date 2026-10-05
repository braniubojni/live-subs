//! One log line per final carries `stt_ms` and `cleanup_ms`. Lives in its own
//! test binary: tracing caches callsite interest process-wide, so a scoped
//! subscriber is unreliable next to tests that log with none installed.

use anyhow::Result;
use live_subs::audio::AudioRx;
use live_subs::cleanup::Cleaner;
use live_subs::pipeline::{self, Cue, CueKind};
use live_subs::stt::Transcriber;
use ringbuf::traits::{Producer, Split};
use ringbuf::HeapRb;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const RATE: u32 = 48_000;

/// 1.5 s tone then 1 s silence at RATE, as one ring fill.
fn audio() -> AudioRx {
    let rb = HeapRb::<f32>::new(RATE as usize * 4);
    let (mut prod, cons) = rb.split();
    prod.push_slice(&vec![0.2_f32; RATE as usize * 3 / 2]);
    prod.push_slice(&vec![0.0_f32; RATE as usize]);
    AudioRx::new(cons, Default::default())
}

/// Collects formatted log output so a test can read it back.
#[derive(Clone, Default)]
struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

struct DelayStt(Duration);

#[async_trait::async_trait]
impl Transcriber for DelayStt {
    async fn transcribe(&self, _pcm16k: &[f32]) -> Result<String> {
        tokio::time::sleep(self.0).await;
        Ok("hello".into())
    }
}

struct DelayCleaner(Duration);

#[async_trait::async_trait]
impl Cleaner for DelayCleaner {
    async fn clean(&self, raw: &str) -> Result<String> {
        tokio::time::sleep(self.0).await;
        Ok(raw.to_uppercase())
    }
}

#[tokio::test(start_paused = true)]
async fn each_final_logs_stt_and_cleanup_ms() {
    let log = LogBuf::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .finish(),
    );
    let (tx, mut rx) = mpsc::channel::<Cue>(32);
    let task = tokio::spawn(pipeline::run(
        audio(),
        RATE,
        DelayStt(Duration::from_millis(50)),
        DelayCleaner(Duration::from_millis(30)),
        tx,
        CancellationToken::new(),
    ));
    while let Ok(Some(cue)) = tokio::time::timeout(Duration::from_millis(1500), rx.recv()).await {
        if matches!(cue.kind, CueKind::Final) {
            break;
        }
    }
    task.abort();
    let out = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let line = out
        .lines()
        .find(|l| l.contains("final timing"))
        .unwrap_or_else(|| panic!("no timing line in: {out}"));
    assert!(line.contains("stt_ms=50"), "{line}");
    assert!(line.contains("cleanup_ms=30"), "{line}");
}
