use std::process::Command;
use std::sync::atomic::Ordering::Relaxed;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4,
};

use crate::host::{MADE, host, transition};
use crate::resource::PlacedFootprint;

use super::stage;

const TEST: &str = "list::texture_upload::tests::boxed_upload_stages_the_rows_it_copies";
const CHILD: &str = "VICTORY_TEST_BOXED_UPLOAD";
const SRC_SIDE: u32 = 1024;
const SRC_OFFSET: usize = 1024;
const DST_WIDTH: u32 = 128;
const PATCHES: u32 = 6;
const PATCH_WIDTH: u32 = 64;
const PATCH_HEIGHT: u32 = 64;
const DST_X: u32 = 32;
const PIXEL_BYTES: u32 = 4;
const UNTOUCHED: u8 = 0xA6;

fn layout(width: u32, height: u32, offset: u64) -> D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        Offset: offset,
        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Width: width,
            Height: height,
            Depth: 1,
            RowPitch: width * PIXEL_BYTES,
        },
    }
}

fn placed(
    buffer: &ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the location borrows the resource.
        pResource: unsafe { std::mem::transmute_copy(buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    }
}

#[test]
fn boxed_upload_stages_the_rows_it_copies() {
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "boxed upload probe: {}",
            output.status
        );
        return;
    }
    let Some(host) = host() else {
        eprintln!("boxed upload probe skipped: D3D12 device unavailable");
        return;
    };

    let dst_height = PATCHES * PATCH_HEIGHT;
    let dst_layout = layout(DST_WIDTH, dst_height, 0);
    let dst_bytes = u64::from(dst_layout.Footprint.RowPitch) * u64::from(dst_height);
    let src_layout = layout(SRC_SIDE, SRC_SIDE, SRC_OFFSET as u64);
    // SAFETY: the runtime passes this repr(C) footprint to D3D12 unchanged.
    let footprint: PlacedFootprint = unsafe { std::mem::transmute(src_layout) };
    let src_bytes = footprint.bytes().unwrap();
    let mut memory = vec![0x13; SRC_OFFSET + src_bytes as usize];
    let data = memory.as_mut_ptr().wrapping_add(SRC_OFFSET);
    let readback = host.read_back_buffer(dst_bytes).unwrap();
    let (seed, seed_data) = host.upload_buffer(dst_bytes).unwrap();
    // SAFETY: the seed's mapped upload allocation has dst_bytes bytes.
    unsafe { seed_data.write_bytes(UNTOUCHED, dst_bytes as usize) };
    let list = host.new_list().unwrap();

    // SAFETY: initialize a texture in COPY_DEST and record the initial fill
    // using the seed upload buffer. Both resources live through execution.
    let (texture, dst) = unsafe {
        let factory = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let adapter = factory
            .EnumAdapterByLuid::<IDXGIAdapter1>(host.device.GetAdapterLuid())
            .unwrap();
        let device = adapter.GetDesc1().unwrap();
        let name = String::from_utf16_lossy(&device.Description);
        eprintln!(
            "adapter={} vendor={:#06x}",
            name.trim_end_matches('\0'),
            device.VendorId
        );
        let mut texture: Option<ID3D12Resource> = None;
        host.device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: u64::from(DST_WIDTH),
                    Height: dst_height,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    ..Default::default()
                },
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut texture,
            )
            .unwrap();
        let texture = texture.unwrap();
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        list.list
            .CopyTextureRegion(&dst, 0, 0, 0, &placed(&seed, dst_layout), None);
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
        (texture, dst)
    };
    let boxes = (0..PATCHES)
        .map(|i| {
            let (left, top) = (128 + i * 27, 21 + i * 151);
            [left, top, 0, left + PATCH_WIDTH, top + PATCH_HEIGHT, 1]
        })
        .collect::<Vec<_>>();
    let (made, bytes) = (MADE[0].load(Relaxed), MADE[1].load(Relaxed));
    let mut staged_bytes = 0;
    for (i, &source_box) in boxes.iter().enumerate() {
        let mut box_for_host = source_box;
        let (source, _buffer) = stage(
            &list,
            host,
            footprint,
            data,
            src_bytes,
            Some(&mut box_for_host),
        )
        .unwrap();
        staged_bytes += source.footprint.bytes().unwrap();
        // SAFETY: use the same layouts as the production copy. The list
        // owns the source upload and the destinations are disjoint patches.
        unsafe {
            list.list.CopyTextureRegion(
                &dst,
                DST_X,
                i as u32 * PATCH_HEIGHT,
                0,
                (&raw const source).cast(),
                Some((&raw const box_for_host).cast()),
            );
        }
    }
    let allocated_buffers = MADE[0].load(Relaxed) - made;
    let allocated_bytes = MADE[1].load(Relaxed) - bytes;

    // SAFETY: the list has not been submitted. Populate its title-memory
    // source now to check that staging still reads at submission time.
    for (i, byte) in unsafe { std::slice::from_raw_parts_mut(data, src_bytes as usize) }
        .iter_mut()
        .enumerate()
    {
        *byte = (i.wrapping_mul(17) ^ (i >> 5) ^ (i >> 12).wrapping_mul(29)) as u8;
    }
    // SAFETY: finish the copies and read the whole destination texture.
    unsafe {
        list.list.ResourceBarrier(&[transition(
            &texture,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.list
            .CopyTextureRegion(&placed(&readback, dst_layout), 0, 0, 0, &dst, None);
    }
    list.close();
    host.execute(&[&list]);

    // SAFETY: the GPU has completed the readback.
    unsafe {
        let mut mapped = std::ptr::null_mut();
        let read = D3D12_RANGE {
            Begin: 0,
            End: dst_bytes as usize,
        };
        readback.Map(0, Some(&read), Some(&mut mapped)).unwrap();
        assert!(!mapped.is_null());
        let result = std::slice::from_raw_parts(mapped.cast::<u8>(), read.End);
        for y in 0..dst_height {
            let [left, top, ..] = boxes[(y / PATCH_HEIGHT) as usize];
            for x in 0..DST_WIDTH {
                let dst_at = ((y * DST_WIDTH + x) * PIXEL_BYTES) as usize;
                let got = &result[dst_at..dst_at + PIXEL_BYTES as usize];
                if (DST_X..DST_X + PATCH_WIDTH).contains(&x) {
                    let src_at = SRC_OFFSET
                        + (((top + y % PATCH_HEIGHT) * SRC_SIDE + left + x - DST_X) * PIXEL_BYTES)
                            as usize;
                    assert_eq!(
                        got,
                        &memory[src_at..src_at + PIXEL_BYTES as usize],
                        "{x},{y}"
                    );
                } else {
                    assert_eq!(got, [UNTOUCHED; PIXEL_BYTES as usize], "{x},{y}");
                }
            }
        }
        readback.Unmap(0, Some(&D3D12_RANGE::default()));
    }
    eprintln!(
        "patches={PATCHES}, upload_buffers_created={allocated_buffers}, upload_bytes_allocated={allocated_bytes}, cpu_staged_bytes={staged_bytes}, texture_bytes_verified={dst_bytes}",
    );
    assert_eq!(allocated_bytes, staged_bytes);
    assert_eq!(
        staged_bytes,
        u64::from(PATCHES * PATCH_HEIGHT * SRC_SIDE * PIXEL_BYTES)
    );
}
