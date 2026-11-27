//! VICTORY_THIN_STATS=1. Group submissions between title Present calls. A
//! frame holds only completion tickets, so delayed translation neither stalls
//! Present nor extends the life of any list or title object. Report completed
//! frames in order every five seconds, including frames with no lists.
//!
//! The repeat rate estimates identical input among the last 4096 submitted
//! lists. Include list kind, scalar arguments, named object addresses and
//! copied descriptors. Addresses can be reused and live descriptors can
//! change. This fingerprint is a diagnostic, never a replay key.
//! Recordings transferring an upload have distinct identities even if their
//! allocator reuses a box address. Translation time sums the recording
//! worker's active intervals, including parallel preparation and its join.
//!
//! For a newly submitted recording, estimate the translation work at stake
//! if a matching stream in that window had been reusable. A candidate must
//! already have finished when the title began this recording. Count later
//! finishes separately. Resubmitting a recording incurs no translation.
//! The estimate assumes the full stream can be recognized for free.
//! Worker intervals across lists can overlap in wall time.
//!
//! At bind time, draw::root_view can copy current title memory into list
//! scratch, and table lookup reads live heap slots. Neither input has a
//! complete content version in this stream. At submission, list::detach
//! transfers the host list, uploads and draw state to that flight. Temporary
//! descriptor slots are returned after it runs (draw::State::tables_done).

use super::{BARRIER, COPY_TEXTURE, CopyLocation, Recording, UPLOAD};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

const HISTORY: usize = 4096;
type Key = [u8; 16];
pub(super) type Ticket = Arc<Observation>;

pub(super) struct Observation {
    began: Instant,
    sample: OnceLock<Sample>,
}

impl Default for Observation {
    fn default() -> Self {
        Self {
            began: Instant::now(),
            sample: OnceLock::new(),
        }
    }
}

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_THIN_STATS").is_some_and(|v| v == "1"))
}

pub(super) fn ticket() -> Option<Ticket> {
    sender().map(|_| Ticket::default())
}

#[derive(Default, Clone, Copy)]
pub(super) struct Timing {
    elapsed: Duration,
    pieces: usize,
    bindings: usize,
}

impl Timing {
    #[cfg(test)]
    pub(super) fn split_counts(&self) -> (usize, usize) {
        (self.pieces, self.bindings)
    }

    pub(super) fn add(&mut self, elapsed: Duration, bindings: Option<usize>) {
        self.elapsed += elapsed;
        if let Some(n) = bindings {
            self.pieces += 1;
            self.bindings += n;
        }
    }
}

struct Sample {
    words: usize,
    key: Key,
    timing: Timing,
    finished: Instant,
    submitted: AtomicBool,
}

// Hash the active members of the two FFI unions. Their inactive bytes and
// alignment padding may be unspecified by the caller.
fn hash_chunk(hash: &mut blake3::Hasher, words: &[u64]) -> bool {
    fn bytes(words: &[u64], len: usize) -> &[u8] {
        assert!(len <= size_of_val(words));
        // SAFETY: callers select only initialized fields in this slice.
        unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), len) }
    }
    let mut from = 0;
    let mut i = 0;
    let mut upload = false;
    while i < words.len() {
        let op = words[i] & 0xffff;
        upload |= op == UPLOAD;
        let end = i + 1 + (words[i] >> 16) as usize;
        let w = &words[i + 1..end];
        if op == BARRIER || op == COPY_TEXTURE {
            hash.update(bytes(&words[from..=i], (i + 1 - from) * 8));
            if op == BARRIER {
                hash.update(&w[0].to_le_bytes());
                for b in w[1..].chunks_exact(size_of::<super::Barrier>() / 8) {
                    let used = match b[0] as u32 {
                        0 => 28, // header, resource, subresource, before, after
                        1 => 24, // header and two resources
                        2 => 16, // header and resource
                        _ => 8,
                    };
                    hash.update(bytes(b, used));
                }
            } else {
                hash.update(bytes(w, 16)); // x, y, z, boxed
                let step = size_of::<CopyLocation>() / 8;
                for loc in w[2..2 + 2 * step].chunks_exact(step) {
                    hash.update(bytes(loc, 12)); // resource and kind
                    let kind = u32::from_le_bytes(bytes(&loc[1..], 4).try_into().unwrap());
                    let used = if kind == 1 { 28 } else { 4 };
                    hash.update(bytes(&loc[2..], used));
                }
                let bounds = &w[2 + 2 * step..];
                hash.update(bytes(bounds, size_of_val(bounds)));
            }
            from = end;
        }
        i = end;
    }
    hash.update(bytes(&words[from..], (words.len() - from) * 8));
    upload
}

