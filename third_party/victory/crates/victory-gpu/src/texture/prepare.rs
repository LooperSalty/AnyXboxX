//! (Neptune) Textures picked by index made ready off the thread that records
//! the frame (`VICTORY_TEXTURE_PREPARE_THREADS=N`; 0 or unset: none, and the
//! pass over the heap's texture views makes them itself, as before).
//!
//! That pass (`crate::draw`, `State::heap_textures`) runs on a thread that
//! records the frame. For each texture it brings in it makes the host copy
//! (a `D3D12` texture of its own, `crate::resource::host_resource`) and a
//! staging buffer (an upload buffer, made or taken from the pool and
//! zeroed), and records the copy; the staging buffer is detiled when the
//! list is submitted. While a long queue is due (the opening cutscene: 2,000
//! to 20,000 slots) it may take 250 ms a pass, 1.5 s while catching up, and
//! took all of it: the frame line's "heap buffers" stood at 964-1,740 ms and
//! the frame at a third to half a frame a second for a minute or two.
//!
//! With the switch the pass hands each texture it would make to N threads
//! of its own. They make the host copy (while the card has room), make the
//! staging buffer and detile the title's memory into it. The pass records
//! the upload of the ones that are ready (a copy per level, as before) and
//! passes over the others, which stay due. What it hands out in a frame is
//! held to the pass's bytes a frame (`VICTORY_TEXTURE_STAGE_MB`, 512 MB while
//! catching up), and so is what is handed out and not yet taken.
//!
//! What stays as it was: the pass asks whether the card has room before it
//! hands a slot out (and again every 32 MiB handed out), and the thread asks
//! again before it makes a new host copy (`crate::resource::room_lately`); a
//! staging buffer is held by the list that records its upload until that
//! list has run; the title writing a texture after it was detiled is found
//! at the list's submission (write watch, guards, hash), and after the
//! upload by the looks of `texture_slots_rechecked`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock};

use super::Staging;
use crate::object::This;

/// The threads that make textures ready (`VICTORY_TEXTURE_PREPARE_THREADS`,
/// at most 16; none when unset or 0).
pub(crate) fn threads() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VICTORY_TEXTURE_PREPARE_THREADS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
            .min(16)
    })
}

/// A texture handed out: the title resource (kept alive until the thread is
/// done with it), the first mip the view that asked for it shows, and its
/// bytes as counted against the budget.
struct Job {
    resource: usize,
    first: u32,
    bytes: u64,
}

/// A texture made ready: its staging buffer, made and detiled (`None`:
/// nothing to stage, the pass goes its old way for it), the first mip that
/// holds, the bytes it counts, when it was made ready
/// (`crate::host::moment`), and the slots that waited for it.
struct Ready {
    staging: Option<Arc<Staging>>,
    first: u32,
    bytes: u64,
    at: u64,
    slots: Vec<u32>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// What is handed out and not done, by resource and first mip, with
    /// the slots that wait for it.
    pending: HashMap<(usize, u32), Vec<u32>>,
    /// The slots of `pending`: the pass passes over them at once.
    waiting: HashSet<u32>,
    /// What is made ready and not taken, by resource.
    ready: HashMap<usize, Vec<Ready>>,
    /// The slots of `ready` (how many entries name each): the pass looks
    /// at them even when it hands out nothing more.
    ready_slots: HashMap<u32, u32>,
    /// Bytes handed out and not done, or made ready and not taken.
    outstanding: u64,
    /// The frame `handed` counts for, and the bytes handed out in it.
    frame: u64,
    handed: u64,
    /// Threads started.
    started: usize,
}

impl Queue {
    fn hint(&mut self, slots: &[u32]) {
        for &slot in slots {
            *self.ready_slots.entry(slot).or_insert(0) += 1;
        }
    }

    fn unhint(&mut self, slots: &[u32]) {
        for slot in slots {
            if let Some(n) = self.ready_slots.get_mut(slot) {
                *n -= 1;
                if *n == 0 {
                    self.ready_slots.remove(slot);
                }
            }
        }
    }

    /// Takes `ready` out of the counts (it leaves the queue).
    fn gone(&mut self, ready: &Ready) {
        self.outstanding = self.outstanding.saturating_sub(ready.bytes);
        self.unhint(&ready.slots);
    }
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Mutex::default);
static WORK: Condvar = Condvar::new();

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Since the last frame line: textures made ready; host copies the threads
/// made; title bytes made ready; the threads' microseconds in all, of them
/// making host copies, making staging buffers, and reading and detiling;
/// textures handed out that found the card without room (looked at again);
/// textures made ready that nothing took (let go after two seconds).
pub(crate) static STATS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

