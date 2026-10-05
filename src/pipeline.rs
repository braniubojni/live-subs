use crate::audio::AudioSource;
use crate::cleanup::Cleaner;
use crate::stt::{with_timeout, Transcriber};
use crate::vad::{Segmenter, VadConfig, VadEvent};
use anyhow::Result;
use rubato::{FastFixedIn, PolynomialDegree, Resampler};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

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

pub async fn run<A, S, C>(
    audio: A,
    in_rate: u32,
    stt: S,
    cleaner: C,
    tx: tokio::sync::mpsc::Sender<Cue>,
) where
    A: AudioSource,
    S: Transcriber + 'static,
    C: Cleaner + 'static,
{
    let stt = Arc::new(stt);
    let cleaner = Arc::new(cleaner);
    if let Err(err) = worker(audio, in_rate, stt, cleaner, tx, TokioClock).await {
        tracing::error!(?err, "pipeline stopped");
    }
}

/// Time source for the pipeline. Tests inject a fake; `TokioClock` also follows
/// `tokio::time::pause`, so integration tests need no real sleeps.
pub trait Clock {
    fn now(&self) -> Instant;
}

pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Device rate to 16 kHz mono, one 960-sample frame at a time.
struct Resample(FastFixedIn<f32>);

impl Resample {
    fn new(in_rate: u32) -> Result<Self> {
        // Capture rate is device-native. Whisper wants 16 kHz.
        Ok(Self(FastFixedIn::<f32>::new(
            16_000.0 / in_rate as f64,
            1.0,
            PolynomialDegree::Linear,
            960,
            1,
        )?))
    }

    /// `None` after a failure: the resampler is reset and the frame skipped.
    fn process(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        match self.0.process(&[frame], None) {
            Ok(mut out) => Some(out.swap_remove(0)),
            Err(err) => {
                tracing::warn!(?err, "resample failed, skipping frame");
                self.0.reset();
                None
            }
        }
    }
}

/// Utterance segmentation over 16 kHz mono.
struct Vad(Segmenter);

impl Vad {
    fn new() -> Self {
        Self(Segmenter::new(16_000, VadConfig::default()))
    }

    fn push(&mut self, mono: &[f32]) -> Option<VadEvent> {
        self.0.push(mono)
    }

    fn speech(&self) -> &[f32] {
        self.0.speech()
    }
}

const PARTIAL_EVERY: Duration = Duration::from_millis(900);
const PARTIAL_MIN_SAMPLES: usize = 16_000 / 2;
/// Finals allowed to reach cleanup at once. Past this, the oldest skips cleanup.
const MAX_LIVE_FINALS: usize = 3;

/// Decides when a partial transcription may start. Owns the busy flag so at
/// most one partial runs at a time. Reads time only through `Clock`.
struct PartialScheduler<K> {
    clock: K,
    /// `None` until the first partial or final, so the first partial is never held back.
    last: Option<Instant>,
    busy: Arc<AtomicBool>,
}

