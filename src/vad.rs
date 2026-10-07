/// Energy VAD. Good enough to cut sentences. Replace with Silero once the
/// pipeline is stable — the segmenter API below does not have to change.
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub struct VadConfig {
    /// Minimum RMS to start speech.
    pub open: f32,
    /// Minimum RMS to keep speech going. Below `open` so quiet word endings do not flap.
    pub close: f32,
    /// Gate follows `noise_floor * noise_ratio` when that beats `open`; `close` scales along.
    pub noise_ratio: f32,
    pub silence_ms: u32,
    pub min_speech_ms: u32,
    pub max_speech_ms: u32,
    pub preroll_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            open: 0.015,
            close: 0.010,
            noise_ratio: 3.0,
            silence_ms: 500,
            min_speech_ms: 250,
            max_speech_ms: 8_000,
            preroll_ms: 200,
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
    preroll: VecDeque<f32>,
    silence_samples: usize,
    voiced_samples: usize,
    in_speech: bool,
    noise_floor: f32,
}

impl Segmenter {
    pub fn new(sample_rate: u32, cfg: VadConfig) -> Self {
        Self {
            cfg,
            sample_rate,
            speech: Vec::with_capacity(sample_rate as usize * 4),
            preroll: VecDeque::new(),
            silence_samples: 0,
            voiced_samples: 0,
            in_speech: false,
            noise_floor: 0.0,
        }
    }

    pub fn push(&mut self, frame: &[f32]) -> Option<VadEvent> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let open = self.cfg.open.max(self.noise_floor * self.cfg.noise_ratio);
        let threshold = if self.in_speech {
            open * self.cfg.close / self.cfg.open
        } else {
            open
        };
        let voiced = rms >= threshold;
        if !self.in_speech && !voiced {
            let alpha = (frame.len() as f32 / self.sample_rate as f32 / FLOOR_TAU_SECS).min(1.0);
            self.noise_floor += alpha * (rms - self.noise_floor);
        }
        if voiced {
            if !self.in_speech {
                self.speech.extend(self.preroll.drain(..));
            }
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
            let keep = samples(self.sample_rate, self.cfg.preroll_ms);
            self.preroll.extend(frame);
            let extra = self.preroll.len().saturating_sub(keep);
            self.preroll.drain(..extra);
            return None;
        }
        self.silence_samples += frame.len();
        self.speech.extend_from_slice(frame);
        if self.silence_samples >= samples(self.sample_rate, self.cfg.silence_ms) {
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

/// Noise floor EMA time constant. Slow, so speech pauses do not drag it up.
const FLOOR_TAU_SECS: f32 = 2.0;

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
        // Real speech after the blip: the 0.2 blip must not be in the buffer.
        assert!(matches!(
            seg.push(&tone(8_000, 0.1)),
            Some(VadEvent::Speaking)
        ));
        match seg.push(&tone(8_000, 0.0)) {
            Some(VadEvent::Ended { speech }) => {
                assert!(speech.iter().all(|s| *s <= 0.1));
                assert_eq!(speech.len(), 8_000 + 8_000);
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    #[test]
    fn hysteresis_keeps_quiet_endings() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        // 0.012 is between close (0.010) and open (0.015): cannot open...
        assert!(seg.push(&tone(1_600, 0.012)).is_none());
        // ...but once open, ramp down through it stays voiced, below close ends.
        for amp in [0.2, 0.05, 0.02, 0.012, 0.011] {
            assert!(matches!(
                seg.push(&tone(1_600, amp)),
                Some(VadEvent::Speaking)
            ));
        }
        assert!(seg.push(&tone(1_600, 0.005)).is_none());
        assert_eq!(seg.silence_samples, 1_600);
    }

    #[test]
    fn constant_hum_never_triggers() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        for _ in 0..100 {
            assert!(seg.push(&tone(1_600, 0.01)).is_none());
        }
        assert!(seg.noise_floor > 0.009);
    }

    #[test]
    fn speech_over_hum_triggers() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        for _ in 0..100 {
            seg.push(&tone(1_600, 0.01));
        }
        // floor ~0.01 -> open ~0.03; 0.05 clears it, 0.02 does not.
        assert!(seg.push(&tone(1_600, 0.02)).is_none());
        assert!(matches!(
            seg.push(&tone(1_600, 0.05)),
            Some(VadEvent::Speaking)
        ));
    }

    #[test]
    fn preroll_is_prepended() {
        let mut seg = Segmenter::new(16_000, VadConfig::default());
        // 1 s quiet, then speech: only the last 200 ms of the quiet part is kept.
        assert!(seg.push(&tone(16_000, 0.001)).is_none());
        seg.push(&tone(8_000, 0.2));
        match seg.push(&tone(8_000, 0.0)) {
            Some(VadEvent::Ended { speech }) => {
                assert_eq!(speech.len(), 3_200 + 8_000 + 8_000);
                assert_eq!(speech[0], 0.001);
                assert_eq!(speech[3_200], 0.2);
            }
            other => panic!("expected Ended, got {other:?}"),
        }
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
