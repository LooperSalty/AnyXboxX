//! Watches the Windows System event log for display-driver events that may
//! precede a GPU timeout or device removal. The first match disables the
//! indirect clamp windows for the rest of the process.
//!
//! Off by default. Set `VICTORY_DRIVER_WATCH=1` (`true` and `on` also work)
//! to subscribe to future events only.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows_sys::Win32::System::EventLog::{
    EVT_HANDLE, EVT_SUBSCRIBE_CALLBACK, EVT_SUBSCRIBE_NOTIFY_ACTION, EvtClose, EvtSubscribe,
    EvtSubscribeActionDeliver, EvtSubscribeToFutureEvents,
};

struct Watcher {
    subscription: OnceLock<Option<Subscription>>,
    errors: AtomicU64,
}

impl Watcher {
    const fn new() -> Self {
        Self {
            subscription: OnceLock::new(),
            errors: AtomicU64::new(0),
        }
    }

    fn start(&self, value: Option<&str>, subscribe: impl FnOnce() -> Option<Subscription>) {
        self.subscription.get_or_init(|| {
            if matches!(value, Some("1" | "true" | "on")) {
                subscribe()
            } else {
                None
            }
        });
    }

    fn tripped(&self) -> bool {
        self.errors() != 0
    }

    fn errors(&self) -> u64 {
        self.errors.load(Relaxed)
    }

    fn seen(&self) -> u64 {
        self.errors.fetch_add(1, Relaxed) + 1
    }
}

// A successful subscription stays alive until process exit.
static WATCHER: Watcher = Watcher::new();

/// Whether any matching display-driver event has been delivered.
pub(crate) fn tripped() -> bool {
    WATCHER.tripped()
}

/// Matching display-driver events seen since the watcher started.
pub(crate) fn errors() -> u64 {
    WATCHER.errors()
}

unsafe extern "system" fn on_event(
    action: EVT_SUBSCRIBE_NOTIFY_ACTION,
    _context: *const core::ffi::c_void,
    _event: EVT_HANDLE,
) -> u32 {
    if action == EvtSubscribeActionDeliver {
        let count = WATCHER.seen();
        tracing::warn!(
            count,
            "display-driver event recorded in the Windows System log{}",
            if count == 1 {
                "; indirect clamp windows are disabled for the rest of this run"
            } else {
                ""
            }
        );
    }
    0
}

// Include the display drivers and the system's Display source (e.g. event
// 4101, driver stopped responding). Keep only levels 1–3: on Windows 11,
// Display also logs informational event 4125 when Auto HDR enhances a game's
// window. That is not a driver fault and must not trip the watcher.
const QUERY: &str = "*[System[Provider[@Name='nvlddmkm' or @Name='amdkmdag' or @Name='amdwddmg' or @Name='igfx' \
                     or @Name='igfxn' or @Name='Display'] and (Level=1 or Level=2 or Level=3)]]";

struct Subscription(EVT_HANDLE);

