//! The blobs a read hands to the title (C++ `XGameSaveBlob` layout): an array of
//! `XGameSaveBlob`, then each name (NUL-terminated) and its data, all in the title's buffer.

use std::ffi::c_char;
use std::mem::size_of;

/// XGameSaveBlobInfo: a name and a size (read by the title, not by Rust).
#[repr(C)]
#[allow(dead_code)]
pub struct GameSaveBlobInfo {
    pub name: *const c_char,
    pub size: u32,
}

/// XGameSaveBlob: the info, then the pointer to the data inside the same buffer.
#[repr(C)]
#[allow(dead_code)]
pub struct GameSaveBlob {
    pub info: GameSaveBlobInfo,
    pub data: *mut u8,
}

/// The blobs of one read, in the order the title asked for them (or the container's order).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlobSet {
    pub names: Vec<Vec<u8>>,
    pub data: Vec<Vec<u8>>,
}

impl BlobSet {
    /// The buffer size the set needs: the array, the names with their NULs, and the data.
    pub fn required(&self) -> usize {
        let names: usize = self.names.iter().map(|name| name.len() + 1).sum();
        let data: usize = self.data.iter().map(Vec::len).sum();
        self.names.len() * size_of::<GameSaveBlob>() + names + data
    }

    /// Writes the set into `buffer`, which must hold [`BlobSet::required`] bytes and be aligned
    /// for `GameSaveBlob`. The pointers of the result point into that same buffer.
    ///
    /// # Safety
    /// `buffer` is writable for `required()` bytes and aligned for `GameSaveBlob`.
    pub unsafe fn layout(&self, buffer: *mut u8) {
        let blobs = buffer.cast::<GameSaveBlob>();
        // SAFETY: the array takes the first `names.len()` records; the rest is names then data.
        let mut cursor = unsafe { buffer.add(self.names.len() * size_of::<GameSaveBlob>()) };
        for (index, (name, data)) in self.names.iter().zip(&self.data).enumerate() {
            // SAFETY: each copy stays inside the `required()` bytes the caller promised.
            unsafe {
                std::ptr::copy_nonoverlapping(name.as_ptr(), cursor, name.len());
                cursor.add(name.len()).write(0);
                let name_ptr = cursor.cast::<c_char>().cast_const();
                cursor = cursor.add(name.len() + 1);
                std::ptr::copy_nonoverlapping(data.as_ptr(), cursor, data.len());
                blobs.add(index).write(GameSaveBlob {
                    info: GameSaveBlobInfo {
                        name: name_ptr,
                        size: data.len() as u32,
                    },
                    data: cursor,
                });
                cursor = cursor.add(data.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn the_layout_fills_the_required_size_and_points_into_the_buffer() {
        let set = BlobSet {
            names: vec![b"save".to_vec(), b"meta".to_vec()],
            data: vec![b"hello".to_vec(), Vec::new()],
        };
        let words = set.required().div_ceil(size_of::<u64>());
        let mut buffer = vec![0u64; words];
        let base = buffer.as_mut_ptr().cast::<u8>();
        // SAFETY: the buffer holds `required()` bytes and is 8-byte aligned.
        unsafe { set.layout(base) };

        // SAFETY: the first records are the two blobs written above.
        let blobs = unsafe { std::slice::from_raw_parts(base.cast::<GameSaveBlob>(), 2) };
        // SAFETY: the name pointers refer to NUL-terminated names inside the buffer.
        let name = unsafe { CStr::from_ptr(blobs[0].info.name) };
        assert_eq!(name.to_bytes(), b"save");
        assert_eq!(blobs[0].info.size, 5);
        // SAFETY: the data pointer refers to `size` bytes inside the buffer.
        let data = unsafe { std::slice::from_raw_parts(blobs[0].data, 5) };
        assert_eq!(data, b"hello");
        // SAFETY: as above, for the empty second blob.
        let name = unsafe { CStr::from_ptr(blobs[1].info.name) };
        assert_eq!((name.to_bytes(), blobs[1].info.size), (&b"meta"[..], 0));
    }

    #[test]
    fn an_empty_set_needs_no_space() {
        assert_eq!(BlobSet::default().required(), 0);
    }
}
