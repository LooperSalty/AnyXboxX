//! Per-call timings on the list translation workers (`VICTORY_THIN_TIMES=1`).
use super::{
    BARRIER, BARRIER_GROUPS, CLEAR_DSV, CLEAR_RTV, CLOSE, COPY_TEXTURE, Plain, RESET, TARGETS,
    UPLOAD, VIEWPORTS,
};
use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_THIN_TIMES"))
}

/// The kinds of calls [`on`] counts apart.
const KINDS: [&str; 9] = [
    "draws",
    "dispatches",
    "mesh dispatches",
    "indirect executions",
    "bindings",
    "barriers",
    "copies and clears",
    "begin, end, uploads",
    "other",
];
/// The kind of call `op` is ([`KINDS`]).
fn kind_of(op: u64) -> usize {
    let is = |p: Plain| op == p as u64;
    if is(Plain::DRAW) || is(Plain::DRAW_INDEXED) {
        0
    } else if is(Plain::DISPATCH) || is(Plain::DISPATCH_X) {
        1
    } else if is(Plain::DISPATCH_MESH) || is(Plain::DISPATCH_MESH_X) {
        2
    } else if is(Plain::EXECUTE_INDIRECT) {
        3
    } else if op == BARRIER || op == BARRIER_GROUPS || is(Plain::SYNC) {
        5
    } else if op == COPY_TEXTURE
        || op == CLEAR_RTV
        || op == CLEAR_DSV
        || is(Plain::COPY_BUFFER)
        || is(Plain::COPY_RESOURCE)
        || is(Plain::COPY_RESOURCE_X)
        || is(Plain::COPY_MEMORY)
        || is(Plain::RESOLVE)
        || is(Plain::RESOLVE_QUERIES)
    {
        6
    } else if op == RESET || op == CLOSE || op == UPLOAD {
        7
    } else if is(Plain::WRITE32)
        || is(Plain::WRITE64)
        || is(Plain::WAIT_VALUE)
        || is(Plain::WAIT_VALUE64)
    {
        8
    } else if op < Plain::END as u64 || (VIEWPORTS..=TARGETS).contains(&op) {
        4
    } else {
        8
    }
}

pub(crate) enum Part {
    Heap,
    Table,
    Readable,
}

const PARTS: [&str; 3] = [
    "the heap pass",
    "views of tables (make_listed)",
    "views made ready (readable)",
];
const SLOTS: usize = KINDS.len() + PARTS.len();

#[derive(Clone, Copy, Default)]
struct Measurement {
    ns: u64,
    calls: u64,
}

impl Measurement {
    const ZERO: Self = Self { ns: 0, calls: 0 };

    fn per_frame(self, name: &str, frames: f64) -> String {
        let us_each = self.ns as f64 / 1000.0 / self.calls.max(1) as f64;
        format!(
            "{name} {:.2} in {:.2} ms ({us_each:.2} us each)",
            self.calls as f64 / frames,
            self.ns as f64 / 1e6 / frames
        )
    }
}

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static LOCAL: RefCell<[Measurement; SLOTS]> = const { RefCell::new([Measurement::ZERO; SLOTS]) };
}
static TOTAL: Mutex<[Measurement; SLOTS]> = Mutex::new([Measurement::ZERO; SLOTS]);

/// Merge once per translated stream. The snapshot sees count and time together.
pub(super) fn stream<R>(translate: impl FnOnce() -> R) -> R {
    struct Flush;
    impl Drop for Flush {
        fn drop(&mut self) {
            ACTIVE.set(false);
            let local = LOCAL.take();
            let mut total = TOTAL.lock().unwrap_or_else(|e| e.into_inner());
            for (sum, part) in total.iter_mut().zip(local) {
                sum.calls += part.calls;
                sum.ns += part.ns;
            }
        }
    }

    let _flush = Flush;
    ACTIVE.set(true);
    translate()
}

pub(super) struct Span {
    start: Instant,
    slot: usize,
}

pub(super) fn call(op: u64) -> Span {
    Span {
        slot: kind_of(op),
        start: Instant::now(),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let ns = self.start.elapsed().as_nanos() as u64;
        LOCAL.with_borrow_mut(|local| {
            local[self.slot].ns += ns;
            local[self.slot].calls += 1;
        });
    }
}

impl Part {
    /// Draw code is also used outside translation workers.
    #[inline]
    pub(crate) fn measure<R>(self, work: impl FnOnce() -> R) -> R {
        if !ACTIVE.get() {
            return work();
        }
        let _span = Span {
            slot: KINDS.len() + self as usize,
            start: Instant::now(),
        };
        work()
    }
}

pub(super) fn line(frames: f64) {
    if on() {
        let totals = std::mem::take(&mut *TOTAL.lock().unwrap_or_else(|e| e.into_inner()));
        let show = |names: &[&str], group: &[Measurement]| {
            names
                .iter()
                .zip(group)
                .map(|(name, m)| m.per_frame(name, frames))
                .collect::<Vec<_>>()
                .join(", ")
        };
        tracing::info!(
            "translating per frame by kind of call (VICTORY_THIN_TIMES): {}; in binds: {}",
            show(&KINDS, &totals),
            show(&PARTS, &totals[KINDS.len()..])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_call_kinds_and_rare_call_means() {
        for (op, k) in [
            (Plain::DRAW as u64, 0),
            (Plain::DISPATCH as u64, 1),
            (Plain::DISPATCH_MESH_X as u64, 2),
            (Plain::EXECUTE_INDIRECT as u64, 3),
            (TARGETS, 4),
            (BARRIER_GROUPS, 5),
            (COPY_TEXTURE, 6),
            (RESET, 7),
            (Plain::WAIT_VALUE64 as u64, 8),
        ] {
            assert_eq!(kind_of(op), k);
        }
        let rare = Measurement {
            ns: 400_000,
            calls: 2,
        }
        .per_frame("copies", 100.0);
        assert_eq!(rare, "copies 0.02 in 0.00 ms (200.00 us each)");
    }

    #[test]
    fn only_executed_worker_calls_and_bind_parts_are_counted() {
        Part::Heap.measure(|| {});
        assert_eq!(LOCAL.with_borrow(|s| s[KINDS.len()].calls), 0);
        stream(|| {
            // Abandoned bindings are skipped before timing any call.
            super::super::translate_stream::<true>(std::ptr::null(), &[TARGETS], || true);
            drop(call(Plain::DRAW as u64));
            Part::Heap.measure(|| {});
            LOCAL.with_borrow(|s| {
                assert_eq!(s[4].calls, 0);
                assert_eq!(s[0].calls, 1);
                assert_eq!(s[KINDS.len()].calls, 1);
            });
        });
        assert!(!ACTIVE.get());
        assert_eq!(LOCAL.with_borrow(|s| s[0].calls), 0);
    }
}
