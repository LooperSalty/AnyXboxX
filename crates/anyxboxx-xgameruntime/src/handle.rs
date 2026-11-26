//! The handles the title holds on objects we create (contexts, containers, queries…). Each is a
//! heap record led by a magic word, so a handle of another kind, or a closed one, is refused
//! rather than read as something else. Port of the C++ `Valid` / `Destroy` pairs.

use std::ffi::c_void;

/// What every handle points at: its magic word, then the value (`repr(C)`, so the magic comes first).
/// The magic is read through a pointer cast, before the kind of the value is known.
#[repr(C)]
#[allow(dead_code)]
pub struct Handle<T> {
    magic: u32,
    value: T,
}

/// Allocates a handle of kind `magic` holding `value`.
pub fn new_handle<T>(magic: u32, value: T) -> *mut c_void {
    Box::into_raw(Box::new(Handle { magic, value })).cast()
}

/// A live handle of kind `magic`, if `handle` is one.
///
/// # Safety
/// `handle` is null or a pointer the title received from this module (its magic word is readable).
pub unsafe fn checked<'a, T>(handle: *mut c_void, magic: u32) -> Option<&'a mut T> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: a non-null handle of this module starts with its magic word.
    if unsafe { *handle.cast::<u32>() } != magic {
        return None;
    }
    // SAFETY: the magic matched, so this is a `Handle<T>` we allocated, and nothing else borrows it.
    Some(unsafe { &mut (*handle.cast::<Handle<T>>()).value })
}

/// Frees a handle of kind `magic`, if it is one.
///
/// # Safety
/// As [`checked`]; the title does not use the handle afterwards.
pub unsafe fn close<T>(handle: *mut c_void, magic: u32) {
    // SAFETY: forwarded to the caller's contract.
    if unsafe { checked::<T>(handle, magic) }.is_some() {
        // SAFETY: a live handle of this kind, allocated by `new_handle`, freed exactly once.
        drop(unsafe { Box::from_raw(handle.cast::<Handle<T>>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_answers_only_its_own_magic_and_closes_once() {
        let handle = new_handle(0x1234, 7u64);
        // SAFETY: the handle is ours and alive.
        assert_eq!(unsafe { checked::<u64>(handle, 0x1234) }.copied(), Some(7));
        // SAFETY: as above; the wrong magic is refused before the value is touched.
        assert!(unsafe { checked::<u64>(handle, 0x9999) }.is_none());
        // SAFETY: as above; closing frees the record once.
        unsafe { close::<u64>(handle, 0x1234) };
    }

    #[test]
    fn a_null_handle_is_refused() {
        // SAFETY: null is refused without being read.
        assert!(unsafe { checked::<u64>(std::ptr::null_mut(), 1) }.is_none());
    }
}
