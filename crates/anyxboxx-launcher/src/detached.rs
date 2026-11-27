//! Starting the game so that it outlives the launcher: a detached process (no console, its own
//! process group), its output written to a log file under `%LOCALAPPDATA%\AnyXboxX\logs` instead
//! of pipes the launcher would have to hold. The launcher then waits briefly and may close.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `DETACHED_PROCESS`: the child gets no console of its own and does not share the launcher's.
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
/// `CREATE_NEW_PROCESS_GROUP`: a console Ctrl+C of the launcher does not reach the game.
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// `CREATE_NO_WINDOW`: a console child (victory.exe, cargo) opens no window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long the launcher waits for the game to be running before it lets go of it.
pub const UP_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(200);

/// The creation flags of the detached game: the three constants above, together.
pub fn spawn_flags() -> u32 {
    DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
}

/// `%LOCALAPPDATA%\AnyXboxX\logs`, `None` when LOCALAPPDATA is not set or empty.
pub fn log_dir(local_app_data: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    local_app_data
        .filter(|v| !v.is_empty())
        .map(|local| PathBuf::from(local).join("AnyXboxX").join("logs"))
}

/// `game-<unix seconds>.log` in `dir`: one file per launch.
pub fn game_log_path(dir: &Path, unix_seconds: u64) -> PathBuf {
    dir.join(format!("game-{unix_seconds}.log"))
}

/// Starts `command` detached, with stdin closed and both output streams written to `log`
/// (created, or truncated, along with its folder).
pub fn spawn_game(mut command: Command, log: &Path) -> io::Result<Child> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::create(log)?;
    let errors = file.try_clone()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors));
    apply_flags(&mut command);
    command.spawn()
}

#[cfg(windows)]
fn apply_flags(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(spawn_flags());
}

#[cfg(not(windows))]
fn apply_flags(_command: &mut Command) {}

/// Waits up to `wait` for the child to be running. `Err` with its exit code when it already
/// ended (the game died at once: the log says why). `Ok` when it is still alive at the end.
pub fn wait_until_up(child: &mut Child, wait: Duration) -> io::Result<Result<(), i32>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Err(status.code().unwrap_or(1)));
        }
        let elapsed = started.elapsed();
        if elapsed >= wait {
            return Ok(Ok(()));
        }
        std::thread::sleep(POLL.min(wait.saturating_sub(elapsed)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spawn_flags_are_the_three_documented_constants() {
        assert_eq!(spawn_flags(), 0x0000_0008 | 0x0000_0200 | 0x0800_0000);
        assert_eq!(DETACHED_PROCESS, 0x8);
        assert_eq!(CREATE_NEW_PROCESS_GROUP, 0x200);
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[test]
    fn logs_live_under_appdata_anyxboxx_logs() {
        let dir = log_dir(Some(std::ffi::OsStr::new(r"C:\Users\me\AppData\Local"))).expect("dir");
        assert!(dir.ends_with(Path::new("AnyXboxX").join("logs")));
        assert!(log_dir(None).is_none());
        assert!(log_dir(Some(std::ffi::OsStr::new(""))).is_none());
        let log = game_log_path(&dir, 1_795_654_414);
        assert_eq!(
            log.file_name().and_then(|n| n.to_str()),
            Some("game-1795654414.log")
        );
    }

    #[test]
    fn a_child_that_exits_at_once_is_reported_with_its_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("logs").join("game-test.log");
        let mut command = Command::new(std::env::current_exe().expect("exe"));
        command.arg("--definitely-not-a-test-name-xyz");
        let mut child = spawn_game(command, &log).expect("spawn");
        let outcome = wait_until_up(&mut child, Duration::from_secs(5)).expect("wait");
        assert!(
            outcome.is_err(),
            "an unknown option ends the test binary at once"
        );
        assert!(
            log.is_file(),
            "the log file is created before the child starts"
        );
    }

    #[test]
    fn a_running_child_is_up_after_the_wait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("game.log");
        let mut command = Command::new(std::env::current_exe().expect("exe"));
        command.args(["--ignored", "--exact", "detached::tests::sleeper"]);
        let mut child = spawn_game(command, &log).expect("spawn");
        let outcome = wait_until_up(&mut child, Duration::from_millis(400)).expect("wait");
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(outcome, Ok(()));
    }

    /// Helper of the test above: a test that waits long enough to still be running. Run only by name.
    #[test]
    #[ignore]
    fn sleeper() {
        std::thread::sleep(Duration::from_secs(30));
    }
}
