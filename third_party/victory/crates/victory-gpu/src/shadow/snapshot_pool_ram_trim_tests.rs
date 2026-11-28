use super::*;
use crate::ram_trim::{VM_PIECE, VM_THRESHOLD};
use crate::shadow::snapshot::Snapshot;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, VirtualQuery};

fn state_at(address: usize) -> u32 {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: the reservation is still live and `info` is a valid output.
    assert_eq!(unsafe { VirtualQuery(address as *const _, &mut info, size_of_val(&info)) }, size_of_val(&info));
    info.State
}

fn backing() -> SnapshotBuffer {
    let mut bytes = SnapshotBuffer::new_zeroed(VM_THRESHOLD);
    assert!(bytes.is_large(), "test requires real VM backing");
    bytes.as_mut_slice_committed(0..VM_THRESHOLD).unwrap().fill(0x55);
    bytes
}

#[test]
fn cold_pool_discards_dead_bytes_and_reuses_the_same_address_on_demand() {
    let mut pool = Pool::default();
    let bytes = backing();
    let base = bytes.base_addr().unwrap();
    pool.insert(bytes, 10);
    let limits = Limits { warm_bytes: VM_THRESHOLD, slots: 2, large_slots: 1, idle_ms: 100 };
    assert!(pool.trim(&limits, 109).is_empty());
    assert_eq!(state_at(base), MEM_COMMIT);

    assert!(pool.trim(&limits, 110).is_empty());
    assert_eq!(pool.committed, 0);
    assert_eq!(state_at(base), MEM_RESERVE);
    let mut reused = pool.take(VM_THRESHOLD).expect("the cold reservation is still reusable");
    assert_eq!(reused.base_addr(), Some(base));
    assert_eq!(reused.committed_bytes(), 0, "take must not recommit untouched pieces");
    let partial = reused.as_mut_slice_committed(17..129).expect("recommit one piece");
    assert!(partial.iter().all(|&b| b == 0), "the previous user's nonzero bytes are dead");
    partial.fill(0x84);
    assert_eq!(reused.committed_bytes(), VM_PIECE);
    assert_eq!(state_at(base), MEM_COMMIT);
    assert_eq!(state_at(base + VM_PIECE), MEM_RESERVE);
    assert!(reused.as_slice_committed(17..129).unwrap().iter().all(|&b| b == 0x84));
}

#[test]
fn pool_caps_apply_to_hot_buffers_and_critical_pressure_drops_reservations() {
    let mut pool = Pool::default();
    let old = backing();
    let newer = backing();
    let newest = newer.base_addr();
    pool.insert(old, 5);
    pool.insert(newer, 10);
    pool.insert(SnapshotBuffer::Small(vec![7; 256]), 11);
    let limits = Limits { warm_bytes: 0, slots: 3, large_slots: 1, idle_ms: 100 };
    assert_eq!(pool.trim(&limits, 12).len(), 2);
    assert_eq!(pool.committed, 0);
    assert_eq!(pool.entries.len(), 1);
    assert_eq!(pool.entries[0].backing.base_addr(), newest);
    assert_eq!(state_at(newest.unwrap()), MEM_RESERVE);

    let critical = Limits { slots: 0, large_slots: 0, ..limits };
    assert_eq!(pool.trim(&critical, 13).len(), 1);
    assert!(pool.take(VM_THRESHOLD).is_none());
}

#[test]
fn storage_uses_legacy_allocation_by_default_and_vm_only_after_opt_in() {
    const CHILD: &str = "VICTORY_TEST_SHADOW_RAM_TRIM_CHILD";
    if let Ok(mode) = std::env::var(CHILD) {
        let mut bytes = Snapshot::take(VM_THRESHOLD);
        match mode.as_str() {
            "absent" | "0" => {
                let Snapshot::Legacy(bytes) = bytes else { panic!("OFF must keep Vec allocation") };
                assert_eq!(bytes.len(), 0);
                assert_eq!(bytes.capacity(), VM_THRESHOLD);
            }
            "1" => assert!(bytes.exclusive_backing().expect("ON must use checked storage").is_large()),
            _ => panic!("unexpected test mode"),
        }
        return;
    }
    let name = concat!(module_path!(), "::storage_uses_legacy_allocation_by_default_and_vm_only_after_opt_in");
    let (_, name) = name.split_once("::").unwrap(); // libtest omits the crate name
    for mode in ["absent", "0", "1"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", name, "--nocapture"]).env(CHILD, mode);
        if mode == "absent" {
            child.env_remove("VICTORY_RAM_TRIM");
        } else {
            child.env("VICTORY_RAM_TRIM", mode);
        }
        assert!(child.status().expect("run allocation-gate child").success(), "VICTORY_RAM_TRIM={mode}");
    }
}
