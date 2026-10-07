//! Silero VAD v5 (ONNX). Speech probability per 512-sample chunk at 16 kHz, fed to the
//! same segmenter state machine as the energy VAD.
use std::path::Path;

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;

use crate::vad::{Segmenter, Vad, VadConfig, VadEvent};

const CHUNK: usize = 512;
/// v5 wants the last 64 samples of the previous chunk prepended.
const CONTEXT: usize = 64;
const STATE_LEN: usize = 2 * 128;
/// Open at 0.5, close at 0.35 (Silero's own neg_threshold = threshold - 0.15).
const OPEN: f32 = 0.5;
const CLOSE: f32 = 0.35;

pub struct SileroVad {
    session: Session,
    seg: Segmenter,
    state: Vec<f32>,
    context: [f32; CONTEXT],
    pending: Vec<f32>,
    speaking: bool,
}

impl SileroVad {
    pub fn new(model: impl AsRef<Path>, cfg: VadConfig) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model.as_ref())
            .with_context(|| format!("load silero model {}", model.as_ref().display()))?;
        Ok(Self {
            session,
            seg: Segmenter::new(16_000, cfg),
            state: vec![0.0; STATE_LEN],
            context: [0.0; CONTEXT],
            pending: Vec::new(),
            speaking: false,
        })
    }

    fn probability(&mut self, chunk: &[f32]) -> Result<f32> {
        let mut input = Vec::with_capacity(CONTEXT + CHUNK);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(chunk);
        self.context.copy_from_slice(&chunk[CHUNK - CONTEXT..]);
        let outputs = self.session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, CONTEXT + CHUNK], input))?,
            "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone()))?,
            "sr" => Tensor::from_array(([0usize; 0], vec![16_000i64]))?,
        ])?;
        let (_, prob) = outputs["output"].try_extract_tensor::<f32>()?;
        let prob = prob[0];
        let (_, next) = outputs["stateN"].try_extract_tensor::<f32>()?;
        self.state.copy_from_slice(next);
        Ok(prob)
    }
}

impl Vad for SileroVad {
    /// Frames of any size. One call can span several chunks; `Ended` wins over `Speaking`.
    fn push(&mut self, frame: &[f32]) -> Option<VadEvent> {
        self.pending.extend_from_slice(frame);
        let mut event = None;
        while self.pending.len() >= CHUNK {
            let chunk: Vec<f32> = self.pending.drain(..CHUNK).collect();
            let prob = self.probability(&chunk).unwrap_or_else(|err| {
                tracing::warn!(%err, "silero inference failed, treating chunk as silence");
                0.0
            });
            self.speaking = prob >= if self.speaking { CLOSE } else { OPEN };
            // ponytail: two Ended in one push (frame > 8 s) drops the first.
            match self.seg.advance(&chunk, self.speaking) {
                Some(e @ VadEvent::Ended { .. }) => event = Some(e),
                Some(e) if event.is_none() => event = Some(e),
                _ => {}
            }
        }
        event
    }

    fn speech(&self) -> &[f32] {
        self.seg.speech()
    }

    fn flush(&mut self) -> Option<Vec<f32>> {
        self.pending.clear();
        self.seg.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "models/silero_vad.onnx";

    fn model() -> Option<SileroVad> {
        if !Path::new(MODEL).exists() {
            eprintln!("skip: {MODEL} missing");
            return None;
        }
        Some(SileroVad::new(MODEL, VadConfig::default()).unwrap())
    }

    /// 16 kHz mono TTS clip, ~2.5 s.
    fn speech() -> Vec<f32> {
        hound::WavReader::open("tests/fixtures/speech.wav")
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect()
    }

    #[test]
    fn silence_and_hum_do_not_trigger() {
        let Some(mut vad) = model() else { return };
        assert!(vad.push(&vec![0.0; 16_000]).is_none());
        let hum: Vec<f32> = (0..16_000)
            .map(|i| 0.02 * (2.0 * std::f32::consts::PI * 50.0 * i as f32 / 16_000.0).sin())
            .collect();
        assert!(vad.push(&hum).is_none());
    }

    #[test]
    fn real_speech_opens_and_closes() {
        let Some(mut vad) = model() else { return };
        let mut opened = false;
        for frame in speech().chunks(1_600) {
            opened |= matches!(vad.push(frame), Some(VadEvent::Speaking));
        }
        assert!(opened, "speech should open the gate");
        let ended = vad.push(&vec![0.0; 16_000]);
        match ended {
            Some(VadEvent::Ended { speech }) => assert!(speech.len() > 16_000),
            other => panic!("expected Ended, got {other:?}"),
        }
    }
}
