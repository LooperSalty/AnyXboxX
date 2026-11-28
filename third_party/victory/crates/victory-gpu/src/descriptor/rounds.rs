//! The heap-sync worker (`victory-heap-sync`) and its rounds.
//!
//! A submission asks for a round that begins after the title's descriptor
//! writes it must see, and waits for that round to finish. `SyncRounds`
//! counts the rounds: `started` and `done` cover every counted pass (asked
//! for or early), and a stamp is the number of the next round to begin.
//!
//! With `VICTORY_EARLY_HEAPS=1` (off by default), the title's Close can wake
//! the worker to consume dirty pages while other lists are still being
//! recorded. Close is a hint: descriptors can be written until submit or a
//! queue wait is met. So a Close pass takes marked slots only, leaves the
//! fallback to the counted passes, and advances no ticket. A requested pass
//! absorbs a pending hint. At most one Close pass runs between requests,
//! including when closed lists are abandoned. Inline heap sync and disabled
//! heap sync do no work at Close.

use super::{SYNCED, scan_heaps, sync_heaps, sync_on};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, atomic::Ordering::Relaxed};
use std::time::{Duration, Instant};

/// Whether a submission's [`sync_heaps`] is done by a thread of its own
/// beside the rest of the preparing (`VICTORY_HEAP_SYNC_ASIDE=0`: in front of
/// it, as before).
///
/// It took 13 ms a frame of the submitting thread (session play-1125-0203:
/// 230 changed heap pages and 470,000 slots in 30 submissions). What it makes
/// are the host heap's descriptors, which the card reads: it has to be done
/// before the lists go to the queue, and nothing of the preparing looks at
/// them (what a draw reads through a table it made itself, when recorded).
fn sync_aside() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| sync_on() && std::env::var_os("VICTORY_HEAP_SYNC_ASIDE").is_none_or(|v| v != "0"))
}

/// How long that thread waits for a submission before it makes a round of its
/// own (`VICTORY_HEAP_SYNC_EARLY=<ms>`, 4 if not given; `0`: none). The title
/// copies its records while it records, and the first submission of a frame
/// found all of them: 8 ms waited for (run cpu-2). A round made meanwhile
/// leaves it those written since.
fn sync_early() -> Option<Duration> {
    static EVERY: OnceLock<Option<Duration>> = OnceLock::new();
    *EVERY.get_or_init(|| {
        let ms = std::env::var("VICTORY_HEAP_SYNC_EARLY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4u64);
        (ms > 0).then(|| Duration::from_millis(ms))
    })
}

/// Whether the title's Close can wake the worker (`VICTORY_EARLY_HEAPS=1`).
fn early_heaps() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_EARLY_HEAPS").is_some_and(|v| v == "1"))
}

/// Whether a Close hint is waiting to be taken by the worker.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CloseHint {
    #[default]
    Available,
    Pending,
    Used,
}

/// What the worker does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    /// A submission asked for it: counted, with the fallback.
    Requested,
    /// The title closed a list: no fallback, not counted.
    Close,
    /// An early round made while nobody asked: counted, with the fallback.
    Timer,
}

/// State of the worker: rounds asked for, started, done, the new-heap floor,
/// and the Close hint.
#[derive(Default)]
struct SyncRounds {
    /// Highest round number requested by a submission.
    asked: u64,
    /// Rounds started by the worker (incremented before the scan reads
    /// write-watch, for asked and early rounds).
    started: u64,
    /// Rounds completed by the worker (incremented after the scan finishes,
    /// for asked and early rounds).
    done: u64,
    /// Minimum round required after a shader-visible host heap is lazily
    /// initialized (e.g. during `VICTORY_THIN=1` translation after enqueue):
    /// earlier rounds skipped that heap while its `OnceLock` was empty.
    floor: u64,
    hint: CloseHint,
}

impl SyncRounds {
    /// Minimum round number that is guaranteed to start *after* any title
    /// descriptor write already visible to this thread.
    fn stamp(&self) -> u64 {
        self.started + 1
    }

    /// Raises `floor` after a new host heap has been published into
    /// `Heap::host`, so stamped submissions wait for a round that observes it.
    fn note_heap(&mut self) {
        self.floor = self.floor.max(self.started + 1);
    }

    /// Determines the round a caller must wait for (if any) and updates
    /// `asked`. Returns `(wait_for_round, should_notify)`.
    fn request(&mut self, stamp: Option<u64>) -> (Option<u64>, bool) {
        let mine = match stamp {
            Some(s) if s > 0 => {
                let need = s.max(self.floor);
                if self.done >= need {
                    return (None, false);
                }
                need
            }
            _ => self.started + 1,
        };
        let notify = self.asked < mine;
        if notify {
            self.asked = mine;
        }
        (Some(mine), notify)
    }

