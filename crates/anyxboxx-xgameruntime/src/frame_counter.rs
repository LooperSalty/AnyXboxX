//! The title's frame counter (`ANYXBOXX_FPS=1` for the window title, `ANYXBOXX_FPS_CSV=<file>` for
//! one line per second). Each `Present` is timed; once a second has passed the frames of the window
//! are reported: frames per second, mean frame time and the slowest 1 % (C++ `frame_counter.cpp`).

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

use crate::log::trace;

/// Length of a measurement window, in seconds.
const REPORT_SECONDS: f64 = 1.0;
/// Share of the slowest frames whose time is reported (« 1 % bas »).
const SLOWEST_SHARE: f64 = 0.01;
const CSV_HEADER: &str = "secondes,images,fps,ms_moyen,ms_p99\n";

/// One measurement, as it is reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    pub frames: usize,
    pub fps: f64,
    pub average_ms: f64,
    pub slowest_ms: f64,
}

/// The state of the counter: when it started, the window, and the frame times of the window.
struct Counter {
    start: Option<Instant>,
    last: Option<Instant>,
    window_start: Option<Instant>,
    frame_ms: Vec<f64>,
    csv_started: bool,
}

static COUNTER: Mutex<Counter> = Mutex::new(Counter {
    start: None,
    last: None,
    window_start: None,
    frame_ms: Vec::new(),
    csv_started: false,
});

/// Summarises the frame times of a window: the rate over `elapsed` seconds, the mean frame time and
/// the slowest share. `None` when no frame was timed or `elapsed` is not positive and finite.
pub fn summarise(frame_ms: &[f64], elapsed: f64) -> Option<Report> {
    if frame_ms.is_empty() || !elapsed.is_finite() || elapsed <= 0.0 {
        return None;
    }
    let mut sorted = frame_ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let frames = sorted.len();
    let total: f64 = sorted.iter().sum();
    let slow_index = (frames - 1).min((frames as f64 * (1.0 - SLOWEST_SHARE)) as usize);
    Some(Report {
        frames,
        fps: frames as f64 / elapsed,
        average_ms: total / frames as f64,
        slowest_ms: sorted[slow_index],
    })
}

/// Converts a slowest-frame duration in milliseconds into frames per second, returning 0 when the
/// duration is non-positive or non-finite so logs and titles never display `inf`.
pub fn low_fps(slowest_ms: f64) -> f64 {
    if slowest_ms.is_finite() && slowest_ms > 0.0 {
        1000.0 / slowest_ms
    } else {
        0.0
    }
}

/// Records one `Present`: the first call starts the clock; each later one adds a frame time and,
/// once a window is complete, reports it (CSV, journal, title).
pub fn frame(title_text: impl FnOnce() -> Option<String>) {
    let now = Instant::now();
    let mut counter = COUNTER.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(last), Some(window_start), Some(start)) =
        (counter.last, counter.window_start, counter.start)
    else {
        counter.start = Some(now);
        counter.last = Some(now);
        counter.window_start = Some(now);
        return;
    };
    counter.frame_ms.push((now - last).as_secs_f64() * 1000.0);
    counter.last = Some(now);
    let elapsed = (now - window_start).as_secs_f64();
    if elapsed < REPORT_SECONDS {
        return;
    }
    let since_start = (now - start).as_secs_f64();
    if let Some(report) = summarise(&counter.frame_ms, elapsed) {
        trace!(
            "[fps] {:.1} i/s · {:.2} ms (1 % bas : {:.1} i/s)",
            report.fps,
            report.average_ms,
            low_fps(report.slowest_ms)
        );
        write_csv(&mut counter, since_start, report);
        show_in_title(title_text, report);
    }
    counter.frame_ms.clear();
    counter.window_start = Some(now);
}

/// Appends the report to `ANYXBOXX_FPS_CSV`, with its header on first use. Errors are ignored.
fn write_csv(counter: &mut Counter, since_start: f64, report: Report) {
    let Some(path) = std::env::var_os("ANYXBOXX_FPS_CSV").filter(|p| !p.is_empty()) else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    if !counter.csv_started {
        let _ = file.write_all(CSV_HEADER.as_bytes());
    }
    counter.csv_started = true;
    let _ = writeln!(
        file,
        "{since_start:.3},{},{:.2},{:.3},{:.3}",
        report.frames, report.fps, report.average_ms, report.slowest_ms
    );
}

/// Shows the rate in the title of the game window, when `ANYXBOXX_FPS=1`. The title is read once,
/// from the first window seen. The window text itself comes from the caller.
fn show_in_title(title_text: impl FnOnce() -> Option<String>, report: Report) {
    if std::env::var("ANYXBOXX_FPS").as_deref() != Ok("1") {
        return;
    }
    if let Some(text) = title_text() {
        crate::dxgi_factory::set_window_title(&format!(
            "{text} | AnyXboxX {:.0} i/s (1 % bas {:.0})",
            report.fps,
            low_fps(report.slowest_ms)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_summarised_with_its_slowest_share() {
        // 100 frames of 10 ms and one of 100 ms over 2 s: 101 frames, 50.5 i/s.
        let mut frames = vec![10.0; 100];
        frames.push(100.0);
        let report = summarise(&frames, 2.0).unwrap();
        assert_eq!(report.frames, 101);
        assert!((report.fps - 50.5).abs() < 1e-9);
        // Slowest 1 %: index min(100, floor(101 * 0.99)) = 99, a 10 ms frame.
        assert_eq!(report.slowest_ms, 10.0);
    }

    #[test]
    fn an_empty_window_has_no_report() {
        assert_eq!(summarise(&[], 1.0), None);
        assert_eq!(summarise(&[16.0], 0.0), None);
        assert_eq!(summarise(&[16.0], f64::NAN), None);
        assert_eq!(summarise(&[16.0], f64::INFINITY), None);
    }

    #[test]
    fn low_fps_avoids_division_by_zero_and_non_finite_values() {
        assert_eq!(low_fps(0.0), 0.0);
        assert_eq!(low_fps(-1.0), 0.0);
        assert_eq!(low_fps(f64::NAN), 0.0);
        assert_eq!(low_fps(f64::INFINITY), 0.0);
        assert!((low_fps(20.0) - 50.0).abs() < 1e-9);
    }
}
