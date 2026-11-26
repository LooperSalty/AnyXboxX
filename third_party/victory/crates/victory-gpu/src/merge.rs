//! (Neptune) Title submissions merged into fewer host submissions
//! (`VICTORY_MERGE_SUBMISSIONS`, see [`level`]).
//!
//! What is decided here, without the host: whether a title submission may
//! join the ones held back so far ([`Plan::refuses`]), and what each call of
//! the title's that could observe them does to the held ones ([`Plan::signal`],
//! [`Plan::value_wait`], [`Plan::carries`]). `crate::host` holds the lists
//! themselves, sends them as one submission when a reason comes ([`Why`]),
//! and counts it for the frame line ([`line`]).

use std::sync::atomic::{AtomicU64, Ordering};

/// (Neptune) Whether a queue's consecutive submissions, sent ahead of the card
/// (`crate::host::late`), are held back and sent together as one host
/// submission (`VICTORY_MERGE_SUBMISSIONS=1`, or `=2`; off by default and off
/// without `crate::host::late`).
///
/// The thread that submits for the title was the frame's limit: some 59 host
/// submissions a frame, each paying the same fixed work whatever it held (the
/// pass over descriptors copied into placed heaps, the write-watch asking for
/// its refreshes, the pass over every copy in the card's memory, the joins of
/// the refresh threads, a list's closing and resetting, a send and a signal,
/// and a flight for the thread that lands them). With the switch a title
/// submission joins those held back since the last send, and they go to the
/// card as one submission, their lists in the order the title gave them, when
/// one of these comes ([`Why`]):
///
/// 1. a fence signal on any queue (with `=2` the signal is carried behind the
///    held submissions instead, and set when they have run: then the title
///    asking about that fence, or a queue told to wait for it, sends them);
/// 2. a queue stands at a wait for a memory value that memory does not hold
///    and a held submission may write there (a bottom-of-pipe write of another
///    value or width, a range its shaders may write, a carried signal's
///    fence); a bottom-of-pipe write of a held submission that meets the wait
///    lets the part behind the wait join instead, behind that write;
/// 3. a present, a synchronous submission of ours (a read-back, a map, an
///    upload run at once), a settle of the flights (a blocking fence wait),
///    and the title's page mappings on a queue;
/// 4. the next submission cannot be prepared after the held ones without a
///    change of what it or they see ([`Plan::refuses`]): it reads title memory
///    the held ones' shaders may write, it copies over title memory a held one
///    brings to the card, or it runs code of ours on title memory; or it would
///    wait for flights in the air on its own, and the held ones with it;
/// 5. the held submissions reach [`most`] lists or submissions;
/// 6. the first of them was held [`window`] ago (a thread of ours sends them):
///    a title that polls a value in memory without a fence sees it at most
///    that much later.
///
/// What it assumes beyond `crate::host::late`: a held submission is prepared
/// when it is sent, not when the title gave it, so it sees title memory as it
/// is then (as the parts a queue holds behind a wait always did). Between the
/// lists of two title submissions a barrier list waits for everything before
/// it (`Host::submit`), as the end of a host submission did.
pub(crate) fn level() -> u8 {
    static LEVEL: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| match std::env::var("VICTORY_MERGE_SUBMISSIONS").as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    })
}

/// The longest a held submission waits for the others before a thread of
/// ours sends them (`VICTORY_MERGE_SUBMISSIONS_US`, 500 microseconds).
pub(crate) fn window() -> std::time::Duration {
    static US: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    std::time::Duration::from_micros(*US.get_or_init(|| {
        std::env::var("VICTORY_MERGE_SUBMISSIONS_US").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(500).max(1)
    }))
}

/// The most host lists (`VICTORY_MERGE_SUBMISSIONS_LISTS`, 256) and title
/// submissions (`VICTORY_MERGE_SUBMISSIONS_MOST`, 32) held back together.
pub(crate) fn most() -> Most {
    static MOST: std::sync::OnceLock<Most> = std::sync::OnceLock::new();
    *MOST.get_or_init(|| {
        let read = |name: &str, usual: usize| std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(usual).max(1);
        Most {
            lists: read("VICTORY_MERGE_SUBMISSIONS_LISTS", 256),
            members: read("VICTORY_MERGE_SUBMISSIONS_MOST", 32),
        }
    })
}

/// How much is held back together at most ([`most`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Most {
    pub(crate) lists: usize,
    pub(crate) members: usize,
}

