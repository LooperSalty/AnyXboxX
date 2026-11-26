//! The checks of the game folder as the window runs them: the quick check that gates PLAY, with a
//! timeout and a report of any failure, and the optional manifest job with progress and cancel.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use crate::app::{App, Verify};
use crate::manifest::{self, Entry, Outcome, Progress};
use crate::verify;

/// How long the quick check may take before it is reported as failed.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// The optional SHA-256 manifest.
#[derive(Debug, Clone)]
pub enum ManifestState {
    NotRun,
    Running(Arc<Progress>),
    Done(Vec<Entry>),
    Cancelled,
    Failed(String),
}

/// The text of a panic payload.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

impl App {
    /// Starts the quick folder check on a thread; a panic or a silence becomes a visible failure.
    pub fn start_verify(&mut self) {
        let Some(dir) = self.settings.game_path() else {
            self.notice = Some("Type the game folder first.".to_string());
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| verify::verify_folder(&dir))
                .map_err(|p| format!("the check crashed: {}", panic_text(&*p)))
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(result);
        });
        self.verify = Verify::Running;
        self.verify_rx = Some(rx);
        self.verify_started = Some(Instant::now());
    }

    pub(crate) fn poll_verify(&mut self) {
        let Some(rx) = &self.verify_rx else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                let late = self
                    .verify_started
                    .is_some_and(|t| t.elapsed() > VERIFY_TIMEOUT);
                if !late {
                    return;
                }
                Err(format!(
                    "no answer after {} s: the folder may be on a slow or disconnected drive",
                    VERIFY_TIMEOUT.as_secs()
                ))
            }
            Err(TryRecvError::Disconnected) => {
                Err("the check stopped without an answer".to_string())
            }
        };
        self.verify_rx = None;
        self.verify_started = None;
        self.verify = match result {
            Ok(report) => Verify::Done(report),
            Err(error) => {
                self.log_line(format!("!! Game folder check failed: {error}"));
                self.notice = Some(format!("Cannot check the game folder: {error}"));
                Verify::Failed(error)
            }
        };
        let launch = std::mem::take(&mut self.launch_when_ready);
        if launch && matches!(&self.verify, Verify::Done(report) if report.is_ok()) {
            self.start(false);
        }
    }

    /// Starts the SHA-256 manifest in the background; PLAY does not wait for it.
    pub fn start_manifest(&mut self) {
        let Some(dir) = self.settings.game_path() else {
            self.notice = Some("Type the game folder first.".to_string());
            return;
        };
        if matches!(self.manifest, ManifestState::Running(_)) {
            return;
        }
        let progress = Arc::new(Progress::new());
        let worker = Arc::clone(&progress);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let cache = manifest::current_cache_path();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                manifest::run(&dir, cache.as_deref(), &worker)
            }))
            .unwrap_or_else(|p| Outcome::Failed(format!("crashed: {}", panic_text(&*p))));
            let _ = tx.send(outcome);
        });
        self.manifest = ManifestState::Running(progress);
        self.manifest_rx = Some(rx);
    }

    /// Asks the manifest job to stop; it ends within a megabyte of reading.
    pub fn cancel_manifest(&mut self) {
        if let ManifestState::Running(progress) = &self.manifest {
            progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn poll_manifest(&mut self) {
        let Some(rx) = &self.manifest_rx else {
            return;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Outcome::Failed("the manifest job stopped without an answer".to_string())
            }
        };
        self.manifest_rx = None;
        self.manifest = match outcome {
            Outcome::Done(entries) => {
                self.notice = Some(format!("Manifest finished: {} files.", entries.len()));
                ManifestState::Done(entries)
            }
            Outcome::Cancelled => {
                self.notice = Some("Manifest cancelled.".to_string());
                ManifestState::Cancelled
            }
            Outcome::Failed(error) => {
                self.log_line(format!("!! Manifest failed: {error}"));
                self.notice = Some(format!("Could not finish the manifest: {error}"));
                ManifestState::Failed(error)
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_payloads_become_text() {
        let from_str = std::panic::catch_unwind(|| panic!("boom")).expect_err("panics");
        assert_eq!(panic_text(&*from_str), "boom");
        let from_string =
            std::panic::catch_unwind(|| panic!("{}", String::from("kaboom"))).expect_err("panics");
        assert_eq!(panic_text(&*from_string), "kaboom");
    }
}
