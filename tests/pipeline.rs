//! Pipeline end to end with fake STT and cleanup. No audio device, no models.
//! Tokio time is paused: sleeps and timeouts advance virtually, none are real.

use anyhow::Result;
use live_subs::audio::{AudioRx, AudioSource};
use live_subs::cleanup::Cleaner;
use live_subs::pipeline::{self, Cue, CueKind};
use live_subs::stt::Transcriber;
use ringbuf::traits::{Producer, Split};
use ringbuf::HeapRb;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const RATE: u32 = 48_000;

/// Short audio (a partial window) is slow; the full utterance is fast.
struct FakeStt {
    partial_delay: Duration,
}

#[async_trait::async_trait]
impl Transcriber for FakeStt {
    async fn transcribe(&self, pcm16k: &[f32]) -> Result<String> {
        if pcm16k.len() < 16_000 * 2 {
            tokio::time::sleep(self.partial_delay).await;
        }
        Ok("hello".into())
    }
}

struct FakeCleaner;

#[async_trait::async_trait]
impl Cleaner for FakeCleaner {
    async fn clean(&self, raw: &str) -> Result<String> {
        Ok(raw.to_uppercase())
    }
}

/// 1.5 s tone then 1 s silence at RATE, as one ring fill.
fn audio() -> AudioRx {
    let rb = HeapRb::<f32>::new(RATE as usize * 4);
    let (mut prod, cons) = rb.split();
    let tone = vec![0.2_f32; RATE as usize * 3 / 2];
    let silence = vec![0.0_f32; RATE as usize];
    prod.push_slice(&tone);
    prod.push_slice(&silence);
    AudioRx::new(cons, Default::default())
}

/// Run the pipeline and collect cues until `idle` passes with none arriving.
async fn collect(
    partial_delay: Duration,
    cap: usize,
    slow_reader: Option<Duration>,
    idle: Duration,
) -> Vec<Cue> {
    let (tx, mut rx) = mpsc::channel::<Cue>(cap);
    let task = tokio::spawn(pipeline::run(
        audio(),
        RATE,
        FakeStt { partial_delay },
        FakeCleaner,
        tx,
    ));
    let mut cues = Vec::new();
    while let Ok(Some(cue)) = tokio::time::timeout(idle, rx.recv()).await {
        cues.push(cue);
        if let Some(d) = slow_reader {
            tokio::time::sleep(d).await;
        }
    }
    task.abort();
    cues
}

#[tokio::test(start_paused = true)]
async fn partial_and_final_share_an_id() {
    let cues = collect(Duration::ZERO, 32, None, Duration::from_millis(1500)).await;
    assert!(cues.iter().any(|c| matches!(c.kind, CueKind::Final)));
    let first = cues[0].id;
    assert!(cues.iter().all(|c| c.id == first), "ids differ: {cues:?}");
}