/// Why held submissions were sent: the places of [`STATS`] after the first
/// two, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    /// A fence signal on a queue.
    Signal,
    /// The title asked about a fence whose signal is carried, or a queue was
    /// told to wait for it (`=2`).
    Fence,
    /// A queue stands at a wait for a memory value a held submission may write.
    Value,
    Present,
    /// A synchronous submission of ours.
    Sync,
    /// The flights in the air were settled (a blocking fence wait, or another
    /// reason of `crate::host::settle_now`).
    Settle,
    /// The title's page mappings on a queue.
    Mappings,
    /// The next submission reads title memory the held ones' shaders may
    /// write, or flights' in the air (on its own it would wait for those, and
    /// the held ones would wait with it).
    Reads,
    /// The next submission copies over title memory a held one brings to the card.
    CopyOver,
    /// The next submission runs code of ours on title memory.
    Code,
    /// [`most`].
    Most,
    /// [`window`].
    Timer,
    /// Bottom-of-pipe writes with `VICTORY_LATE_WAIT_WRITES=0`: told at once, as before.
    Writes,
}

const WHYS: usize = 13;

/// Since the last frame-rate line: title submissions given to [`level`]'s
/// merging, host submissions sent for them, the sends by [`Why`], waits for
/// memory values met by a held submission's bottom-of-pipe write, fence
/// signals carried behind held submissions.
static STATS: [AtomicU64; 4 + WHYS] = [const { AtomicU64::new(0) }; 4 + WHYS];

const OFFERED: usize = 0;
const SENT: usize = 1;
const MET: usize = 2 + WHYS;
const CARRIED: usize = 3 + WHYS;

/// A title submission was given to the merging.
pub(crate) fn offered() {
    STATS[OFFERED].fetch_add(1, Ordering::Relaxed);
}

/// Held submissions went to the card as one, for `why`.
pub(crate) fn sent(why: Why) {
    STATS[SENT].fetch_add(1, Ordering::Relaxed);
    STATS[2 + why as usize].fetch_add(1, Ordering::Relaxed);
}

/// A wait for a memory value was met by a held submission's write.
pub(crate) fn met() {
    STATS[MET].fetch_add(1, Ordering::Relaxed);
}

/// A fence signal was carried behind held submissions.
pub(crate) fn carried() {
    STATS[CARRIED].fetch_add(1, Ordering::Relaxed);
}

/// For the frame-rate line: what the merging did per frame since the last one.
pub(crate) fn line(frames: f64) {
    if level() == 0 {
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|s| s.swap(0, Ordering::Relaxed) as f64 / frames).collect();
    let why = |w: Why| s[2 + w as usize];
    let most = most();
    tracing::info!(
        "title submissions merged per frame (VICTORY_MERGE_SUBMISSIONS={}): {:.1} title submissions went to the card in {:.1} \
         host submissions ({:.2} each); sent for: a fence signal {:.1}, a fence asked about {:.1}, a wait for a memory value {:.1}, \
         a present {:.1}, a synchronous submission of ours {:.1}, a settle {:.1}, page mappings {:.1}, a submission that reads \
         what held or flying shaders write {:.1}, a copy over what a held submission brings {:.1}, a step of ours on title memory {:.1}, \
         {} lists or {} submissions held {:.1}, the timer ({} us) {:.1}, bottom-of-pipe writes told at once {:.1}; \
         {:.1} waits for memory values met by a held write, {:.1} fence signals carried behind held submissions",
        level(),
        s[OFFERED],
        s[SENT],
        if s[SENT] > 0.0 { s[OFFERED] / s[SENT] } else { 0.0 },
        why(Why::Signal),
        why(Why::Fence),
        why(Why::Value),
        why(Why::Present),
        why(Why::Sync),
        why(Why::Settle),
        why(Why::Mappings),
        why(Why::Reads),
        why(Why::CopyOver),
        why(Why::Code),
        most.lists,
        most.members,
        why(Why::Most),
        window().as_micros(),
        why(Why::Timer),
        why(Why::Writes),
        s[MET],
        s[CARRIED],
    );
}

