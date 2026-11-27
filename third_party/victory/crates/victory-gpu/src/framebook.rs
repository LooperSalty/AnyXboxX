//! The book of frames (`VICTORY_FRAME_BOOK=<ms>`; off by default, every
//! build): where the time of each frame went, with a line in the log for
//! each frame longer than `<ms>` beside the median of the frames before it,
//! what grew most first; with `VICTORY_FRAME_BOOK_FILE=<file>`, every frame
//! as a row of that file (CSV).
//!
//! The frame-time line of every 5 s counts the frames over 50 ms; their
//! parts were known only summed over seconds, among the frames of 33 ms
//! around them.
//!
//! A frame here runs from the end of one present by the thread that serves
//! the line of submissions (`crate::queue::behind`) to the end of the next:
//! what that thread did (sending submissions, the present, the rest), what
//! it waited for (nothing in the line; with entries in it, by what ended
//! the wait: lists translated, a memory value written, one a submission
//! sent writes, something free to run that came in), the parts of its
//! preparing (the split of `crate::host`), the title's side (when it called
//! the present, how long it waited there, how long it slept in its frame
//! waits) and what other threads did meanwhile (the relay's landing, the
//! heap pass, the translating of lists and how long after their submission
//! they were translated, scratch chunks and upload buffers made, the card's
//! idle time). A present reads some forty counters that only grow; the rows
//! are written and the lines made by a thread of the book's own.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What is counted on its way (microseconds that only grow; `Runs` a count).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Part {
    /// The line's thread sent submissions (preparing them).
    Send = 0,
    /// It presented.
    Show,
    /// It did the rest (signals, waits that were met).
    Other,
    /// It waited with nothing in the line.
    Idle,
    /// It waited with entries in the line, and lists being translated that a
    /// queue had first were translated.
    Translating,
    /// It waited with entries in the line, and something free to run came
    /// in (or a fence was reached).
    Standing,
    /// The title slept in its frame waits.
    Paced,
    /// Submissions sent.
    Runs,
    /// It waited with entries in the line, and a memory value a queue had
    /// first came to be in memory (the title's processor wrote it).
    StandMemory,
    /// It waited with entries in the line, and a memory value a queue had
    /// first is written by a submission sent meanwhile.
    StandFlight,
}

const PARTS: usize = 10;
static COUNTS: [AtomicU64; PARTS] = [const { AtomicU64::new(0) }; PARTS];

/// Of the lists translated since the last present: how long after its
/// submission the last of them was translated to its end, at most
/// (microseconds), and how many were translated after their submission.
static LATE_LISTS: Mutex<[u64; 2]> = Mutex::new([0; 2]);

/// Fence registrations completed in the interval. Snapshot all three fields
/// together even when several queues signal on different threads.
#[derive(Default)]
struct EventWaits {
    longest_us: u64,
    total_us: u64,
    count: u64,
}

impl EventWaits {
    fn record(&mut self, us: u64) {
        self.longest_us = self.longest_us.max(us);
        self.total_us += us;
        self.count += 1;
    }

    fn columns(self) -> [f32; 3] {
        [self.longest_us as f32 / 1e3, self.total_us as f32 / 1e3, self.count as f32]
    }
}

static EVENTS: Mutex<EventWaits> = Mutex::new(EventWaits { longest_us: 0, total_us: 0, count: 0 });

/// Called only for a registration armed while the book was enabled.
pub(crate) fn title_event_waited(us: u64) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).record(us);
}

/// Microseconds since the book's clock began.
pub(crate) fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// A list submitted by the title was translated to its end `us`
/// microseconds after its submission.
pub(crate) fn translated_after(us: u64) {
    if on() {
        let mut late = LATE_LISTS.lock().unwrap_or_else(|e| e.into_inner());
        late[0] = late[0].max(us);
        late[1] += 1;
    }
}

/// The columns of a row, in milliseconds but for those [`COUNTED`].
const COLUMNS: [&str; 41] = [
    "frame",
    "title_gap",
    "lag",
    "title_wait",
    "paced",
    "lane_send",
    "lane_show",
    "lane_other",
    "lane_idle",
    "lane_translating",
    "lane_standing",
    "lane_stand_memory",
    "lane_stand_flight",
    "runs",
    "ask",
    "under",
    "steps",
    "refreshes",
    "copies",
    "refresh_front",
    "preludes",
    "card_copies",
    "send_lists",
    "lock",
    "plan",
    "flights",
    "heaps",
    "heaps_end",
    "card_idle",
    "card_stood",
    "landing",
    "heap_pass",
    "translate",
    "draws",
    "lists_late_most",
    "lists_late",
    "chunks_made",
    "uploads_made",
    "events_most",
    "events_all",
    "events",
];
const COUNTED: [usize; 6] = [13, 33, 35, 36, 37, 40];