impl Drop for Subscription {
    fn drop(&mut self) {
        // SAFETY: this is the live subscription handle owned by self.
        unsafe { EvtClose(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn subscribe(
    channel: &str,
    query: &str,
    callback: EVT_SUBSCRIBE_CALLBACK,
) -> std::io::Result<Subscription> {
    let (channel, query) = (wide(channel), wide(query));
    // SAFETY: both strings are NUL-terminated and live for the call. The
    // callback may outlive the call, but receives no user context.
    let handle = unsafe {
        EvtSubscribe(
            0,
            std::ptr::null_mut(),
            channel.as_ptr(),
            query.as_ptr(),
            0,
            std::ptr::null(),
            callback,
            EvtSubscribeToFutureEvents,
        )
    };
    if handle == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(Subscription(handle))
    }
}

/// Starts the process-wide watcher once, and only when explicitly enabled.
pub(crate) fn start() {
    let value = std::env::var("VICTORY_DRIVER_WATCH").ok();
    WATCHER.start(value.as_deref(), || match subscribe("System", QUERY, Some(on_event)) {
        Ok(subscription) => {
            tracing::info!("watching the Windows System log for display-driver events");
            Some(subscription)
        }
        Err(error) => {
            tracing::warn!(%error, "could not subscribe to display-driver events in the Windows System log");
            None
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_off_leaves_watcher_inert() {
        for value in [
            None,
            Some("0"),
            Some(""),
            Some("false"),
            Some("yes"),
            Some("TRUE"),
            Some(" 1 "),
        ] {
            let watcher = Watcher::new();
            watcher.start(value, || panic!("{value:?} must not subscribe"));
            assert!(!watcher.tripped());
            assert_eq!(watcher.errors(), 0);
        }
    }

    #[test]
    fn opt_in_starts_once() {
        for value in ["1", "true", "on"] {
            let watcher = Watcher::new();
            let mut attempts = 0;
            watcher.start(Some(value), || {
                attempts += 1;
                None
            });
            watcher.start(Some(value), || panic!("must not subscribe twice"));
            assert_eq!(attempts, 1, "{value:?}");
        }
    }

    #[test]
    fn the_first_error_trips_the_switch_and_every_one_is_counted() {
        let watcher = Watcher::new();
        assert!(!watcher.tripped());
        assert_eq!(watcher.seen(), 1);
        assert!(watcher.tripped());
        assert_eq!(watcher.seen(), 2);
        assert_eq!(watcher.errors(), 2);
    }

    /// End to end: leaves one informational entry in the Application log.
    #[test]
    #[ignore = "writes one entry to the Application event log: run it by name with --ignored"]
    fn an_entry_written_reaches_a_subscription() {
        use windows_sys::Win32::System::EventLog::{
            DeregisterEventSource, EVENTLOG_INFORMATION_TYPE, RegisterEventSourceW, ReportEventW,
        };

        static GOT: AtomicU64 = AtomicU64::new(0);
        unsafe extern "system" fn received(
            action: EVT_SUBSCRIBE_NOTIFY_ACTION,
            _: *const core::ffi::c_void,
            _: EVT_HANDLE,
        ) -> u32 {
            if action == EvtSubscribeActionDeliver {
                GOT.fetch_add(1, Relaxed);
            }
            0
        }

        let _subscription = subscribe(
            "Application",
            "*[System[Provider[@Name='display-driver-watch-test']]]",
            Some(received),
        )
        .expect("subscribe to the Application log");
        let name = wide("display-driver-watch-test");
        let text = wide("Display-driver watcher subscription test");
        // SAFETY: all strings live for the calls. The valid source handle
        // is deregistered even if ReportEventW fails.
        let report_error = unsafe {
            let source = RegisterEventSourceW(std::ptr::null(), name.as_ptr());
            assert!(!source.is_null(), "{}", std::io::Error::last_os_error());
            let strings = [text.as_ptr()];
            let reported = ReportEventW(
                source,
                EVENTLOG_INFORMATION_TYPE,
                0,
                1,
                std::ptr::null_mut(),
                1,
                0,
                strings.as_ptr(),
                std::ptr::null(),
            );
            let report_error = (reported == 0).then(std::io::Error::last_os_error);
            assert_ne!(
                DeregisterEventSource(source),
                0,
                "{}",
                std::io::Error::last_os_error()
            );
            report_error
        };
        if let Some(error) = report_error {
            panic!("could not write the test event: {error}");
        }

        let started = std::time::Instant::now();
        while GOT.load(Relaxed) == 0 && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(GOT.load(Relaxed), 1, "the entry was not delivered in 5 s");
    }

    #[test]
    fn the_system_takes_the_subscription() {
        // Do not touch process-wide watcher state, even if a driver event is delivered.
        unsafe extern "system" fn ignore_event(
            _: EVT_SUBSCRIBE_NOTIFY_ACTION,
            _: *const core::ffi::c_void,
            _: EVT_HANDLE,
        ) -> u32 {
            0
        }
        let _subscription =
            subscribe("System", QUERY, Some(ignore_event)).expect("subscribe to the System log");
    }
}
