//! Restrict the opt-in bind shortcut to the translation of a THIN chunk.
use std::cell::Cell;
use std::sync::OnceLock;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn wanted() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("VICTORY_THIN_FAST").is_some_and(|v| v == "1"))
}

pub(crate) fn active() -> bool {
    ACTIVE.get()
}

pub(super) fn scope<T>(enabled: bool, work: impl FnOnce() -> T) -> T {
    ACTIVE.with(|active| {
        struct Restore<'a>(&'a Cell<bool>, bool);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(active, active.replace(enabled));
        work()
    })
}