type Row = [f32; COLUMNS.len()];

/// `VICTORY_FRAME_BOOK=<ms>` and `VICTORY_FRAME_BOOK_FILE`, read once.
fn asked() -> Option<&'static (f32, Option<String>)> {
    static ASKED: OnceLock<Option<(f32, Option<String>)>> = OnceLock::new();
    ASKED
        .get_or_init(|| {
            let spec = std::env::var("VICTORY_FRAME_BOOK").ok()?;
            let Some(ms) = parse(&spec) else {
                tracing::warn!(spec, "VICTORY_FRAME_BOOK: expected milliseconds above 0; no book of frames");
                return None;
            };
            let file = std::env::var("VICTORY_FRAME_BOOK_FILE").ok().map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
            Some((ms, file))
        })
        .as_ref()
}

/// `50` → 50 ms; not a number, or not above 0 → `None`.
fn parse(spec: &str) -> Option<f32> {
    spec.trim().parse::<f32>().ok().filter(|ms| ms.is_finite() && *ms > 0.0)
}

/// Whether the book is kept.
#[inline]
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| asked().is_some())
}

/// Counts `us` microseconds (or one, for a count) of `part`.
#[inline]
pub(crate) fn add(part: Part, us: u64) {
    if on() {
        COUNTS[part as usize].fetch_add(us, Relaxed);
    }
}

/// What each of `counters` grew by since the last call with the same `last`
/// (for counters that only grow and are read by more than one reader).
pub(crate) fn grown<const N: usize>(counters: &[AtomicU64; N], last: &Mutex<[u64; N]>) -> [u64; N] {
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    std::array::from_fn(|i| {
        let now = counters[i].load(Relaxed);
        now.wrapping_sub(std::mem::replace(&mut last[i], now))
    })
}

/// Existing log counters reset as usual when there is no second reader.
pub(crate) fn for_line<const N: usize>(counters: &[AtomicU64; N], last: &Mutex<[u64; N]>) -> [u64; N] {
    if on() {
        grown(counters, last)
    } else {
        counters.each_ref().map(|n| n.swap(0, Relaxed))
    }
}

/// The counters a row is made of, as they are now: [`COUNTS`], then the
/// others in the order of [`COLUMNS`] from `ask` on (nanoseconds for the
/// split, microseconds for the rest, draws a count).
fn counters() -> [u64; PARTS + 20] {
    let mut out = [0; PARTS + 20];
    for (out, n) in out.iter_mut().zip(&COUNTS) {
        *out = n.load(Relaxed);
    }
    let (split, card, landing) = crate::host::book_counters();
    // The split's parts in the order of the columns (`crate::host::split_line`).
    for (k, i) in [0, 1, 2, 6, 7, 16, 17, 4, 5, 11, 12, 13, 14, 15].into_iter().enumerate() {
        out[PARTS + k] = split[i];
    }
    out[PARTS + 14] = card[0];
    out[PARTS + 15] = card[2];
    out[PARTS + 16] = landing;
    out[PARTS + 17] = crate::draw::HEAP_BUFFER_MICROS.load(Relaxed);
    out[PARTS + 18] = crate::list::thin::translated_us();
    out[PARTS + 19] = crate::draw::drawn();
    out
}

