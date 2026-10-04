use crate::audio::AudioRx;
use crate::cleanup::Cleaner;
use crate::stt::{with_timeout, Transcriber};
use crate::vad::{Segmenter, VadConfig, VadEvent};
use anyhow::Result;
use rubato::{FastFixedIn, PolynomialDegree, Resampler};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueKind {
    Partial,
    Final,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cue {
    pub kind: CueKind,
    pub text: String,
    pub id: u64,
}

pub async fn run<S, C>(
    audio: AudioRx,
    in_rate: u32,
    stt: S,
    cleaner: C,
    tx: tokio::sync::mpsc::Sender<Cue>,
) where
    S: Transcriber + 'static,
    C: Cleaner + 'static,
{
    let stt = Arc::new(stt);
    let cleaner = Arc::new(cleaner);
    if let Err(err) = worker(audio, in_rate, stt, cleaner, tx).await {
        tracing::error!(?err, "pipeline stopped");
    }
}

async fn worker<S, C>(
    mut audio: AudioRx,
    in_rate: u32,
    stt: Arc<S>,
    cleaner: Arc<C>,
    tx: tokio::sync::mpsc::Sender<Cue>,
) -> Result<()>
where
    S: Transcriber + 'static,
    C: Cleaner + 'static,
{
    // Capture rate is device-native. Whisper wants 16 kHz.
    let mut resampler = FastFixedIn::<f32>::new(
        16_000.0 / in_rate as f64,
        1.0,
        PolynomialDegree::Linear,
        960,
        1,
    )?;
    let mut seg = Segmenter::new(16_000, VadConfig::default());
    let mut pending: Vec<f32> = Vec::new();
    let mut id: u64 = 0;
    let mut last_partial = Instant::now() - Duration::from_secs(10);
    let partial_busy = Arc::new(AtomicBool::new(false));
    // Count of utterances closed so far. A partial for id k is stale once this is > k.
    let ended = Arc::new(AtomicU64::new(0));

    loop {
        let mut chunk = Vec::new();
        audio.drain(&mut chunk);
        if chunk.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }
        pending.extend(chunk);
        while pending.len() >= 960 {
            let frame: Vec<f32> = pending.drain(..960).collect();
            let out = match resampler.process(&[frame], None) {
                Ok(out) => out,
                Err(err) => {
                    tracing::warn!(?err, "resample failed, skipping frame");
                    resampler.reset();
                    continue;
                }
            };
            let mono = &out[0];
            if let Some(ev) = seg.push(mono) {
                match ev {
                    VadEvent::Speaking => {
                        if last_partial.elapsed() >= Duration::from_millis(900)
                            && seg.speech().len() > 16_000 / 2
                            && !partial_busy.swap(true, Ordering::AcqRel)
                        {
                            last_partial = Instant::now();
                            let speech = seg.speech().to_vec();
                            let stt = stt.clone();
                            let tx = tx.clone();
                            let busy = partial_busy.clone();
                            let ended = ended.clone();
                            let cue_id = id;
                            tokio::spawn(async move {
                                let text = stt.transcribe(&speech).await.unwrap_or_default();
                                busy.store(false, Ordering::Release);
                                // ponytail: check-then-send is not atomic; the final needs a
                                // full STT run first, so the window is negligible. Per-utterance
                                // lock if it ever shows up.
                                if !text.is_empty() && ended.load(Ordering::Acquire) <= cue_id {
                                    // Partials are droppable. A full channel must not stall.
                                    let _ = tx.try_send(Cue {
                                        kind: CueKind::Partial,
                                        text,
                                        id: cue_id,
                                    });
                                }
                            });
                        }
                    }
                    VadEvent::Ended { speech } => {
                        let cue_id = id;
                        id += 1;
                        ended.store(id, Ordering::Release);
                        let stt = stt.clone();
                        let cleaner = cleaner.clone();
                        let tx = tx.clone();
                        last_partial = Instant::now();
                        // Final transcription is off the capture loop. Cleanup
                        // starts only after that line exists, and is itself capped.
                        tokio::spawn(async move {
                            let raw = with_timeout(stt.transcribe(&speech), Duration::from_secs(4))
                                .await
                                .unwrap_or_default();
                            if raw.is_empty() {
                                return;
                            }
                            let _ = tx.try_send(Cue {
                                kind: CueKind::Partial,
                                text: raw.clone(),
                                id: cue_id,
                            });
                            let text = cleaner.clean(&raw).await.unwrap_or(raw);
                            let sent = tx
                                .send(Cue {
                                    kind: CueKind::Final,
                                    text,
                                    id: cue_id,
                                })
                                .await;
                            if sent.is_err() {
                                tracing::debug!(cue_id, "cue channel closed");
                            }
                        });
                    }
                }
            }
        }
    }
}