impl<K: Clock> PartialScheduler<K> {
    fn new(clock: K) -> Self {
        Self {
            clock,
            last: None,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// If a partial should start now, mark it busy and return the flag.
    /// The task must store `false` into it when done.
    fn try_fire(&mut self, speech_len: usize) -> Option<Arc<AtomicBool>> {
        let now = self.clock.now();
        let due = self
            .last
            .is_none_or(|last| now.duration_since(last) >= PARTIAL_EVERY);
        if due && speech_len > PARTIAL_MIN_SAMPLES && !self.busy.swap(true, Ordering::AcqRel) {
            self.last = Some(now);
            return Some(self.busy.clone());
        }
        None
    }

    /// An utterance closed. Restart the partial interval.
    fn reset(&mut self) {
        self.last = Some(self.clock.now());
    }
}

type LiveFinals = VecDeque<(u64, oneshot::Sender<()>)>;

/// Owns the utterance id and emits cues from detached tasks.
struct Finalizer<S, C> {
    stt: Arc<S>,
    cleaner: Arc<C>,
    tx: tokio::sync::mpsc::Sender<Cue>,
    id: u64,
    /// Count of utterances closed so far. A partial for id k is stale once this is > k.
    ended: Arc<AtomicU64>,
    /// Finals still eligible for cleanup, oldest first. Shedding signals the front.
    live: Arc<Mutex<LiveFinals>>,
}

impl<S, C> Finalizer<S, C>
where
    S: Transcriber + 'static,
    C: Cleaner + 'static,
{
    fn new(stt: Arc<S>, cleaner: Arc<C>, tx: tokio::sync::mpsc::Sender<Cue>) -> Self {
        Self {
            stt,
            cleaner,
            tx,
            id: 0,
            ended: Arc::new(AtomicU64::new(0)),
            live: Arc::default(),
        }
    }

    fn spawn_partial(&self, speech: Vec<f32>, busy: Arc<AtomicBool>) {
        let stt = self.stt.clone();
        let tx = self.tx.clone();
        let ended = self.ended.clone();
        let cue_id = self.id;
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

    /// Close the current utterance and transcribe it off the capture loop.
    /// Cleanup starts only after that line exists. At most `MAX_LIVE_FINALS`
    /// finals stay eligible for cleanup; the oldest beyond that emits its raw
    /// line as the final instead. Capture never waits.
    fn end(&mut self, speech: Vec<f32>) {
        let cue_id = self.id;
        self.id += 1;
        self.ended.store(self.id, Ordering::Release);
        let (skip_tx, mut skip_rx) = oneshot::channel();
        {
            let mut live = self.live.lock().unwrap();
            if live.len() >= MAX_LIVE_FINALS {
                if let Some((old, skip)) = live.pop_front() {
                    tracing::warn!(
                        old,
                        cue_id,
                        "stt lagging: cleanup dropped, raw line will be the final"
                    );
                    let _ = skip.send(());
                }
            }
            live.push_back((cue_id, skip_tx));
        }
        let stt = self.stt.clone();
        let cleaner = self.cleaner.clone();
        let tx = self.tx.clone();
        let live = self.live.clone();
        tokio::spawn(async move {
            let raw = with_timeout(stt.transcribe(&speech), Duration::from_secs(4))
                .await
                .unwrap_or_default();
            if !raw.is_empty() {
                let _ = tx.try_send(Cue {
                    kind: CueKind::Partial,
                    text: raw.clone(),
                    id: cue_id,
                });
                // biased: a final already shed must not start cleanup at all.
                let text = tokio::select! {
                    biased;
                    _ = &mut skip_rx => raw,
                    cleaned = cleaner.clean(&raw) => cleaned.unwrap_or(raw),
                };
                live.lock().unwrap().retain(|(i, _)| *i != cue_id);
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
            } else {
                live.lock().unwrap().retain(|(i, _)| *i != cue_id);
            }
        });
    }
}

async fn worker<A, S, C, K>(
    mut audio: A,
    in_rate: u32,
    stt: Arc<S>,
    cleaner: Arc<C>,
    tx: tokio::sync::mpsc::Sender<Cue>,
    clock: K,
) -> Result<()>
where
    A: AudioSource,
    S: Transcriber + 'static,
    C: Cleaner + 'static,
    K: Clock,
{
    let mut resample = Resample::new(in_rate)?;
    let mut vad = Vad::new();
    let mut partials = PartialScheduler::new(clock);
    let mut finals = Finalizer::new(stt, cleaner, tx);
    let mut pending: Vec<f32> = Vec::new();

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
            let Some(mono) = resample.process(&frame) else {
                continue;
            };
            match vad.push(&mono) {
                Some(VadEvent::Speaking) => {
                    if let Some(busy) = partials.try_fire(vad.speech().len()) {
                        finals.spawn_partial(vad.speech().to_vec(), busy);
                    }
                }
                Some(VadEvent::Ended { speech }) => {
                    finals.end(speech);
                    partials.reset();
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeClock {
        base: Instant,
        offset: Rc<Cell<Duration>>,
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.base + self.offset.get()
        }
    }

    fn scheduler() -> (PartialScheduler<FakeClock>, Rc<Cell<Duration>>) {
        let offset = Rc::new(Cell::new(Duration::ZERO));
        let clock = FakeClock {
            base: Instant::now(),
            offset: offset.clone(),
        };
        (PartialScheduler::new(clock), offset)
    }

    const LONG: usize = PARTIAL_MIN_SAMPLES + 1;

    #[test]
    fn first_partial_fires_immediately_once_speech_is_long_enough() {
        let (mut s, _) = scheduler();
        assert!(s.try_fire(PARTIAL_MIN_SAMPLES).is_none(), "too short");
        assert!(s.try_fire(LONG).is_some());
    }

    #[test]
    fn waits_for_interval_and_for_busy_to_clear() {
        let (mut s, t) = scheduler();
        let busy = s.try_fire(LONG).expect("first fire");
        t.set(PARTIAL_EVERY - Duration::from_millis(1));
        busy.store(false, Ordering::Release);
        assert!(s.try_fire(LONG).is_none(), "interval not elapsed");
        t.set(PARTIAL_EVERY);
        let busy = s.try_fire(LONG).expect("due and idle");
        t.set(PARTIAL_EVERY * 3);
        assert!(s.try_fire(LONG).is_none(), "still busy");
        busy.store(false, Ordering::Release);
        assert!(s.try_fire(LONG).is_some());
    }

    #[test]
    fn busy_is_not_taken_when_not_due() {
        let (mut s, _) = scheduler();
        assert!(s.try_fire(10).is_none());
        assert!(s.try_fire(LONG).is_some(), "short call must not set busy");
    }

    #[test]
    fn reset_restarts_the_interval() {
        let (mut s, t) = scheduler();
        s.try_fire(LONG)
            .expect("first fire")
            .store(false, Ordering::Release);
        t.set(Duration::from_secs(5));
        s.reset();
        assert!(s.try_fire(LONG).is_none());
        t.set(Duration::from_secs(5) + PARTIAL_EVERY);
        assert!(s.try_fire(LONG).is_some());
    }
}
