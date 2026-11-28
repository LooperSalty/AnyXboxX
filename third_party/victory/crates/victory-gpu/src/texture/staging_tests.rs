use super::{Arc, staging, unstage};
use crate::object::This;
use crate::resource::{HostTexture, host_copy, placed_bytes, staged_first, texture_at};
use crate::texture_mip_clamp::{enabled, test_mode};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};

fn bc7_copy(side: u32) -> (This, Arc<HostTexture>) {
    let desc = [3, 0, 0, 0, side, 0, side, 0x7_0001, 98, 1, 0, 0x105, 0, 0];
    let bytes = placed_bytes(&desc).unwrap();
    // SAFETY: this reservation is kept for the lifetime of the placed texture table.
    let at = unsafe {
        VirtualAlloc(std::ptr::null(), bytes as usize, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
    };
    assert!(!at.is_null());
    let resource = texture_at(at as u64, &desc).unwrap();
    (resource, host_copy(resource).expect("BC7 host copy"))
}

#[test]
fn default_staging_uses_main_cache_and_bc7_footprints() {
    if crate::host::host().is_none() {
        return;
    }
    let _mode = test_mode(None);
    assert!(!enabled());
    for (side, first) in [(64, 5), (100, 1)] {
        let (resource, copy) = bc7_copy(side);
        let original = staging(resource, &copy, first).expect("BC7 staging");
        assert!(Arc::ptr_eq(&original, &staging(resource, &copy, first).unwrap()));
        assert_eq!(original.clamp_host_size, None);
        if side == 100 {
            let footprint = original.footprints[0].Footprint;
            assert_eq!((original.first, footprint.Width, footprint.Height), (1, 52, 52));
        }

        let _mode = test_mode(Some("1024"));
        assert!(enabled());
        let checked = staging(resource, &copy, first).expect("staging with shape checks");
        assert!(!Arc::ptr_eq(&original, &checked));
        assert_eq!(checked.clamp_host_size, Some((u64::from(side), side)));
        assert!(Arc::ptr_eq(&checked, &staging(resource, &copy, first).unwrap()));
        unstage(resource);
    }
}

#[test]
fn only_opt_in_staging_keeps_a_newer_cached_copy() {
    if crate::host::host().is_none() {
        return;
    }
    let _mode = test_mode(None);
    let (resource, old) = bc7_copy(100);
    let (_, new) = bc7_copy(100);
    assert!(new.serial > old.serial);
    // Exercise staging publication with equal-shaped copies in reverse serial
    // order. Neither copy is marked replaced, and no upload is recorded.
    staging(resource, &new, 0).unwrap();
    staging(resource, &old, 1).unwrap();
    assert_eq!(staged_first(resource), 1);
    unstage(resource);

    let _mode = test_mode(Some("1024"));
    let cached = staging(resource, &new, 0).unwrap();
    assert_eq!(staging(resource, &old, 1).unwrap().first, 1);
    assert_eq!(staged_first(resource), 0);
    assert!(Arc::ptr_eq(&cached, &staging(resource, &new, 0).unwrap()));
    unstage(resource);
}