    /// Whether a caller is waiting for a round that has not finished yet.
    fn has_asked(&self) -> bool {
        self.asked > self.done
    }

    /// Marks the start of a round (before the scan reads write-watch).
    fn start_round(&mut self) {
        self.started += 1;
    }

    /// Marks the completion of a round (after the scan finishes).
    fn finish_round(&mut self) {
        self.done += 1;
    }

    /// Takes the hint a closed list leaves, unless a round is requested or a
    /// Close pass already ran since the last request.
    fn list_closed(&mut self) -> bool {
        if self.has_asked() || self.hint != CloseHint::Available {
            return false;
        }
        self.hint = CloseHint::Pending;
        true
    }

    /// The next pass due, if any: a requested round first, then a Close hint.
    fn take_pass(&mut self) -> Option<Pass> {
        if self.has_asked() {
            self.hint = CloseHint::Available;
            Some(Pass::Requested)
        } else if self.hint == CloseHint::Pending {
            self.hint = CloseHint::Used;
            Some(Pass::Close)
        } else {
            None
        }
    }
}

static SYNC_ROUNDS: (Mutex<SyncRounds>, Condvar) = (
    Mutex::new(SyncRounds {
        asked: 0,
        started: 0,
        done: 0,
        floor: 0,
        hint: CloseHint::Available,
    }),
    Condvar::new(),
);

fn rounds() -> MutexGuard<'static, SyncRounds> {
    SYNC_ROUNDS.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the worker once. `false`: it could not be started, and the caller
/// syncs in front of it.
fn worker() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| std::thread::Builder::new().name("victory-heap-sync".into()).spawn(run).is_ok())
}

/// Returns the earliest heap-sync round guaranteed to begin *after* title
/// descriptor writes made up to this moment (used both to stamp submissions at
/// enqueue time in `queue::behind::submit` and to raise a queue's post-wait
/// floor when a wait is met). Returns `0` when asynchronous heap sync is off.
pub(crate) fn heap_sync_stamp() -> u64 {
    if !sync_aside() {
        return 0;
    }
    rounds().stamp()
}

/// The title's Close of a list: with `VICTORY_EARLY_HEAPS=1`, a hint for the
/// worker to make a pass without the fallback while nobody waits.
pub(crate) fn list_closed() {
    if early_heaps() && sync_aside() && worker() && rounds().list_closed() {
        // The worker and submissions waiting for their ticket share this condvar.
        SYNC_ROUNDS.1.notify_all();
    }
}

/// Publishes a host heap made by `Heap::host` (`raise`: one was built). With
/// asynchronous sync on, the publication and the floor it raises happen under
/// one lock, so no thread that sees the heap can stamp a round that misses it.
pub(crate) fn publish_host(raise: bool, publish: impl FnOnce()) {
    if !(raise && sync_aside()) {
        publish();
        return;
    }
    let mut rounds = rounds();
    publish();
    rounds.note_heap();
}

