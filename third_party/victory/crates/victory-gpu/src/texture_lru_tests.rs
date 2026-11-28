//! Run the same fence and upload cases on hardware and with
//! `VICTORY_D3D12_WARP=1` (diagnostics build).

use super::*;
use crate::object::S_OK;
use crate::resource::{HeapProperties, RESOURCE_IIDS, create_committed, host_copy, resource, resource_release, staged_first};
use std::collections::BTreeMap;
use std::ptr::null;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

pub(crate) static GPU_CASE: Mutex<()> = Mutex::new(());

pub(crate) struct Texture(pub(crate) This);

impl Texture {
    pub(crate) fn new() -> Self {
        Self::with_samples(1)
    }

    fn with_samples(sample_count: u32) -> Self {
        let desc = ResourceDesc {
            dimension: 3,
            alignment: 0,
            width: 128,
            height: 128,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: 28,
            sample_count,
            sample_quality: 0,
            // Use the size fallback for the invalid multisample fixture.
            layout: if sample_count == 1 { 0 } else { 2 },
            flags: 0,
        };
        let heap = HeapProperties {
            kind: 1,
            cpu_pages: 0,
            pool: 0,
            creation_nodes: 0,
            visible_nodes: 0,
        };
        let mut out = null();
        assert_eq!(create_committed(null(), &heap, 0, &desc, 0, null(), &RESOURCE_IIDS[0], &mut out), S_OK);
        Self(out)
    }

    pub(crate) fn resident(&self) -> bool {
        resource(self.0).host.made.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub(crate) fn age(&self, frames: u64) {
        let copy = host_copy(self.0).unwrap();
        let r = resource(self.0);
        let then = frame::current().saturating_sub(frames);
        r.host.used.store(then << 32, Relaxed);
        copy.last_used_frame.store(then, Relaxed);
    }

    fn write(&self, byte: u8) {
        let r = resource(self.0);
        // SAFETY: this fixture owns the backing and has waited for its uploads.
        unsafe { std::ptr::write_bytes(r.data(), byte, footprint(&r.desc).unwrap().size as usize) };
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        resource_release(self.0);
    }
}

pub(crate) struct Limit(Option<u64>);

impl Limit {
    pub(crate) fn set(bytes: u64) -> Self {
        Self(TEST_LIMIT.replace(Some(bytes)))
    }
}

impl Drop for Limit {
    fn drop(&mut self) {
        TEST_LIMIT.set(self.0);
    }
}

pub(crate) struct EvictionMode(Option<bool>);

impl EvictionMode {
    pub(crate) fn set(enabled: bool) -> Self {
        Self(TEST_EVICTION.replace(Some(enabled)))
    }
}

impl Drop for EvictionMode {
    fn drop(&mut self) {
        TEST_EVICTION.set(self.0);
    }
}

#[test]
fn lru_requires_an_explicit_opt_in() {
    use crate::switch::opt_in;
    for value in [None, Some("0"), Some("false"), Some("off"), Some(""), Some("yes"), Some(" 1 ")] {
        assert!(!opt_in(value), "{value:?}");
    }
    for value in ["1", "true", "on"] {
        assert!(opt_in(Some(value)), "{value:?}");
    }
}

#[test]
fn submit_pressure_preserves_textures_until_lru_is_enabled() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let h = host::host().expect("D3D12 adapter (or WARP)");
    let _eviction = EvictionMode::set(crate::switch::opt_in(None));
    let _room = Limit::set(u64::MAX);
    let old = Texture::new();
    frame::advance_for_test(4);
    old.age(4);
    if host::coarse_ms() < 1_100 {
        std::thread::sleep(std::time::Duration::from_millis(1_100));
    }
    let _pressure = Limit::set(0);
    h.with_submit_lock_for_test(|| {
        assert!(!needs_submit_maintenance());
        evict_if_pressured(h);
        assert!(old.resident());

        let _eviction = EvictionMode::set(crate::switch::opt_in(Some("1")));
        evict_if_pressured(h);
        assert!(!old.resident());
    });
}

