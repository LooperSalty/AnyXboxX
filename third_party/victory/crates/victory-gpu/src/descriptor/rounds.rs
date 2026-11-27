//! Descriptor heap scans on the existing heap-sync worker. With
//! `VICTORY_EARLY_HEAPS=1` (off by default), the title's Close can wake it
//! to consume dirty pages while other lists are still being recorded. Close
//! is a hint: descriptors can be written until submit or a queue wait is met.
//! Only a requested pass advances the tickets used by the submitting thread
//! and by `VICTORY_PORT_JOURNAL` at enqueue and at its dependency floors.
//! A requested pass absorbs a pending hint. At most one extra pass can run
//! between requests, including when closed lists are abandoned. Inline heap
//! sync and disabled heap sync do no work at Close.

use super::{SYNCED, sync_heaps, sync_on};
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
    *ON.get_or_init(|| {
        sync_on() && std::env::var_os("VICTORY_HEAP_SYNC_ASIDE").is_none_or(|v| v != "0")
    })
}

/// How long that thread waits for a submission before it makes a round of its
/// own (`VICTORY_HEAP_SYNC_EARLY=<ms>`, 4 if not given; `0`: none). The title
/// copies its records while it records, and the first submission of a frame
/// found all of them: 8 ms waited for (run cpu-2). A round made meanwhile
/// leaves it those written since.
fn sync_early() -> Option<Duration> {
    static EVERY: OnceLock<Option<Duration>> = OnceLock::new();
    *EVERY.get_or_init(|| {
        let ms = std::env::var("VICTORY_HEAP_SYNC_EARLY")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(4u64);
        (ms > 0).then(|| Duration::from_millis(ms))
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseHint {
    Available,
    Pending,
    Used,
}

struct Rounds {
    asked: u64,
    done: u64,
    hint: CloseHint,
}

impl Rounds {
    const fn new() -> Self {
        Self {
            asked: 0,
            done: 0,
            hint: CloseHint::Available,
        }
    }

    fn list_closed(&mut self) -> bool {
        if self.asked != self.done || self.hint != CloseHint::Available {
            return false;
        }
        self.hint = CloseHint::Pending;
        true
    }

    /// Called under the same lock as a request, before reading any pages.
    fn take(&mut self) -> Option<Pass> {
        if self.asked > self.done {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Requested,
    Close,
    Timer,
}

static ROUNDS: Mutex<Rounds> = Mutex::new(Rounds::new());
static MOVED: Condvar = Condvar::new();

fn rounds() -> MutexGuard<'static, Rounds> {
    ROUNDS.lock().unwrap_or_else(|e| e.into_inner())
}

fn early_heaps() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_EARLY_HEAPS").is_some_and(|v| v == "1"))
}

pub(crate) fn list_closed() {
    if early_heaps() && sync_aside() && worker() && rounds().list_closed() {
        // The worker and submissions waiting for their ticket share this condvar.
        MOVED.notify_all();
    }
}

fn worker() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        std::thread::Builder::new()
            .name("victory-heap-sync".into())
            .spawn(run)
            .is_ok()
    })
}

fn next_pass(last_request: Option<Instant>) -> Pass {
    let mut r = rounds();
    loop {
        if let Some(pass) = r.take() {
            return pass;
        }
        if let Some(every) = sync_early()
            .filter(|_| last_request.is_some_and(|t| t.elapsed() < Duration::from_millis(500)))
        {
            let (back, timeout) = MOVED
                .wait_timeout(r, every)
                .unwrap_or_else(|e| e.into_inner());
            r = back;
            if timeout.timed_out() {
                return r.take().unwrap_or(Pass::Timer);
            }
        } else {
            r = MOVED.wait(r).unwrap_or_else(|e| e.into_inner());
        }
    }
}

fn run() {
    crate::cores::keep_on_fast_cores("victory-heap-sync");
    // Preserve the original timer window at startup for a submit-started
    // worker. With Close hints the first submission may be much later.
    let mut last_request = (!early_heaps()).then(Instant::now);
    loop {
        let pass = next_pass(last_request);
        let _span = crate::timeline::span(match pass {
            Pass::Requested => "the heaps' records, for a submission",
            Pass::Close => "the heaps' records, at list close",
            Pass::Timer => "the heaps' records, early",
        });
        if pass == Pass::Requested {
            last_request = Some(Instant::now());
        }
        if std::panic::catch_unwind(|| super::scan_heaps(pass != Pass::Close)).is_err()
            && early_heaps()
        {
            crate::host::fail(format_args!("the descriptor heap scan panicked"));
        }
        if pass == Pass::Requested {
            rounds().done += 1;
            MOVED.notify_all();
        }
    }
}

/// A submission waits for its requested pass when this is dropped.
pub(crate) struct SyncedHeaps(Option<u64>);

pub(crate) fn begin_sync_heaps() -> SyncedHeaps {
    if !sync_aside() || !worker() {
        sync_heaps();
        return SyncedHeaps(None);
    }
    let mut r = rounds();
    r.asked += 1;
    let mine = r.asked;
    drop(r);
    MOVED.notify_all();
    SyncedHeaps(Some(mine))
}

/// Renderer port #7 (`crate::port_journal`, from renderer patch #4): [`begin_sync_heaps`] for a
/// submission that needs round `mark` (one that began after what it waits
/// for was written, `crate::port_journal::round_after`): asked for if it has not
/// been. 0: as [`begin_sync_heaps`].
pub(crate) fn begin_sync_heaps_after(mark: u64) -> SyncedHeaps {
    if mark == 0 || !sync_aside() {
        return begin_sync_heaps();
    }
    // No requested round yet: establish the first submission's ticket.
    let mut rounds = rounds();
    if rounds.asked == 0 {
        drop(rounds);
        return begin_sync_heaps();
    }
    let asked = rounds.asked < mark;
    rounds.asked = rounds.asked.max(mark);
    drop(rounds);
    if asked {
        MOVED.notify_all();
        crate::port_journal::STATS[crate::port_journal::LATE_ROUNDS].fetch_add(1, Relaxed);
    } else {
        crate::port_journal::STATS[crate::port_journal::EARLY_ROUNDS].fetch_add(1, Relaxed);
    }
    SyncedHeaps(Some(mark))
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
    if rounds.asked == 0 {
        return 0;
    }
    let (round, ask) = crate::port_journal::round_after(rounds.asked, rounds.done);
    if ask {
        rounds.asked = round;
        drop(rounds);
        MOVED.notify_all();
        crate::port_journal::STATS[crate::port_journal::ROUNDS_ASKED].fetch_add(1, Relaxed);
    }
    round
}

/// Renderer port #7: the number of the first round that begins after this
/// call, not asked for ([`begin_sync_heaps_after`] asks for it if nobody did).
pub(crate) fn round_mark() -> u64 {
    if !sync_aside() {
        return 0;
    }
    let rounds = rounds();
    if rounds.asked == 0 {
        return 0;
    }
    rounds.asked + 1
}

impl Drop for SyncedHeaps {
    fn drop(&mut self) {
        let Some(mine) = self.0 else {
            return;
        };
        let began = std::time::Instant::now();
        let mut rounds = rounds();
        while rounds.done < mine {
            rounds = MOVED.wait(rounds).unwrap_or_else(|e| e.into_inner());
        }
        drop(rounds);
        SYNCED[6].fetch_add(began.elapsed().as_micros() as u64, Relaxed);
    }
}

#[cfg(test)]
mod tests;
