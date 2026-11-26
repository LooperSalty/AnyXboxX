//! Global memory of the model: a sparse 64-bit address space of 4 KiB pages, created by the first write
//! (a read of a page never written gives zeros). Port of `Memory` in `emu/rdna2_exec_memory.py`.
//!
//! The buffer descriptors (V#, four SGPRs) are read here too: base address (48 bits), stride and number
//! of records, as the Python `descriptor`.

use std::collections::BTreeMap;

use crate::wave::Wave;

/// The size of one page.
pub const PAGE: u64 = 4096;

/// The sparse pages, by page number; `BTreeMap` keeps them in address order for the state output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    pages: BTreeMap<u64, Box<[u8; PAGE as usize]>>,
}

impl Memory {
    /// `size` bytes at `address`; bytes of pages never written are zero.
    pub fn read(&self, address: u64, size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(size);
        let mut address = address;
        let mut remaining = size;
        while remaining > 0 {
            let page = address / PAGE;
            let offset = (address % PAGE) as usize;
            let count = remaining.min(PAGE as usize - offset);
            match self.pages.get(&page) {
                Some(bytes) => out.extend_from_slice(&bytes[offset..offset + count]),
                None => out.resize(out.len() + count, 0),
            }
            address = address.wrapping_add(count as u64);
            remaining -= count;
        }
        out
    }

    /// Writes `data` at `address`, creating the pages it touches.
    pub fn write(&mut self, address: u64, data: &[u8]) {
        let mut address = address;
        let mut rest = data;
        while !rest.is_empty() {
            let page = address / PAGE;
            let offset = (address % PAGE) as usize;
            let count = rest.len().min(PAGE as usize - offset);
            let bytes = self
                .pages
                .entry(page)
                .or_insert_with(|| Box::new([0u8; PAGE as usize]));
            bytes[offset..offset + count].copy_from_slice(&rest[..count]);
            address = address.wrapping_add(count as u64);
            rest = &rest[count..];
        }
    }

    /// Writes 32-bit words, little-endian, at `address`.
    pub fn write_words(&mut self, address: u64, words: &[u32]) {
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        self.write(address, &bytes);
    }

    /// The created pages, in address order: (address of the page, its bytes).
    pub fn pages(&self) -> impl Iterator<Item = (u64, &[u8; PAGE as usize])> {
        self.pages
            .iter()
            .map(|(page, bytes)| (page * PAGE, &**bytes))
    }
}

/// The little-endian 32-bit words of `bytes` (a whole number of words).
pub fn le_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// The buffer descriptor (V#) at SGPRs `first`..`first+3`: (base address, stride, number of records).
pub fn descriptor(wave: &Wave, first: u32) -> (u64, u32, u64) {
    let first = first as usize;
    let base = u64::from(wave.s[first]) | (u64::from(wave.s[first + 1]) & 0xFFFF) << 32;
    let stride = (wave.s[first + 1] >> 16) & 0x3FFF;
    (base, stride, u64::from(wave.s[first + 2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwritten_memory_reads_zero_and_writes_cross_pages() {
        let mut memory = Memory::default();
        assert_eq!(memory.read(0x1000, 4), vec![0; 4]);
        memory.write(PAGE - 2, &[1, 2, 3, 4]);
        assert_eq!(memory.read(PAGE - 2, 4), vec![1, 2, 3, 4]);
        assert_eq!(memory.pages().count(), 2);
    }

    #[test]
    fn words_are_written_little_endian() {
        let mut memory = Memory::default();
        memory.write_words(0x40, &[0x0403_0201]);
        assert_eq!(memory.read(0x40, 4), vec![1, 2, 3, 4]);
    }
}
