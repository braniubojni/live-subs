/// Energy VAD. Good enough to cut sentences. Replace with Silero once the
/// pipeline is stable — the segmenter API below does not have to change.

#[derive(Clone, Copy)]
pub struct VadConfig {
    pub threshold: f32,
    pub silence_ms: u32,
    pub min_speech_ms: u32,
    pub max_speech_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.015,
            silence_ms: 500,
            min_speech_ms: 250,
            max_speech_ms: 8_000,
        }
    }
}

#[derive(Debug)]
pub enum VadEvent {
    /// Speech still going. Read samples with `Segmenter::speech`.
    Speaking,
    /// Silence closed the utterance. Samples are 16 kHz mono.
    Ended { speech: Vec<f32> },
}

pub struct Segmenter {
    cfg: VadConfig,
    sample_rate: u32,
    speech: Vec<f32>,
    silence_samples: u32,
    in_speech: bool,
}

impl Segmenter {
    pub fn new(sample_rate: u32, cfg: VadConfig) -> Self {
        Self {
            cfg,
            sample_rate,
            speech: Vec::with_capacity(sample_rate as usize * 4),
            silence_samples: 0,
            in_speech: false,
        }
    }

    pub fn push(&mut self, frame: &[f32]) -> Option<VadEvent> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let voiced = rms >= self.cfg.threshold;
        if voiced {
            self.in_speech = true;
            self.silence_samples = 0;
            self.speech.extend_from_slice(frame);
            if self.speech.len() >= samples(self.sample_rate, self.cfg.max_speech_ms) {
                return Some(self.take_end());
            }
            return Some(VadEvent::Speaking);
        }
        if !self.in_speech {
            return None;
        }
        self.silence_samples += frame.len() as u32;
        self.speech.extend_from_slice(frame);
        if self.silence_samples >= samples(self.sample_rate, self.cfg.silence_ms) as u32
            && self.speech.len() >= samples(self.sample_rate, self.cfg.min_speech_ms)
        {
            return Some(self.take_end());
        }
        None
    }

    pub fn speech(&self) -> &[f32] {
        &self.speech
    }

    fn take_end(&mut self) -> VadEvent {
        let speech = std::mem::take(&mut self.speech);
        self.in_speech = false;
        self.silence_samples = 0;
        VadEvent::Ended { speech }
    }
}

fn samples(rate: u32, ms: u32) -> usize {
    (rate as usize * ms as usize) / 1000
}
