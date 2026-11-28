use super::*;
use crate::object::{S_OK, slot};
use crate::resource::{
    HeapProperties, PAGEABLE_IIDS, RESOURCE_IIDS, create_committed, held_only, host_bytes, host_copy,
    placed_bytes, texture_at, want_level,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_BC7_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

struct TestMode {
    mode: Option<MipClampMode>,
    size_cap: Option<u32>,
}

impl TestMode {
    fn set(max_dim: u32, size_cap: u32) -> Self {
        Self {
            mode: TEST_MODE.replace(Some(MipClampMode::MaxDim(max_dim))),
            size_cap: crate::tiles::TEST_MAX_SIZE.replace(Some(size_cap)),
        }
    }
}

impl Drop for TestMode {
    fn drop(&mut self) {
        TEST_MODE.set(self.mode);
        crate::tiles::TEST_MAX_SIZE.set(self.size_cap);
    }
}

fn texture(side: u32, format: u32, mips: u16) -> ResourceDesc {
    ResourceDesc {
        dimension: 3,
        width: u64::from(side),
        height: side,
        alignment: 0,
        depth_or_array_size: 1,
        mip_levels: mips,
        format,
        sample_count: 1,
        sample_quality: 0,
        layout: 0,
        flags: 0,
    }
}

#[test]
fn mip_clamp_is_disabled_when_unset() {
    if std::env::var_os("VICTORY_TEXTURE_AUTO_MIP_CLAMP").is_none() {
        assert!(!enabled());
    }
    let _mode = test_mode(None);
    assert!(!enabled());
}

#[test]
fn mip_clamp_policy_respects_budget_pressure_tail_and_bc_block_alignment() {
    for raw in [None, Some(""), Some("0"), Some("off"), Some("garbage")] {
        assert_eq!(parse_mip_clamp_mode(raw), MipClampMode::Off);
    }
    for raw in ["1", "auto", " TRUE ", "on"] {
        assert_eq!(parse_mip_clamp_mode(Some(raw)), MipClampMode::Auto);
    }
    assert_eq!(parse_mip_clamp_mode(Some("1024")), MipClampMode::MaxDim(1024));
    assert_eq!(parse_mip_clamp_mode(Some("2048")), MipClampMode::MaxDim(2048));

    const GIB: u64 = 1 << 30;
    let target = |mode, budget: u64, used: u64, reserved: u64| {
        auto_mip_clamp_target_dim(mode, Some([budget * GIB, used * GIB, 0, 0]), reserved * GIB, 0, None)
    };
    assert_eq!(target(MipClampMode::Off, 8, 7, 0), None);
    assert_eq!(target(MipClampMode::Auto, 16, 8, 0), None);
    assert_eq!(target(MipClampMode::Auto, 16, 12, 2), Some(1024));
    assert_eq!(target(MipClampMode::Auto, 11, 5, 0), Some(2048));
    assert_eq!(target(MipClampMode::Auto, 11, 10, 0), Some(1024));
    assert_eq!(target(MipClampMode::Auto, 8, 2, 0), Some(1024));
    assert_eq!(auto_mip_clamp_target_dim(MipClampMode::Auto, None, 0, 0, Some(7 * GIB)), Some(1024));

    let bc7 = texture(4096, 98, 13);
    assert!(clamp_eligible(&bc7, true, 0));
    assert_eq!(compute_mip_clamp_base(&bc7, 1024), 2);
    assert_eq!(compute_mip_clamp_base(&bc7, 2048), 1);
    assert_eq!(compute_mip_clamp_base(&texture(2052, 98, 12), 1024), 0);
    assert_eq!(compute_mip_clamp_base(&texture(4104, 98, 13), 1024), 1);
    assert_eq!(compute_mip_clamp_base(&texture(4096, 28, 1), 1024), 0);
    let rgba = texture(4096, 28, 13);
    assert!(!clamp_eligible(&ResourceDesc { flags: 1, ..rgba }, false, 0));
    assert!(!clamp_eligible(&ResourceDesc { depth_or_array_size: 2, ..rgba }, true, 0));
    assert!(!clamp_eligible(&rgba, false, D3D12_RESOURCE_STATE_COPY_DEST.0 as u32));
}

fn committed(desc: &ResourceDesc) -> This {
    const PROPERTIES: HeapProperties = HeapProperties {
        kind: 1,
        cpu_pages: 0,
        pool: 0,
        creation_nodes: 0,
        visible_nodes: 0,
    };
    let null = std::ptr::null();
    let mut made = null;
    assert_eq!(create_committed(null, &PROPERTIES, 0, desc, 0, null.cast(), &RESOURCE_IIDS[0], &mut made), S_OK);
    made
}

fn release(object: This) {
    assert_eq!(slot::<extern "system" fn(This) -> u32>(object, 0x10)(object), 0);
}

#[test]
fn mip_clamped_textures_create_valid_resources_and_srvs_with_measured_vram_savings() {
    let Some(host) = crate::host::host() else { return };
    let _mode = TestMode::set(512, 0);
    let desc = texture(2048, 28, 12);
    let tex = committed(&desc);
    let (data, original) = crate::resource::texture_memory(tex).unwrap();
    assert_eq!(original.shape(), [2048, 2048, 28, 12]);
    let layout = &texture_layout(&original).unwrap().planes[0].layout;
    // SAFETY: the texture owns at least `layout.size` writable bytes.
    unsafe {
        data.cast_mut().write_bytes(0x33, layout.size as usize);
        let first = layout.mips[2].offset as usize;
        let end = layout.mips[1].offset as usize;
        data.cast_mut().add(first).write_bytes(0x7a, end - first);
    }

    let clamped = host_copy(tex).expect("clamped RGBA8 texture");
    // SAFETY: a live texture.
    let d = unsafe { clamped.texture.GetDesc() };
    assert_eq!((clamped.base, d.Width, d.Height, d.MipLevels), (2, 512, 512, 10));

    let pitch = 512 * 4;
    let readback = host.read_back_buffer(u64::from(pitch) * 512).unwrap();
    let list = host.new_list().unwrap();
    crate::texture::upload(&list, tex, &clamped.texture);
    if let Some((t, before)) = list.transition(tex, D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32) {
        // SAFETY: this barrier names the live copy held by the list.
        unsafe {
            list.list.ResourceBarrier(&[crate::host::transition(
                t,
                D3D12_RESOURCE_STATES(before as i32),
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
        }
    }
    // SAFETY: the locations borrow two live resources and do not release them.
    unsafe {
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&readback),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        Width: 512,
                        Height: 512,
                        Depth: 1,
                        RowPitch: pitch,
                    },
                },
            },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&clamped.texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
    }
    list.close();
    host.execute(&[&list]);
    drop(list);
    // SAFETY: the synchronous copy filled at least `pitch` bytes.
    unsafe {
        let mut at = std::ptr::null_mut();
        readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: pitch as usize }), Some(&mut at)).unwrap();
        let row = std::slice::from_raw_parts(at.cast::<u8>(), pitch as usize);
        assert!(row.iter().all(|&byte| byte == 0x7a), "host mip 0 must contain original mip 2");
        readback.Unmap(0, None);
    }

    let mut heap = std::ptr::null();
    let heap_desc = crate::descriptor::HeapDesc { kind: 0, count: 16, flags: 1, nodes: 0 };
    assert_eq!(
        crate::descriptor::create_heap(std::ptr::null(), &heap_desc, &PAGEABLE_IIDS[0], &mut heap),
        S_OK,
    );
    let heap_obj = crate::descriptor::heap(heap);
    let srv = heap_obj.host().unwrap();
    for (index, mips) in [(0, 12), (1, 1)] {
        let view = [28, 4, 5768, 0, 0, mips, 0, 0, 0, 0];
        let handle = heap_obj.handle(index).unwrap();
        crate::descriptor::create_srv(std::ptr::null(), tex, &view, handle);
        // The texture is already uploaded. Publish explicitly if another
        // test's submission postponed the initial mirror.
        let (_, key) = srv.prepare_texture_slot(index, handle).unwrap();
        srv.publish_uploaded_slot(index, handle, key);
        assert_eq!(srv.kept_in(index).unwrap().as_raw(), clamped.texture.as_raw());
    }

    let snapshot = srv.dynamic(1).expect("dynamic SRV slot");
    let view = crate::descriptor::View::Shader(tex as usize, Some([28, 4, 5768, 0, 0, 12, 0, 0, 0, 0]));
    srv.make_at(snapshot, &view);
    let old_texture = clamped.texture.as_raw();
    assert_eq!(srv.kept_in(snapshot).unwrap().as_raw(), old_texture);

    ensure_unclamped(tex);
    let whole = host_copy(tex).expect("unclamped RGBA8 texture");
    // SAFETY: a live texture.
    let d = unsafe { whole.texture.GetDesc() };
    assert_eq!((whole.base, d.Width, d.Height, d.MipLevels), (0, 2048, 2048, 12));
    let saved = clamped.clamp_saved_bytes.unwrap();
    assert_eq!(saved, host_bytes(&whole.texture) - host_bytes(&clamped.texture));
    assert!(saved >= 19 << 20);

    assert_eq!(crate::texture::staging_shape_for_test(tex, &whole), Some((12, 0, 2048, 2048)));
    assert_eq!(crate::texture::staging_shape_for_test(tex, &clamped), Some((10, 2, 512, 512)));
    assert_eq!(crate::texture::cached_staging_mips_for_test(tex), Some(12));
    drop(clamped);

    let upload = host.new_list().unwrap();
    crate::texture::upload(&upload, tex, &whole.texture);
    upload.close();
    host.execute(&[&upload]);
    for index in 0..2 {
        assert_eq!(srv.kept_in(index).unwrap().as_raw(), whole.texture.as_raw());
    }
    assert_eq!(srv.kept_in(snapshot).unwrap().as_raw(), old_texture);
    srv.release(srv.id(), snapshot, 1);
    assert!(srv.kept_in(snapshot).is_none());
    release(heap);
    release(tex);

    TEST_MODE.set(Some(MipClampMode::MaxDim(1024)));
    let bc7 = committed(&texture(4096, 98, 13));
    let copy = host_copy(bc7).expect("clamped BC7 texture");
    // SAFETY: a live texture.
    let d = unsafe { copy.texture.GetDesc() };
    assert_eq!((copy.base, d.Width, d.Height, d.MipLevels, d.Format), (2, 1024, 1024, 11, DXGI_FORMAT_BC7_UNORM));
    assert!(copy.clamp_saved_bytes.unwrap() >= 19 << 20);
    release(bc7);
}

