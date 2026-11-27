//! Translation time on either side of the title's first ExecuteCommandLists.

use super::held;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_THIN_EARLY").is_some_and(|v| v == "1"))
}

#[derive(Default)]
struct Sample {
    lists: u64,
    before: Duration,
    total: Duration,
}

impl Sample {
    fn collect(&mut self, intervals: &mut Vec<(Instant, Instant)>, submit: Instant) {
        if intervals.is_empty() {
            return;
        }
        self.lists += 1;
        for (start, end) in std::mem::take(intervals) {
            self.total += end - start;
            self.before += end.min(submit).saturating_duration_since(start);
        }
    }
}

static SAMPLE: Mutex<Sample> = Mutex::new(Sample {
    lists: 0,
    before: Duration::ZERO,
    total: Duration::ZERO,
});

/// Called on unfold. A second Execute sees an empty vector, and an abandoned
/// recording never unfolds. Publish the numerator and denominator together.
pub(super) fn count(intervals: &mut Vec<(Instant, Instant)>, at: Instant) {
    held(&SAMPLE).collect(intervals, at);
}

pub(super) fn take() -> (u64, Duration, Duration) {
    let s = std::mem::take(&mut *held(&SAMPLE));
    (s.lists, s.before, s.total)
}

pub(super) fn line(frames: f64) {
    let (lists, before, total) = take();
    if !total.is_zero() {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / frames;
        tracing::info!(
            "VICTORY_THIN_EARLY: {:.1}% of translation before first submit, {:.2} ms before and {:.2} ms after per frame (all workers, {} recordings)",
            100.0 * before.as_secs_f64() / total.as_secs_f64(),
            ms(before),
            ms(total - before),
            lists
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_active_work_at_submit_and_counts_each_recording_once() {
        let epoch = Instant::now();
        let t = |n| epoch + Duration::from_micros(n);
        let mut chunks = vec![(t(1), t(4)), (t(20), t(29)), (t(30), t(37))];
        let mut sample = Sample::default();
        sample.collect(&mut chunks, t(25));
        sample.collect(&mut chunks, t(40));
        assert_eq!(sample.lists, 1);
        assert_eq!(sample.before, Duration::from_micros(8));
        assert_eq!(sample.total, Duration::from_micros(19));
        // One list completed before Execute, another only started afterwards.
        sample.collect(&mut vec![(t(1), t(5))], t(10));
        sample.collect(&mut vec![(t(12), t(14))], t(10));
        assert_eq!(sample.lists, 3);
        assert_eq!(sample.before, Duration::from_micros(12));
        assert_eq!(sample.total, Duration::from_micros(25));
    }
}
