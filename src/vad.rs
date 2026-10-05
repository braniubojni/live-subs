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
    voiced_samples: usize,
    in_speech: bool,
}

impl Segmenter {
    pub fn new(sample_rate: u32, cfg: VadConfig) -> Self {
        Self {
            cfg,
            sample_rate,
            speech: Vec::with_capacity(sample_rate as usize * 4),
            silence_samples: 0,
            voiced_samples: 0,
            in_speech: false,
        }
    }

    pub fn push(&mut self, frame: &[f32]) -> Option<VadEvent> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let voiced = rms >= self.cfg.threshold;
        if voiced {
            self.in_speech = true;
            self.silence_samples = 0;
            self.voiced_samples += frame.len();
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
        if self.silence_samples >= samples(self.sample_rate, self.cfg.silence_ms) as u32 {
            if self.voiced_samples >= samples(self.sample_rate, self.cfg.min_speech_ms) {
                return Some(self.take_end());
            }
            // Blip shorter than min speech: drop it so it cannot merge with the next burst.
            self.speech.clear();
            self.in_speech = false;
            self.silence_samples = 0;
            self.voiced_samples = 0;
        }
        None
    }

    pub fn speech(&self) -> &[f32] {
        &self.speech
    }

    /// Close an utterance still open (shutdown). Same min-speech rule as `push`.
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        let long_enough = self.voiced_samples >= samples(self.sample_rate, self.cfg.min_speech_ms);
        match self.take_end() {
            VadEvent::Ended { speech } if long_enough => Some(speech),
            _ => None,
        }
    }

    fn take_end(&mut self) -> VadEvent {
        let speech = std::mem::take(&mut self.speech);
        self.in_speech = false;
        self.silence_samples = 0;
        self.voiced_samples = 0;
        VadEvent::Ended { speech }
    }
}

fn samples(rate: u32, ms: u32) -> usize {
    (rate as usize * ms as usize) / 1000
}

#[cfg(test)]
mod tests {
    use super::{Segmenter, VadConfig, VadEvent};

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        vec![amp; n]
    }

    #[test]
    fn silence_emits_nothing() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        assert!(seg.push(&tone(16_000, 0.0)).is_none());
    }

    #[test]
    fn short_blip_is_dropped() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        assert!(matches!(
            seg.push(&tone(1_600, 0.2)),
            Some(VadEvent::Speaking)
        ));
        assert!(seg.push(&tone(8_000, 0.0)).is_none());
    }

    #[test]
    fn speech_then_silence_ends_the_utterance() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        assert!(matches!(
            seg.push(&tone(8_000, 0.2)),
            Some(VadEvent::Speaking)
        ));
        match seg.push(&tone(8_000, 0.0)) {
            Some(VadEvent::Ended { speech }) => assert!(speech.len() > 8_000),
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    #[test]
    fn long_speech_is_cut_at_the_max() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        let event = seg.push(&tone(16_000 * 8, 0.2));
        assert!(matches!(event, Some(VadEvent::Ended { .. })));
    }
}