#[tokio::test(start_paused = true)]
async fn late_partial_never_follows_final() {
    let cues = collect(
        Duration::from_millis(1200),
        32,
        None,
        Duration::from_millis(2500),
    )
    .await;
    let final_at = cues
        .iter()
        .position(|c| matches!(c.kind, CueKind::Final))
        .expect("a final cue");
    assert!(
        cues[final_at + 1..]
            .iter()
            .all(|c| !matches!(c.kind, CueKind::Partial)),
        "partial after final: {cues:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn final_survives_full_channel() {
    let cues = collect(
        Duration::ZERO,
        1,
        Some(Duration::from_millis(300)),
        Duration::from_millis(2500),
    )
    .await;
    assert!(
        cues.iter().any(|c| matches!(c.kind, CueKind::Final)),
        "final dropped: {cues:?}"
    );
}

/// Hands out one scripted chunk per `drain`, then nothing.
struct FakeSource(VecDeque<Vec<f32>>);

impl AudioSource for FakeSource {
    fn drain(&mut self, out: &mut Vec<f32>) {
        if let Some(chunk) = self.0.pop_front() {
            out.extend(chunk);
        }
    }
}

/// Names utterances in call order. Speech here is too short for a partial.
struct CountingStt(AtomicUsize);

#[async_trait::async_trait]
impl Transcriber for CountingStt {
    async fn transcribe(&self, _pcm16k: &[f32]) -> Result<String> {
        Ok(["first", "second"][self.0.fetch_add(1, Ordering::SeqCst)].into())
    }
}

#[tokio::test(start_paused = true)]
async fn speech_silence_speech_yields_exact_cues() {
    let n = |secs: f32| (RATE as f32 * secs) as usize;
    // 0.4 s speech (under the partial threshold) + 0.6 s silence, twice.
    let utterance = || vec![vec![0.2_f32; n(0.4)], vec![0.0_f32; n(0.6)]];
    let source = FakeSource(utterance().into_iter().chain(utterance()).collect());
    let (tx, mut rx) = mpsc::channel::<Cue>(32);
    let task = tokio::spawn(pipeline::run(
        source,
        RATE,
        CountingStt(AtomicUsize::new(0)),
        FakeCleaner,
        tx,
    ));
    let mut got = Vec::new();
    while let Ok(Some(cue)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
        got.push((format!("{:?}", cue.kind), cue.id, cue.text));
    }
    task.abort();
    let want = [
        ("Partial", 0, "first"),
        ("Final", 0, "FIRST"),
        ("Partial", 1, "second"),
        ("Final", 1, "SECOND"),
    ]
    .map(|(k, id, t)| (k.to_string(), id, t.to_string()));
    assert_eq!(got, want);
}

/// Every call takes 3 s of virtual time, under the 4 s STT timeout.
struct SlowStt;

#[async_trait::async_trait]
impl Transcriber for SlowStt {
    async fn transcribe(&self, _pcm16k: &[f32]) -> Result<String> {
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok("hello".into())
    }
}

/// Tracks how many cleanups run at once.
struct GaugedCleaner {
    now: AtomicUsize,
    max: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Cleaner for GaugedCleaner {
    async fn clean(&self, raw: &str) -> Result<String> {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(1)).await;
        self.now.fetch_sub(1, Ordering::SeqCst);
        Ok(raw.to_uppercase())
    }
}

/// Like `FakeSource`, but exposes how many chunks are still undrained.
struct CountedSource(VecDeque<Vec<f32>>, Arc<AtomicUsize>);

impl AudioSource for CountedSource {
    fn drain(&mut self, out: &mut Vec<f32>) {
        if let Some(chunk) = self.0.pop_front() {
            out.extend(chunk);
        }
        self.1.store(self.0.len(), Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn slow_stt_sheds_oldest_cleanups_and_never_blocks_capture() {
    let n = |secs: f32| (RATE as f32 * secs) as usize;
    // Six utterances, each too short for a partial. All end at virtual t=0.
    let chunks: VecDeque<_> = (0..6)
        .flat_map(|_| [vec![0.2_f32; n(0.4)], vec![0.0_f32; n(0.6)]])
        .collect();
    let left = Arc::new(AtomicUsize::new(chunks.len()));
    let max = Arc::new(AtomicUsize::new(0));
    let cleaner = GaugedCleaner {
        now: AtomicUsize::new(0),
        max: max.clone(),
    };
    let (tx, mut rx) = mpsc::channel::<Cue>(32);
    let task = tokio::spawn(pipeline::run(
        CountedSource(chunks, left.clone()),
        RATE,
        SlowStt,
        cleaner,
        tx,
    ));

    // STT is still busy at 1 s, yet capture has drained everything.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(left.load(Ordering::SeqCst), 0, "capture stuck");
    assert!(rx.try_recv().is_err(), "no cue before STT finishes");

    let mut finals = Vec::new();
    while let Ok(Some(cue)) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
        if matches!(cue.kind, CueKind::Final) {
            finals.push((cue.id, cue.text));
        }
    }
    task.abort();
    finals.sort();
    assert!(max.load(Ordering::SeqCst) <= 3, "cap broken");
    // Oldest three lose cleanup and emit the raw line; newest three are cleaned.
    let want = [
        (0, "hello"),
        (1, "hello"),
        (2, "hello"),
        (3, "HELLO"),
        (4, "HELLO"),
        (5, "HELLO"),
    ]
    .map(|(id, t)| (id, t.to_string()));
    assert_eq!(finals, want);
}
