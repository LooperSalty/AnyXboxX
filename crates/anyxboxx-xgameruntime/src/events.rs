//! Offline subscriptions (invites, activations, protocols) and the XError interface (C++
//! `event_interface.cpp`). Offline no invite or activation arrives: a subscription succeeds with a
//! fresh token and its callback never runs. Accepting an invite stays E_NOTIMPL (the default).

use std::collections::HashSet;
use std::ffi::{c_char, c_void};
use std::sync::Mutex;

use crate::object::{E_INVALIDARG, HResult, S_OK};

/// Registration tokens already handed out, and those still live.
#[derive(Default)]
struct Subscriptions {
    last: u64,
    live: HashSet<u64>,
}

static SUBSCRIPTIONS: Mutex<Option<Subscriptions>> = Mutex::new(None);

/// Runs `action` on the subscriptions, which start empty.
fn with_subscriptions<T>(action: impl FnOnce(&mut Subscriptions) -> T) -> T {
    let mut guard = SUBSCRIPTIONS.lock().unwrap_or_else(|e| e.into_inner());
    action(guard.get_or_insert_with(Subscriptions::default))
}

/// Adds a subscription: a new token, written to `token`, unless the callback or the token is null.
pub fn add_subscription(callback: *const c_void, token: *mut u64) -> HResult {
    if callback.is_null() || token.is_null() {
        return E_INVALIDARG;
    }
    let next = with_subscriptions(|subscriptions| {
        subscriptions.last += 1;
        subscriptions.live.insert(subscriptions.last);
        subscriptions.last
    });
    // SAFETY: non-null and writable, per the contract of the caller (the title's token).
    unsafe { token.write(next) };
    S_OK
}

/// Removes a subscription; false when it is not live (already removed, or never added).
pub fn remove_subscription(token: u64) -> bool {
    with_subscriptions(|subscriptions| subscriptions.live.remove(&token))
}

/// Subscribe methods of the invite, activation and protocol objects: (this, queue, context,
/// callback, token).
pub(crate) unsafe extern "system" fn subscribe(
    _this: *mut c_void,
    _queue: *mut c_void,
    _context: *mut c_void,
    callback: *const c_void,
    token: *mut u64,
) -> HResult {
    add_subscription(callback, token)
}

/// Unsubscribe methods: (this, token, wait).
pub(crate) unsafe extern "system" fn unsubscribe(
    _this: *mut c_void,
    token: u64,
    _wait: bool,
) -> bool {
    remove_subscription(token)
}

/// The title's error callback: (hr, message, context) → handled.
type ErrorCallback = unsafe extern "system" fn(HResult, *const c_char, *mut c_void) -> bool;

struct ErrorState {
    callback: Option<ErrorCallback>,
    context: *mut c_void,
}

// SAFETY: the context is an opaque value the title gave us; we only pass it back to its callback.
unsafe impl Send for ErrorState {}

static ERROR_STATE: Mutex<ErrorState> = Mutex::new(ErrorState {
    callback: None,
    context: std::ptr::null_mut(),
});

/// XErrorReport (error slot 3): logged, then the title's callback if it set one.
unsafe extern "system" fn error_report(
    _this: *mut c_void,
    hr: HResult,
    message: *const c_char,
) -> HResult {
    let (callback, context) = {
        let state = ERROR_STATE.lock().unwrap_or_else(|e| e.into_inner());
        (state.callback, state.context)
    };
    let text = if message.is_null() {
        String::new()
    } else {
        // SAFETY: the title passes a NUL-terminated string.
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    crate::log::write(format_args!(
        "[runtime] XErrorReport 0x{:08X} : {text}",
        hr as u32
    ));
    if let Some(callback) = callback {
        // SAFETY: the title's own callback, with the arguments it expects.
        unsafe { callback(hr, message, context) };
    }
    S_OK
}

/// XErrorSetCallback (error slot 4): replaces the callback and its context.
unsafe extern "system" fn error_set_callback(
    _this: *mut c_void,
    callback: Option<ErrorCallback>,
    context: *mut c_void,
) {
    let mut state = ERROR_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.callback = callback;
    state.context = context;
}

/// XErrorSetOptions (error slot 5): noted, never applied (the emulator must not stop on an error
/// the title handles).
unsafe extern "system" fn error_set_options(
    _this: *mut c_void,
    when_debugger: u32,
    without_debugger: u32,
) {
    crate::log::trace!("[runtime] XErrorSetOptions({when_debugger}, {without_debugger}) noté");
}

/// The method table of one of this module's objects, by its C++ name.
pub fn methods(object: &str) -> Vec<(usize, *const c_void)> {
    let subscribe_pair = vec![
        (3, subscribe as *const c_void),
        (4, unsubscribe as *const c_void),
    ];
    match object {
        "InviteInterface" | "ActivationInterface" | "ProtocolInterface" => subscribe_pair,
        "PendingInviteInterface" => vec![
            (5, subscribe as *const c_void),
            (6, unsubscribe as *const c_void),
        ],
        "ErrorInterface" => vec![
            (3, error_report as *const c_void),
            (4, error_set_callback as *const c_void),
            (5, error_set_options as *const c_void),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_removed_once() {
        let mut first = 0u64;
        let mut second = 0u64;
        let callback = std::ptr::dangling::<c_void>();
        assert_eq!(add_subscription(callback, &mut first), S_OK);
        assert_eq!(add_subscription(callback, &mut second), S_OK);
        assert!(first != 0 && second != first);
        assert!(remove_subscription(first));
        assert!(!remove_subscription(first));
        assert!(remove_subscription(second));
    }

    #[test]
    fn a_subscription_needs_a_callback_and_a_token() {
        let mut token = 0u64;
        assert_eq!(add_subscription(std::ptr::null(), &mut token), E_INVALIDARG);
        assert_eq!(
            add_subscription(std::ptr::dangling::<c_void>(), std::ptr::null_mut()),
            E_INVALIDARG
        );
    }

    #[test]
    fn the_error_callback_receives_the_report() {
        use std::sync::atomic::{AtomicI32, Ordering};
        static SEEN: AtomicI32 = AtomicI32::new(0);
        unsafe extern "system" fn record(
            hr: HResult,
            _message: *const c_char,
            _context: *mut c_void,
        ) -> bool {
            SEEN.store(hr, Ordering::SeqCst);
            true
        }
        // SAFETY: the callback has the signature the module expects.
        unsafe { error_set_callback(std::ptr::null_mut(), Some(record), std::ptr::null_mut()) };
        // SAFETY: a valid C string.
        let message = c"parametre invalide";
        // SAFETY: as above.
        unsafe { error_report(std::ptr::null_mut(), E_INVALIDARG, message.as_ptr()) };
        assert_eq!(SEEN.load(Ordering::SeqCst), E_INVALIDARG);
        // SAFETY: clearing the callback again.
        unsafe { error_set_callback(std::ptr::null_mut(), None, std::ptr::null_mut()) };
    }
}
