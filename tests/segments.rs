//! Integration tests see the library crate only, not the binary.
//! Device capture is not exercised here. CI has no audio device.

use live_subs::cleanup::accept_cleanup;
use live_subs::vad::{Segmenter, VadConfig, VadEvent};

fn tone(n: usize, amp: f32) -> Vec<f32> {
    vec![amp; n]
}

#[test]
fn utterance_closes_after_silence() {
    let mut seg = Segmenter::new(16_000, VadConfig::default());
    assert!(seg.push(&tone(8_000, 0.1)).is_some());
    let event = seg
        .push(&tone(8_000, 0.0))
        .expect("500 ms of silence closes the utterance");
    assert!(matches!(event, VadEvent::Ended { .. }));
}

#[test]
fn cleanup_keeps_raw_line_when_model_expands_it() {
    let raw = "ship it friday";
    assert_eq!(accept_cleanup(raw, "Ship it Friday."), "Ship it Friday.");
    assert_eq!(
        accept_cleanup(
            raw,
            "We should definitely ship it on Friday and tell the team."
        ),
        raw
    );
    assert_eq!(accept_cleanup(raw, "   "), raw);
}
