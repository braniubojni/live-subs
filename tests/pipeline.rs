//! Pipeline end to end with fake STT and cleanup. No audio device, no models.
//! Tokio time is paused: sleeps and timeouts advance virtually, none are real.

use anyhow::Result;
use live_subs::audio::AudioRx;
use live_subs::cleanup::Cleaner;
use live_subs::pipeline::{self, Cue, CueKind};
use live_subs::stt::Transcriber;
use ringbuf::traits::{Producer, Split};
use ringbuf::HeapRb;
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