struct Last {
    /// The end of the last present, and the title's call of it.
    present: Instant,
    called: Instant,
    counters: [u64; PARTS + 20],
    /// Scratch chunks and upload buffers made so far.
    made: [u64; 2],
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

/// The thread that serves the line has presented a frame the title asked
/// for at `called`: its row.
pub(crate) fn presented(called: Instant, waited: Option<std::sync::mpsc::Receiver<u64>>) {
    if !on() {
        return;
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let counters = counters();
    let [late_most, late] = std::mem::take(&mut *LATE_LISTS.lock().unwrap_or_else(|e| e.into_inner()));
    let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
    let made = [
        crate::host::CHUNKS_MADE.load(Relaxed),
        crate::host::MADE[0].load(Relaxed),
    ];
    let Some(was) = last.replace(Last { present: now, called, counters, made }) else {
        tracing::info!("the book of frames begins (VICTORY_FRAME_BOOK): its rows count from here");
        return;
    };
    let ms = |from: Instant, to: Instant| to.saturating_duration_since(from).as_secs_f32() * 1e3;
    let mut row: Row = [0.0; COLUMNS.len()];
    const FIRST_SPLIT: usize = 14; // ask
    row[0] = ms(was.present, now);
    row[1] = ms(was.called, called);
    row[2] = ms(called, now);
    // Synchronous presents block the title for the whole call.
    row[3] = row[2];
    let grew = |i: usize| counters[i].wrapping_sub(was.counters[i]) as f32;
    for i in 0..PARTS {
        // The columns of the parts, in the order of [`Part`].
        let column = [5, 6, 7, 8, 9, 10, 4, 13, 11, 12][i];
        row[column] = if i == Part::Runs as usize { grew(i) } else { grew(i) / 1e3 };
    }
    for k in 0..20 {
        let column = FIRST_SPLIT + k;
        let n = grew(PARTS + k);
        row[column] = if COUNTED.contains(&column) {
            n
        } else if k < 14 {
            n / 1e6
        } else {
            n / 1e3
        };
    }
    row[34] = late_most as f32 / 1e3;
    row[35] = late as f32;
    row[36] = made[0].wrapping_sub(was.made[0]) as f32;
    row[37] = made[1].wrapping_sub(was.made[1]) as f32;
    row[38..41].copy_from_slice(&events.columns());
    if let Some(writer) = writer()
        && writer.send((now, row, waited)).is_err()
    {
        tracing::warn!("frame book writer disconnected");
    }
}

type Entry = (Instant, Row, Option<std::sync::mpsc::Receiver<u64>>);

fn writer() -> Option<&'static std::sync::mpsc::Sender<Entry>> {
    static WRITER: OnceLock<Option<std::sync::mpsc::Sender<Entry>>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (to, from) = std::sync::mpsc::channel();
        let (slow, file) = asked()?.clone();
        let started = Instant::now();
        match std::thread::Builder::new().name("victory-frame-book".into()).spawn(move || write(from, slow, file, started)) {
            Ok(_) => Some(to),
            Err(error) => {
                tracing::warn!(%error, "could not start frame book writer");
                None
            }
        }
    }).as_ref()
}

/// Frames the median is taken over.
const AROUND: usize = 150;

/// The book's thread: rows into the file, and a line for each slow frame.
fn write(from: std::sync::mpsc::Receiver<Entry>, slow: f32, file: Option<String>, started: Instant) {
    use std::io::Write as _;
    let mut out = file.as_deref().and_then(|path| match std::fs::File::create(path) {
        Ok(f) => Some(std::io::BufWriter::new(f)),
        Err(e) => {
            tracing::warn!(path, error = %e, "VICTORY_FRAME_BOOK: the file was not made; lines only");
            None
        }
    });
    if let Some(file) = &mut out
        && let Err(error) = writeln!(file, "at_s,{}", COLUMNS.join(",")) {
        tracing::warn!(%error, "could not write frame book header");
        out = None;
    }
    let mut before = std::collections::VecDeque::with_capacity(AROUND);
    for (at, mut row, waited) in from {
        if let Some(waited) = waited {
            let Ok(us) = waited.recv() else {
                tracing::warn!("present ended without a title wait measurement");
                continue;
            };
            row[3] = us as f32 / 1e3;
        }
        if let Some(file) = &mut out {
            let mut line = format!("{:.3}", at.saturating_duration_since(started).as_secs_f64());
            for v in row {
                line.push_str(&format!(",{v:.3}"));
            }
            if let Err(error) = writeln!(file, "{line}").and_then(|_| file.flush()) {
                tracing::warn!(%error, "could not write frame book row");
                out = None;
            }
        }
        if row[0] > slow {
            if before.is_empty() {
                tracing::info!("a frame of {:.1} ms (VICTORY_FRAME_BOOK; first interval, no baseline)", row[0]);
            } else {
                tracing::info!("{}", slow_line(&row, &before));
            }
        }
        if before.len() == AROUND {
            before.pop_front();
        }
        before.push_back(row);
    }
}

