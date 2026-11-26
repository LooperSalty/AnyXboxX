//! Animation maths: plain functions of time, so the window only feeds them the clock.

use std::f64::consts::TAU;

/// Clamps to 0..=1.
pub fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Fast start, soft landing.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    1.0 - (1.0 - t).powi(3)
}

/// Progress of an animation that began at `started` and lasts `duration` seconds, eased.
pub fn eased_progress(now: f64, started: f64, duration: f64) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    ease_out_cubic(((now - started) / duration) as f32)
}

/// A smooth 0..1..0 wave with the given period in seconds.
pub fn pulse(time: f64, period: f64) -> f32 {
    (0.5 - 0.5 * (time / period * TAU).cos()) as f32
}

/// A sawtooth 0..1 that repeats every `period` seconds.
pub fn sweep(time: f64, period: f64) -> f32 {
    (time / period).fract() as f32
}

const BAND: f32 = 0.3;

/// The moving segment (start, end), as fractions of a bar: a band that slides across it.
pub fn moving_band(time: f64, period: f64) -> (f32, f32) {
    let head = sweep(time, period) * (1.0 + BAND);
    (clamp01(head - BAND), clamp01(head))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_runs_from_zero_to_one_and_clamps() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-4.0), 0.0);
        assert_eq!(ease_out_cubic(9.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn progress_follows_the_clock() {
        assert_eq!(eased_progress(10.0, 10.0, 0.3), 0.0);
        assert_eq!(eased_progress(10.3, 10.0, 0.3), 1.0);
        assert_eq!(eased_progress(9.0, 10.0, 0.3), 0.0);
        assert_eq!(eased_progress(1.0, 10.0, 0.0), 1.0);
    }

    #[test]
    fn pulse_stays_in_range_and_repeats() {
        assert!(pulse(0.0, 2.0).abs() < 1e-6);
        assert!((pulse(1.0, 2.0) - 1.0).abs() < 1e-6);
        assert!((pulse(0.7, 2.0) - pulse(2.7, 2.0)).abs() < 1e-5);
    }

    #[test]
    fn the_band_stays_inside_the_bar() {
        for i in 0..200 {
            let (a, b) = moving_band(f64::from(i) * 0.037, 1.5);
            assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b) && a <= b);
        }
    }
}