pub(super) fn complete(ticket: &Ticket, kind: u32, chunks: &[Vec<u64>], timing: Timing) {
    let finished = Instant::now();
    let mut hash = blake3::Hasher::new();
    hash.update(&kind.to_le_bytes());
    let mut upload = false;
    for chunk in chunks {
        upload |= hash_chunk(&mut hash, chunk);
    }
    if upload {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        hash.update(&NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    }
    let key = hash.finalize().as_bytes()[..16].try_into().unwrap();
    let words = chunks.iter().map(Vec::len).sum();
    let _ = ticket.sample.set(Sample {
        words,
        key,
        timing,
        finished,
        submitted: AtomicBool::new(false),
    });
}

#[derive(Default)]
struct Frames {
    current: Vec<Ticket>,
    pending: VecDeque<(u64, Vec<Ticket>)>,
    recent: VecDeque<Key>,
    // Completion times per fingerprint, in submission order (workers may
    // finish out of order). The two queues expire the same occurrence.
    finishes: HashMap<Key, VecDeque<Instant>>,
}

impl Frames {
    fn remember(&mut self, key: Key, began: Instant, finished: Instant) -> (bool, bool) {
        let times = self.finishes.entry(key).or_default();
        let repeated = !times.is_empty();
        let ready = times.iter().any(|&end| end <= began);
        times.push_back(finished);
        self.recent.push_back(key);
        if self.recent.len() > HISTORY {
            let old = self.recent.pop_front().unwrap();
            let times = self.finishes.get_mut(&old).unwrap();
            times.pop_front();
            if times.is_empty() {
                self.finishes.remove(&old);
            }
        }
        (repeated, ready)
    }

    fn take_ready(&mut self) -> Vec<Report> {
        let mut reports = Vec::new();
        while self
            .pending
            .front()
            .is_some_and(|(_, lists)| lists.iter().all(|t| t.sample.get().is_some()))
        {
            let (frame, tickets) = self.pending.pop_front().unwrap();
            let mut row = Report {
                frame,
                ..Report::default()
            };
            for t in tickets {
                let s = t.sample.get().unwrap();
                row.lists.push((s.words, s.timing.elapsed.as_micros()));
                let (repeated, ready) = self.remember(s.key, t.began, s.finished);
                row.repeated += usize::from(repeated);
                if s.submitted.swap(true, Ordering::Relaxed) {
                    row.resubmitted_lists += 1;
                } else {
                    row.translation += s.timing.elapsed;
                    if ready {
                        row.reuse_candidate_lists += 1;
                        row.reuse_candidate_work += s.timing.elapsed;
                    } else if repeated {
                        row.prior_unready_lists += 1;
                        row.prior_unready_work += s.timing.elapsed;
                    }
                }
                row.split_chunks += s.timing.pieces;
                row.root_bindings += s.timing.bindings;
            }
            reports.push(row);
        }
        reports
    }
}

#[derive(Default)]
struct Report {
    frame: u64,
    lists: Vec<(usize, u128)>,
    repeated: usize,
    translation: Duration,
    reuse_candidate_lists: usize,
    reuse_candidate_work: Duration,
    prior_unready_lists: usize,
    prior_unready_work: Duration,
    resubmitted_lists: usize,
    split_chunks: usize,
    root_bindings: usize,
}

impl Report {
    fn sizes(&self) -> (usize, f64) {
        let mut sizes: Vec<_> = self.lists.iter().map(|l| l.0).collect();
        sizes.sort_unstable();
        let n = sizes.len();
        match sizes.last() {
            Some(&max) => (max, (sizes[(n - 1) / 2] as f64 + sizes[n / 2] as f64) / 2.0),
            None => (0, 0.0),
        }
    }

    fn log(self) {
        let (max_words, median_words) = self.sizes();
        let n = self.lists.len();
        tracing::info!(frame = self.frame, lists = n, max_words, median_words,
            repeated = self.repeated, repeat_pct = 100.0 * self.repeated as f64 / n.max(1) as f64,
            translation_worker_ms = self.translation.as_secs_f64() * 1e3,
            reuse_candidate_lists = self.reuse_candidate_lists,
            reuse_candidate_worker_ms = self.reuse_candidate_work.as_secs_f64() * 1e3,
            prior_unready_lists = self.prior_unready_lists,
            prior_unready_worker_ms = self.prior_unready_work.as_secs_f64() * 1e3,
            resubmitted_lists = self.resubmitted_lists,
            split_chunks = self.split_chunks, root_bindings = self.root_bindings,
            words_and_translation_us = ?self.lists, "THIN submitted frame (VICTORY_THIN_STATS)");
    }
}

// Only send tickets and frame boundaries while the queue holds its lock.
// Hash history, sorting and log I/O belong to this optional observer.
enum Event {
    Lists(Vec<Ticket>),
    Present(u64),
}

fn observe(events: mpsc::Receiver<Event>) {
    let interval = Duration::from_secs(5);
    let mut next = Instant::now() + interval;
    let mut frames = Frames::default();
    loop {
        match events.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(Event::Lists(tickets)) => frames.current.extend(tickets),
            Ok(Event::Present(frame)) => {
                frames
                    .pending
                    .push_back((frame, std::mem::take(&mut frames.current)));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= next {
            for report in frames.take_ready() {
                report.log();
            }
            next = Instant::now() + interval;
        }
    }
}

fn sender() -> Option<&'static mpsc::Sender<Event>> {
    static SENDER: OnceLock<Option<mpsc::Sender<Event>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            if !on() {
                return None;
            }
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("victory-thin-stats".into())
                .spawn(move || observe(rx))
            {
                Ok(_) => Some(tx),
                Err(e) => {
                    tracing::warn!(error = %e, "could not start THIN statistics");
                    None
                }
            }
        })
        .as_ref()
}

pub(crate) fn submitted(lists: &[Arc<Recording>]) {
    if let Some(tx) = sender() {
        let tickets = lists.iter().filter_map(|r| r.sample.clone()).collect();
        let _ = tx.send(Event::Lists(tickets));
    }
}

pub(crate) fn present(frame: u64) {
    if super::wanted()
        && let Some(tx) = sender()
    {
        let _ = tx.send(Event::Present(frame));
    }
}

#[cfg(test)]
mod tests;
