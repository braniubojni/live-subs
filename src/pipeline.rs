use crate::audio::AudioSource;
use crate::cleanup::Cleaner;
use crate::stt::{with_timeout, Transcriber};
use crate::vad::{Segmenter, VadConfig, VadEvent};
use anyhow::Result;
use rubato::{FastFixedIn, PolynomialDegree, Resampler};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

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
    /// Milliseconds from run start (first audio sample) to utterance start.
    pub start_ms: u64,
    /// Milliseconds from run start to where the cue was cut: now for a
    /// partial, the end of the VAD segment (incl. trailing silence) for a final.
    pub end_ms: u64,
    /// Language code, omitted when unknown. Nothing sets it yet: `Transcriber`
    /// returns text only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// STT line before cleanup. Finals only, for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// Where an utterance sits on the run timeline.
#[derive(Debug, Clone, Copy)]
struct Span {
    start_ms: u64,
    end_ms: u64,
}

pub async fn run<A, S, C>(
    audio: A,
    in_rate: u32,
    stt: S,
    cleaner: C,
    tx: tokio::sync::mpsc::Sender<Cue>,
    cancel: CancellationToken,
) where
    A: AudioSource,
    S: Transcriber + 'static,
    C: Cleaner + 'static,
{
    let stt = Arc::new(stt);
    let cleaner = Arc::new(cleaner);
    if let Err(err) = worker(audio, in_rate, stt, cleaner, tx, TokioClock, cancel).await {
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

#[cfg(test)]
thread_local! {
    static FAIL_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
        // Test hook: a wrong-sized frame makes rubato fail for real.
        #[cfg(test)]
        let frame = if FAIL_NEXT.replace(false) {
            &frame[1..]
        } else {
            frame
        };
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

    fn flush(&mut self) -> Option<Vec<f32>> {
        self.0.flush()
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

/// Holds finished finals until every lower id has been sent. An utterance with
/// no final (empty STT) still releases its slot with `None`.
#[derive(Default)]
struct Reorder {
    next: u64,
    held: BTreeMap<u64, Option<Cue>>,
}

impl Reorder {
    /// Record `id` as done and return the finals now sendable, in id order.
    fn release(&mut self, id: u64, cue: Option<Cue>) -> Vec<Cue> {
        self.held.insert(id, cue);
        let mut ready = Vec::new();
        while let Some(slot) = self.held.remove(&self.next) {
            self.next += 1;
            ready.extend(slot);
        }
        ready
    }
}

/// Sends finals in id order. The async lock is held across `send`, so a full
/// channel stalls only other finishing tasks, never the capture loop.
async fn emit_final(
    order: &tokio::sync::Mutex<Reorder>,
    tx: &tokio::sync::mpsc::Sender<Cue>,
    id: u64,
    cue: Option<Cue>,
) {
    let mut order = order.lock().await;
    for cue in order.release(id, cue) {
        if tx.send(cue).await.is_err() {
            tracing::debug!(id, "cue channel closed");
        }
    }
}

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
    /// Reorder buffer: finals leave in utterance-id order.
    order: Arc<tokio::sync::Mutex<Reorder>>,
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
            order: Arc::default(),
        }
    }

    fn spawn_partial(&self, speech: Vec<f32>, span: Span, busy: Arc<AtomicBool>) {
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
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                    lang: None,
                    raw: None,
                });
            }
        });
    }

    /// Close the current utterance and transcribe it off the capture loop.
    /// Cleanup starts only after that line exists. At most `MAX_LIVE_FINALS`
    /// finals stay eligible for cleanup; the oldest beyond that emits its raw
    /// line as the final instead. Capture never waits.
    fn end(&mut self, speech: Vec<f32>, span: Span) {
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
        let order = self.order.clone();
        let ended_at = Instant::now();
        tokio::spawn(async move {
            let raw = with_timeout(stt.transcribe(&speech), Duration::from_secs(4))
                .await
                .unwrap_or_default();
            let stt_done = Instant::now();
            if !raw.is_empty() {
                let _ = tx.try_send(Cue {
                    kind: CueKind::Partial,
                    text: raw.clone(),
                    id: cue_id,
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                    lang: None,
                    raw: None,
                });
                // biased: a final already shed must not start cleanup at all.
                let (text, cleanup) = tokio::select! {
                    biased;
                    _ = &mut skip_rx => (raw.clone(), Duration::ZERO),
                    cleaned = cleaner.clean(&raw) => {
                        (cleaned.unwrap_or_else(|_| raw.clone()), stt_done.elapsed())
                    }
                };
                // One line per final, logged before the reorder wait so that
                // queueing behind an older utterance is not counted.
                tracing::info!(
                    id = cue_id,
                    stt_ms = (stt_done - ended_at).as_millis() as u64,
                    cleanup_ms = cleanup.as_millis() as u64,
                    "final timing"
                );
                live.lock().unwrap().retain(|(i, _)| *i != cue_id);
                let cue = Cue {
                    kind: CueKind::Final,
                    text,
                    id: cue_id,
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                    lang: None,
                    raw: Some(raw),
                };
                emit_final(&order, &tx, cue_id, Some(cue)).await;
            } else {
                live.lock().unwrap().retain(|(i, _)| *i != cue_id);
                emit_final(&order, &tx, cue_id, None).await;
            }
        });
    }
}

