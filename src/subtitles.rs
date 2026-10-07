//! What the overlay shows: a few lines, newest last. Pure state, no GUI deps.
//!
//! A partial replaces the line with the same id, a final replaces the partial,
//! and a partial arriving after its final is dropped. Lines expire `ttl` after
//! their last update; at most `max_lines` are kept.

use crate::pipeline::{Cue, CueKind};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Line {
    pub id: u64,
    pub text: String,
    pub is_partial: bool,
    at: Instant,
}

#[derive(Debug)]
pub struct SubtitleState {
    lines: VecDeque<Line>,
    ttl: Duration,
    max_lines: usize,
}

impl SubtitleState {
    pub fn new(ttl: Duration, max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            ttl,
            max_lines: max_lines.max(1),
        }
    }

    pub fn apply(&mut self, cue: &Cue, now: Instant) {
        let is_partial = matches!(cue.kind, CueKind::Partial);
        match self.lines.iter_mut().find(|l| l.id == cue.id) {
            Some(l) if !l.is_partial && is_partial => {} // late partial
            Some(l) => {
                l.text.clone_from(&cue.text);
                l.is_partial = is_partial;
                l.at = now;
            }
            None => {
                // ponytail: an expired id's late partial would reappear; ids are
                // monotonic, so also drop anything older than the newest line.
                if self.lines.back().is_some_and(|l| l.id > cue.id) {
                    return;
                }
                self.lines.push_back(Line {
                    id: cue.id,
                    text: cue.text.clone(),
                    is_partial,
                    at: now,
                });
                if self.lines.len() > self.max_lines {
                    self.lines.pop_front();
                }
            }
        }
    }

    pub fn expire(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.lines.retain(|l| now.duration_since(l.at) < ttl);
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    /// When the next line expires, for scheduling a repaint.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.lines.iter().map(|l| l.at + self.ttl).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(kind: CueKind, id: u64, text: &str) -> Cue {
        Cue {
            kind,
            text: text.into(),
            id,
            start_ms: 0,
            end_ms: 0,
            lang: None,
            raw: None,
        }
    }

    fn texts(s: &SubtitleState) -> Vec<(String, bool)> {
        s.lines().map(|l| (l.text.clone(), l.is_partial)).collect()
    }

    #[test]
    fn partial_then_final_replace_same_line() {
        let t = Instant::now();
        let mut s = SubtitleState::new(Duration::from_secs(4), 2);
        s.apply(&cue(CueKind::Partial, 1, "hel"), t);
        s.apply(&cue(CueKind::Partial, 1, "hello"), t);
        assert_eq!(texts(&s), [("hello".into(), true)]);
        s.apply(&cue(CueKind::Final, 1, "Hello."), t);
        assert_eq!(texts(&s), [("Hello.".into(), false)]);
    }

    #[test]
    fn late_partial_after_final_is_ignored() {
        let t = Instant::now();
        let mut s = SubtitleState::new(Duration::from_secs(4), 2);
        s.apply(&cue(CueKind::Final, 1, "Hello."), t);
        s.apply(&cue(CueKind::Partial, 1, "hel"), t);
        assert_eq!(texts(&s), [("Hello.".into(), false)]);
    }

    #[test]
    fn late_partial_of_expired_utterance_is_ignored() {
        let t = Instant::now();
        let mut s = SubtitleState::new(Duration::from_secs(1), 2);
        s.apply(&cue(CueKind::Final, 1, "a"), t);
        s.apply(&cue(CueKind::Final, 2, "b"), t);
        s.expire(t + Duration::from_secs(2));
        s.apply(&cue(CueKind::Partial, 3, "c"), t + Duration::from_secs(2));
        s.apply(&cue(CueKind::Partial, 1, "a"), t + Duration::from_secs(2));
        assert_eq!(texts(&s), [("c".into(), true)]);
    }

    #[test]
    fn third_line_evicts_oldest() {
        let t = Instant::now();
        let mut s = SubtitleState::new(Duration::from_secs(4), 2);
        for (id, w) in [(1, "a"), (2, "b"), (3, "c")] {
            s.apply(&cue(CueKind::Final, id, w), t);
        }
        assert_eq!(texts(&s), [("b".into(), false), ("c".into(), false)]);
    }

    #[test]
    fn expiry_counts_from_last_update() {
        let t = Instant::now();
        let ttl = Duration::from_secs(4);
        let mut s = SubtitleState::new(ttl, 2);
        s.apply(&cue(CueKind::Partial, 1, "a"), t);
        s.apply(&cue(CueKind::Final, 2, "b"), t + Duration::from_secs(2));
        assert_eq!(s.next_expiry(), Some(t + ttl));
        s.expire(t + ttl - Duration::from_millis(1));
        assert_eq!(s.lines().count(), 2);
        s.expire(t + ttl);
        assert_eq!(texts(&s), [("b".into(), false)]);
        s.expire(t + Duration::from_secs(6));
        assert_eq!(s.next_expiry(), None);
    }
}
