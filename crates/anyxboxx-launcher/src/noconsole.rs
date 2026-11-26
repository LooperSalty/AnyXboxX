//! Keeps child processes from opening a console window. The launcher is a Windows-subsystem
//! program (no console of its own), and cargo, VIctory, `tasklist` and `rustc` would each flash
//! one; their output is captured into the log drawer instead. The game's own window is not a
//! console and stays visible.

use std::process::Command;

/// `CREATE_NO_WINDOW` of `CreateProcess`.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Marks `command` to start without a console window (no effect off Windows).
pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_create_no_window() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[cfg(windows)]
    #[test]
    fn a_hidden_child_still_runs_and_reports() {
        let mut command = Command::new("cmd");
        command.args(["/C", "echo hidden"]);
        let output = hide_console(&mut command).output().expect("runs");
        assert!(String::from_utf8_lossy(&output.stdout).contains("hidden"));
    }
}
