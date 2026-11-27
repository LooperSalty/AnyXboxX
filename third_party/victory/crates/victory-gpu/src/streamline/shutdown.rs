//! The CLI owns its process. Keep its diagnostic output and error status if
//! the driver hangs unloading NGX after all GPU work has completed.
use super::Sl;
use std::sync::mpsc;
use std::time::Duration;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn shut_down(sl: &Sl, standalone_device_check: bool) -> Result<i32, String> {
    let (finished, receiver) = mpsc::channel();
    let guard = if standalone_device_check {
        Some(std::thread::Builder::new().name("Streamline shutdown monitor".into())
            .spawn(move || {
                if timed_out(receiver, SHUTDOWN_TIMEOUT) {
                    eprintln!("Error: slShutdown did not return within 30 seconds; terminating the standalone upscaler-check");
                    // ExitProcess can itself wait on the loader lock in the stuck
                    // driver. Only this standalone diagnostic process is stopped.
                    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                    unsafe { TerminateProcess(GetCurrentProcess(), 1) };
                }
            }).map_err(|e| format!("cannot monitor slShutdown: {e}"))?)
    } else {
        None
    };
    // SAFETY: the check has retired every present and releases its swap chain
    // first. Shutdown runs on the same thread as init.
    let code = unsafe { (sl.shutdown)() };
    drop(finished);
    if let Some(guard) = guard {
        guard.join().map_err(|_| "Streamline shutdown monitor failed".to_string())?;
    }
    Ok(code)
}

fn timed_out(receiver: mpsc::Receiver<()>, limit: Duration) -> bool {
    matches!(receiver.recv_timeout(limit), Err(mpsc::RecvTimeoutError::Timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completed_shutdown_disarms_the_monitor() {
        let (finished, receiver) = mpsc::channel();
        drop(finished);
        assert!(!timed_out(receiver, Duration::ZERO));
        let (_still_shutting_down, receiver) = mpsc::channel();
        assert!(timed_out(receiver, Duration::ZERO));
    }
}
