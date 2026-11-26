//! What the full-window launch screen shows, computed from plain facts: the stage of the launch
//! (from the job's step labels), the fraction of the bar (from VIctory's progress lines), the
//! elapsed time and the time left. The clock is passed in as seconds, so the tests need no timer.

use crate::progress::Progress;

/// The three stages a player sees, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Build,
    Shaders,
    Start,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Build, Stage::Shaders, Stage::Start];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Build => "Building VIctory",
            Stage::Shaders => "Preloading shaders",
            Stage::Start => "Starting the game",
        }
    }

    /// The stage of a job step, from the labels `launch` gives its steps.
    pub fn of_step(label: &str) -> Stage {
        let lower = label.to_lowercase();
        if lower.contains("build") {
            Stage::Build
        } else if lower.contains("shader") || lower.contains("warm") || lower.contains("preload") {
            Stage::Shaders
        } else {
            Stage::Start
        }
    }
}

/// The stages a launch goes through, in order and without repeats (two shader steps are one stage).
pub fn stages_of(step_labels: &[String]) -> Vec<Stage> {
    let mut stages: Vec<Stage> = step_labels.iter().map(|l| Stage::of_step(l)).collect();
    stages.dedup();
    stages
}

/// Seconds of measuring before a time left is shown: the first seconds of a warm are not its pace.
const MIN_SAMPLE_SECS: f64 = 3.0;

/// Follows one run of progress lines (one file, one kind of count) and its pace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tracker {
    stage: Option<Stage>,
    launch_started: f64,
    last: Option<Progress>,
    /// Where the current run began: (seconds, items done then).
    origin: Option<(f64, u64)>,
    last_at: f64,
}

impl Tracker {
    /// A launch begins at `now`.
    pub fn start(now: f64) -> Self {
        Tracker {
            launch_started: now,
            last_at: now,
            ..Tracker::default()
        }
    }

    /// A step began: progress from the previous step no longer counts.
    pub fn step(&mut self, label: &str, now: f64) {
        self.stage = Some(Stage::of_step(label));
        self.last_at = now;
        self.last = None;
        self.origin = None;
    }

    /// One progress line arrived at `now`. A new file, a new kind of count or a count going back
    /// starts a new run, so the pace is never mixed across files.
    pub fn observe(&mut self, progress: &Progress, now: f64) {
        let same_run = self.last.as_ref().is_some_and(|last| {
            last.file == progress.file
                && last.what == progress.what
                && last.total == progress.total
                && progress.done >= last.done
        });
        if !same_run {
            self.origin = Some((now, progress.done));
        }
        self.last = Some(progress.clone());
        self.last_at = now;
    }

    pub fn stage(&self) -> Option<Stage> {
        self.stage
    }

    pub fn progress(&self) -> Option<&Progress> {
        self.last.as_ref()
    }

    /// The bar's fraction; `None` while the step reports no count (the bar then slides).
    pub fn fraction(&self) -> Option<f32> {
        self.last.as_ref().map(Progress::fraction)
    }

    /// Seconds since PLAY.
    pub fn elapsed(&self, now: f64) -> f64 {
        (now - self.launch_started).max(0.0)
    }

    /// Seconds left in the current run at its average pace so far; `None` until the pace is known.
    pub fn eta(&self, now: f64) -> Option<f64> {
        let last = self.last.as_ref()?;
        let (t0, done0) = self.origin?;
        eta_from(t0, done0, self.last_at, last.done, last.total, now)
    }
}

/// Time left for `total` items, from `done0` at `t0` to `done` at `t_last`, as seen at `now`.
/// `None` before `MIN_SAMPLE_SECS` of measuring or while nothing moved. The silence since the last
/// line is taken off the estimate, never below zero.
pub fn eta_from(t0: f64, done0: u64, t_last: f64, done: u64, total: u64, now: f64) -> Option<f64> {
    let span = t_last - t0;
    if span < MIN_SAMPLE_SECS || done <= done0 || total == 0 {
        return None;
    }
    let rate = (done - done0) as f64 / span;
    let left = total.saturating_sub(done) as f64 / rate;
    Some((left - (now - t_last).max(0.0)).max(0.0))
}

/// `42 s`, `3:07`, `1:02:09`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else if m > 0 {
        format!("{m}:{s:02}")
    } else {
        format!("{s} s")
    }
}

/// The line under the bar: elapsed time, and the time left once it is known.
pub fn timing_line(elapsed: f64, eta: Option<f64>) -> String {
    match eta {
        Some(left) => format!("{} elapsed, about {} left", clock(elapsed), clock(left)),
        None => format!("{} elapsed", clock(elapsed)),
    }
}

