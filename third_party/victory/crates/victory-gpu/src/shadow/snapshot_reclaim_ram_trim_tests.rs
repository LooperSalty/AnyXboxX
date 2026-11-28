use super::*;
use crate::ram_trim::{SnapshotBuffer, VM_PIECE as PIECE};
use crate::shadow::{SNAPS, find_all, snapshot::Snapshot, take_snapshots};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};

const LEN: usize = 4 * PIECE;

struct TitleMemory(*mut u8);

impl TitleMemory {
    fn new() -> Self {
        const MEM_WRITE_WATCH: u32 = 0x20_0000;
        // SAFETY: private title fixture, independent of the snapshot reservation.
        let bytes = unsafe {
            VirtualAlloc(std::ptr::null(), LEN + PIECE, MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH, PAGE_READWRITE)
        }.cast::<u8>();
        assert!(!bytes.is_null());
        Self(bytes)
    }
}

impl Drop for TitleMemory {
    fn drop(&mut self) {
        crate::watch::title_memory_freed(self.0 as usize, LEN + PIECE);
        // SAFETY: owned by the fixture and no longer used by the tests.
        assert_ne!(unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }, 0);
    }
}

fn shadow_at(start: u64) -> Arc<Shadow> {
    let (buffer, data, gpu) = if let Some((buffer, data)) = crate::host::host().and_then(|h| h.shared_buffer(LEN as u64)) {
        // SAFETY: a live D3D12 resource.
        let gpu = unsafe { buffer.GetGPUVirtualAddress() };
        (buffer, data, gpu)
    } else {
        crate::indirect::tests::WarpHarness::new().expect("D3D12 hardware or WARP").shared_buffer(LEN as u64)
    };
    Arc::new(Shadow::new(buffer, data, gpu, start, LEN as u64))
}

fn before_of(shadow: &Shadow, fill_from: usize, value: u8) {
    let mut backing = SnapshotBuffer::new_zeroed(LEN);
    assert!(backing.is_large(), "test requires real VM backing");
    backing.as_mut_slice_committed(fill_from..LEN).unwrap().fill(value);
    let mut snapshot = Snapshot::trimmed(backing);
    snapshot.mark_settled(100);
    *shadow.before.lock().unwrap() = Some(snapshot);
    shadow.kept.lock().unwrap().0 = vec![true; shadow.pieces(0, LEN).end];
}

fn committed(shadow: &Shadow) -> usize {
    shadow.before.lock().unwrap().as_mut().unwrap().exclusive_backing().unwrap().committed_bytes()
}

#[test]
fn remap_discards_only_whole_vm_pieces_and_invalidates_intersecting_title_pieces() {
    const CHILD: &str = "VICTORY_TEST_SHADOW_RAM_TRIM_REMAP";
    if std::env::var_os(CHILD).is_none() {
        let name = concat!(module_path!(), "::remap_discards_only_whole_vm_pieces_and_invalidates_intersecting_title_pieces");
        let (_, name) = name.split_once("::").unwrap();
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, "1")
            .env("VICTORY_RAM_TRIM", "1")
            .status().expect("run shadow remap child").success());
        return;
    }
    let title = TitleMemory::new();
    // Not aligned to a title piece; VM backing pieces are always relative to the copy.
    let start = title.0 as u64 + 4;
    let shadow = shadow_at(start);
    before_of(&shadow, 0, 0x3c);
    let key = (start, LEN as u64);
    assert!(EXACT.write().unwrap().insert(key, (shadow.clone(), Default::default())).is_none());
    // This notice is sent before freeing/decommitting title pages. The title
    // span is aligned even though the exact view itself starts at +4.
    crate::watch::title_memory_freed(title.0 as usize + PIECE, 2 * PIECE);
    assert!(EXACT.write().unwrap().remove(&key).is_some());
    assert_eq!(committed(&shadow), 3 * PIECE);
    assert_eq!(shadow.kept.lock().unwrap().0, [true, false, false, true, true]);

    let mut before = shadow.before.lock().unwrap();
    let bytes = before.as_mut().unwrap().exclusive_backing().unwrap();
    let data = bytes.as_slice_committed(0..LEN).unwrap();
    assert!(data[..PIECE].iter().all(|&b| b == 0x3c));
    assert!(data[PIECE..2 * PIECE].iter().all(|&b| b == 0));
    assert!(data[2 * PIECE..].iter().all(|&b| b == 0x3c));
}