/// Where the pass stands with a slot before it looks at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Slot {
    /// Its texture is handed out and not ready: passed over.
    Waiting,
    /// Its texture was made ready for it.
    Ready,
    /// Neither.
    Other,
}

/// See [`Slot`].
pub(crate) fn slot(index: u32) -> Slot {
    let q = queue();
    if q.waiting.contains(&index) {
        Slot::Waiting
    } else if q.ready_slots.contains_key(&index) {
        Slot::Ready
    } else {
        Slot::Other
    }
}

/// What the pass does with a slot whose texture it would make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Look {
    /// Its old way: the texture is ready (or there is nothing to hand out).
    Go,
    /// Handed out already, for another slot: passed over, it stays due.
    Wait,
    /// Handed out now (its bytes): passed over, it stays due.
    Handed(u64),
    /// The budget is spent: passed over, nothing more is handed out.
    Full,
}

/// Slot `slot` shows title texture `resource` from mip `first` on and the
/// pass would make it ready now (its host copy and staging buffer).
/// `recorded`: what this pass has recorded uploads of, by resource and first
/// mip (a second view of one is the old way's). `budget`: the bytes that
/// may be handed out in frame `frame`, and be out at once.
pub(crate) fn look(
    slot: u32,
    resource: This,
    first: u32,
    recorded: &HashSet<(usize, u32)>,
    budget: u64,
    frame: u64,
) -> Look {
    if threads() == 0
        || !crate::resource::title_fills(resource)
        || crate::resource::host_refused(resource)
        || recorded.contains(&(resource as usize, first))
    {
        return Look::Go;
    }
    let made = crate::resource::host_copy_made(resource);
    // Staged already (draws read it through a table): the old way.
    if made && super::staged_from(resource) <= first {
        return Look::Go;
    }
    let bytes = super::staged_bytes(resource, first);
    let mut q = queue();
    if q.ready.get(&(resource as usize)).is_some_and(|all| all.iter().any(|r| r.first <= first)) {
        return Look::Go;
    }
    if let Some(key) = (0..=first).map(|f| (resource as usize, f)).find(|key| q.pending.contains_key(key)) {
        if let Some(slots) = q.pending.get_mut(&key) {
            slots.push(slot);
        }
        q.waiting.insert(slot);
        return Look::Wait;
    }
    if q.frame != frame {
        q.frame = frame;
        q.handed = 0;
    }
    if q.handed >= budget || q.outstanding >= budget {
        return Look::Full;
    }
    crate::resource::keep_alive(resource);
    q.handed += bytes;
    q.outstanding += bytes;
    q.pending.insert((resource as usize, first), vec![slot]);
    q.waiting.insert(slot);
    q.jobs.push_back(Job {
        resource: resource as usize,
        first,
        bytes,
    });
    let start = threads().saturating_sub(q.started);
    q.started += start;
    drop(q);
    for _ in 0..start {
        start_thread();
    }
    WORK.notify_one();
    Look::Handed(bytes)
}

fn start_thread() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Relaxed);
    let started = std::thread::Builder::new()
        .name(format!("victory texture prepare {n}"))
        .spawn(run);
    if let Err(e) = started {
        tracing::warn!(error = %e, "a texture prepare thread could not be started");
        // Its share is done here, on the thread that hands out: nothing
        // waits for a thread that is not there.
        queue().started -= 1;
        loop {
            let Some(job) = queue().jobs.pop_front() else {
                break;
            };
            let outcome = work(&job);
            finish(job, outcome);
        }
    } else if n == 0 {
        tracing::info!(
            threads = threads(),
            "textures picked by index are made ready on threads of their own (VICTORY_TEXTURE_PREPARE_THREADS)"
        );
    }
}

/// What a thread made of a texture.
enum Outcome {
    /// The card had no room for a new host copy: nothing made, the slot is
    /// looked at again.
    NoRoom,
    /// Made ready (`None`: the host copy if any, but nothing to stage).
    Done(Option<Arc<Staging>>),
}

