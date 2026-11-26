use std::process::Command;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4,
};
use windows::core::Interface;

use super::{SLAB, alloc, stats};

const TEST_NAME: &str = "windowup::tests::reuses_live_slab_tails";
const CHILD: &str = "VICTORY_TEST_WINDOW_UPLOAD_PROBE";
const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;
const ROUNDS: usize = 6;

#[test]
fn reuses_live_slab_tails() {
    if std::env::var_os(CHILD).is_none() {
        // A fresh process keeps the small cap and pool counters local to this test.
        let child = Command::new(std::env::current_exe().unwrap())
            .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("VICTORY_CARD_WINDOW_UPLOAD_MB", "32")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&child.stdout));
        eprint!("{}", String::from_utf8_lossy(&child.stderr));
        assert!(
            child.status.success(),
            "window upload probe: {}",
            child.status
        );
        return;
    }

    let Some(host) = crate::host::host() else {
        eprintln!("window upload probe skipped: D3D12 device unavailable");
        return;
    };
    let device = &host.device;
    // Leave 768 KiB at the end of the first live slab. The next 1 MiB request
    // starts a second slab, which ends with 256 KiB free and cannot serve the
    // two 384 KiB requests. Keep every chunk live through the GPU copies.
    let mut sizes = vec![MIB; 15];
    sizes.push(256 * KIB);
    sizes.extend([MIB; 15]);
    sizes.push(768 * KIB);
    sizes.extend([384 * KIB; 2]);
    let bytes: u64 = sizes.iter().sum();
    let readback = host.read_back_buffer(bytes).expect("readback buffer");

    // SAFETY: fresh D3D12 objects. Every round completes before the list
    // and allocator are reset, and the copy ranges stay inside both buffers.
    unsafe {
        let factory = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let adapter = factory
            .EnumAdapterByLuid::<IDXGIAdapter1>(device.GetAdapterLuid())
            .unwrap();
        let desc = adapter.GetDesc1().unwrap();
        let name = String::from_utf16_lossy(&desc.Description);
        eprintln!(
            "adapter={} vendor={:#06x}",
            name.trim_end_matches('\0'),
            desc.VendorId
        );

        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap();
        let allocator: ID3D12CommandAllocator = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .unwrap();
        let list: ID3D12GraphicsCommandList = device
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
            .unwrap();
        let command_list: ID3D12CommandList = list.cast().unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let mut expected = vec![0u8; bytes as usize];
        let mut peak_upload = 0;

        for round in 0..ROUNDS {
            if round > 0 {
                allocator.Reset().unwrap();
                list.Reset(&allocator, None).unwrap();
            }
            let mut chunks = Vec::with_capacity(sizes.len());
            let mut dst = 0;
            for (i, &size) in sizes.iter().enumerate() {
                let chunk = alloc(size).expect("upload chunk");
                let byte = (round * sizes.len() + i + 1) as u8;
                chunk.ptr().write_bytes(byte, size as usize);
                expected[dst as usize..(dst + size) as usize].fill(byte);
                list.CopyBufferRegion(&readback, dst, chunk.buffer(), chunk.offset, size);
                chunks.push(chunk);
                dst += size;
            }
            let [pooled, _, overflow_live, _] = stats();
            peak_upload = peak_upload.max(pooled + overflow_live);

            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(command_list.clone())]);
            let submitted = round as u64 + 1;
            queue.Signal(&fence, submitted).unwrap();
            // A null event makes the native fence call wait for completion.
            fence
                .SetEventOnCompletion(submitted, HANDLE::default())
                .unwrap();
            assert_eq!(fence.GetCompletedValue(), submitted);

            let mut data = std::ptr::null_mut();
            let range = D3D12_RANGE {
                Begin: 0,
                End: bytes as usize,
            };
            readback.Map(0, Some(&range), Some(&mut data)).unwrap();
            assert!(!data.is_null());
            let actual = std::slice::from_raw_parts(data.cast::<u8>(), bytes as usize);
            let matches = actual == expected;
            readback.Unmap(0, Some(&D3D12_RANGE::default()));
            assert!(matches, "upload bytes differ in round {round}");
            drop(chunks);
            assert_eq!(stats()[2], 0, "overflow buffers outlived their chunks");
        }

        let [pooled, _, _, overflows] = stats();
        let created = pooled / SLAB + overflows;
        let verified = bytes * ROUNDS as u64;
        eprintln!(
            "pool_cap_bytes={}, peak_upload_bytes={peak_upload}, buffers_created={created}, overflow_allocations={overflows}, verified_bytes={verified}",
            2 * SLAB,
        );
        assert_eq!(pooled, 2 * SLAB);
        assert_eq!(overflows, 0, "live slabs had free tails for these uploads");
        assert_eq!(peak_upload, 2 * SLAB);
    }
}
