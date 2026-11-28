//! Walk already-made, registered shader heaps for idle-submit texture eviction.

use super::{Heap, MADE_LATE, SHADER_HEAPS, WAITING};
use crate::texture_heap_evict::{IdleWindow, ResourceRecency};
use std::collections::{BTreeSet, HashMap, HashSet};

pub(crate) fn collect_recency(window: IdleWindow) -> HashMap<usize, ResourceRecency> {
    let mut recency = HashMap::new();
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    for &(_, _, at) in heaps.iter() {
        // SAFETY: registered heaps leave the table under its write lock.
        let heap = unsafe { &*(at as *const Heap) };
        if let Some(Some(host)) = heap.host.get() {
            host.collect_resource_recency(window, &mut recency);
        }
    }
    recency
}

/// The caller pins the selected copies. Release displaced COM references only
/// after the heap and slot locks are gone, before detachment checks their count.
pub(crate) fn unmake(victims: &HashSet<usize>, window: IdleWindow) -> BTreeSet<usize> {
    let mut resources = BTreeSet::new();
    let mut removed = Vec::new();
    let mut slots = Vec::new();
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    for &(_, _, at) in heaps.iter() {
        // SAFETY: registered heaps leave the table under its write lock.
        let heap = unsafe { &*(at as *const Heap) };
        let Some(Some(host)) = heap.host.get() else {
            continue;
        };
        host.evict_kept_with(victims, window, &mut removed, &mut slots);
        if !slots.is_empty() {
            let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
            for (resource, index) in slots.drain(..) {
                if let Some(handle) = heap.handle(index) {
                    let handles = waiting.entry(resource).or_default();
                    if !handles.contains(&handle) {
                        handles.push(handle);
                    }
                    resources.insert(resource);
                }
            }
        }
    }
    drop(heaps);
    drop(removed);
    resources
}

pub(crate) fn discard_wakeup(resource: usize) {
    MADE_LATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|&at| at != resource);
}
