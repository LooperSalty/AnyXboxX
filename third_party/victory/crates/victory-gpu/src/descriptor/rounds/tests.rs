use super::*;
use crate::descriptor::{self, HeapDesc, Placement, SAMPLERS, SHADER_VISIBLE};
use crate::object::{IUNKNOWN, S_OK, This, slot};
use std::ptr::null;

#[test]
fn closes_coalesce_and_a_pending_close_yields_to_a_request() {
    let mut r = Rounds::new();
    assert!(r.list_closed());
    assert!(!r.list_closed());
    r.asked = 1;
    assert_eq!(r.take(), Some(Pass::Requested));
    assert!(!r.list_closed());
    r.done = 1;
    assert_eq!(r.take(), None);
    assert!(r.list_closed());
    assert_eq!(r.take(), Some(Pass::Close));
    assert!(
        !r.list_closed(),
        "abandoned lists cannot keep the worker busy"
    );
    assert_eq!(r.take(), None);
    assert_eq!((r.asked, r.done), (1, 1));
}

// Each run owns the worker, write watch and environment. In particular no
// timer or concurrent test can consume the dirty page before Close.
#[test]
fn list_close_and_submit_mirror_the_same_bytes() {
    const CHILD: &str = "VICTORY_TEST_CLOSE_HEAPS";
    if let Some(path) = std::env::var_os(CHILD) {
        std::fs::write(path, check_heap()).unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut baseline = None;
    for (early, journal, thin) in [
        ("0", "0", "0"),
        ("1", "0", "0"),
        ("0", "1", "1"),
        ("1", "1", "1"),
    ] {
        let path = dir.path().join(format!("records-{early}-{journal}-{thin}"));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "descriptor::rounds::tests::list_close_and_submit_mirror_the_same_bytes",
            ])
            .env(CHILD, &path)
            .env("VICTORY_EARLY_HEAPS", early)
            .env("VICTORY_PORT_JOURNAL", journal)
            .env("VICTORY_THIN", thin)
            .env("VICTORY_RELAY", "1")
            .env("VICTORY_HEAP_SYNC", "1")
            .env("VICTORY_HEAP_SYNC_ASIDE", "1")
            .env("VICTORY_HEAP_SYNC_EARLY", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "early={early} journal={journal} thin={thin}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(baseline.get_or_insert_with(|| bytes.clone()), &bytes);
    }
}

fn check_heap() -> Vec<u8> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;
    const SLOTS: usize = 64;
    type Record = [u8; 16];
    // SAFETY: fresh write-watched memory for our synthetic placed sampler heap.
    let memory = unsafe {
        VirtualAlloc(
            null(),
            crate::watch::PAGE,
            MEM_COMMIT | MEM_RESERVE | MEM_WRITE_WATCH,
            PAGE_READWRITE,
        )
    }
    .cast::<Record>();
    assert!(!memory.is_null());
    let mut heap = null();
    let desc = HeapDesc {
        kind: SAMPLERS,
        count: SLOTS as u32,
        flags: SHADER_VISIBLE,
        nodes: 1,
    };
    let placement = Placement {
        address: memory as u64,
        size: (SLOTS * 16) as u64,
    };
    assert_eq!(descriptor::tests::create(desc, placement, &mut heap), S_OK);
    let host = descriptor::heap(heap).host().expect("D3D12 sampler heap");
    let mut sampler = [0u8; 16];
    descriptor::create_sampler(
        null(),
        &[21, 3, 3, 3, 0, 1, 0, 0, 0, 0, 0, 0x7f7f_ffff, 0],
        sampler.as_mut_ptr() as u64,
    );
    assert_ne!(sampler, [0; 16]);

    let (mut alloc, mut list): (This, This) = (null(), null());
    assert_eq!(
        crate::list::create_allocator(null(), 0, &IUNKNOWN, &mut alloc),
        S_OK
    );
    assert_eq!(
        crate::list::create_list(null(), 0, 0, alloc, null(), &IUNKNOWN, &mut list),
        S_OK
    );
    assert_eq!(
        crate::list::thin::on(),
        std::env::var("VICTORY_THIN").unwrap() == "1"
    );
    // Establish the worker so the journal can issue a ticket at enqueue.
    drop(begin_sync_heaps());
    let mirrored = || SYNCED[2].load(Relaxed);
    let before_close = mirrored();
    let mut expected = [[0u8; 16]; SLOTS];
    expected[16..48].fill(sampler);
    expected[3] = sampler;
    // SAFETY: the synthetic heap holds SLOTS complete records.
    unsafe { std::ptr::copy_nonoverlapping(expected.as_ptr(), memory, SLOTS) };
    assert_eq!(
        slot::<extern "system" fn(This) -> i32>(list, 0x48)(list),
        S_OK
    );
    let early = std::env::var("VICTORY_EARLY_HEAPS").unwrap() == "1";
    if early {
        let start = Instant::now();
        while mirrored() == before_close {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "Close did not mirror the raw descriptor"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(host.made(3), early);
    let before_submit = mirrored();
    // A clear and a new descriptor after Close must both reach the host.
    unsafe {
        memory.add(3).write([0; 16]);
        memory.add(5).write(sampler);
    }
    if crate::port_journal::on() {
        let mark = round_after_now();
        assert_ne!(mark, 0);
        drop(begin_sync_heaps_after(mark));
    } else {
        drop(begin_sync_heaps());
    }
    // The required pass mirrors the two late changes or builds all 33 live slots.
    assert_eq!(mirrored() - before_submit, if early { 2 } else { 33 });
    expected[3] = [0; 16];
    expected[5] = sampler;
    let copy = host.copied_begin(16);
    // SAFETY: the copied buffer is locked and no jobs write these slots.
    let bytes = unsafe { copy.seen(0, SLOTS) }.to_vec();
    assert_eq!(bytes, expected.as_flattened());
    drop(copy);
    for (i, record) in expected.iter().enumerate() {
        assert_eq!(host.made(i as u32), *record != [0; 16], "slot {i}");
    }
    for obj in [list, alloc, heap] {
        slot::<extern "system" fn(This) -> u32>(obj, 0x10)(obj);
    }
    // SAFETY: the heap using this allocation has been released.
    unsafe { VirtualFree(memory.cast(), 0, MEM_RELEASE) };
    bytes
}