/// Waits for the next pass and, for a counted one, starts it under the lock.
/// `last`: when the worker last ran a requested round, while early rounds are
/// on (`None`: no early round until one is requested).
fn next_pass(last: Option<Instant>) -> Pass {
    let mut r = rounds();
    let pass = loop {
        if let Some(pass) = r.take_pass() {
            break pass;
        }
        let Some(every) = sync_early().filter(|_| last.is_some_and(|t| t.elapsed() < Duration::from_millis(500))) else {
            r = SYNC_ROUNDS.1.wait(r).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let (back, timeout) = SYNC_ROUNDS.1.wait_timeout(r, every).unwrap_or_else(|e| e.into_inner());
        r = back;
        if timeout.timed_out() {
            break r.take_pass().unwrap_or(Pass::Timer);
        }
    };
    if pass != Pass::Close {
        r.start_round();
    }
    pass
}

/// A round is over, however it ends (asked for or early).
struct Over;

impl Drop for Over {
    fn drop(&mut self) {
        rounds().finish_round();
        SYNC_ROUNDS.1.notify_all();
    }
}

fn run() {
    crate::cores::keep_on_fast_cores("victory-heap-sync");
    let mut priority = crate::cores::Priority::new();
    // A worker started by a submission times its early rounds from now. With
    // Close hints it has none until a round is requested.
    let mut last = (!early_heaps()).then(Instant::now);
    loop {
        let pass = next_pass(last);
        if pass == Pass::Requested {
            last = Some(Instant::now());
        }
        priority.keep();
        let counted = pass != Pass::Close;
        let _span = crate::timeline::span(match pass {
            Pass::Requested => "the heaps' records, for a submission",
            Pass::Close => "the heaps' records, at list close",
            Pass::Timer => "the heaps' records, early",
        });
        // `then_some` would build the guard even for a Close pass and finish a round at once.
        let _over = counted.then(|| Over);
        if std::panic::catch_unwind(|| scan_heaps(counted)).is_err() && early_heaps() {
            crate::host::fail(format_args!("the descriptor heap scan panicked"));
        }
    }
}

/// [`sync_heaps`], begun; done when it is dropped (with [`sync_aside`]), or
/// when this returns.
pub(crate) struct SyncedHeaps(Option<u64>);

/// Begins (or reuses an already-completed/in-flight) heap-sync round that
/// covers all title descriptor writes up to `stamp` (or up to now if `None`).
pub(crate) fn begin_sync_heaps_for(stamp: Option<u64>) -> SyncedHeaps {
    if !sync_aside() || !worker() {
        sync_heaps();
        return SyncedHeaps(None);
    }
    let (mine, notify) = rounds().request(stamp);
    if notify {
        SYNC_ROUNDS.1.notify_all();
    }
    SyncedHeaps(mine)
}

/// Renderer port #7 (`crate::port_journal`, from renderer patch #4): [`begin_sync_heaps_for`] for a
/// submission that needs round `mark` (one that began after what it waits
/// for was written, via [`crate::port_journal::prep_wrote`]): asked for if it has not
/// been. 0: as [`begin_sync_heaps_for`] without a stamp.
pub(crate) fn begin_sync_heaps_after(mark: u64) -> SyncedHeaps {
    if mark == 0 || !sync_aside() {
        return begin_sync_heaps_for(None);
    }
    // (The worker is started by the first `begin_sync_heaps_for`.)
    let mut rounds = rounds();
    if rounds.done == 0 && rounds.asked == 0 {
        drop(rounds);
        return begin_sync_heaps_for(None);
    }
    let (mine, notify) = rounds.request(Some(mark));
    drop(rounds);
    if notify {
        SYNC_ROUNDS.1.notify_all();
        crate::port_journal::STATS[crate::port_journal::LATE_ROUNDS].fetch_add(1, Relaxed);
    } else {
        crate::port_journal::STATS[crate::port_journal::EARLY_ROUNDS].fetch_add(1, Relaxed);
    }
    SyncedHeaps(mine)
}

/// Renderer port #7: the number of a round of the heaps' records that
/// begins after this call, asked for if none is waiting to begin (at the
/// title's submit: the round runs while the submission waits in the line).
/// 0 if the records are not synced by a thread of their own (or it has not
/// started yet).
pub(crate) fn round_after_now() -> u64 {
    if !sync_aside() {
        return 0;
    }
    let mut rounds = rounds();
    if rounds.done == 0 && rounds.asked == 0 {
        return 0;
    }
    let round = rounds.stamp();
    if rounds.asked < round {
        rounds.asked = round;
        drop(rounds);
        SYNC_ROUNDS.1.notify_all();
        crate::port_journal::STATS[crate::port_journal::ROUNDS_ASKED].fetch_add(1, Relaxed);
    }
    round
}

/// Renderer port #7: the number of the first round that begins after this
/// call, not asked for ([`begin_sync_heaps_after`] asks for it if nobody did).
/// Also raises `SyncRounds::floor` so stamped submissions queued before a
/// preparation-time write to a placed descriptor heap still wait for a round
/// that observes it.
pub(crate) fn round_mark() -> u64 {
    if !sync_aside() {
        return 0;
    }
    let mut rounds = rounds();
    if rounds.done == 0 && rounds.asked == 0 {
        return 0;
    }
    rounds.note_heap();
    rounds.floor
}

impl Drop for SyncedHeaps {
    fn drop(&mut self) {
        let Some(mine) = self.0 else {
            return;
        };
        let began = std::time::Instant::now();
        let mut rounds = rounds();
        while rounds.done < mine {
            rounds = SYNC_ROUNDS.1.wait(rounds).unwrap_or_else(|e| e.into_inner());
        }
        drop(rounds);
        SYNCED[6].fetch_add(began.elapsed().as_micros() as u64, Relaxed);
    }
}

#[cfg(test)]
mod tests;