#[test]
fn warp_creates_a_clamped_bc7_resource_and_srv_with_measured_savings() {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    let Ok(adapter) = crate::host::warp_adapter() else { return };
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: the adapter is live and the output pointer is ours.
    if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device) }.is_err() {
        return;
    }
    let device = device.unwrap();
    let full = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: 4096,
        Height: 4096,
        DepthOrArraySize: 1,
        MipLevels: 13,
        Format: DXGI_FORMAT_BC7_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let tail = D3D12_RESOURCE_DESC { Width: 1024, Height: 1024, MipLevels: 11, ..full };
    // SAFETY: descriptor creation and size queries on our device.
    unsafe {
        let full_bytes = device.GetResourceAllocationInfo(0, &[full]).SizeInBytes;
        let tail_bytes = device.GetResourceAllocationInfo(0, &[tail]).SizeInBytes;
        assert_ne!(full_bytes, u64::MAX);
        assert_ne!(tail_bytes, u64::MAX);
        assert!(full_bytes.saturating_sub(tail_bytes) >= 19 << 20);
        let props = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let mut texture: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(
            &props, D3D12_HEAP_FLAG_NONE, &tail, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, None, &mut texture,
        ).expect("create BC7 tail on WARP");
        let heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: 1,
            ..Default::default()
        }).unwrap();
        let mut srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: tail.Format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MipLevels: 13, ..Default::default() },
            },
        };
        crate::descriptor::host::adjust_srv_for_base_mip(&mut srv, 2, 11);
        device.CreateShaderResourceView(&texture.unwrap(), Some(&srv), heap.GetCPUDescriptorHandleForHeapStart());
        assert!(device.GetDeviceRemovedReason().is_ok());
    }
}