/// What one title submission's preparing reads and writes, from its lists'
/// submission-time steps (`crate::host`'s `Step`): title addresses, start and
/// end.
#[derive(Clone, Debug, Default)]
pub(crate) struct Touches {
    /// Host lists it runs.
    pub(crate) lists: usize,
    /// Title memory its shaders may write (declared writes and copies on the
    /// card).
    pub(crate) shader: Vec<(usize, usize)>,
    /// Title memory it brings to host copies and staging buffers, which its
    /// lists read on the card (refreshes, textures).
    pub(crate) brought: Vec<(usize, usize)>,
    /// Title memory its preparing reads or writes as it is, which must hold
    /// what earlier shaders wrote (buffer copies' sources and destinations,
    /// fills of upload memory other than constants, textures): what
    /// `crate::host::late_blocked` looks at.
    pub(crate) read: Vec<(usize, usize)>,
    /// Title memory its preparing writes (buffer copies' destinations).
    pub(crate) written: Vec<(usize, usize)>,
    /// Whether a step runs code of ours that may read or write title memory
    /// anywhere.
    pub(crate) code: bool,
    /// Its bottom-of-pipe writes: address, the value (`None`: a timestamp),
    /// bytes.
    pub(crate) writes: Vec<(usize, Option<u64>, usize)>,
}

/// What a queue's fence signal does to held submissions ([`Plan::signal`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Nothing is held: the signal goes on as before.
    Alone,
    /// The held submissions go to the card first.
    Send,
    /// It is carried behind them (`VICTORY_MERGE_SUBMISSIONS=2`).
    Carried,
}

/// What a queue's wait for a memory value that memory does not hold does to
/// held submissions ([`Plan::value_wait`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Nothing held writes there: the wait is looked at as before.
    Alone,
    /// A held bottom-of-pipe write meets it: the part behind it may join them.
    Met,
    /// A held submission may write there in a way not known here: they go to
    /// the card first, and the wait is looked at as before.
    Send,
}

/// The decision side of the submissions held back so far: what they touch,
/// joined, and the signals carried behind them.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    members: usize,
    lists: usize,
    /// [`Touches::shader`] of all of them, in order and joined.
    shader: Vec<(usize, usize)>,
    /// [`Touches::brought`] of all of them, in order and joined.
    brought: Vec<(usize, usize)>,
    writes: Vec<(usize, Option<u64>, usize)>,
    /// Fence (its address), value, and where its value is (address, bytes).
    signals: Vec<(usize, u64, (usize, usize))>,
}

impl Plan {
    pub(crate) const fn new() -> Plan {
        Plan {
            members: 0,
            lists: 0,
            shader: Vec::new(),
            brought: Vec::new(),
            writes: Vec::new(),
            signals: Vec::new(),
        }
    }

    /// Whether nothing is held.
    pub(crate) fn is_empty(&self) -> bool {
        self.members == 0
    }

    pub(crate) fn lists(&self) -> usize {
        self.lists
    }

    /// Why `next` cannot join the held submissions, if so: prepared after
    /// them and sent with them, it or they would see title memory otherwise
    /// than sent on its own behind them.
    pub(crate) fn refuses(&self, next: &Touches, most: Most) -> Option<Why> {
        if self.members == 0 {
            return None;
        }
        if self.members >= most.members || self.lists + next.lists > most.lists {
            return Some(Why::Most);
        }
        // Code of ours may read what the held shaders write, or write what the
        // held lists read on the card, anywhere.
        if next.code {
            return Some(Why::Code);
        }
        // On its own it would wait until what the held shaders wrote is in
        // title memory (`crate::host::late_blocked`); prepared with them it
        // would read the bytes from before.
        if next.read.iter().any(|&(from, to)| touches(&self.shader, from, to)) {
            return Some(Why::Reads);
        }
        // On its own its copy reached the card after the held lists (their
        // bytes went up ahead of them); prepared with them, a later refresh of
        // the same bytes would send the copy's result up ahead of all of them.
        if next.written.iter().any(|&(from, to)| touches(&self.brought, from, to)) {
            return Some(Why::CopyOver);
        }
        None
    }

    /// Counts a submission whose touches are taken later ([`Plan::learn`]).
    pub(crate) fn count(&mut self, lists: usize) {
        self.members += 1;
        self.lists += lists;
    }

    /// Takes in what a counted submission touches.
    pub(crate) fn learn(&mut self, touches: Touches) {
        join(&mut self.shader, touches.shader);
        join(&mut self.brought, touches.brought);
        self.writes.extend(touches.writes);
    }

    /// [`Plan::count`] and [`Plan::learn`].
    pub(crate) fn take(&mut self, touches: Touches) {
        self.count(touches.lists);
        self.learn(touches);
    }

