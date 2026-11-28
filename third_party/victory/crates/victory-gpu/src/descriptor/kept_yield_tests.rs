//! Superseded views, pressure defaults, and pending descriptor flips.

use super::*;
use crate::descriptor::{self, HEAP_IIDS, HeapDesc, Placement, words_for};
use crate::object::{S_OK, This};
use crate::texture_lru::tests::{EvictionMode, GPU_CASE, Limit, Texture};
use std::cell::Cell;
use std::ptr::null;

#[test]
fn keep_shown_only_checks_pressure_with_explicit_yield_opt_in() {
    use crate::switch::opt_in;
    let unexpected_budget_check = || panic!("must not query the texture budget");
    for value in [None, Some("0"), Some("false"), Some("off"), Some("yes")] {
        assert!(!opt_in(value), "{value:?}");
        assert_eq!(keep_shown_with(None, opt_in(value), unexpected_budget_check), 1);
    }
    for value in ["1", "true", "on"] {
        assert!(opt_in(Some(value)), "{value:?}");
        assert_eq!(keep_shown_with(None, opt_in(Some(value)), || true), 1);
        assert_eq!(keep_shown_with(None, opt_in(Some(value)), || false), 0);
    }
    for mode in 0..=2 {
        assert_eq!(keep_shown_with(Some(mode), true, unexpected_budget_check), mode);
    }
}

struct YieldMode(Option<bool>);

impl YieldMode {
    fn set(enabled: bool) -> Self {
        Self(TEST_YIELD.replace(Some(enabled)))
    }
}

impl Drop for YieldMode {
    fn drop(&mut self) {
        TEST_YIELD.set(self.0);
    }
}

fn words(view: View) -> [u32; 3] {
    let [index, generation] = words_for(view);
    [VIEW_TAG, index, generation]
}

fn placed_words(first: u32, address: u32) -> [u32; 3] {
    let shape = [3, 0, 0, 0, 128, 0, 128, (4 << 16) | 1, 28, 1, 0, 0, 0, 0];
    let mut desc = [0; 16];
    (desc[0], desc[1], desc[2], desc[4], desc[5], desc[10]) = (28, 4, 5768, first, 4 - first, address);
    // Fixture records are only compared by mirror; the source is a real host resource.
    words(View::Placed(false, shape, desc))
}

fn seed(heap: &HostHeap, index: u32, key: u64, texture: &Texture) {
    let copy = crate::resource::host_copy(texture.0).unwrap();
    let _made = heap.made.lock(index);
    heap.made.set(index, key);
    heap.quick_is(index, key);
    heap.keep(index, Some(copy.texture.clone()));
}

#[repr(C)]
struct HeapWithPlacement {
    desc: HeapDesc,
    placement: Placement,
}

struct RegisteredHeap(This);

impl RegisteredHeap {
    fn new() -> Self {
        let args = HeapWithPlacement {
            desc: HeapDesc { kind: 0, count: 16, flags: 1, nodes: 0 },
            placement: Placement { address: 0, size: 0 },
        };
        let mut out = null();
        assert_eq!(descriptor::create_heap(null(), &args.desc, &HEAP_IIDS[0], &mut out), S_OK);
        Self(out)
    }
}

impl Drop for RegisteredHeap {
    fn drop(&mut self) {
        descriptor::release(self.0);
    }
}