/// The count shown above the bar: `1,280 of 25,689 containers (4%)`.
pub fn count_line(progress: &Progress) -> String {
    format!(
        "{} of {} {} ({}%)",
        grouped(progress.done),
        grouped(progress.total),
        progress.what,
        progress.percent
    )
}

/// Digits in groups of three, for counts that run into the tens of thousands.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(file: &str, done: u64, total: u64) -> Progress {
        Progress {
            file: file.into(),
            done,
            total,
            percent: (done * 100).checked_div(total).unwrap_or(0),
            what: "containers".into(),
        }
    }

    #[test]
    fn step_labels_map_to_the_three_stages() {
        assert_eq!(Stage::of_step("Building VIctory"), Stage::Build);
        assert_eq!(Stage::of_step("Warming the shader cache"), Stage::Shaders);
        assert_eq!(Stage::of_step("Preloading shaders"), Stage::Shaders);
        assert_eq!(Stage::of_step("Running the game"), Stage::Start);
    }

    #[test]
    fn two_shader_steps_are_one_stage() {
        let labels: Vec<String> = [
            "Building VIctory",
            "Warming the shader cache",
            "Preloading shaders",
            "Running the game",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(stages_of(&labels), Stage::ALL.to_vec());
        assert_eq!(
            stages_of(&["Running the game".to_string()]),
            vec![Stage::Start]
        );
    }

    #[test]
    fn eta_waits_for_a_pace_then_extrapolates() {
        assert_eq!(eta_from(0.0, 0, 2.0, 50, 100, 2.0), None, "too early");
        assert_eq!(
            eta_from(0.0, 10, 10.0, 10, 100, 10.0),
            None,
            "nothing moved"
        );
        assert_eq!(eta_from(0.0, 0, 10.0, 10, 0, 10.0), None, "no total");
        // 40 items in 10 s: 4 per second, 60 left = 15 s.
        let eta = eta_from(0.0, 0, 10.0, 40, 100, 10.0).expect("eta");
        assert!((eta - 15.0).abs() < 1e-9);
    }

    #[test]
    fn silence_since_the_last_line_counts_down_but_never_below_zero() {
        let eta = eta_from(0.0, 0, 10.0, 40, 100, 14.0).expect("eta");
        assert!((eta - 11.0).abs() < 1e-9);
        assert_eq!(eta_from(0.0, 0, 10.0, 40, 100, 99.0), Some(0.0));
    }

    #[test]
    fn the_tracker_measures_one_run_at_a_time() {
        let mut t = Tracker::start(100.0);
        t.step("Warming the shader cache", 101.0);
        t.observe(&p("a.rpf", 0, 1000), 101.0);
        t.observe(&p("a.rpf", 500, 1000), 106.0);
        assert_eq!(t.fraction(), Some(0.5));
        let eta = t.eta(106.0).expect("pace known");
        assert!((eta - 5.0).abs() < 1e-9);
        // A new file starts a new run: no pace yet.
        t.observe(&p("b.rpf", 10, 400), 107.0);
        assert_eq!(t.eta(107.0), None);
        assert!((t.elapsed(107.0) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn a_new_step_drops_the_previous_progress() {
        let mut t = Tracker::start(0.0);
        t.step("Warming the shader cache", 0.0);
        t.observe(&p("a.rpf", 10, 10), 5.0);
        t.step("Running the game", 6.0);
        assert_eq!(t.stage(), Some(Stage::Start));
        assert_eq!(t.fraction(), None);
        assert_eq!(t.eta(7.0), None);
    }

    #[test]
    fn a_count_going_back_restarts_the_pace() {
        let mut t = Tracker::start(0.0);
        t.observe(&p("a.rpf", 0, 100), 0.0);
        t.observe(&p("a.rpf", 90, 100), 9.0);
        t.observe(&p("a.rpf", 5, 100), 10.0);
        assert_eq!(t.eta(10.0), None);
    }

    #[test]
    fn clocks_read_naturally() {
        assert_eq!(clock(0.4), "0 s");
        assert_eq!(clock(42.0), "42 s");
        assert_eq!(clock(187.0), "3:07");
        assert_eq!(clock(3729.0), "1:02:09");
        assert_eq!(clock(-5.0), "0 s");
    }

    #[test]
    fn timing_and_count_lines() {
        assert_eq!(timing_line(65.0, None), "1:05 elapsed");
        assert_eq!(
            timing_line(65.0, Some(30.0)),
            "1:05 elapsed, about 30 s left"
        );
        assert_eq!(
            count_line(&p("a", 1280, 25689)),
            "1,280 of 25,689 containers (4%)"
        );
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
