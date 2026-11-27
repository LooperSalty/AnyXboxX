//! List calls that only write down their arguments (`VICTORY_THIN=1`); what
//! they mean is worked out by threads of ours, beside the title's.
//!
//! Why: the title makes 45,000 to 60,000 list calls a frame. Each was
//! translated inside the call, 3.5 us of it, 160 to 190 ms of processor time a
//! frame on the title's five recording threads; its own code between the calls
//! is a tenth of that. Nothing else of the title's frame could go on
//! meanwhile: recording took 55 of 65 ms, and behind it the card stood idle
//! for half the frame (timeline waits-3). On the console such a call writes a
//! few words into the list's memory and the card reads them when the list
//! runs. So it is here now: a call appends its arguments (and copies of what
//! they point at) to a stream of words; threads of ours run the stream, call
//! by call and in order, through the list methods there were before, on a list
//! of ours ([`Recording::back`]) that the title never sees. They begin while
//! the title still records (every [`CHUNK`] words are handed over), several
//! lists at a time. A submission goes into the line of submissions
//! (`crate::queue::behind`) as it was made, by the recordings' names; the
//! thread that serves the line goes on with it when they are translated.
//! `VICTORY_THIN_EARLY=1` measures how much translation precedes the
//! title's first submission of each recording.
//!
//! What may be looked at later than before, and why that is all right: all
//! that the card of the console looks at when the list runs (descriptor heaps,
//! constants, upload memory, indirect arguments): it must stay as it is until
//! the list has run, and no fence behind the list is set before. What D3D12
//! reads at the call is copied at the call: the arrays and structures passed,
//! and the render target and depth views, whose slots may be written again
//! right after. Objects named (pipelines, resources) must live until the list
//! has run; a recording the title resets or releases without submitting it is
//! not run any further.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

mod commands;
mod early;

use commands::{CLOSE, RESET, translate};
pub(in crate::list) use commands::table;
use std::time::Instant;

/// Words after which a stream is handed to the translating threads.
const CHUNK: usize = 1024;

/// Whether list calls are only written down (`VICTORY_THIN=1`; needs the line
/// of submissions, which it turns on).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        wanted() && crate::host::host().is_some() && crate::queue::line_on() && {
            start();
            true
        }
    })
}

/// Whether it is asked for.
pub(crate) fn wanted() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_THIN").is_some_and(|v| v == "1"))
}

/// What a title list keeps: the recording being written and the words of it
/// not handed over yet.
#[derive(Default)]
pub(super) struct Front {
    recording: Option<Arc<Recording>>,
    words: Vec<u64>,
    closed: bool,
    submitted: bool,
}

impl Drop for Front {
    fn drop(&mut self) {
        self.leave();
    }
}

impl Front {
    /// Lets go of the recording there is: one never submitted is not run any
    /// further.
    fn leave(&mut self) {
        if let Some(recording) = self.recording.take()
            && !self.submitted
        {
            recording.abandoned.store(true, Release);
        }
        self.words.clear();
    }

    /// Begins a recording, with `pipeline` bound.
    pub(super) fn begin(&mut self, kind: u32, pipeline: This) {
        self.leave();
        self.recording = Some(Arc::new(Recording::new(kind)));
        self.closed = false;
        self.submitted = false;
        self.put(RESET, &[pipeline as u64]);
    }

    #[inline]
    fn put(&mut self, op: u64, args: &[u64]) {
        if self.closed || self.recording.is_none() {
            return;
        }
        self.words.push(op | (args.len() as u64) << 16);
        self.words.extend_from_slice(args);
        if self.words.len() >= CHUNK {
            self.hand_over();
        }
    }

    /// [`Self::put`] of some words and then the bytes of some things.
    fn put_with(&mut self, op: u64, args: &[u64], blobs: &[&[u8]]) {
        if self.closed || self.recording.is_none() {
            return;
        }
        let at = self.words.len();
        self.words.push(0);
        self.words.extend_from_slice(args);
        for blob in blobs {
            let from = self.words.len();
            self.words.resize(from + blob.len().div_ceil(8), 0);
            // SAFETY: the words just made hold the bytes; plain data.
            unsafe {
                std::ptr::copy_nonoverlapping(blob.as_ptr(), self.words.as_mut_ptr().add(from).cast(), blob.len())
            };
        }
        let len = (self.words.len() - at - 1) as u64;
        self.words[at] = op | len << 16;
        if self.words.len() >= CHUNK {
            self.hand_over();
        }
    }

