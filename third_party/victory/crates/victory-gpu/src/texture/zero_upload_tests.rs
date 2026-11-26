use std::process::Command;
use std::sync::atomic::Ordering::Relaxed;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4,
};

use crate::host::{MADE, host, transition};

use super::record_zero_level;

const CHILD: &str = "VICTORY_TEST_MIP_ZERO_UPLOAD";
const TEST: &str = "texture::zero_upload_tests::fresh_mips_share_zero_uploads";
const SIDE: u32 = 512;
const MIPS: usize = 3;
const ROUNDS: usize = 6;
const MARKER: u8 = 0xa6;

fn placed(
    buffer: &ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the location borrows the resource and never releases it.
        pResource: unsafe { std::mem::transmute_copy(buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: layout,
        },
    }
}

fn mip(texture: &ID3D12Resource, level: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the location borrows the resource and never releases it.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: level,
        },
    }
}

#[test]
fn fresh_mips_share_zero_uploads() {
    if std::env::var_os(CHILD).is_none() {
        // Keep the device's cache and allocation counters local to this test.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "mip zero upload probe: {}",
            output.status
        );
        return;
    }
    let Some(host) = host() else {
        eprintln!("mip zero upload probe skipped: D3D12 device unavailable");
        return;
    };
    let device = &host.device;
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(SIDE),
        Height: SIDE,
        DepthOrArraySize: 1,
        MipLevels: MIPS as u16,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    };
    let mut layouts = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); MIPS];
    let mut per_texture = 0;
    // SAFETY: the output array has one entry per mip of the test texture.
    unsafe {
        device.GetCopyableFootprints(
            &desc,
            0,
            MIPS as u32,
            0,
            Some(layouts.as_mut_ptr()),
            None,
            None,
            Some(&mut per_texture),
        );
    }
    let alignment = u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT);
    let stride = per_texture.next_multiple_of(alignment);
    let readback = host.read_back_buffer(stride * ROUNDS as u64).unwrap();
    let seed_size = u64::from(SIDE) * u64::from(SIDE) * 4;
    let (seed, data) = host.upload_buffer(seed_size).unwrap();
    // SAFETY: a freshly mapped upload buffer with seed_size bytes.
    unsafe { data.write_bytes(MARKER, seed_size as usize) };
    let mut textures = Vec::new();
    let mut lists = Vec::new();

    let before = [MADE[0].load(Relaxed), MADE[1].load(Relaxed)];
    // SAFETY: each copy uses valid footprints and live resources. The textures,
    // seed, read-back buffer and lists all live through the waited submission.
    unsafe {
        let factory = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let adapter = factory
            .EnumAdapterByLuid::<IDXGIAdapter1>(device.GetAdapterLuid())
            .unwrap();
        let desc1 = adapter.GetDesc1().unwrap();
        let name = String::from_utf16_lossy(&desc1.Description);
        eprintln!(
            "adapter={} vendor={:#06x}",
            name.trim_end_matches('\0'),
            desc1.VendorId
        );

        for round in 0..ROUNDS {
            let mut texture: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut texture,
                )
                .unwrap();
            let texture = texture.unwrap();
            let list = host.new_list().unwrap();
            let order_writes = || {
                list.list.ResourceBarrier(&[
                    transition(
                        &texture,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    ),
                    transition(
                        &texture,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    ),
                ]);
            };
            for (i, layout) in layouts.iter().enumerate() {
                let input = placed(
                    &seed,
                    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        ..*layout
                    },
                );
                list.list
                    .CopyTextureRegion(&mip(&texture, i as u32), 0, 0, 0, &input, None);
            }
            order_writes();
            // Request a small mip, then grow the zero source while the first
            // copy is still pending. The third mip fits the larger buffer.
            for level in [1, 0, 2] {
                record_zero_level(&list, host, &texture, &desc, level).unwrap();
            }
            order_writes();
            for (i, layout) in layouts.iter().enumerate() {
                let row = D3D12_BOX {
                    right: layout.Footprint.Width,
                    bottom: 1,
                    back: 1,
                    ..Default::default()
                };
                let input = placed(
                    &seed,
                    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        ..*layout
                    },
                );
                list.list
                    .CopyTextureRegion(&mip(&texture, i as u32), 0, 0, 0, &input, Some(&row));
            }
            list.list.ResourceBarrier(&[transition(
                &texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            for (i, layout) in layouts.iter().enumerate() {
                let output = placed(
                    &readback,
                    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: stride * round as u64 + layout.Offset,
                        ..*layout
                    },
                );
                list.list
                    .CopyTextureRegion(&output, 0, 0, 0, &mip(&texture, i as u32), None);
            }
            list.close();
            textures.push(texture);
            lists.push(list);
        }
    }
    let buffers = MADE[0].load(Relaxed) - before[0];
    let zeroed = MADE[1].load(Relaxed) - before[1];
    host.execute(&lists.iter().collect::<Vec<_>>());

    let mut verified = 0usize;
    // SAFETY: the copies finished. Read each mip inside its read-back range.
    unsafe {
        let mut data = std::ptr::null_mut();
        let range = D3D12_RANGE {
            Begin: 0,
            End: stride as usize * ROUNDS,
        };
        readback.Map(0, Some(&range), Some(&mut data)).unwrap();
        assert!(!data.is_null());
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), range.End);
        for round in 0..ROUNDS {
            for (i, layout) in layouts.iter().enumerate() {
                let offset = round * stride as usize + layout.Offset as usize;
                let pitch = layout.Footprint.RowPitch as usize;
                assert_eq!(pitch, layout.Footprint.Width as usize * 4);
                let len = pitch * layout.Footprint.Height as usize;
                let (written, cleared) = bytes[offset..offset + len].split_at(pitch);
                assert!(
                    written.iter().all(|&b| b == MARKER),
                    "partial write at round {round}, mip {i}"
                );
                assert!(
                    cleared.iter().all(|&b| b == 0),
                    "cleared pixels at round {round}, mip {i}"
                );
                verified += len;
            }
        }
        readback.Unmap(0, Some(&D3D12_RANGE::default()));
    }
    eprintln!(
        "textures={ROUNDS}, fresh_mips={}, zero_buffers_created={buffers}, cpu_zeroed_bytes={zeroed}, verified_bytes={verified}",
        ROUNDS * MIPS,
    );
    assert!(
        buffers <= 2,
        "fresh mips should share the zeroed upload source"
    );
    assert!(zeroed <= seed_size + seed_size / 4);
}
