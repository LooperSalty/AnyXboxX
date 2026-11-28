//! Recency and unmaking of host heap slots at an idle submit.

use super::{
    FLIPS, Flip, HostHeap, PENDING_LIST_COUNT, PENDING_LIST_FLIPS, TextureQueue, Waiting, null_like,
    null_view,
};
use crate::descriptor::{View, view_at};
use crate::texture_heap_evict::{COOLDOWN_FRAMES, IdleWindow, ResourceRecency};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

impl HostHeap {
    #[inline]
    pub(super) fn note_slot_used(&self, index: u32, frame: u64, lock_for_read: bool) {
        if let Some(stamp) = self.last_used_frame.get(index as usize)
            && stamp.load(Relaxed) < frame
        {
            // A first read this frame serializes with unmaking. Installation
            // already holds the retained reference's lock.
            let _made = lock_for_read.then(|| self.made.lock(index));
            stamp.fetch_max(frame, Relaxed);
        }
    }

    fn slot_flip_pending(&self, index: u32) -> bool {
        let own = std::ptr::from_ref(self) as usize;
        let matches = |flip: &Flip| flip.heap == own && flip.index == index;
        (PENDING_LIST_COUNT.load(Acquire) != 0
            && PENDING_LIST_FLIPS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|item| matches(&item.flip)))
            || FLIPS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(matches)
    }

    /// Counts only this heap's references; other COM owners remain pins.
    pub(crate) fn collect_resource_recency(
        &self,
        window: IdleWindow,
        out: &mut HashMap<usize, ResourceRecency>,
    ) {
        if self.last_used_frame.is_empty() {
            return;
        }
        let taken = self.next.lock().unwrap_or_else(|e| e.into_inner()).1.clone();
        let own = std::ptr::from_ref(self) as usize;
        let mut pending = HashSet::new();
        if PENDING_LIST_COUNT.load(Acquire) != 0 {
            let flips = PENDING_LIST_FLIPS.lock().unwrap_or_else(|e| e.into_inner());
            pending.extend(
                flips
                    .iter()
                    .filter(|item| item.flip.heap == own)
                    .map(|item| item.flip.index),
            );
        }
        pending.extend(
            FLIPS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|flip| flip.heap == own)
                .map(|flip| flip.index),
        );
        for (index, (slot, stamp)) in self.kept.iter().zip(&self.last_used_frame).enumerate() {
            let slot = slot.lock().unwrap_or_else(|e| e.into_inner());
            let Some(resource) = slot.as_ref() else { continue };
            let used = stamp.load(Relaxed);
            let entry = out.entry(resource.as_raw() as usize).or_default();
            entry.last_used_frame = entry.last_used_frame.max(used);
            entry.kept_refs = entry.kept_refs.saturating_add(1);
            entry.protected |= !window.is_cold(used)
                || if index < self.count as usize {
                    self.made.get(index as u32).unwrap_or(0) == 0 || pending.contains(&(index as u32))
                } else {
                    taken[index - self.count as usize]
                };
        }
    }

    /// Nulls cold mirrored slots and untaken dynamic slots for the victims.
    /// The caller drops the displaced references after the heap walk.
    pub(crate) fn evict_kept_with(
        &self,
        victims: &HashSet<usize>,
        window: IdleWindow,
        removed: &mut Vec<ID3D12Resource>,
        shader_slots: &mut Vec<(usize, u32)>,
    ) {
        let matches = |slot: &Option<ID3D12Resource>| {
            slot.as_ref()
                .is_some_and(|res| victims.contains(&(res.as_raw() as usize)))
        };
        let mut candidates = Vec::new();
        for (index, (slot, stamp)) in self.kept.iter().zip(&self.last_used_frame).enumerate() {
            let used = stamp.load(Relaxed);
            if window.is_cold(used) && matches(&slot.lock().unwrap_or_else(|e| e.into_inner())) {
                candidates.push((used, index as u32));
            }
        }
        candidates.sort_unstable();
        for (_, index) in candidates {
            let (_made, _next, key) = if index < self.count {
                let made = self.made.lock(index);
                let key = self.made.get(index).unwrap_or(0);
                if key == 0 || self.slot_flip_pending(index) {
                    continue;
                }
                (Some(made), None, key)
            } else {
                let next = self.next.lock().unwrap_or_else(|e| e.into_inner());
                if next.1[(index - self.count) as usize] {
                    continue;
                }
                (None, Some(next), 0)
            };
            let mut slot = self.kept[index as usize]
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !window.is_cold(self.last_used_frame[index as usize].load(Relaxed)) || !matches(&slot) {
                continue;
            }
            let view = (key != 0)
                .then(|| view_at(key as u32, (key >> 32) as u32))
                .flatten();
            // Derive the null SRV from the record without dereferencing a title resource.
            let words = match &view {
                Some(View::Shader(_, desc)) => desc.as_ref(),
                Some(View::Placed(false, shape, desc)) if shape[0] != 1 => desc[..10].try_into().ok(),
                None => None,
                _ => continue,
            };
            self.clear_keeping(index);
            self.quick_is(index, 0);
            self.made.set(index, 0);
            if let Some(words) = words {
                null_like(words, self.at(index));
            } else {
                null_view(self.at(index));
            }
            self.put(index);
            self.clear_twin(index);
            self.write_view_metadata(index, 0, 0);
            removed.extend(slot.take());
            drop(slot);
            let texture_view = match view {
                Some(View::Shader(resource, _)) => {
                    shader_slots.push((resource, index));
                    true
                }
                Some(View::Placed(false, ..)) => true,
                _ => false,
            };
            if texture_view && !self.tabled_lately(index, window.frame) {
                let due = window.frame.saturating_add(COOLDOWN_FRAMES);
                self.textures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .hold_evicted(index, window.frame, due);
            }
        }
    }
}

impl TextureQueue {
    /// Room notifications and stale passes cannot advance this retry, but a
    /// new title write can replace it. Bump the revision to invalidate a bake.
    pub(super) fn hold_evicted(&mut self, index: u32, frame: u64, due: u64) {
        self.held.remove(&index);
        let due = self.slots.get(&index).map_or(due, |w| w.due.max(due));
        let write = self.next_revision();
        self.insert(
            index,
            Waiting {
                due,
                written: frame,
                write,
                eviction_retry: true,
                looks: 0,
                mapped: 0,
                checked: (0, 0, 0, 0, 0),
                since: crate::host::now_ms(),
                waited: false,
                away: 0,
            },
        );
    }
}
