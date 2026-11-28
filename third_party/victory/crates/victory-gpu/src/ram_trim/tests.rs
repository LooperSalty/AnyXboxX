use super::{
    GIB, HostRamStatus, HostTier, MIB, SnapshotBuffer, TrimPolicy, VM_PIECE, VM_THRESHOLD, VmBlock,
};

#[test]
fn disabled_policy_has_no_thresholds_even_under_pressure() {
    assert_eq!(TrimPolicy::default().thresholds(), None);
    for total in [0, 16 * GIB, 32 * GIB, 64 * GIB] {
        for available in [0, GIB, 3 * GIB, 8 * GIB] {
            let policy = TrimPolicy::for_host(false, total, available, available);
            assert_eq!(policy.thresholds(), None);
        }
    }
}

#[test]
fn cached_policy_respects_environment() {
    const EXPECTED: &str = "VICTORY_RAM_TRIM_TEST_EXPECTED";
    const TEST: &str = "ram_trim::tests::cached_policy_respects_environment";
    if let Ok(expected) = std::env::var(EXPECTED) {
        let enabled = expected == "enabled";
        let policy = TrimPolicy::cached();
        assert_eq!(policy.thresholds().is_some(), enabled);
        let mut scratch = Vec::with_capacity(64 * VM_PIECE);
        scratch.push(42);
        let capacity = scratch.capacity();
        super::maybe_shrink_scratch_vec(&mut scratch, 64, VM_PIECE);
        assert!(scratch.is_empty());
        if !enabled {
            assert_eq!(policy, TrimPolicy::default());
            assert_eq!(scratch.capacity(), capacity);
        }
        return;
    }

    // A new process for each setting tests the real cache without modifying the
    // parallel test process's environment or trying to reset its OnceLock.
    let exe = std::env::current_exe().unwrap();
    for (setting, expected) in [
        (None, "disabled"),
        (Some("0"), "disabled"),
        (Some("yes"), "disabled"),
        (Some("1"), "enabled"),
        (Some("true"), "enabled"),
        (Some("on"), "enabled"),
    ] {
        let mut command = std::process::Command::new(&exe);
        command.args(["--exact", TEST, "--color", "never"]);
        command.env(EXPECTED, expected);
        if let Some(setting) = setting {
            command.env("VICTORY_RAM_TRIM", setting);
        } else {
            command.env_remove("VICTORY_RAM_TRIM");
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(&format!("test {TEST} ... ok")),
            "setting={setting:?}: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn opted_in_policy_scales_by_host_tier_and_memory_pressure() {
    let policy = TrimPolicy::opt_in(HostRamStatus {
        total_phys: 32 * GIB,
        avail_phys: 20 * GIB,
        avail_commit: 24 * GIB,
        ..HostRamStatus::default()
    });
    let standard = policy.thresholds().unwrap();
    assert_eq!(policy.tier, HostTier::Standard32);
    assert_eq!(standard.decommit_cold_reference_ms, Some(4_000));
    assert_eq!(standard.shadow_pool_warm_bytes, 32 * MIB as u64);
    assert_eq!(standard.staging_pool_warm_blocks, 1);
    assert!(!standard.low_memory);

    let tight = TrimPolicy::for_host(true, 16 * GIB, 8 * GIB, 8 * GIB);
    assert_eq!(tight.tier, HostTier::Tight16);
    let tight = tight.thresholds().unwrap();
    assert_eq!(tight.staging_pool_warm_blocks, 0);
    assert_eq!(tight.decommit_cold_reference_ms, Some(2_000));

    let large = TrimPolicy::for_host(true, 64 * GIB, 3 * GIB, 8 * GIB);
    assert_eq!(large.tier, HostTier::Large);
    let large = large.thresholds().unwrap();
    assert!(large.low_memory && !large.critical_memory);
    assert_eq!(large.shadow_pool_warm_bytes, 16 * MIB as u64);
    assert_eq!(large.pool_idle_ms, 2_500);

    let critical = TrimPolicy::for_host(true, 32 * GIB, 8 * GIB, GIB);
    let critical = critical.thresholds().unwrap();
    assert!(critical.low_memory && critical.critical_memory);
    assert_eq!(critical.shadow_pool_warm_bytes, 0);
    assert_eq!(critical.decommit_cold_reference_ms, Some(1_000));
}

// Both buffer types expose the same access paths. Re-release before every path
// and check the full pattern again so unchanged bytes and reservation tails count.
macro_rules! exercise_read_through {
    ($buffer:expr, $reclaimed:expr) => {{
        let mut buffer = $buffer;
        let reclaimed = $reclaimed;
        let len = buffer.len();
        let mut expected: Vec<u8> = (0..len).map(|i| (i % 251 + 1) as u8).collect();
        let at = if len > VM_PIECE {
            VM_PIECE - 3
        } else {
            len / 2
        };
        let end = at + 8;
        assert!(buffer.write_committed_range(0, &expected));

        assert_eq!(buffer.decommit_all(), reclaimed);
        assert!(buffer.ensure_commit(0..len));
        // SAFETY: the full range was just ensured and the buffer outlives the slice.
        let raw = unsafe { std::slice::from_raw_parts(buffer.raw_ptr(), len) };
        assert_eq!(raw, expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        assert_eq!(
            buffer.as_slice_committed(at..end).unwrap(),
            &expected[at..end]
        );
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        let mut copied = [0; 8];
        assert!(buffer.read_committed_range(at..end, &mut copied));
        assert_eq!(copied, expected[at..end]);
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        let patch = [201, 2, 53, 74, 125, 16, 242, 83];
        assert!(buffer.write_committed_range(at, &patch));
        expected[at..end].copy_from_slice(&patch);
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        let slice = buffer.as_mut_slice_committed(at..end).unwrap();
        assert_eq!(slice, &expected[at..end]);
        slice.reverse();
        expected[at..end].reverse();
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);

        assert_eq!(buffer.decommit_all(), reclaimed);
        assert!(buffer.ensure_range_committed(at, end));
        // SAFETY: the offsets of this slice were just ensured on this buffer.
        let raw = unsafe { std::slice::from_raw_parts_mut(buffer.raw_mut_ptr().add(at), end - at) };
        assert_eq!(raw, &expected[at..end]);
        raw.rotate_left(1);
        expected[at..end].rotate_left(1);
        assert_eq!(buffer.as_slice_committed(0..len).unwrap(), expected);
    }};
}

#[test]
fn small_snapshot_read_through_after_decommit_all() {
    let small = SnapshotBuffer::new_zeroed(713);
    assert!(matches!(small, SnapshotBuffer::Small(_)));
    exercise_read_through!(small, 0);
}

#[test]
fn large_snapshot_read_through_after_decommit_all() {
    let len = VM_THRESHOLD + 19;
    let large = SnapshotBuffer::new_zeroed(len);
    assert!(matches!(large, SnapshotBuffer::Large(_)));
    exercise_read_through!(large, expected_vm_commit(len));
}

#[test]
fn vm_block_read_through_after_decommit_all() {
    let len = 3 * VM_PIECE + 37;
    let block = VmBlock::new_zeroed(len).unwrap();
    exercise_read_through!(block, expected_vm_commit(len));
}

fn expected_vm_commit(len: usize) -> usize {
    if cfg!(windows) {
        len.next_multiple_of(VM_PIECE)
    } else {
        0
    }
}

#[test]
fn checked_access_rejects_invalid_bounds_without_touching_data() {
    let mut small = SnapshotBuffer::new_zeroed(23);
    let mut large = SnapshotBuffer::new_zeroed(VM_THRESHOLD);
    for buffer in [&mut small, &mut large] {
        let len = buffer.len();
        let reversed = std::ops::Range { start: len, end: 0 };
        assert!(buffer.write_committed_range(len - 1, &[9]));
        assert!(!buffer.ensure_commit(reversed));
        assert!(!buffer.ensure_range_committed(len + 1, len + 1));
        assert!(buffer.as_slice_committed(len..len + 1).is_none());
        assert!(buffer.as_mut_slice_committed(len..len).unwrap().is_empty());
        let mut dst = [17; 2];
        assert!(!buffer.read_committed_range(len - 1..len, &mut dst));
        assert_eq!(dst, [17; 2]);
        assert!(!buffer.write_committed_range(usize::MAX, &[7]));
        assert!(!buffer.write_committed_range(len - 1, &[7, 8]));
        assert_eq!(buffer.as_slice_committed(len - 1..len), Some(&[9][..]));
    }
}

#[test]
fn prepare_for_overwrite_retains_prefix_but_not_truncated_data() {
    let len = VM_THRESHOLD + 19;
    let mut buffer = SnapshotBuffer::new_zeroed(len);
    assert!(buffer.write_committed_range(0, &vec![31; len]));
    assert_eq!(buffer.decommit_all(), expected_vm_commit(len));
    let short = VM_PIECE + 7;
    assert!(buffer.prepare_for_overwrite(short));
    assert_eq!(
        buffer.as_slice_committed(0..short).unwrap(),
        vec![31; short]
    );
    assert!(buffer.prepare_for_overwrite(len));
    assert_eq!(
        buffer.as_slice_committed(short..len).unwrap(),
        vec![0; len - short]
    );
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_WRITE, GetWriteWatch, MEM_COMMIT, MEM_IMAGE, MEM_MAPPED,
        MEM_PRIVATE, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile, VirtualAlloc, VirtualFree, VirtualQuery,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;

    use super::super::verify_safe_for_decommit;

    fn query(ptr: *const c_void) -> MEMORY_BASIC_INFORMATION {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: valid output structure; the address is queried, not accessed.
        let result = unsafe { VirtualQuery(ptr, &mut info, size_of_val(&info)) };
        assert_eq!(result, size_of_val(&info));
        info
    }

    #[test]
    fn full_decommit_leaves_reservation_and_offset_read_recommits_only_touched_pieces() {
        let len = 4 * VM_PIECE;
        let mut block = VmBlock::new_zeroed(len).unwrap();
        let base = block.base_addr();
        let ptr = |offset| (base + offset) as *const c_void;
        let at = 2 * VM_PIECE - 2;
        let expected = [0x4a, 0x91, 0x35, 0x63];
        assert!(block.write_committed_range(at, &expected));
        assert_eq!(block.decommit_all(), len);
        assert_eq!(
            block.decommit_all(),
            0,
            "second release must retain the saved bytes"
        );
        assert_eq!(block.committed_bytes(), 0);
        for piece in 0..4 {
            let info = query(ptr(piece * VM_PIECE));
            assert_eq!(info.State, MEM_RESERVE);
            assert_eq!(info.AllocationBase as usize, base);
        }
        assert_eq!(block.as_slice_committed(at..at + 4).unwrap(), expected);
        assert_eq!(block.committed_bytes(), 2 * VM_PIECE);
        for (piece, state) in [MEM_RESERVE, MEM_COMMIT, MEM_COMMIT, MEM_RESERVE]
            .into_iter()
            .enumerate()
        {
            assert_eq!(query(ptr(piece * VM_PIECE)).State, state);
        }
        assert_eq!(block.base_addr(), base);
    }

    #[test]
    fn zero_piece_decommit_then_write_and_read_preserves_neighbours() {
        let len = 4 * VM_PIECE + 5;
        let mut snapshot = SnapshotBuffer::new_zeroed(len);
        let base = snapshot.base_addr().unwrap();
        let at = 2 * VM_PIECE - 2;
        let mut expected = vec![0; len];
        expected[0] = 0x74;
        expected[2 * VM_PIECE..3 * VM_PIECE].fill(0x67);
        assert!(snapshot.write_committed_range(0, &expected));
        assert_eq!(snapshot.decommit_zero_pieces(), 3 * VM_PIECE);
        assert_eq!(query((base + VM_PIECE) as *const c_void).State, MEM_RESERVE);
        assert_eq!(
            query((base + 4 * VM_PIECE) as *const c_void).State,
            MEM_RESERVE
        );
        let patch = [0x21, 0x42, 0x63, 0x84];
        assert!(snapshot.write_committed_range(at, &patch));
        expected[at..at + patch.len()].copy_from_slice(&patch);
        let mut dst = vec![0xff; len];
        assert!(snapshot.read_committed_range(0..len, &mut dst));
        assert_eq!(dst, expected);
    }

    #[test]
    fn range_decommit_preserves_data_and_explicit_discard_clears_saved_pieces() {
        let len = 3 * VM_PIECE + 13;
        let mut block = VmBlock::new_zeroed(len).unwrap();
        let mut expected = vec![0x51; len];
        assert!(block.write_committed_range(0, &expected));
        assert_eq!(block.decommit_byte_range(1, VM_PIECE + 1), 0);
        assert_eq!(block.decommit_byte_range(2 * VM_PIECE, len), 2 * VM_PIECE);
        assert_eq!(block.as_slice_committed(len - 13..len).unwrap(), [0x51; 13]);
        assert_eq!(block.discard_byte_range(2 * VM_PIECE, len), VM_PIECE);
        expected[2 * VM_PIECE..].fill(0);
        assert_eq!(block.as_slice_committed(0..len).unwrap(), expected);
        assert_eq!(block.decommit_all(), 4 * VM_PIECE);
        assert_eq!(block.discard_all(), 0);
        assert!(super::super::is_all_zero(
            block.as_slice_committed(0..len).unwrap()
        ));
    }

    #[test]
    fn safety_guard_accepts_committed_private_memory_but_rejects_reserved_pages() {
        let mut block = VmBlock::new_zeroed(2 * VM_PIECE).unwrap();
        let ptr = block.base_addr() as *const c_void;
        assert_eq!(query(ptr).Type, MEM_PRIVATE);
        assert!(!verify_safe_for_decommit(null(), VM_PIECE));
        assert!(!verify_safe_for_decommit(ptr, 0));
        assert!(verify_safe_for_decommit(ptr, 2 * VM_PIECE));
        assert_eq!(block.discard_byte_range(VM_PIECE, 2 * VM_PIECE), VM_PIECE);
        assert!(verify_safe_for_decommit(ptr, VM_PIECE));
        assert!(!verify_safe_for_decommit(ptr, 2 * VM_PIECE));
    }

    #[test]
    fn safety_guard_rejects_write_watch_without_clearing_dirty_bits() {
        // SAFETY: allocates an independent test reservation with write-watch enabled.
        let ptr = unsafe {
            VirtualAlloc(
                null(),
                VM_PIECE,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        };
        assert!(!ptr.is_null());
        let accepts_clean = verify_safe_for_decommit(ptr.cast_const(), VM_PIECE);
        // SAFETY: writes a byte inside the committed test page.
        unsafe { ptr.cast::<u8>().write_volatile(0x65) };
        let accepts_dirty = verify_safe_for_decommit(ptr.cast_const(), VM_PIECE);
        let mut dirty = null_mut();
        let mut count = 1;
        let mut granularity = 0;
        // SAFETY: only the first page of the test range is dirty, so one watch
        // slot suffices. The second call then frees the original allocation.
        let (status, released) = unsafe {
            let status = GetWriteWatch(0, ptr, VM_PIECE, &mut dirty, &mut count, &mut granularity);
            (status, VirtualFree(ptr, 0, MEM_RELEASE))
        };
        assert_ne!(released, 0);
        assert!(!accepts_clean && !accepts_dirty);
        assert_eq!(status, 0);
        assert_eq!(count, 1);
        assert_eq!(dirty, ptr);
    }

    #[test]
    fn safety_guard_rejects_mapped_view() {
        // SAFETY: creates a standalone pagefile-backed mapping.
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                null(),
                PAGE_READWRITE,
                0,
                VM_PIECE as u32,
                null(),
            )
        };
        assert!(!section.is_null());
        // SAFETY: maps the live mapping handle within its size.
        let view = unsafe { MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, VM_PIECE) };
        assert!(!view.Value.is_null());
        let info = query(view.Value);
        let accepted = verify_safe_for_decommit(view.Value, VM_PIECE);
        // SAFETY: releases the view and handle owned by this test.
        let (unmapped, closed) = unsafe { (UnmapViewOfFile(view), CloseHandle(section)) };
        assert_ne!(unmapped, 0);
        assert_ne!(closed, 0);
        assert_eq!(info.Type, MEM_MAPPED);
        assert!(!accepted);
    }

    #[test]
    fn safety_guard_rejects_image_range() {
        // SAFETY: a null name queries the already loaded test executable.
        let image = unsafe { GetModuleHandleW(null()) }.cast::<c_void>();
        assert!(!image.is_null());
        assert_eq!(query(image).Type, MEM_IMAGE);
        assert!(!verify_safe_for_decommit(image, 1));
    }
}