    /// A queue signals `fence` (its value at `place`) with `value`, at
    /// [`level`] `level`: with submissions held, they go to the card first,
    /// or (`=2`) the signal is carried behind them.
    pub(crate) fn signal(&mut self, level: u8, fence: usize, value: u64, place: (usize, usize)) -> Signal {
        if self.members == 0 {
            return Signal::Alone;
        }
        if level >= 2 {
            self.signals.push((fence, value, place));
            return Signal::Carried;
        }
        Signal::Send
    }

    /// A queue stands at a wait for the value at `place` (address, bytes),
    /// which memory does not hold now; `holds` says whether a value meets it.
    /// As `crate::host::late_met` does for the flights in the air: a held
    /// bottom-of-pipe write of a value that meets it, at its address and as
    /// wide, says yes; any other held write there, a range held shaders may
    /// write, or a carried signal's fence there is not known here.
    pub(crate) fn value_wait(&self, (address, bytes): (usize, usize), holds: impl Fn(u64) -> bool) -> Wait {
        if self.members == 0 {
            return Wait::Alone;
        }
        let end = address.saturating_add(bytes);
        let mut unsure = touches(&self.shader, address, end)
            || self.signals.iter().any(|&(_, _, (at, n))| at < end && address < at + n);
        for &(at, value, n) in &self.writes {
            if at >= end || address >= at + n {
                continue;
            }
            match value {
                Some(v) if at == address && n >= bytes => {
                    if holds(v) {
                        return Wait::Met;
                    }
                }
                _ => unsure = true,
            }
        }
        if unsure { Wait::Send } else { Wait::Alone }
    }

    /// Whether a signal of `fence` is carried behind the held submissions.
    pub(crate) fn carries(&self, fence: usize) -> bool {
        self.signals.iter().any(|s| s.0 == fence)
    }

    /// The held submissions are sent: the signals carried behind them (fence,
    /// value), in order, and nothing is held.
    pub(crate) fn clear(&mut self) -> Vec<(usize, u64)> {
        let signals = self.signals.drain(..).map(|(fence, value, _)| (fence, value)).collect();
        *self = Plan::new();
        signals
    }
}

/// Whether `[from, to)` touches one of `ranges` (in order, joined).
fn touches(ranges: &[(usize, usize)], from: usize, to: usize) -> bool {
    if from >= to {
        return false;
    }
    let at = ranges.partition_point(|r| r.1 <= from);
    ranges.get(at).is_some_and(|r| r.0 < to)
}