async fn worker<A, S, C, K>(
    mut audio: A,
    mut in_rate: u32,
    stt: Arc<S>,
    cleaner: Arc<C>,
    tx: tokio::sync::mpsc::Sender<Cue>,
    clock: K,
    cancel: CancellationToken,
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
    let mut frame: Vec<f32> = Vec::with_capacity(960);
    // Input samples consumed, skipped frames included. The run timeline is
    // sample-counted, so it does not depend on wall time or resampler output.
    let mut consumed: u64 = 0;

    while !cancel.is_cancelled() {
        let mut chunk = Vec::new();
        audio.drain(&mut chunk);
        // Device switched: new rate, so a new resampler and a fresh frame.
        if let Some(rate) = audio.new_rate() {
            in_rate = rate;
            resample = Resample::new(rate)?;
            pending.clear();
        }
        if chunk.is_empty() {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            continue;
        }
        pending.extend(chunk);
        while pending.len() >= 960 {
            frame.clear();
            frame.extend(pending.drain(..960));
            consumed += 960;
            let Some(mono) = resample.process(&frame) else {
                continue;
            };
            match vad.push(&mono) {
                Some(VadEvent::Speaking) => {
                    if let Some(busy) = partials.try_fire(vad.speech().len()) {
                        let speech = vad.speech().to_vec();
                        let span = span_at(consumed, in_rate, speech.len());
                        finals.spawn_partial(speech, span, busy);
                    }
                }
                Some(VadEvent::Ended { speech }) => {
                    let span = span_at(consumed, in_rate, speech.len());
                    finals.end(speech, span);
                    partials.reset();
                }
                None => {}
            }
        }
    }
    // Cancelled: an open utterance becomes one last final. The detached final
    // task holds its own `tx`, so the channel closes once it has sent.
    if let Some(speech) = vad.flush() {
        let span = span_at(consumed, in_rate, speech.len());
        finals.end(speech, span);
    }
    Ok(())
}

/// Position of a segment of `samples` 16 kHz samples ending after `consumed` input samples.
fn span_at(consumed: u64, in_rate: u32, samples: usize) -> Span {
    let end_ms = consumed * 1000 / in_rate as u64;
    Span {
        start_ms: end_ms.saturating_sub(samples as u64 * 1000 / 16_000),
        end_ms,
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

    struct OneChunk(Option<Vec<f32>>);

    impl AudioSource for OneChunk {
        fn drain(&mut self, out: &mut Vec<f32>) {
            out.extend(self.0.take().unwrap_or_default());
        }
    }

    struct Fixed;

    #[async_trait::async_trait]
    impl Transcriber for Fixed {
        async fn transcribe(&self, _pcm16k: &[f32]) -> Result<String> {
            Ok("hello".into())
        }
    }

    #[async_trait::async_trait]
    impl Cleaner for Fixed {
        async fn clean(&self, raw: &str) -> Result<String> {
            Ok(raw.to_uppercase())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn resample_error_skips_frame_and_worker_keeps_emitting() {
        const RATE: usize = 48_000;
        // First frame fails; 0.5 s speech then 0.6 s silence follow.
        FAIL_NEXT.set(true);
        let audio = [vec![0.2_f32; RATE / 2], vec![0.0_f32; RATE * 6 / 10]].concat();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Cue>(32);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(worker(
            OneChunk(Some(audio)),
            RATE as u32,
            Arc::new(Fixed),
            Arc::new(Fixed),
            tx,
            TokioClock,
            cancel.clone(),
        ));
        let mut got = Vec::new();
        while let Ok(Some(cue)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            got.push((format!("{:?}", cue.kind), cue.text));
        }
        assert!(!FAIL_NEXT.get(), "fault was never consumed");
        assert!(!task.is_finished(), "worker died after resample error");
        assert_eq!(
            got.last(),
            Some(&("Final".to_string(), "HELLO".to_string()))
        );
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    fn fin(id: u64) -> Option<Cue> {
        Some(Cue {
            kind: CueKind::Final,
            text: String::new(),
            id,
            start_ms: 0,
            end_ms: 0,
            lang: None,
            raw: None,
        })
    }

    #[test]
    fn reorder_holds_until_lower_ids_arrive() {
        let ids = |v: Vec<Cue>| v.iter().map(|c| c.id).collect::<Vec<_>>();
        let mut r = Reorder::default();
        assert!(r.release(1, fin(1)).is_empty());
        assert_eq!(ids(r.release(0, fin(0))), [0, 1]);
        assert!(r.release(3, fin(3)).is_empty());
        assert!(ids(r.release(2, None)).eq(&[3]), "None frees the slot");
    }

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

    #[test]
    fn cue_json_snapshot() {
        let partial = Cue {
            kind: CueKind::Partial,
            text: "hel".into(),
            id: 7,
            start_ms: 1200,
            end_ms: 2100,
            lang: None,
            raw: None,
        };
        let fin = Cue {
            kind: CueKind::Final,
            text: "Hello.".into(),
            id: 7,
            start_ms: 1200,
            end_ms: 2900,
            lang: Some("en".into()),
            raw: Some("hello".into()),
        };
        assert_eq!(
            serde_json::to_string(&partial).unwrap(),
            r#"{"kind":"partial","text":"hel","id":7,"start_ms":1200,"end_ms":2100}"#
        );
        assert_eq!(
            serde_json::to_string(&fin).unwrap(),
            r#"{"kind":"final","text":"Hello.","id":7,"start_ms":1200,"end_ms":2900,"lang":"en","raw":"hello"}"#
        );
    }
}