/// The line of a slow frame: its parts beside their medians over `before`,
/// what grew most first.
fn slow_line(row: &Row, before: &std::collections::VecDeque<Row>) -> String {
    let median = |i: usize| {
        let mut v: Vec<f32> = before.iter().map(|r| r[i]).collect();
        v.sort_unstable_by(f32::total_cmp);
        (v[(v.len() - 1) / 2] + v[v.len() / 2]) * 0.5
    };
    let medians: Vec<f32> = (0..COLUMNS.len()).map(median).collect();
    let mut parts: Vec<(usize, f32)> = (1..COLUMNS.len()).map(|i| (i, row[i] - medians[i])).collect();
    // Times by how much they grew; counts last.
    parts.sort_by(|a, b| {
        COUNTED.contains(&a.0).cmp(&COUNTED.contains(&b.0)).then(b.1.total_cmp(&a.1))
    });
    let shown: Vec<String> = parts
        .iter()
        .filter(|(i, d)| COUNTED.contains(i) || d.abs() >= 0.5)
        .map(|&(i, _)| {
            if COUNTED.contains(&i) {
                format!("{} {:.0} ({:.0})", COLUMNS[i], row[i], medians[i])
            } else {
                format!("{} {:.1} ({:.1})", COLUMNS[i], row[i], medians[i])
            }
        })
        .collect();
    format!(
        "a frame of {:.1} ms (VICTORY_FRAME_BOOK; the median of the {} before it {:.1}): the parts that changed by half a ms \
         or more, ms (median), what grew most first: {}",
        row[0],
        before.len(),
        medians[0],
        shown.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_events_report_max_total_and_count_in_the_completion_interval() {
        let mut waits = EventWaits::default();
        for us in [1500, 5000, 0] {
            waits.record(us);
        }
        assert_eq!(std::mem::take(&mut waits).columns(), [5.0, 6.5, 3.0]);
        assert_eq!(waits.columns(), [0.0; 3]);
        assert_eq!(&COLUMNS[38..], ["events_most", "events_all", "events"]);
        assert!(COUNTED.contains(&40));
    }

    #[test]
    fn the_switch_reads_a_threshold() {
        assert_eq!(parse("50"), Some(50.0));
        assert_eq!(parse(" 40.5 "), Some(40.5));
        assert_eq!(parse("0"), None);
        assert_eq!(parse("-3"), None);
        assert_eq!(parse("slow"), None);
    }

    #[test]
    fn csv_pairs_each_present_with_its_own_title_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frames.csv");
        let (send, recv) = std::sync::mpsc::channel();
        let start = Instant::now();
        for (ms, us) in [(30, 2400), (96, 850)] {
            let (to, from) = std::sync::mpsc::channel();
            let mut row = [0.0; COLUMNS.len()];
            row[0] = ms as f32;
            send.send((start + std::time::Duration::from_millis(ms), row, Some(from))).unwrap();
            to.send(us).unwrap();
        }
        drop(send);
        write(recv, 50.0, Some(path.to_str().unwrap().into()), start);
        let csv = std::fs::read_to_string(path).unwrap();
        let lines: Vec<_> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], format!("at_s,{}", COLUMNS.join(",")));
        assert!(lines[1].starts_with("0.030,30.000,0.000,0.000,2.400,"));
        assert!(lines[2].starts_with("0.096,96.000,0.000,0.000,0.850,"));
    }

    #[test]
    fn counters_grow_for_each_reader_apart() {
        static C: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
        static A: Mutex<[u64; 2]> = Mutex::new([0; 2]);
        static B: Mutex<[u64; 2]> = Mutex::new([0; 2]);
        C[0].fetch_add(5, Relaxed);
        assert_eq!(grown(&C, &A), [5, 0]);
        C[1].fetch_add(3, Relaxed);
        assert_eq!(grown(&C, &A), [0, 3]);
        assert_eq!(grown(&C, &B), [5, 3]);
    }

    #[test]
    fn a_slow_frame_says_what_grew_first() {
        let mut before = std::collections::VecDeque::new();
        for _ in 0..20 {
            let mut r: Row = [0.0; COLUMNS.len()];
            r[0] = 33.0;
            r[5] = 20.0;
            r[8] = 10.0;
            before.push_back(r);
        }
        let mut row = before[0];
        row[0] = 60.0;
        row[5] = 45.0;
        row[15] = 3.0;
        let line = slow_line(&row, &before);
        let send = line.find("lane_send 45.0 (20.0)").expect(&line);
        let under = line.find("under 3.0 (0.0)").expect(&line);
        assert!(send < under, "{line}");
        assert!(line.contains("a frame of 60.0 ms"), "{line}");
        assert!(!line.contains("lane_idle"), "{line}");
    }
}