/// `more` into `into` (in order, joined), joined where they overlap or touch:
/// `more` is sorted, the two are walked once.
fn join(into: &mut Vec<(usize, usize)>, mut more: Vec<(usize, usize)>) {
    more.retain(|r| r.0 < r.1);
    if more.is_empty() {
        return;
    }
    more.sort_unstable();
    let old = std::mem::take(into);
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(old.len() + more.len());
    let (mut a, mut b) = (old.into_iter().peekable(), more.into_iter().peekable());
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x <= y => a.next(),
            (Some(_), Some(_)) => b.next(),
            (Some(_), None) => a.next(),
            (None, Some(_)) => b.next(),
            (None, None) => break,
        };
        let Some((from, to)) = next else { break };
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    *into = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOST: Most = Most { lists: 256, members: 32 };

    /// A submission of `lists` lists that touches nothing.
    fn plain(lists: usize) -> Touches {
        Touches { lists, ..Default::default() }
    }

    /// One thing the title does, as the merging sees it.
    enum Call {
        Submit(&'static str, Touches),
        Signal(usize, u64),
        Present,
    }

    /// What reaches the card for `calls` at `level`, as `crate::host` drives
    /// the plan: the host submissions (the title submissions in each) and why
    /// each was sent. Level 0 is the path without the switch: each title
    /// submission is a host submission of its own.
    fn sends(level: u8, calls: Vec<Call>) -> Vec<(Vec<&'static str>, Why)> {
        type Sent = Vec<(Vec<&'static str>, Why)>;
        fn send(plan: &mut Plan, held: &mut Vec<&'static str>, out: &mut Sent, why: Why) {
            if !held.is_empty() {
                out.push((std::mem::take(held), why));
                plan.clear();
            }
        }
        let mut plan = Plan::new();
        let mut held: Vec<&'static str> = Vec::new();
        let mut out: Sent = Vec::new();
        for call in calls {
            match call {
                Call::Submit(name, touches) => {
                    if level == 0 {
                        out.push((vec![name], Why::Sync));
                        continue;
                    }
                    if let Some(why) = plan.refuses(&touches, MOST) {
                        send(&mut plan, &mut held, &mut out, why);
                    }
                    plan.take(touches);
                    held.push(name);
                }
                Call::Signal(fence, value) => {
                    if level > 0 && plan.signal(level, fence, value, (fence, 8)) == Signal::Send {
                        send(&mut plan, &mut held, &mut out, Why::Signal);
                    }
                }
                Call::Present => send(&mut plan, &mut held, &mut out, Why::Present),
            }
        }
        send(&mut plan, &mut held, &mut out, Why::Timer);
        out
    }

    /// Submissions with nothing between them reach the card as one; a fence
    /// signal ends the run, and so does a present. Without the switch each is
    /// a host submission of its own.
    #[test]
    fn consecutive_submissions_reach_the_card_as_one() {
        let calls = || {
            vec![
                Call::Submit("a", plain(3)),
                Call::Submit("b", plain(1)),
                Call::Submit("c", plain(2)),
                Call::Signal(0x100, 1),
                Call::Submit("d", plain(1)),
                Call::Submit("e", plain(1)),
                Call::Present,
                Call::Submit("f", plain(1)),
            ]
        };
        let merged = sends(1, calls());
        assert_eq!(
            merged,
            [
                (vec!["a", "b", "c"], Why::Signal),
                (vec!["d", "e"], Why::Present),
                (vec!["f"], Why::Timer)
            ]
        );
        // The path without the switch: one host submission per title submission.
        let alone = sends(0, calls());
        assert_eq!(alone.len(), 6);
        assert!(merged.len() < alone.len());
        // With signals carried (`=2`) the signal does not end the run.
        let carried = sends(2, calls());
        assert_eq!(carried, [(vec!["a", "b", "c", "d", "e"], Why::Present), (vec!["f"], Why::Timer)]);
    }

    /// A submission that reads what the held shaders may write is not
    /// prepared with them: on its own it waited for their results.
    #[test]
    fn a_read_of_what_held_shaders_write_is_not_merged() {
        let mut plan = Plan::new();
        plan.take(Touches { lists: 1, shader: vec![(0x1000, 0x2000)], ..Default::default() });
        // A buffer copy reading inside the range, a texture over its end.
        for read in [(0x1800, 0x1810), (0x1ff0, 0x3000), (0x0, 0x1001)] {
            let next = Touches { lists: 1, read: vec![read], ..Default::default() };
            assert_eq!(plan.refuses(&next, MOST), Some(Why::Reads), "{read:x?}");
        }
        // Right after the range, or before it: no.
        for read in [(0x2000, 0x2100), (0x0, 0x1000)] {
            let next = Touches { lists: 1, read: vec![read], ..Default::default() };
            assert_eq!(plan.refuses(&next, MOST), None, "{read:x?}");
        }
        // Shaders of the next one writing the same range run after the held
        // ones on the card, in order: no reason to send.
        let next = Touches { lists: 1, shader: vec![(0x1000, 0x2000)], brought: vec![(0x1000, 0x2000)], ..Default::default() };
        assert_eq!(plan.refuses(&next, MOST), None);
    }

    /// A copy over title memory a held submission brings to the card is not
    /// prepared with it; a copy elsewhere is.
    #[test]
    fn a_copy_over_what_a_held_submission_reads_is_not_merged() {
        let mut plan = Plan::new();
        plan.take(Touches { lists: 2, brought: vec![(0x10_000, 0x20_000), (0x40_000, 0x50_000)], ..Default::default() });
        let copy = |dst: (usize, usize)| Touches { lists: 1, read: vec![dst], written: vec![dst], ..Default::default() };
        assert_eq!(plan.refuses(&copy((0x18_000, 0x18_100)), MOST), Some(Why::CopyOver));
        assert_eq!(plan.refuses(&copy((0x4f_ff0, 0x50_010)), MOST), Some(Why::CopyOver));
        assert_eq!(plan.refuses(&copy((0x30_000, 0x30_100)), MOST), None);
        // Code of ours on title memory: never with others.
        let code = Touches { lists: 1, code: true, ..Default::default() };
        assert_eq!(plan.refuses(&code, MOST), Some(Why::Code));
        // Nothing held: anything starts a new run.
        assert_eq!(Plan::new().refuses(&code, MOST), None);
    }

    /// The most lists and submissions held together.
    #[test]
    fn the_held_submissions_stop_at_the_most() {
        let most = Most { lists: 8, members: 3 };
        let mut plan = Plan::new();
        plan.take(plain(4));
        assert_eq!(plan.refuses(&plain(4), most), None);
        assert_eq!(plan.refuses(&plain(5), most), Some(Why::Most));
        plan.take(plain(1));
        plan.take(plain(1));
        assert_eq!(plan.lists(), 6);
        assert_eq!(plan.refuses(&plain(1), most), Some(Why::Most), "three submissions held");
    }

    /// A wait for a memory value: met by a held write of a value that meets
    /// it, sent for by any other held write there, left alone otherwise.
    #[test]
    fn a_value_wait_is_met_sent_for_or_left_alone() {
        let at_least = |reference: u64| move |v: u64| v >= reference;
        let mut plan = Plan::new();
        assert_eq!(Plan::new().value_wait((0x500, 4), at_least(1)), Wait::Alone, "nothing held");
        plan.take(Touches {
            lists: 1,
            shader: vec![(0x9000, 0xa000)],
            writes: vec![(0x500, Some(7), 4), (0x600, None, 8), (0x700, Some(3), 4)],
            ..Default::default()
        });
        assert_eq!(plan.value_wait((0x500, 4), at_least(7)), Wait::Met);
        // The same write, a value that does not meet the wait: the held ones
        // cannot meet it; left alone.
        assert_eq!(plan.value_wait((0x500, 4), at_least(8)), Wait::Alone);
        // A timestamp there, a write narrower than the wait, shaders there.
        assert_eq!(plan.value_wait((0x600, 8), at_least(1)), Wait::Send);
        assert_eq!(plan.value_wait((0x700, 8), at_least(1)), Wait::Send);
        assert_eq!(plan.value_wait((0x9ffc, 4), at_least(1)), Wait::Send);
        // Nothing held writes there.
        assert_eq!(plan.value_wait((0x800, 8), at_least(1)), Wait::Alone);
        // A carried signal's fence value there (`=2`).
        assert_eq!(plan.signal(2, 0xf00, 5, (0xf08, 8)), Signal::Carried);
        assert_eq!(plan.value_wait((0xf08, 8), at_least(5)), Wait::Send);
        assert!(plan.carries(0xf00));
        assert!(!plan.carries(0xf08));
    }

    /// A signal with nothing held goes on as before; with something held it
    /// sends it first, or (`=2`) is carried and comes back when it is sent.
    #[test]
    fn a_signal_sends_the_held_submissions_or_is_carried() {
        let mut plan = Plan::new();
        assert_eq!(plan.signal(1, 0x100, 1, (0x108, 8)), Signal::Alone);
        assert_eq!(plan.signal(2, 0x100, 1, (0x108, 8)), Signal::Alone);
        plan.take(plain(1));
        assert_eq!(plan.signal(1, 0x100, 1, (0x108, 8)), Signal::Send);
        assert!(!plan.carries(0x100), "at level 1 nothing is carried");
        assert_eq!(plan.signal(2, 0x100, 2, (0x108, 8)), Signal::Carried);
        assert_eq!(plan.signal(2, 0x200, 9, (0x208, 8)), Signal::Carried);
        assert!(plan.carries(0x100) && plan.carries(0x200));
        assert_eq!(plan.clear(), [(0x100, 2), (0x200, 9)]);
        assert!(plan.is_empty() && !plan.carries(0x100) && !plan.carries(0x200));
    }

    /// Ranges taken in are kept in order and joined, as a sort and a join of
    /// all of them would leave them.
    #[test]
    fn ranges_taken_in_stay_in_order_and_joined() {
        let mut into = Vec::new();
        join(&mut into, vec![(40, 50), (10, 20), (20, 25), (7, 7)]);
        assert_eq!(into, [(10, 25), (40, 50)]);
        join(&mut into, vec![(45, 60), (0, 5), (26, 30), (100, 101)]);
        assert_eq!(into, [(0, 5), (10, 25), (26, 30), (40, 60), (100, 101)]);
        join(&mut into, vec![(25, 26), (5, 10)]);
        assert_eq!(into, [(0, 30), (40, 60), (100, 101)]);
        assert!(touches(&into, 29, 31) && !touches(&into, 30, 40) && !touches(&into, 60, 100));
    }
}
