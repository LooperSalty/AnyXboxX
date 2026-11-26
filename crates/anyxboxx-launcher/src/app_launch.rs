//! The launch side of the window state: the outcome card of the last job, cancelling a job, the
//! log as text for "Copy log", the events of the running job, and the command line (`--page`,
//! `--victory-exe`, `--preview-launch`).

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::app::{App, Tab};
use crate::job::Event;

/// The outcome of the last job, shown as a card on the Home and Play pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Banner {
    Success(String),
    Error(String),
}

impl Banner {
    /// The card for a job that ended with `code`.
    pub fn of_exit(code: i32, message: &str) -> Banner {
        if code == 0 {
            Banner::Success(message.to_string())
        } else {
            Banner::Error(message.to_string())
        }
    }
}

/// What the command line asks for: the first page, a prebuilt `victory.exe`, and the launch screen
/// preview.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    pub page: Option<Tab>,
    pub victory_exe: Option<PathBuf>,
    pub preview_launch: bool,
}

/// The page named on the command line (`home`, `setup`, `folder`, `shaders`, `play`).
pub fn tab_named(name: &str) -> Option<Tab> {
    match name.to_ascii_lowercase().as_str() {
        "home" => Some(Tab::Home),
        "setup" => Some(Tab::Setup),
        "folder" | "files" | "game-folder" => Some(Tab::Files),
        "shaders" => Some(Tab::Shaders),
        "play" => Some(Tab::Play),
        _ => None,
    }
}

fn non_empty_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"');
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Reads `--page <name>`, `--victory-exe <path>` (or `--victory-exe=<path>`), and
/// `--preview-launch`; anything else is ignored. The preview is only honoured by debug builds
/// (see `App::apply_args`).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Args {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--victory-exe=") {
            out.victory_exe = non_empty_path(value);
            continue;
        }
        match arg.as_str() {
            "--page" => out.page = args.next().as_deref().and_then(tab_named),
            "--victory-exe" => {
                out.victory_exe = args.next().as_deref().and_then(non_empty_path);
            }
            "--preview-launch" => out.preview_launch = true,
            _ => {}
        }
    }
    out
}

impl App {
    /// Seconds since the window opened (the clock of the launch screen).
    pub fn now_secs(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// Applies the command line: the first page, a prebuilt `victory.exe`, and the preview in
    /// debug builds.
    pub fn apply_args(&mut self, args: Args) {
        if let Some(page) = args.page {
            self.tab = page;
            self.shown_tab = page;
        }
        if let Some(exe) = args.victory_exe {
            self.victory_exe = Some(exe);
        }
        self.preview_launch = cfg!(debug_assertions) && args.preview_launch;
    }

    /// Asks the running job to stop; the job thread kills its command and reports `Cancelled`.
    pub fn cancel_job(&mut self) {
        if let Some(running) = &self.running {
            running.cancel.store(true, Ordering::Relaxed);
            self.log_line("== cancel requested ==".to_string());
        }
        self.preview_launch = false;
    }

    /// The whole log as text, for the clipboard.
    pub fn log_text(&self) -> String {
        self.log.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

impl App {
    /// Takes the events of the running job: its lines, its steps and how it ended.
    pub(crate) fn poll_job(&mut self) {
        let Some(running) = &self.running else {
            return;
        };
        let shaders_only = running.shaders_only;
        let events: Vec<Event> = running.events.try_iter().collect();
        let mut outcome = None;
        for event in events {
            match &event {
                Event::Finished(code) => {
                    let message = finish_message(*code, shaders_only);
                    self.banner = Some(Banner::of_exit(*code, &message));
                    outcome = Some(message);
                }
                Event::Failed(error) => {
                    let message = format!("Could not run: {error}");
                    self.banner = Some(Banner::Error(message.clone()));
                    outcome = Some(message);
                }
                Event::Cancelled => outcome = Some("Launch cancelled.".to_string()),
                Event::Detached { pid, log } => {
                    outcome = Some(format!(
                        "VIctory is running (process {pid}). Its log: {}",
                        log.display()
                    ));
                    self.close_requested = !self.settings.keep_open_after_launch;
                }
                Event::Started(_) | Event::Line(_) => {}
            }
            self.record(event);
        }
        if let Some(message) = outcome {
            self.running = None;
            self.step = None;
            self.notice = Some(message);
        }
    }

    fn record(&mut self, event: Event) {
        let now = self.now_secs();
        let line = match event {
            Event::Started(label) => {
                let line = format!("== {label} ==");
                self.tracker.step(&label, now);
                self.step = Some(label);
                line
            }
            Event::Line(line) => {
                self.summary = self.summary.with_line(&line);
                if let Some(progress) = crate::progress::parse_progress(&line) {
                    self.tracker.observe(&progress, now);
                }
                line
            }
            Event::Cancelled => "== cancelled ==".to_string(),
            Event::Finished(code) => format!("== finished, exit code {code} =="),
            Event::Failed(error) => format!("!! {error}"),
            Event::Detached { pid, log } => format!(
                "== VIctory is running (process {pid}), log: {} ==",
                log.display()
            ),
        };
        self.log_line(line);
    }
}

fn finish_message(code: i32, shaders_only: bool) -> String {
    match (code, shaders_only) {
        (0, true) => "Shader preloading finished.".to_string(),
        (0, false) => "VIctory exited normally.".to_string(),
        (code, _) => format!("VIctory stopped with exit code {code}: see the log."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        parse_args(list.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn the_command_line_names_the_first_page() {
        assert_eq!(args(&["--page", "shaders"]).page, Some(Tab::Shaders));
        assert_eq!(args(&["--page", "Folder"]).page, Some(Tab::Files));
        assert_eq!(args(&["--page", "nowhere"]).page, None);
        assert_eq!(args(&["--page"]).page, None);
        assert_eq!(args(&[]), Args::default());
    }

    #[test]
    fn the_preview_flag_is_read() {
        let a = args(&["--preview-launch", "--page", "play"]);
        assert!(a.preview_launch);
        assert_eq!(a.page, Some(Tab::Play));
    }

    #[test]
    fn the_victory_exe_flag_reads_separate_and_equals_forms() {
        let separate = args(&[
            "--victory-exe",
            "Z:\\prebuilt\\victory.exe",
            "--page",
            "play",
        ]);
        assert_eq!(
            separate.victory_exe,
            Some(PathBuf::from("Z:\\prebuilt\\victory.exe"))
        );
        assert_eq!(separate.page, Some(Tab::Play));

        let equals = args(&["--victory-exe=\"C:\\out\\victory.exe\""]);
        assert_eq!(
            equals.victory_exe,
            Some(PathBuf::from("C:\\out\\victory.exe"))
        );

        assert_eq!(args(&["--victory-exe"]).victory_exe, None);
        assert_eq!(args(&["--victory-exe", "   "]).victory_exe, None);
        assert_eq!(args(&["--victory-exe="]).victory_exe, None);
    }

    #[test]
    fn finish_messages_say_what_ended() {
        assert_eq!(finish_message(0, true), "Shader preloading finished.");
        assert_eq!(finish_message(0, false), "VIctory exited normally.");
        assert!(finish_message(2, false).contains("exit code 2"));
    }

    #[test]
    fn a_non_zero_exit_is_an_error_card() {
        assert_eq!(Banner::of_exit(0, "ok"), Banner::Success("ok".into()));
        assert_eq!(Banner::of_exit(3, "bad"), Banner::Error("bad".into()));
    }
}