#[test]
fn held_only_and_auto_mip_clamp_compose_without_thrashing() {
    use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
    if crate::host::host().is_none() || !held_only() {
        return;
    }
    let _mode = TestMode::set(128, 64);
    let desc = [3, 0, 0, 0, 256, 0, 256, 0x9_0001, 98, 1, 0, 0x105, 0, 0];
    let bytes = placed_bytes(&desc).unwrap();
    // SAFETY: a dedicated address range held by the placed texture table.
    let at = unsafe { VirtualAlloc(std::ptr::null(), bytes as usize, MEM_RESERVE, PAGE_READWRITE) };
    assert!(!at.is_null());
    let tex = texture_at(at as u64, &desc).unwrap();

    want_level(tex, 3);
    let held = host_copy(tex).unwrap();
    assert_eq!(held.base, 3);

    want_level(tex, 0);
    let capped = host_copy(tex).unwrap();
    assert_eq!(capped.base, 2, "the size cap is stricter than the budget floor");
    assert_ne!(capped.serial, held.serial);
    want_level(tex, 0);
    assert_eq!(host_copy(tex).unwrap().serial, capped.serial);

    ensure_unclamped(tex);
    let whole = host_copy(tex).unwrap();
    assert_eq!(whole.base, 0);
    want_level(tex, 0);
    assert_eq!(host_copy(tex).unwrap().serial, whole.serial);

    // Repeat with the budget floor stricter than the size cap.
    TEST_MODE.set(Some(MipClampMode::MaxDim(32)));
    let at = unsafe { VirtualAlloc(std::ptr::null(), bytes as usize, MEM_RESERVE, PAGE_READWRITE) };
    assert!(!at.is_null());
    let tex = texture_at(at as u64, &desc).unwrap();
    let clamped = host_copy(tex).unwrap();
    assert_eq!(clamped.base, 3);
    want_level(tex, 0);
    assert_eq!(host_copy(tex).unwrap().serial, clamped.serial);
    ensure_unclamped(tex);
    assert_eq!(host_copy(tex).unwrap().base, 0);
}
