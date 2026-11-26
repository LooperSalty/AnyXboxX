//! Runs a list of commands one after the other on a background thread, streaming their output
//! lines to the window. A step that exits non-zero ends the list; the exit code is reported.
//! A step with a log file (the game) is started detached instead: its output goes to that file
//! and the list ends with `Event::Detached` once the game is up. A job can be cancelled: the
//! running command is killed and the list ends with `Event::Cancelled`.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

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
    /// The user cancelled: the running command was stopped and no further step ran.
    Cancelled,
}

/// How often a running command is looked at for its end or a cancel.
const WAIT_POLL: Duration = Duration::from_millis(100);

/// One command of a job, with the name the window shows for it.
pub struct Step {
    pub label: String,
    pub command: Command,
    /// `Some(log)`: start detached, with the output written to `log` (see `detached`).
    pub detach_log: Option<PathBuf>,
    /// Runs after the command exits 0 (e.g. writes the warm marker); an error is shown as a line.
    pub after: Option<AfterSuccess>,
    /// Asked just before the step starts; `false` skips it (e.g. a warm decided after a build).
    pub run_if: Option<RunIf>,
}

/// A hook run after a successful step.
pub type AfterSuccess = Box<dyn FnOnce() -> Result<(), String> + Send>;

/// A check made when the step is reached: `false` skips the step.
pub type RunIf = Box<dyn FnOnce() -> bool + Send>;

impl Step {
    /// A step whose output is forwarded to the window.
    pub fn new(label: impl Into<String>, command: Command) -> Self {
        Step {
            label: label.into(),
            command,
            detach_log: None,
            after: None,
            run_if: None,
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

    /// Runs the step only if `check` says so when the step is reached.
    pub fn with_run_if(mut self, check: RunIf) -> Self {
        self.run_if = Some(check);
        self
    }
}

/// Starts the steps on a thread; events go to `tx`. Returns the thread to join when wanted.
#[cfg(test)]
pub fn spawn(steps: Vec<Step>, tx: Sender<Event>) -> JoinHandle<()> {
    spawn_cancellable(steps, tx, Arc::new(AtomicBool::new(false)))
}

/// As `spawn`, stopping when `cancel` turns true: the running command is killed, the steps after
/// it never start. A game already started detached is left running (it belongs to the player).
pub fn spawn_cancellable(
    steps: Vec<Step>,
    tx: Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let code = run_steps(steps, &tx, &cancel);
        // The window may have closed: nothing to report to then.
        let _ = tx.send(code);
    })
}

fn run_steps(steps: Vec<Step>, tx: &Sender<Event>, cancel: &AtomicBool) -> Event {
    for step in steps {
        if cancel.load(Ordering::Relaxed) {
            return Event::Cancelled;
        }
        let mut step = step;
        if let Some(check) = step.run_if.take()
            && !check()
        {
            let _ = tx.send(Event::Line(format!(
                "{}: skipped, nothing changed",
                step.label
            )));
            continue;
        }
        let _ = tx.send(Event::Started(step.label.clone()));
        let outcome = match step.detach_log {
            Some(log) => run_detached(step.command, log, tx),
            None => match run_one(step.command, tx, cancel) {
                Ok(Some(code)) => Ok(Event::Finished(code)),
                Ok(None) => return Event::Cancelled,
                Err(error) => Err(error.to_string()),
            },
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

/// Runs one command, forwarding both of its streams line by line; returns its exit code, or
/// `None` when it was cancelled (killed; its readers are left to end with the pipes, since a
/// grandchild may still hold them).
fn run_one(
    mut command: Command,
    tx: &Sender<Event>,
    cancel: &AtomicBool,
) -> std::io::Result<Option<i32>> {
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
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(WAIT_POLL);
    };
    for reader in readers {
        let _ = reader.join();
    }
    // A process killed without an exit code counts as failed, as the PowerShell launcher did.
    Ok(Some(status.code().unwrap_or(1)))
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
    fn a_step_whose_check_says_no_is_skipped_and_the_next_one_runs() {
        let events = collect(vec![
            Step::new("skipped", shell("echo never")).with_run_if(Box::new(|| false)),
            Step::new("kept", shell("echo yes")).with_run_if(Box::new(|| true)),
        ]);
        assert!(!events.contains(&Event::Started("skipped".into())));
        assert!(events.contains(&Event::Line("skipped: skipped, nothing changed".into())));
        assert!(events.contains(&Event::Line("yes".into())));
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

    #[cfg(windows)]
    #[test]
    fn a_cancelled_job_kills_its_step_and_runs_nothing_after() {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut slow = Command::new(std::env::current_exe().expect("exe"));
        slow.args(["--ignored", "--exact", "detached::tests::sleeper"]);
        let handle = spawn_cancellable(
            vec![
                Step::new("slow", slow),
                Step::new("never", shell("echo nope")),
            ],
            tx,
            Arc::clone(&cancel),
        );
        std::thread::sleep(Duration::from_millis(300));
        cancel.store(true, Ordering::Relaxed);
        let started = std::time::Instant::now();
        handle.join().expect("job thread");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stopped quickly"
        );
        let events: Vec<Event> = rx.try_iter().collect();
        assert_eq!(events.last(), Some(&Event::Cancelled));
        assert!(!events.contains(&Event::Started("never".into())));
    }

    #[test]
    fn a_job_cancelled_before_it_starts_runs_nothing() {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(true));
        spawn_cancellable(
            vec![Step::new(
                "ghost",
                Command::new("anyxboxx-no-such-program-xyz"),
            )],
            tx,
            cancel,
        )
        .join()
        .expect("job thread");
        let events: Vec<Event> = rx.try_iter().collect();
        assert_eq!(events, vec![Event::Cancelled]);
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