#[test]
fn cold_reclaim_waits_for_raw_workers_and_declared_ranges_then_recaptures() {
    const CHILD: &str = "VICTORY_TEST_SHADOW_RAM_TRIM_FLIGHT";
    if std::env::var_os(CHILD).is_none() {
        let name = concat!(module_path!(), "::cold_reclaim_waits_for_raw_workers_and_declared_ranges_then_recaptures");
        let (_, name) = name.split_once("::").unwrap();
        for keep in ["0", "1"] {
            assert!(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--nocapture"])
                .env(CHILD, "1")
                .env("VICTORY_RAM_TRIM", "1")
                .env("VICTORY_KEEP_SNAPSHOTS", keep)
                .env("VICTORY_WIDE_BACK", "1")
                .status().expect("run shadow copy-back child").success(), "VICTORY_KEEP_SNAPSHOTS={keep}");
        }
        return;
    }
    let title = TitleMemory::new();
    let start = title.0 as usize;
    let shadow = shadow_at(start as u64);
    before_of(&shadow, PIECE, 0x3c);
    let hot = Reclaim::Sweep { cold_ms: 10, critical: false, now: 109 };
    let cold = || Reclaim::Sweep { cold_ms: 10, critical: false, now: 110 };
    assert_eq!(reclaim(&shadow, hot), PIECE, "only the zero piece can be dropped while warm");
    assert!(shadow.kept.lock().unwrap().0.iter().all(|&is_current| is_current));
    assert_eq!(committed(&shadow), 3 * PIECE);
    let pin = shadow.before.lock().unwrap().as_ref().unwrap().pin().unwrap();
    assert_eq!(reclaim(&shadow, cold()), 0, "detached workers keep the VM backing live");
    drop(pin);
    assert_eq!(reclaim(&shadow, cold()), 3 * PIECE);
    assert_eq!(committed(&shadow), 0);
    assert!(shadow.kept.lock().unwrap().0.is_empty());

    // SAFETY: both mapped ranges are committed and only this test accesses them.
    unsafe {
        shadow.data.write_bytes(0x55, LEN);
        title.0.write_bytes(0x55, LEN);
        title.0.add(PIECE + 4).write_bytes(0x77, 4);
    }
    let a = start + PIECE;
    shadow.writing(a, PIECE);
    if !keeps() {
        assert_eq!(SNAPS.lock().unwrap().pins.len(), 1);
    }
    take_snapshots();
    assert_eq!(committed(&shadow), PIECE);
    assert_eq!(reclaim(&shadow, cold()), 0, "a declared range must survive sweeping");
    assert_eq!(reclaim(&shadow, Reclaim::Remap { at: a as u64, end: (a + PIECE) as u64 }), 0);
    assert_eq!(find_all(&[(shadow.clone(), a, a + PIECE)]), [vec![]]);

    // Stand in for a shader changing one dword of a real mapped D3D12 resource.
    // SAFETY: inside the mapping, now after capture and before comparison.
    unsafe { shadow.data.add(PIECE).write_bytes(0xa2, 4) };
    let mut found = find_all(&[(shadow.clone(), a, a + PIECE)]);
    assert_eq!(found, [vec![(PIECE, PIECE)]]);
    assert_eq!(shadow.copy_back_with(a, PIECE, found.pop()), 4);
    // SAFETY: inside the fixture, after copy-back.
    let expected = [0xa2, 0xa2, 0xa2, 0xa2, 0x77, 0x77, 0x77, 0x77, 0x55, 0x55, 0x55, 0x55];
    assert_eq!(unsafe { std::slice::from_raw_parts(title.0.add(PIECE), 12) }, &expected);
    shadow.copy_back_done(&[(a, a + PIECE)]);
}
