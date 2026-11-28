use super::*;
use crate::ram_trim::{GIB, TrimPolicy};

#[test]
fn staging_sweep_keeps_live_resting_and_newest_warm_blocks() {
    let policy = TrimPolicy::for_host(true, 32 * GIB, 8 * GIB, 8 * GIB);
    let idle_ms = policy.thresholds().unwrap().pool_idle_ms;
    let mut pool = Pool::new(STAGING_KEPT);
    // Fill staging blocks in halves so the texture make-ahead path does not
    // create an extra spare or reuse an allocation ID.
    let mut take = |id| pool.take(BLOCK / 2, 512, 0, || Some(id)).unwrap().0;
    let [[old, old_tail], [warm, warm_tail], [resting, rested]] =
        [1, 2, 3].map(|id| [take(id), take(id)]);
    let live = take(4);
    let sweep = REST + idle_ms + 500;
    for piece in [old, old_tail] {
        pool.give(piece, 10);
    }
    for piece in [warm, warm_tail] {
        pool.give(piece, 20);
    }
    pool.give(rested, sweep - REST);
    pool.give(resting, sweep - REST + 1);

    let gone = sweep_staging(&mut pool, sweep, STAGING_KEPT, &policy);
    assert_eq!(gone.iter().map(|block| block.memory).collect::<Vec<_>>(), [1]);
    assert!(pool.blocks[old.block].is_none());
    assert!(pool.blocks[warm.block].as_ref().unwrap().free.whole(BLOCK));
    for piece in [resting, live] {
        assert!(!pool.blocks[piece.block].as_ref().unwrap().free.whole(BLOCK));
    }
    assert_eq!(pool.resting.len(), 1);

    let critical = TrimPolicy::for_host(true, 32 * GIB, GIB, GIB);
    let gone = sweep_staging(&mut pool, sweep, STAGING_KEPT, &critical);
    assert_eq!(gone.len(), 1);
    assert!(pool.blocks[warm.block].is_none());
    assert_eq!(pool.blocks(), 2, "critical pressure cannot release occupied blocks");
}

#[test]
fn mapped_upload_respects_opt_in_and_can_be_allocated_again() {
    const CHILD: &str = "VICTORY_RAM_TRIM_POOL_TEST";
    const TEST: &str = "pool::ram_trim_tests::mapped_upload_respects_opt_in_and_can_be_allocated_again";
    let Ok(opt_in) = std::env::var(CHILD) else {
        let exe = std::env::current_exe().unwrap();
        for opt_in in [false, true] {
            let mut command = std::process::Command::new(&exe);
            command.args(["--exact", TEST]).env(CHILD, if opt_in { "1" } else { "0" });
            if opt_in {
                command.env("VICTORY_RAM_TRIM", "1");
            } else {
                command.env_remove("VICTORY_RAM_TRIM");
            }
            assert!(command.status().unwrap().success(), "opt-in={opt_in}");
        }
        return;
    };
    let opt_in = opt_in == "1";
    assert_eq!(crate::ram_trim::enabled(), opt_in);
    let policy = TrimPolicy::for_host(crate::ram_trim::enabled(), 16 * GIB, 8 * GIB, 8 * GIB);
    let host = crate::host::host().expect("D3D12 device for the mapped staging pool");
    assert_requested_adapter(host);
    let make = || host.upload_buffer(BLOCK).map(|(buffer, data)| Upload(buffer, data));
    let mut pool = Pool::new(STAGING_KEPT);
    let (piece, upload, again) = pool.take(512, 512, 0, make).unwrap();
    assert!(!again);
    // SAFETY: this piece is held exclusively and its upload mapping is live.
    unsafe { upload.1.add(piece.start as usize).write(0xa5) };
    drop(upload);
    pool.give(piece, 0);
    assert!(sweep_staging(&mut pool, REST - 1, STAGING_KEPT, &policy).is_empty());
    let idle_ms = TrimPolicy::for_host(true, 16 * GIB, 8 * GIB, 8 * GIB)
        .thresholds().unwrap().pool_idle_ms;
    assert!(sweep_staging(&mut pool, REST + idle_ms - 1, STAGING_KEPT, &policy).is_empty());
    let gone = sweep_staging(&mut pool, REST + idle_ms, STAGING_KEPT, &policy);
    assert_eq!(gone.len(), usize::from(opt_in));
    assert_eq!(pool.blocks(), usize::from(!opt_in));
    drop(gone);

    let (piece, upload, again) = pool.take(512, 512, REST + idle_ms, make).unwrap();
    assert_eq!(again, !opt_in);
    // SAFETY: the new or reused piece has a live mapping; no GPU work uses it.
    assert_eq!(unsafe { upload.1.add(piece.start as usize).read() }, if opt_in { 0 } else { 0xa5 });
    assert!(host.log_messages().is_none(), "D3D12 reported a validation error");
}

fn assert_requested_adapter(host: &crate::host::Host) {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1,
        IDXGIFactory4,
    };

    let warp = crate::switch::on("VICTORY_D3D12_WARP");
    let debug = crate::switch::on("VICTORY_D3D12_DEBUG");
    if warp || debug {
        // SAFETY: the device is live; these are read-only DXGI queries.
        let desc = unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
            let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(host.device.GetAdapterLuid()).unwrap();
            adapter.GetDesc1().unwrap()
        };
        assert_eq!(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0, warp);
    }
    if debug {
        assert!(host.device.cast::<ID3D12InfoQueue>().is_ok(), "D3D12 debug layer was requested");
    }
}