fn test_policy() -> EvictPolicy {
    EvictPolicy { min_idle_ms: 0, min_idle_frames: 2, max_victims: 256 }
}

fn wait(h: &host::Host, fence: u64) {
    // SAFETY: event belongs to this wait; the fence is owned by the host.
    unsafe {
        let event = CreateEventW(None, false, false, None).expect("fence event");
        h.fence().SetEventOnCompletion(fence, event).expect("set fence event");
        let result = WaitForSingleObject(event, 10_000);
        CloseHandle(event).expect("close fence event");
        assert_eq!(result, WAIT_OBJECT_0);
    }
}

struct QueueGate(ID3D12Fence);

impl Drop for QueueGate {
    fn drop(&mut self) {
        // SAFETY: release the queue wait even if an assertion above failed.
        unsafe { let _ = self.0.Signal(1); }
    }
}

#[test]
fn in_flight_fenced_textures_are_never_evicted_early() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let _eviction = EvictionMode::set(true);
    let Some(h) = host::host() else { return };
    let _room = Limit::set(u64::MAX);
    let settled = Texture::new();
    let pending = Texture::new();
    let record = |t: &Texture| {
        let list = h.new_list().unwrap();
        let initial = host_copy(t.0).unwrap().state.load(Relaxed);
        assert!(list.transition(t.0, initial).is_none());
        let wanted = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let (texture, before) = list.transition(t.0, wanted.0 as u32).unwrap();
        // SAFETY: open list and a live texture held by its recorded states.
        unsafe {
            list.list.ResourceBarrier(&[host::transition(texture, D3D12_RESOURCE_STATES(before as i32), wanted)]);
        }
        list.close();
        list
    };
    let settled_list = record(&settled);
    let complete = h.submit_fenced_for_test(&settled_list, None);
    wait(h, complete);
    let pending_list = record(&pending);
    // SAFETY: wait on a new fence, which the CPU signals below.
    let gate = QueueGate(unsafe { h.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap() });
    let incomplete = h.submit_fenced_for_test(&pending_list, Some(&gate.0));
    frame::advance_for_test(4);
    let completed = h.completed_fence();
    let last_signalled = h.last_signalled_fence();
    let outcome = evict_lru_textures(u64::MAX, completed, test_policy());
    let (kept_settled, kept_pending) = (settled.resident(), pending.resident());
    drop(gate);
    wait(h, incomplete);
    assert!(completed < incomplete);
    assert!(last_signalled >= incomplete);
    assert!(outcome.skipped_in_flight >= 1);
    assert!(!kept_settled);
    assert!(kept_pending, "only its incomplete fence is protecting it");
    evict_lru_textures(u64::MAX, h.completed_fence(), test_policy());
    assert!(!pending.resident());
}

