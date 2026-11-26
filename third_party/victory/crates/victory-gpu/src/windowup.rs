//! (Neptune) Pooled upload memory for transient windows (`VICTORY_CARD_WINDOW_UPLOAD=1`,
//! `crate::shadow::window_upload`): a window with a card side keeps no CPU side; the pages
//! that changed are copied into a [`Chunk`] here and sent from it to the card side. (With
//! `VICTORY_WINDOW_SKIP_SAME=1` not those that hold what their window last sent:
//! `crate::shadow::window_skip_same`.)
//!
//! A chunk keeps its slab busy until it is dropped. A chunk is dropped when the copy sends it
//! and its plan is let go (`Host::card_done`, after the submission that copied it has run on
//! the card), or earlier if it was never sent (the copy was let go, or another refresh of the
//! same pages came after). A slab with no chunk left is reused from its start.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

const SLAB: u64 = 16 << 20;
const ALIGN: u64 = 256;

fn most() -> u64 {
    static MOST: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("VICTORY_CARD_WINDOW_UPLOAD_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(512) << 20
    })
}

struct Slab {
    buffer: ID3D12Resource,
    base: *mut u8,
    size: u64,
    /// Chunks of the slab not dropped yet. Raised only under the pool's lock.
    live: AtomicU64,
    /// Made beyond the cap for one chunk: not kept by the pool.
    overflow: bool,
}

// SAFETY: a D3D12 resource and its persistent mapping; each chunk's bytes are written by one
// thread (its allocator) before the chunk is shared.
unsafe impl Send for Slab {}
// SAFETY: as above.
unsafe impl Sync for Slab {}

impl Drop for Slab {
    fn drop(&mut self) {
        if self.overflow {
            OVERFLOW_LIVE.fetch_sub(self.size, Ordering::Relaxed);
        }
    }
}

/// A piece of a slab, at `offset`.
pub(crate) struct Chunk {
    slab: Arc<Slab>,
    pub(crate) offset: u64,
}

impl Chunk {
    pub(crate) fn ptr(&self) -> *mut u8 {
        // SAFETY: inside the slab's mapping (`alloc`).
        unsafe { self.slab.base.add(self.offset as usize) }
    }

    pub(crate) fn buffer(&self) -> &ID3D12Resource {
        &self.slab.buffer
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        self.slab.live.fetch_sub(1, Ordering::Release);
    }
}

struct PooledSlab {
    slab: Arc<Slab>,
    /// End of the reserved ranges. A live slab can still append after this point.
    at: u64,
}

impl PooledSlab {
    fn fits(&mut self, n: u64) -> bool {
        if self.slab.live.load(Ordering::Acquire) == 0 {
            self.at = 0;
        }
        self.at + n <= self.slab.size
    }
}

struct Pool {
    slabs: Vec<PooledSlab>,
    /// The last slab used, tried first for the next chunk.
    cur: usize,
}

static POOL: Mutex<Pool> = Mutex::new(Pool { slabs: Vec::new(), cur: 0 });
/// Bytes of slabs in the pool (peak), of overflow slabs alive (now), allocations that needed one.
static POOLED: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static OVERFLOW_LIVE: AtomicU64 = AtomicU64::new(0);
static OVERFLOWS: AtomicU64 = AtomicU64::new(0);
/// Bytes sent from chunks since the last [`stats`].
pub(crate) static SENT: AtomicU64 = AtomicU64::new(0);

fn make(size: u64, overflow: bool) -> Option<Arc<Slab>> {
    let (buffer, base) = crate::host::host()?.upload_buffer(size)?;
    Some(Arc::new(Slab { buffer, base, size, live: AtomicU64::new(0), overflow }))
}

/// `n` bytes of upload memory. Past the cap a slab of its own is made (counted and logged:
/// nothing blocks, and nothing is overwritten while the card may read it); `None` only if
/// the device gives no memory.
pub(crate) fn alloc(n: u64) -> Option<Chunk> {
    let n = n.next_multiple_of(ALIGN);
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = &mut *pool;
    if n <= SLAB {
        // Try the current slab, then space left in any earlier slab.
        let mut slot = if pool.slabs.get_mut(pool.cur).is_some_and(|slab| slab.fits(n)) {
            Some(pool.cur)
        } else {
            pool.slabs.iter_mut().position(|slab| slab.fits(n))
        };
        if slot.is_none()
            && POOLED.load(Ordering::Relaxed) + SLAB <= most()
            && let Some(slab) = make(SLAB, false)
        {
            let total = POOLED.fetch_add(SLAB, Ordering::Relaxed) + SLAB;
            PEAK.fetch_max(total, Ordering::Relaxed);
            pool.slabs.push(PooledSlab { slab, at: 0 });
            slot = Some(pool.slabs.len() - 1);
        }
        if let Some(i) = slot {
            pool.cur = i;
            let entry = &mut pool.slabs[i];
            let slab = entry.slab.clone();
            slab.live.fetch_add(1, Ordering::Relaxed);
            let offset = entry.at;
            entry.at += n;
            return Some(Chunk { slab, offset });
        }
    }
    let said = OVERFLOWS.fetch_add(1, Ordering::Relaxed) + 1;
    if said <= 16 || said.is_power_of_two() {
        tracing::warn!(
            said,
            bytes = n,
            pooled_mb = POOLED.load(Ordering::Relaxed) >> 20,
            "window upload pool is full: a slab of its own (VICTORY_CARD_WINDOW_UPLOAD)"
        );
    }
    let slab = make(n, true)?;
    OVERFLOW_LIVE.fetch_add(n, Ordering::Relaxed);
    slab.live.fetch_add(1, Ordering::Relaxed);
    Some(Chunk { slab, offset: 0 })
}

/// For the ram line: pool bytes now, peak, overflow slabs alive (bytes), overflow allocations so far.
pub(crate) fn stats() -> [u64; 4] {
    [
        POOLED.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed),
        OVERFLOW_LIVE.load(Ordering::Relaxed),
        OVERFLOWS.load(Ordering::Relaxed),
    ]
}

#[cfg(test)]
mod tests;