    fn hand_over(&mut self) {
        let Some(recording) = &self.recording else { return };
        if self.words.is_empty() {
            return;
        }
        let fresh = held(&WORDS).pop().unwrap_or_else(|| Vec::with_capacity(CHUNK + 256));
        let words = std::mem::replace(&mut self.words, fresh);
        CHUNKS.fetch_add(1, Relaxed);
        let schedule = {
            let mut inner = held(&recording.inner);
            inner.chunks.push_back(words);
            !std::mem::replace(&mut inner.scheduled, true)
        };
        if schedule {
            held(&READY).push_back(recording.clone());
            WORK.notify_one();
        }
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.put(CLOSE, &[]);
        self.closed = true;
        self.hand_over();
    }
}

/// Streams' memory to use again.
static WORDS: Mutex<Vec<Vec<u64>>> = Mutex::new(Vec::new());
/// Recordings with words to translate.
static READY: Mutex<VecDeque<Arc<Recording>>> = Mutex::new(VecDeque::new());
static WORK: Condvar = Condvar::new();
/// Lists of ours nobody uses, closed.
static BACKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Since the last line of the log: streams handed over; calls translated and
/// the microseconds that took (all threads), recordings translated, and
/// recordings let go of in the middle.
static CHUNKS: AtomicU64 = AtomicU64::new(0);
/// How many translating threads were started.
static THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn held<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One recording of a title list: what was written of it and not translated
/// yet, and the list of ours it goes to.
pub(crate) struct Recording {
    kind: u32,
    inner: Mutex<Inner>,
    /// Translated to its end.
    done: AtomicBool,
    moved: Condvar,
    abandoned: AtomicBool,
    first_submit: OnceLock<Instant>,
}

#[derive(Default)]
struct Inner {
    chunks: VecDeque<Vec<u64>>,
    /// Streams translated (commands may name places in them until the end).
    kept: Vec<Vec<u64>>,
    /// It is among [`READY`] or being translated.
    scheduled: bool,
    /// The list of ours (0: none yet).
    back: usize,
    /// That list was closed by the stream.
    closed: bool,
    /// Worker intervals, kept only when measuring time before submit.
    intervals: Vec<(Instant, Instant)>,
}

impl Recording {
    fn new(kind: u32) -> Self {
        Recording {
            kind,
            inner: Mutex::default(),
            done: AtomicBool::new(false),
            moved: Condvar::new(),
            abandoned: AtomicBool::new(false),
            first_submit: OnceLock::new(),
        }
    }

    pub(crate) fn done(&self) -> bool {
        self.done.load(Acquire)
    }

