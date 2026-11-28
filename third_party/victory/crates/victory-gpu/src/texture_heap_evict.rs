//! At an idle submit, opt-in heap eviction can unmake cold views of reloadable
//! textures that ordinary LRU cannot detach. Unknown references remain pins.

use crate::descriptor::{self, heap_evict};
use crate::resource::HostTexture;
use crate::texture_lru::scan::Candidate;
use crate::{frame, switch};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = TEST_ENABLED.get() {
        return enabled;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| switch::opt_in(std::env::var("VICTORY_TEXTURE_LRU_HEAP_EVICT").ok().as_deref()))
}

pub(crate) fn parse_idle_frames(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse().ok()).unwrap_or(120)
}

fn idle_frames() -> u64 {
    #[cfg(test)]
    if let Some(frames) = TEST_IDLE_FRAMES.get() {
        return frames;
    }
    static FRAMES: OnceLock<u64> = OnceLock::new();
    *FRAMES.get_or_init(|| {
        parse_idle_frames(
            std::env::var("VICTORY_TEXTURE_LRU_HEAP_IDLE_FRAMES")
                .ok()
                .as_deref(),
        )
    })
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_ENABLED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    pub(crate) static TEST_IDLE_FRAMES: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(crate) const COOLDOWN_FRAMES: u64 = 30;

#[derive(Clone, Copy)]
pub(crate) struct IdleWindow {
    pub(crate) frame: u64,
    pub(crate) frames: u64,
}

impl IdleWindow {
    pub(crate) fn is_cold(self, used: u64) -> bool {
        self.frame.saturating_sub(used) >= self.frames
    }
}

#[derive(Default)]
pub(crate) struct ResourceRecency {
    pub(crate) last_used_frame: u64,
    pub(crate) kept_refs: u32,
    pub(crate) protected: bool,
}

pub(crate) struct Scan {
    window: IdleWindow,
    recency: HashMap<usize, ResourceRecency>,
    candidates: Vec<(Candidate, u32)>,
}

impl Scan {
    pub(crate) fn new() -> Self {
        let window = IdleWindow {
            frame: frame::current(),
            frames: idle_frames(),
        };
        Self {
            window,
            recency: heap_evict::collect_recency(window),
            candidates: Vec::new(),
        }
    }

    pub(crate) fn consider(&mut self, texture: &ID3D12Resource, mut candidate: Candidate, refs: u32) {
        if let Some(recency) = self.recency.get(&(texture.as_raw() as usize))
            && !recency.protected
            && refs == recency.kept_refs.saturating_add(1)
        {
            candidate.0 = candidate.0.max(recency.last_used_frame);
            if self.window.is_cold(candidate.0) {
                self.candidates.push((candidate, refs));
            }
        }
    }

    /// Pin the oldest eligible copies up to the remaining byte and victim limits.
    /// The callback checks the current host copy, including the expected COM count.
    pub(crate) fn select(
        mut self,
        mut bytes_left: u64,
        count_left: usize,
        mut try_pin: impl FnMut(Candidate, u32, IdleWindow) -> Option<(Arc<HostTexture>, u64)>,
    ) -> (Vec<Candidate>, Batch) {
        self.candidates.sort_unstable();
        let mut selected = Vec::new();
        let mut pinned = Vec::new();
        for (candidate, refs) in self.candidates {
            if bytes_left == 0 || selected.len() == count_left {
                break;
            }
            if let Some((copy, bytes)) = try_pin(candidate, refs, self.window) {
                bytes_left = bytes_left.saturating_sub(bytes);
                pinned.push(copy);
                selected.push(candidate);
            }
        }
        (
            selected,
            Batch {
                pinned,
                waits: BTreeSet::new(),
                idle_frames: self.window.frames,
            },
        )
    }
}

/// The extra Arcs keep raw identities live until unmaking and detachment finish.
pub(crate) struct Batch {
    pinned: Vec<Arc<HostTexture>>,
    waits: BTreeSet<usize>,
    idle_frames: u64,
}

impl Batch {
    pub(crate) fn unmake(&mut self) {
        let victims: HashSet<_> = self
            .pinned
            .iter()
            .map(|copy| copy.texture.as_raw() as usize)
            .collect();
        let window = IdleWindow {
            frame: frame::current(),
            frames: self.idle_frames,
        };
        self.waits = heap_evict::unmake(&victims, window);
    }

    pub(crate) fn still_cold(&self, used: u64) -> bool {
        IdleWindow {
            frame: frame::current(),
            frames: self.idle_frames,
        }
        .is_cold(used)
    }

    /// Call while holding the host-copy lock: only a real remake may wake these slots.
    pub(crate) fn detached(&mut self, address: usize) {
        self.waits.remove(&address);
        heap_evict::discard_wakeup(address);
    }

    /// If a selected texture stayed resident, let its waiting slots recover.
    pub(crate) fn wake_resident(&self, mut resident: impl FnMut(usize) -> bool) {
        for &address in &self.waits {
            if resident(address) {
                descriptor::texture_made(address);
            }
        }
    }
}