fn run() {
    loop {
        let job = {
            let mut q = queue();
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    break job;
                }
                q = WORK.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = std::time::Instant::now();
        let ([made, made_us], [_, staged_us, ..]) = (crate::resource::made_here(), super::staged_here());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job))).unwrap_or_else(|_| {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::error!("a texture prepare thread failed on a texture; it is looked at again"));
            Outcome::NoRoom
        });
        let ([made_now, made_us_now], [_, staged_us_now, ..]) = (crate::resource::made_here(), super::staged_here());
        STATS[1].fetch_add(made_now - made, Relaxed);
        STATS[3].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
        STATS[4].fetch_add(made_us_now - made_us, Relaxed);
        STATS[5].fetch_add(staged_us_now - staged_us, Relaxed);
        finish(job, outcome);
    }
}

/// Makes `job`'s texture ready: its host copy (only while the card has room
/// for a new one), its staging buffer, and the title's memory detiled into
/// it.
fn work(job: &Job) -> Outcome {
    let resource = job.resource as This;
    if !crate::resource::host_copy_made(resource) && !crate::resource::room_lately() {
        return Outcome::NoRoom;
    }
    let Some(host) = crate::resource::host_copy(resource) else {
        return Outcome::Done(None);
    };
    if !super::title_written(&host.texture) {
        return Outcome::Done(None);
    }
    let Some(staging) = super::prepared_staging(resource, &host.texture, job.first) else {
        return Outcome::Done(None);
    };
    let started = std::time::Instant::now();
    // Not `refresh`: that counts as the submitting thread's "textures".
    staging.refresh_now();
    STATS[6].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
    Outcome::Done(Some(staging))
}

fn finish(job: Job, outcome: Outcome) {
    let replaced = {
        let mut q = queue();
        let slots = q.pending.remove(&(job.resource, job.first)).unwrap_or_default();
        for slot in &slots {
            q.waiting.remove(slot);
        }
        match outcome {
            Outcome::NoRoom => {
                STATS[7].fetch_add(1, Relaxed);
                q.outstanding = q.outstanding.saturating_sub(job.bytes);
                None
            }
            Outcome::Done(staging) => {
                let bytes = if staging.is_some() { job.bytes } else { 0 };
                q.outstanding = q.outstanding.saturating_sub(job.bytes - bytes);
                STATS[0].fetch_add(1, Relaxed);
                STATS[2].fetch_add(bytes, Relaxed);
                let first = staging.as_ref().map_or(job.first, |s| s.first);
                q.hint(&slots);
                let all = q.ready.entry(job.resource).or_default();
                // One from the same mip already: the new one takes its place,
                // and the slots that waited for that one (still hinted).
                let mut old = all.iter().position(|r| r.first == first).map(|i| all.swap_remove(i));
                let mut slots = slots;
                if let Some(old) = &mut old {
                    slots.append(&mut old.slots);
                }
                all.push(Ready {
                    staging,
                    first,
                    bytes,
                    at: crate::host::moment(),
                    slots,
                });
                if let Some(old) = &old {
                    q.gone(old);
                }
                old
            }
        }
    };
    // Outside the lock: the last of the title's resource may go with it.
    drop(replaced);
    crate::resource::let_go_kept(job.resource as This);
}