#[test]
fn pressure_yields_only_superseded_slots_and_live_pins_survive_lru() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = host() else { return };
    assert!(keep_views(), "run with view keeping enabled");
    let _eviction = EvictionMode::set(true);
    let _yield = YieldMode::set(true);
    let _room = Limit::set(u64::MAX);
    let registered = RegisteredHeap::new();
    let title_heap = descriptor::heap(registered.0);
    let heap = title_heap.host().unwrap();
    let superseded = Texture::new();
    let live = Texture::new();
    let restored = Texture::new();
    let record = Cell::new(placed_words(2, 0x10000));
    let handle = record.as_ptr() as u64;
    let initial = record_key(handle);
    seed(heap, 0, initial, &superseded);
    record.set(placed_words(2, 0x40000));
    seed(heap, 2, record_key(handle), &restored);
    drop(crate::resource::host_copy(live.0).unwrap());
    descriptor::create_srv(null(), live.0, null(), title_heap.start + 32);
    assert!(heap.made(1));

    record.set(placed_words(0, 0x10000));
    assert!(heap.mirror(0, handle, false).is_some());
    record.set(placed_words(0, 0x40000));
    assert!(heap.mirror(2, handle, false).is_some());
    assert_eq!(heap.keeping.lock().unwrap().len(), 2);
    record.set(placed_words(2, 0x40000));
    assert!(heap.mirror(2, handle, false).is_some());
    assert_eq!(heap.keeping.lock().unwrap().len(), 1);

    crate::frame::advance_for_test(4);
    superseded.age(4);
    live.age(4);
    if crate::host::coarse_ms() < 1_100 {
        std::thread::sleep(std::time::Duration::from_millis(1_100));
    }
    let _pressure = Limit::set(0);
    h.with_submit_lock_for_test(|| {
        {
            let _yield = YieldMode::set(crate::switch::opt_in(None));
            crate::texture_lru::evict_if_pressured(h);
            assert!(heap.made(0) && superseded.resident());
        }
        {
            let _eviction = EvictionMode::set(false);
            crate::texture_lru::evict_if_pressured(h);
            assert!(!heap.made(0) && heap.kept_in(0).is_none());
            assert!(superseded.resident());
            assert!(heap.made(1) && heap.made(2));
        }
        h.flush_retired_for_test();
        crate::texture_lru::evict_if_pressured(h);
    });
    assert!(!superseded.resident());
    assert!(live.resident() && restored.resident());
    assert!(crate::host::references(&heap.kept_in(1).unwrap()) >= 3);
}

#[test]
fn a_scheduled_flip_clears_the_yield_mark_and_still_publishes() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = host() else { return };
    let _yield = YieldMode::set(true);
    let _room = Limit::set(u64::MAX);
    let heap = HostHeap::new(false, 4).unwrap();
    let old = Texture::new();
    let replacement = Texture::new();
    let record = Cell::new(placed_words(2, 0x10000));
    let handle = record.as_ptr() as u64;
    seed(&heap, 0, record_key(handle), &old);
    record.set(placed_words(0, 0x10000));
    assert!(heap.mirror(0, handle, false).is_some());
    assert!(!heap.keeping.lock().unwrap().is_empty());

    let list = h.new_list().unwrap();
    record.set(words(View::Shader(replacement.0 as usize, None)));
    let new_key = record_key(handle);
    heap.texture_slot_flip(&list, 0, handle, new_key);
    assert!(heap.keeping.lock().unwrap().is_empty());
    {
        let _made = heap.made.lock(0);
        heap.set_keeping(0, heap.made.get(0).unwrap());
    }
    assert_eq!(heap.yield_kept_views_into(&mut Vec::new()), 0);
    assert!(heap.made(0));
    list.close();
    h.execute(&[&list]);
    assert_eq!(heap.texture_slots_flip(), 1);
    assert_eq!(heap.made.get(0), Some(new_key));
}

#[test]
fn default_yield_does_not_track_superseded_views() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let _yield = YieldMode::set(crate::switch::opt_in(None));
    let heap = HostHeap::new(false, 4).expect("D3D12 adapter (or WARP)");
    let _made = heap.made.lock(0);
    heap.set_keeping(0, 1);
    assert!(heap.keeping.lock().unwrap().is_empty());
    assert!(heap.keeping_slots.0.is_empty());
}

#[test]
fn dropping_a_heap_removes_its_outstanding_marks() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_) = host() else { return };
    let _yield = YieldMode::set(true);
    let heap = HostHeap::new(false, 4).unwrap();
    let old = Texture::new();
    let record = Cell::new(placed_words(2, 0x10000));
    let handle = record.as_ptr() as u64;
    seed(&heap, 0, record_key(handle), &old);
    let before = keeping_total();
    record.set(placed_words(0, 0x10000));
    assert!(heap.mirror(0, handle, false).is_some());
    assert_eq!(keeping_total(), before + 1);
    drop(heap);
    assert_eq!(keeping_total(), before);
}
