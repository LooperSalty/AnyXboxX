//! Runs a list of commands one after the other on a background thread, streaming their output
//! lines to the window. A step that exits non-zero ends the list; the exit code is reported.
//! A step with a log file (the game) is started detached instead: its output goes to that file
//! and the list ends with `Event::Detached` once the game is up.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use crate::detached;
use crate::progress::LineSplitter;

/// What the window receives from a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A step is starting.
    Started(String),
    /// One line of a step's output (stdout or stderr).
    Line(String),
    /// The list ended: exit code of the last step run (0 when all passed).
    Finished(i32),
    /// A step could not be started.
    Failed(String),
    /// The game is running on its own (process id, log file): the launcher may close.
    Detached { pid: u32, log: PathBuf },
}

/// One command of a job, with the name the window shows for it.
pub struct Step {
    pub label: String,
    pub command: Command,
    /// `Some(log)`: start detached, with the output written to `log` (see `detached`).
    pub detach_log: Option<PathBuf>,
    /// Runs after the command exits 0 (e.g. writes the warm marker); an error is shown as a line.
    pub after: Option<AfterSuccess>,
}

/// A hook run after a successful step.
pub type AfterSuccess = Box<dyn FnOnce() -> Result<(), String> + Send>;

impl Step {
    /// A step whose output is forwarded to the window.
    pub fn new(label: impl Into<String>, command: Command) -> Self {
        Step {
            label: label.into(),
            command,
            detach_log: None,
            after: None,
        }
    }

    /// The game: started detached, its output in `log`.
    pub fn detached(label: impl Into<String>, command: Command, log: PathBuf) -> Self {
        Step {
            detach_log: Some(log),
            ..Step::new(label, command)
        }
    }

    /// Adds a hook to run once the step succeeded.
    pub fn with_after(mut self, hook: AfterSuccess) -> Self {
        self.after = Some(hook);
        self
    }
}

/// Starts the steps on a thread; events go to `tx`. Returns the thread to join when wanted.
pub fn spawn(steps: Vec<Step>, tx: Sender<Event>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let code = run_steps(steps, &tx);
        // The window may have closed: nothing to report to then.
        let _ = tx.send(code);
    })
}

fn run_steps(steps: Vec<Step>, tx: &Sender<Event>) -> Event {
    for step in steps {
        let _ = tx.send(Event::Started(step.label.clone()));
        let outcome = match step.detach_log {
            Some(log) => run_detached(step.command, log, tx),
            None => run_one(step.command, tx)
                .map(Event::Finished)
                .map_err(|e| e.to_string()),
        };
        match outcome {
            Ok(Event::Finished(0)) => {
                if let Some(hook) = step.after
                    && let Err(error) = hook()
                {
                    let _ = tx.send(Event::Line(format!("Warning: {error}")));
                }
            }
            Ok(event) => return event,
            Err(error) => return Event::Failed(format!("{}: {error}", step.label)),
        }
    }
    Event::Finished(0)
}

/// Starts the game detached and waits for it to be up. Its output goes to `log`, which the
/// window shows; a game that ends at once reports its exit code.
fn run_detached(command: Command, log: PathBuf, tx: &Sender<Event>) -> Result<Event, String> {
    let _ = tx.send(Event::Line(format!("Game log: {}", log.display())));
    let mut child = detached::spawn_game(command, &log).map_err(|e| e.to_string())?;
    match detached::wait_until_up(&mut child, detached::UP_WAIT).map_err(|e| e.to_string())? {
        Ok(()) => Ok(Event::Detached {
            pid: child.id(),
            log,
        }),
        Err(code) => {
            let _ = tx.send(Event::Line(format!(
                "The game ended at once with exit code {code}."
            )));
            Ok(Event::Finished(code))
        }
    }
}

/// Runs one command, forwarding both of its streams line by line; returns its exit code.
fn run_one(mut command: Command, tx: &Sender<Event>) -> std::io::Result<i32> {
    let mut child = crate::noconsole::hide_console(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let readers: Vec<JoinHandle<()>> = [
        child
            .stdout
            .take()
            .map(|s| forward(Box::new(s), tx.clone())),
        child
            .stderr
            .take()
            .map(|s| forward(Box::new(s), tx.clone())),
    ]
    .into_iter()
    .flatten()
    .collect();
    let status = child.wait()?;
    for reader in readers {
        let _ = reader.join();
    }
    // A process killed without an exit code counts as failed, as the PowerShell launcher did.
    Ok(status.code().unwrap_or(1))
}

fn forward(mut stream: Box<dyn Read + Send>, tx: Sender<Event>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut splitter = LineSplitter::default();
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            for line in splitter.push(&buffer[..read]) {
                let _ = tx.send(Event::Line(line));
            }
        }
        if let Some(line) = splitter.finish() {
            let _ = tx.send(Event::Line(line));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn collect(steps: Vec<Step>) -> Vec<Event> {
        let (tx, rx) = mpsc::channel();
        spawn(steps, tx).join().expect("job thread");
        rx.try_iter().collect()
    }

    #[cfg(windows)]
    fn shell(line: &str) -> Command {
        let mut command = Command::new("cmd");
        command.args(["/C", line]);
        command
    }

    #[cfg(windows)]
    #[test]
    fn steps_run_in_order_and_their_output_is_forwarded() {
        let events = collect(vec![
            Step::new("one", shell("echo first")),
            Step::new("two", shell("echo second")),
        ]);
        assert_eq!(events.first(), Some(&Event::Started("one".into())));
        assert!(events.contains(&Event::Line("first".into())));
        assert!(events.contains(&Event::Started("two".into())));
        assert_eq!(events.last(), Some(&Event::Finished(0)));
    }

    #[cfg(windows)]
    #[test]
    fn a_failing_step_stops_the_list_with_its_exit_code() {
        let events = collect(vec![
            Step::new("bad", shell("exit 3")),
            Step::new("never", shell("echo nope")),
        ]);
        assert_eq!(events.last(), Some(&Event::Finished(3)));
        assert!(!events.iter().any(|e| *e == Event::Started("never".into())));
    }

    #[test]
    fn a_missing_program_is_reported_as_failed() {
        let events = collect(vec![Step::new(
            "ghost",
            Command::new("anyxboxx-no-such-program-xyz"),
        )]);
        assert!(matches!(events.last(), Some(Event::Failed(m)) if m.starts_with("ghost:")));
    }

    #[test]
    fn a_detached_game_that_ends_at_once_reports_its_exit_code_and_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("game.log");
        let mut command = Command::new(std::env::current_exe().expect("exe"));
        command.arg("--definitely-not-a-test-name-xyz");
        let events = collect(vec![Step::detached("game", command, log.clone())]);
        assert!(events.contains(&Event::Line(format!("Game log: {}", log.display()))));
        assert!(matches!(events.last(), Some(Event::Finished(code)) if *code != 0));
    }
}