#[test]
fn oldest_completed_texture_evicts_under_cap_and_uploads_again_on_reuse() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let _eviction = EvictionMode::set(true);
    let Some(h) = host::host() else { return };
    let _room = Limit::set(u64::MAX);
    let old = Texture::new();
    old.write(0x55);
    let list = h.new_list().unwrap();
    texture::upload_from(&list, old.0, 0);
    list.close();
    h.execute(&[&list]);
    drop(list);
    assert_eq!(staged_first(old.0), 0);
    let copy = host_copy(old.0).unwrap();
    let (serial, bytes) = (copy.serial, copy.card_bytes());
    assert!(copy.last_fence_value.load(Acquire) > 0);
    drop(copy);
    frame::advance_for_test(4);
    let recent = Texture::new();
    drop(host_copy(recent.0).unwrap());

    let limit = Limit::set(Budget::read(None).filled_bytes);
    assert!(!Budget::read(None).has_room);
    let outcome = evict_lru_textures(bytes, h.completed_fence(), test_policy());
    assert!(outcome.evicted_bytes >= bytes);
    assert!(!old.resident());
    assert!(recent.resident());
    assert_eq!(staged_first(old.0), u32::MAX);
    // The baseline delays retirement; a submitted fence is not a budget update.
    h.with_submit_lock_for_test(|| h.flush_retired_for_test());
    assert!(Budget::read(None).has_room);
    drop(limit);

    old.write(0x9b);
    let remade = host_copy(old.0).unwrap();
    assert_ne!(remade.serial, serial);
    let readback = h.read_back_buffer(128 * 512).unwrap();
    let list = h.new_list().unwrap();
    texture::upload_from(&list, old.0, 0);
    let source = D3D12_RESOURCE_STATE_COPY_SOURCE;
    let dst = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: non-owning pointer to the readback buffer held by this test.
        pResource: unsafe { std::mem::transmute_copy(&readback) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: DXGI_FORMAT(28), Width: 128, Height: 128, Depth: 1, RowPitch: 512,
                },
            },
        },
    };
    let src = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: non-owning pointer to the texture held by `remade`.
        pResource: unsafe { std::mem::transmute_copy(&remade.texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    // SAFETY: open list and valid copy locations kept alive until execute returns.
    unsafe {
        if let Some((t, before)) = list.transition(old.0, source.0 as u32) {
            list.list.ResourceBarrier(&[host::transition(t, D3D12_RESOURCE_STATES(before as i32), source)]);
        }
        list.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
    }
    list.close();
    h.execute(&[&list]);
    let mut mapped = std::ptr::null_mut();
    // SAFETY: synchronous execute completed the copy; check the entire first row.
    unsafe {
        readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 512 }), Some(&mut mapped)).unwrap();
        let all_match = std::slice::from_raw_parts(mapped.cast::<u8>(), 512).iter().all(|&b| b == 0x9b);
        readback.Unmap(0, None);
        assert!(all_match, "rewritten bytes must reach the new host copy");
    }
}

#[derive(Default)]
struct Warnings(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

impl tracing::Subscriber for Warnings {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(BTreeMap<String, String>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.insert(field.name().to_owned(), format!("{value:?}"));
            }
        }
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        if fields.0.contains_key("remaining_pressure") {
            self.0.lock().unwrap().push(fields.0);
        }
    }
}

#[test]
fn refusal_and_uncleared_submit_pressure_log_outcome_and_budget() {
    let _case = GPU_CASE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = host::host() else { return };
    let _eviction = EvictionMode::set(true);
    let capture = Warnings::default();
    let events = capture.0.clone();
    let _subscriber = tracing::subscriber::set_default(capture);
    let old = Texture::new();
    frame::advance_for_test(4);
    old.age(4);
    if host::coarse_ms() < 1_100 {
        std::thread::sleep(std::time::Duration::from_millis(1_100));
    }
    let _limit = Limit::set(0);
    // Beyond the host's maximum sample count, but with valid backing memory.
    let refused = Texture::with_samples(33);
    assert!(host_copy(refused.0).is_none());
    assert_eq!(resource(refused.0).host.retry_at.load(Relaxed), u64::MAX);
    h.with_submit_lock_for_test(|| evict_if_pressured(h));

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2, "both pressure failures must be warned");
    assert!(events[0]["evicted_count"].parse::<u64>().unwrap() >= 1);
    assert!(events[0]["evicted_bytes"].parse::<u64>().unwrap() > 0);
    let address = format!("Some({})", refused.0 as usize);
    for (event, reason) in events.iter().zip(["allocation_refused", "submit_pressure"]) {
        assert_eq!(event["reason"], format!("{reason:?}"));
        assert_eq!(event["resource"], address);
        assert_eq!(event["width"], "Some(128)");
        assert_eq!(event["height"], "Some(128)");
        assert_eq!(event["format"], "Some(28)");
        assert!(event["texture_bytes"].starts_with("Some("));
        assert!(event.contains_key("evicted_count") && event.contains_key("evicted_bytes"));
        assert_eq!(event["remaining_pressure"], "true");
        assert_eq!(event["texture_limit_bytes"], "Some(0)");
        assert!(event.contains_key("accounted_bytes"));
    }
}
