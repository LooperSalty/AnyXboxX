//! Tier-3 recency, slot ownership, and deferred refills.

use super::*;
use crate::descriptor::{self, HEAP_IIDS, HeapDesc, Placement};
use crate::object::{S_OK, This};
use crate::texture_heap_evict::{COOLDOWN_FRAMES, IdleWindow};
use crate::texture_lru::tests::{GPU_CASE, HeapMode, Limit, Texture};
use std::collections::{HashMap, HashSet};
use std::ptr::null;
use windows::core::Interface;

#[repr(C)]
struct HeapWithPlacement {
    desc: HeapDesc,
    placement: Placement,
}

pub(crate) struct RegisteredHeap(This);

impl RegisteredHeap {
    pub(crate) fn new() -> Self {
        let args = HeapWithPlacement {
            desc: HeapDesc {
                kind: 0,
                count: 16,
                flags: 1,
                nodes: 0,
            },
            placement: Placement { address: 0, size: 0 },
        };
        let mut out = null();
        assert_eq!(
            descriptor::create_heap(null(), &args.desc, &HEAP_IIDS[0], &mut out),
            S_OK
        );
        Self(out)
    }

    pub(crate) fn host(&self) -> &HostHeap {
        descriptor::heap(self.0).host().unwrap()
    }

    pub(crate) fn handle(&self, index: u32) -> u64 {
        descriptor::heap(self.0).handle(index).unwrap()
    }

    pub(crate) fn bind(&self, index: u32, texture: &Texture) {
        descriptor::create_srv(null(), texture.0, null(), self.handle(index));
        assert!(self.host().made(index));
    }
}

impl Drop for RegisteredHeap {
    fn drop(&mut self) {
        descriptor::release(self.0);
    }
}

#[test]
fn heap_eviction_cooldown_survives_room_and_follows_fresh_writes() {
    let mut queue = TextureQueue::default();
    queue.written(0, 90, 100, false);
    let write = queue.slots[&0].write;
    queue.hold(0, write, 105);
    queue.finish_bake(queue.revision);
    assert!(queue.bake_covered());
    queue.hold_evicted(0, 100, 130);
    assert!(!queue.bake_covered());
    queue.done(0, write);
    assert_eq!(queue.room(101), 0);
    assert!(queue.due(129, 8).is_empty());
    for index in 1..=3 {
        queue.written(index, 129, 129, false);
    }
    let at_due: Vec<_> = queue.due(130, 4).into_iter().map(|(index, _)| index).collect();
    assert_eq!(at_due, [3, 1, 2, 0]);
    assert_eq!(queue.due(130, 3).len(), 3);

    queue.hold_evicted(0, 130, 160);
    queue.written(0, 131, 131, false);
    assert_eq!(
        queue.due(131, 1)[0].0,
        0,
        "an explicit rewrite supersedes the retry"
    );
}

#[test]
fn heap_eviction_tracks_aliases_and_unmakes_cold_untaken_slots_oldest_first() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let h = host().expect("D3D12 adapter (or WARP)");
    {
        let _off = HeapMode::set(crate::switch::opt_in(None));
        let disabled = RegisteredHeap::new();
        assert!(disabled.host().last_used_frame.is_empty());
    }
    let _mode = HeapMode::set(crate::switch::opt_in(Some("1")));
    let _room = Limit::set(u64::MAX);
    let registered = RegisteredHeap::new();
    let heap = registered.host();
    let cold = Texture::new();
    let hot = Texture::new();
    let cold_copy = crate::resource::host_copy(cold.0).unwrap();
    let hot_copy = crate::resource::host_copy(hot.0).unwrap();
    registered.bind(0, &cold);
    registered.bind(1, &cold);
    registered.bind(2, &hot);

    let taken = heap.dynamic(1).unwrap();
    let untaken = heap.dynamic(1).unwrap();
    heap.make_at(taken, &View::Shader(hot.0 as usize, None));
    heap.release(heap.id(), untaken, 1);
    // Defensive fixture: ordinary release already clears the retained ref.
    heap.make_at(untaken, &View::Shader(cold.0 as usize, None));
    for (index, used) in [(0, 80), (1, 20), (2, 81), (taken, 0), (untaken, 0)] {
        heap.last_used_frame[index as usize].store(used, Ordering::Relaxed);
    }

    let cold_raw = cold_copy.texture.as_raw() as usize;
    let hot_raw = hot_copy.texture.as_raw() as usize;
    let mut recency = HashMap::new();
    let window = IdleWindow {
        frame: 100,
        frames: 20,
    };
    heap.collect_resource_recency(window, &mut recency);
    assert_eq!(recency[&cold_raw].kept_refs, 3);
    assert_eq!(recency[&cold_raw].last_used_frame, 80);
    assert!(!recency[&cold_raw].protected);
    assert_eq!(recency[&hot_raw].kept_refs, 2);
    assert!(recency[&hot_raw].protected);
    recency.clear();
    heap.collect_resource_recency(IdleWindow { frames: 0, ..window }, &mut recency);
    assert!(
        recency[&hot_raw].protected,
        "a taken slot protects its resource even with idle checks disabled"
    );

    let mut removed = Vec::new();
    let mut slots = Vec::new();
    h.with_submit_lock_for_test(|| {
        heap.evict_kept_with(
            &HashSet::from([cold_raw, hot_raw]),
            window,
            &mut removed,
            &mut slots,
        );
        assert_eq!(slots, [(cold.0 as usize, 1), (cold.0 as usize, 0)]);
        assert_eq!(removed.len(), 3);
        assert!(!heap.made(0) && !heap.made(1) && heap.made(2));
        assert!(heap.kept_in(untaken).is_none() && heap.kept_in(taken).is_some());
        let due = window.frame + COOLDOWN_FRAMES;
        assert!(
            !heap
                .texture_slots_all_due(due - 1)
                .iter()
                .any(|&(index, _)| index < 2)
        );
        assert_eq!(
            heap.texture_slots_all_due(due)
                .iter()
                .filter(|&&(index, _)| index < 2)
                .count(),
            2
        );
    });
    drop(removed);
    heap.release(heap.id(), taken, 1);
}