    /// Waits until it is translated.
    pub(crate) fn wait(&self) {
        let mut inner = held(&self.inner);
        while !self.done() {
            inner = self.moved.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The list of ours it went to (null: it was never begun).
    pub(crate) fn back(&self) -> This {
        let mut inner = held(&self.inner);
        if let Some(&at) = self.first_submit.get() {
            early::count(&mut inner.intervals, at);
        }
        inner.back as This
    }

    /// Translates what there is.
    fn run(&self) {
        loop {
            let (chunk, back) = {
                let mut inner = held(&self.inner);
                let Some(chunk) = inner.chunks.pop_front() else {
                    inner.scheduled = false;
                    return;
                };
                if inner.back == 0 {
                    inner.back = back_list(self.kind) as usize;
                }
                (chunk, inner.back as This)
            };
            let began = Instant::now();
            let (calls, closed) = translate(back, &chunk, || self.abandoned.load(Acquire));
            STATS[0].fetch_add(calls, Relaxed);
            let elapsed = began.elapsed();
            STATS[1].fetch_add(elapsed.as_micros() as u64, Relaxed);
            let mut inner = held(&self.inner);
            if early::on() {
                inner.intervals.push((began, began + elapsed));
            }
            inner.kept.push(chunk);
            if closed {
                inner.closed = true;
                let kept = std::mem::take(&mut inner.kept);
                self.done.store(true, Release);
                self.moved.notify_all();
                drop(inner);
                give_back(kept);
                STATS[2].fetch_add(1, Relaxed);
                crate::queue::line_moved();
            }
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        give_back(inner.chunks.drain(..).chain(std::mem::take(&mut inner.kept)).collect());
        if inner.back != 0 {
            if !inner.closed {
                // Left in the middle.
                STATS[3].fetch_add(1, Relaxed);
                super::close(inner.back as This);
            }
            held(&BACKS).push(inner.back);
        }
    }
}

fn give_back(streams: Vec<Vec<u64>>) {
    let mut words = held(&WORDS);
    for mut s in streams {
        if words.len() < 1024 && s.capacity() <= 16 * CHUNK {
            s.clear();
            words.push(s);
        }
    }
}

/// A list of ours, closed.
fn back_list(kind: u32) -> This {
    if let Some(back) = held(&BACKS).pop() {
        // SAFETY: a list of ours nobody uses.
        unsafe { (*(back as *mut List)).kind = kind };
        return back as This;
    }
    let list: This = Box::into_raw(super::boxed(kind, std::ptr::null(), false)).cast_const().cast();
    super::close(list);
    list
}

fn start() {
    let threads = std::env::var("VICTORY_THIN_THREADS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(8usize).clamp(1, 32);
    for i in 0..threads {
        let worker = move || {
            crate::cores::keep_many_on_fast_cores("victory-list");
            crate::queue::no_drain_here();
            loop {
                let recording = {
                    let mut ready = held(&READY);
                    loop {
                        match ready.pop_front() {
                            Some(r) => break r,
                            None => ready = WORK.wait(ready).unwrap_or_else(|e| e.into_inner()),
                        }
                    }
                };
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recording.run())).is_err() {
                    crate::host::fail(format_args!("a thread that translates lists panicked"));
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name(format!("victory-list-{i}")).spawn(worker) {
            tracing::warn!(error = %e, "a thread less to translate lists");
        }
    }
    THREADS.store(threads, Relaxed);
    crate::overlay::set_pool_threads(crate::overlay::Pool::Thin, threads);
    tracing::info!(threads, "list calls are written down and translated by threads of ours (VICTORY_THIN=1)");
}

/// The recordings of submitted lists, in order (one that was not begun is left
/// out).
pub(crate) fn submitted(lists: &[This]) -> Arc<[Arc<Recording>]> {
    // All lists in this Execute share a cutoff, even if closing one takes time.
    let at = early::on().then(Instant::now);
    lists
        .iter()
        .filter(|l| !l.is_null())
        .filter_map(|&l| {
            // SAFETY: the title passes its lists, which nobody records into now.
            let front = unsafe { &mut (*l.cast_mut().cast::<List>()).thin };
            if let (Some(recording), Some(at)) = (&front.recording, at) {
                recording.first_submit.get_or_init(|| at);
            }
            front.close();
            front.submitted = true;
            front.recording.clone()
        })
        .collect()
}

/// For the frame-rate line.
pub(crate) fn line(frames: f64) {
    if !wanted() || !on() {
        crate::overlay::set_pool_off(crate::overlay::Pool::Thin);
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|s| s.swap(0, Relaxed) as f64 / frames).collect();
    if early::on() {
        early::line(frames);
    }
    // The overlay: the work of all threads over the frame's time.
    crate::overlay::set_pool(crate::overlay::Pool::Thin, s[1] / 1000.0, crate::overlay::frame_ms(), THREADS.load(Relaxed));
    tracing::info!(
        "list calls translated by threads of ours per frame (VICTORY_THIN): {:.0} in {:.1} ms (all threads), {:.1} lists, {:.1} pieces; left in the middle {:.2}",
        s[0],
        s[1] / 1000.0,
        s[2],
        CHUNKS.swap(0, Relaxed) as f64 / frames,
        s[3]
    );
}
