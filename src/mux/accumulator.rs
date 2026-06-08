use std::ops::Range;
use std::time::Instant;

// ── BatchAccumulator ──────────────────────────────────────────────────────────

/// Accumulates texts from multiple callers into a single provider batch.
///
/// Tracks per-caller ranges so results can be demultiplexed after the
/// provider responds.
pub struct BatchAccumulator {
    /// All accumulated texts (from all callers).
    pub texts: Vec<String>,
    /// Maps caller_id → range of text indices in `texts`.
    pub caller_ranges: Vec<(usize, Range<usize>)>,
    /// Time when the first text was added (used to enforce batch_window_ms).
    pub deadline: Instant,
    /// Maximum texts per batch (from provider config).
    pub max_texts: usize,
}

impl BatchAccumulator {
    /// Create a new empty accumulator.
    pub fn new(max_texts: usize, deadline: Instant) -> Self {
        Self {
            texts: Vec::new(),
            caller_ranges: Vec::new(),
            deadline,
            max_texts,
        }
    }

    /// Returns true if the accumulator has no texts.
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// Returns the number of texts currently accumulated.
    pub fn len(&self) -> usize {
        self.texts.len()
    }

    /// Returns true if adding `count` more texts would exceed `max_texts`.
    pub fn would_overflow(&self, count: usize) -> bool {
        self.texts.len() + count > self.max_texts
    }

    /// Add texts from a caller and record their range.
    ///
    /// Returns `false` if adding these texts would exceed `max_texts`, `true` on success.
    pub fn add_caller(&mut self, caller_id: usize, texts: Vec<String>) -> bool {
        if self.texts.len() + texts.len() > self.max_texts {
            return false;
        }
        let start = self.texts.len();
        let end = start + texts.len();
        self.texts.extend(texts);
        self.caller_ranges.push((caller_id, start..end));
        true
    }

    /// Return the number of distinct callers accumulated.
    pub fn caller_count(&self) -> usize {
        self.caller_ranges.len()
    }

    /// Extract caller ranges for demultiplexing results.
    /// Returns a clone of the caller_ranges slice.
    pub fn drain_caller_ranges(&mut self) -> Vec<(usize, Range<usize>)> {
        std::mem::take(&mut self.caller_ranges)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_acc(max_texts: usize) -> BatchAccumulator {
        BatchAccumulator::new(max_texts, Instant::now() + Duration::from_millis(50))
    }

    #[test]
    fn test_accumulator_starts_empty() {
        let acc = make_acc(10);
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
        assert_eq!(acc.caller_count(), 0);
    }

    #[test]
    fn test_accumulator_add_single_caller() {
        let mut acc = make_acc(10);
        let texts = vec!["hello".to_string(), "world".to_string()];
        assert!(acc.add_caller(0, texts), "should not overflow");
        assert_eq!(acc.len(), 2);
        assert_eq!(acc.caller_count(), 1);
        assert_eq!(acc.texts[0], "hello");
        assert_eq!(acc.texts[1], "world");
    }

    #[test]
    fn test_accumulator_tracks_caller_ranges() {
        let mut acc = make_acc(20);
        assert!(acc.add_caller(0, vec!["a".to_string(), "b".to_string()]), "no overflow");
        assert!(
            acc.add_caller(1, vec!["c".to_string(), "d".to_string(), "e".to_string()]),
            "no overflow"
        );

        assert_eq!(acc.caller_ranges[0], (0, 0..2));
        assert_eq!(acc.caller_ranges[1], (1, 2..5));
    }

    #[test]
    fn test_accumulator_add_multiple_callers_correct_text_order() {
        let mut acc = make_acc(100);
        assert!(acc.add_caller(42, vec!["x".to_string()]));
        assert!(acc.add_caller(99, vec!["y".to_string(), "z".to_string()]));

        assert_eq!(acc.texts, vec!["x", "y", "z"]);
        assert_eq!(acc.caller_ranges[0], (42, 0..1));
        assert_eq!(acc.caller_ranges[1], (99, 1..3));
    }

    #[test]
    fn test_accumulator_overflow_returns_error() {
        let mut acc = make_acc(3);
        assert!(
            acc.add_caller(0, vec!["a".to_string(), "b".to_string(), "c".to_string()]),
            "exactly at capacity"
        );
        // Adding one more should fail
        let result = acc.add_caller(1, vec!["d".to_string()]);
        assert!(!result, "overflow should return false");
        // Texts should not have changed
        assert_eq!(acc.len(), 3);
    }

    #[test]
    fn test_accumulator_would_overflow_true() {
        let mut acc = make_acc(3);
        assert!(acc.add_caller(0, vec!["a".to_string(), "b".to_string()]));
        assert!(acc.would_overflow(2), "2 more would exceed max 3");
    }

    #[test]
    fn test_accumulator_would_overflow_false() {
        let mut acc = make_acc(4);
        assert!(acc.add_caller(0, vec!["a".to_string(), "b".to_string()]));
        assert!(!acc.would_overflow(2), "2 more fits within max 4");
    }

    #[test]
    fn test_accumulator_drain_caller_ranges() {
        let mut acc = make_acc(10);
        assert!(acc.add_caller(5, vec!["hello".to_string()]));
        assert!(acc.add_caller(6, vec!["world".to_string()]));

        let ranges = acc.drain_caller_ranges();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], (5, 0..1));
        assert_eq!(ranges[1], (6, 1..2));
        // After drain, caller_ranges is empty
        assert_eq!(acc.caller_count(), 0);
    }

    #[test]
    fn test_accumulator_single_text_single_caller() {
        let mut acc = make_acc(128);
        assert!(acc.add_caller(0, vec!["single".to_string()]));
        assert_eq!(acc.len(), 1);
        assert_eq!(acc.caller_count(), 1);
        assert_eq!(acc.caller_ranges[0], (0, 0..1));
    }

    #[test]
    fn test_accumulator_exactly_at_max_texts() {
        let mut acc = make_acc(2);
        assert!(
            acc.add_caller(0, vec!["a".to_string(), "b".to_string()]),
            "exactly at max must succeed"
        );
        assert_eq!(acc.len(), 2);
        assert!(!acc.would_overflow(0));
    }
}