thread_local! {
    /// The slot whose upload the pass records now ([`serving`]).
    static SERVING: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// The pass records the upload of slot `slot` now (`None`: no longer): what
/// [`take_ready`] gives it serves that slot.
pub(crate) fn serving(slot: Option<u32>) {
    SERVING.set(slot);
}

/// The staging state made ready for title texture `resource` that serves
/// from mip `first` on (for [`super::staging`], which puts it in the table).
/// It stays made ready while a slot that waited for it is not served yet: a
/// slot of the same texture may take it first, in the very pass that handed
/// it out, and the one that waited would hand it out anew (the host copy
/// has the contents by then: its upload records no copy, only the check at
/// submission). `on_demand`: whether the host copy is made level by level
/// now; one made the other way is not taken.
pub(crate) fn take_ready(resource: This, first: u32, on_demand: bool) -> Option<Arc<Staging>> {
    if threads() == 0 {
        return None;
    }
    let (staging, gone) = {
        let mut q = queue();
        let key = resource as usize;
        let all = q.ready.get_mut(&key)?;
        let best = all
            .iter()
            .enumerate()
            .filter(|(_, r)| r.first <= first)
            .max_by_key(|(_, r)| r.first)
            .map(|(i, _)| i)?;
        let staging = all[best].staging.clone();
        // The slot served now waits for none of them any more.
        let mut served = 0;
        let serving = SERVING.get();
        if let Some(slot) = serving {
            for ready in all.iter_mut() {
                if let Some(at) = ready.slots.iter().position(|&s| s == slot) {
                    ready.slots.swap_remove(at);
                    served += 1;
                }
            }
        }
        // What no slot waits for any more goes (taken now, or not needed).
        let mut gone = Vec::new();
        let mut i = 0;
        while i < all.len() {
            if all[i].slots.is_empty() {
                gone.push(all.swap_remove(i));
            } else {
                i += 1;
            }
        }
        if all.is_empty() {
            q.ready.remove(&key);
        }
        if let Some(slot) = serving {
            for _ in 0..served {
                q.unhint(&[slot]);
            }
        }
        for ready in &gone {
            q.gone(ready);
        }
        (staging, gone)
    };
    drop(gone);
    let staging = staging?;
    if staging.on_demand != on_demand {
        return None;
    }
    // Read while not all of its memory was mapped: the submission reads it
    // again (a partly mapped texture is otherwise read every 30 frames, and
    // what arrived since the thread read it would wait for that).
    staging.last.lock().unwrap_or_else(|e| e.into_inner()).partial = None;
    staging.used.store(crate::host::moment(), Relaxed);
    Some(staging)
}

/// Slot `slot` of title texture `resource` was served another way (by the
/// upload another slot recorded in the same pass): it waits for nothing made
/// ready, and what no slot waits for any more goes.
pub(crate) fn served(slot: u32, resource: This) {
    if threads() == 0 {
        return;
    }
    let gone = {
        let mut q = queue();
        let key = resource as usize;
        let Some(all) = q.ready.get_mut(&key) else {
            return;
        };
        let mut served = 0;
        for ready in all.iter_mut() {
            if let Some(at) = ready.slots.iter().position(|&s| s == slot) {
                ready.slots.swap_remove(at);
                served += 1;
            }
        }
        if served == 0 {
            return;
        }
        let mut gone = Vec::new();
        let mut i = 0;
        while i < all.len() {
            if all[i].slots.is_empty() {
                gone.push(all.swap_remove(i));
            } else {
                i += 1;
            }
        }
        if all.is_empty() {
            q.ready.remove(&key);
        }
        for _ in 0..served {
            q.unhint(&[slot]);
        }
        for ready in &gone {
            q.gone(ready);
        }
        gone
    };
    drop(gone);
}

/// The title released `resource`: what was made ready for it goes.
pub(crate) fn forget(resource: usize) {
    if threads() == 0 {
        return;
    }
    let gone = {
        let mut q = queue();
        let gone = q.ready.remove(&resource).unwrap_or_default();
        for ready in &gone {
            q.gone(ready);
        }
        gone
    };
    drop(gone);
}

/// Lets go of what was made ready two seconds and two frames ago and not
/// taken (its slot was written again, or is no longer due). Called by the
/// pass before it looks at the slots.
pub(crate) fn prune() {
    if threads() == 0 {
        return;
    }
    let mut gone = Vec::new();
    {
        let mut q = queue();
        if q.ready.is_empty() {
            return;
        }
        let mut stale = Vec::new();
        for all in q.ready.values_mut() {
            let mut i = 0;
            while i < all.len() {
                if crate::host::long_ago(all[i].at, 2000, 2) {
                    stale.push(all.swap_remove(i));
                } else {
                    i += 1;
                }
            }
        }
        q.ready.retain(|_, all| !all.is_empty());
        for ready in &stale {
            q.gone(ready);
        }
        gone.extend(stale);
    }
    STATS[8].fetch_add(gone.len() as u64, Relaxed);
    drop(gone);
}

/// For the frame line: textures handed out and not done, made ready and not
/// taken, and the bytes of both.
pub(crate) fn census() -> [u64; 3] {
    let q = queue();
    [
        q.pending.len() as u64,
        q.ready.values().map(|all| all.len() as u64).sum(),
        q.outstanding,
    ]
}

/// Until nothing is handed out and not done, at most `most`: whether that
/// came (tests).
#[cfg(test)]
pub(crate) fn wait_idle(most: std::time::Duration) -> bool {
    let until = std::time::Instant::now() + most;
    loop {
        {
            let q = queue();
            if q.jobs.is_empty() && q.pending.is_empty() {
                return true;
            }
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    /// The switch is off unless set: the pass makes its textures itself.
    #[test]
    fn prepare_threads_are_off_unless_set() {
        if std::env::var_os("VICTORY_TEXTURE_PREPARE_THREADS").is_none() {
            assert_eq!(super::threads(), 0);
            assert!(super::take_ready(std::ptr::null(), 0, false).is_none());
        }
    }
}
