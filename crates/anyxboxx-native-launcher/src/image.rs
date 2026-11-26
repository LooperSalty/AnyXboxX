//! PE32+ image layout and base relocations for the native launcher.
//!
//! This module prepares bytes only; it does not allocate executable memory or call an entry point.

use std::fmt;

const DOS_HEADER_SIZE: usize = 64;
const NT_HEADERS_SIZE: usize = 264;
const SECTION_SIZE: usize = 40;
const RELOCATION_DIRECTORY: usize = 5;
const DIR64: u16 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    InvalidPe,
    NotPe32Plus,
    SectionOutsideImage,
    MissingRelocations,
    InvalidRelocations,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPe => "fichier PE invalide",
            Self::NotPe32Plus => "seul le format PE32+ x86-64 est accepté",
            Self::SectionOutsideImage => "section hors de l'image",
            Self::MissingRelocations => "base préférée occupée et aucune table de relocation",
            Self::InvalidRelocations => "table de relocation invalide",
        })
    }
}

impl std::error::Error for ImageError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub address: u32,
    pub virtual_size: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

/// Image data and the fields needed for allocation and section protection.
pub struct Image {
    pub bytes: Vec<u8>,
    pub preferred_base: u64,
    pub entry_rva: u32,
    pub headers_size: u32,
    pub sections: Vec<Section>,
    directories: [(u32, u32); 16],
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn range(offset: usize, size: usize, limit: usize) -> Option<std::ops::Range<usize>> {
    let end = offset.checked_add(size)?;
    (end <= limit).then_some(offset..end)
}

impl Image {
    pub fn parse(file: &[u8]) -> Result<Self, ImageError> {
        if file.len() < DOS_HEADER_SIZE || u16_at(file, 0) != Some(0x5a4d) {
            return Err(ImageError::InvalidPe);
        }
        let nt = u32_at(file, 60).ok_or(ImageError::InvalidPe)? as usize;
        range(nt, NT_HEADERS_SIZE, file.len()).ok_or(ImageError::InvalidPe)?;
        if u16_at(file, nt + 4) != Some(0x8664) || u16_at(file, nt + 24) != Some(0x20b) {
            return Err(ImageError::NotPe32Plus);
        }
        let optional = nt + 24;
        let image_size = u32_at(file, optional + 56).ok_or(ImageError::InvalidPe)? as usize;
        let headers_size = u32_at(file, optional + 60).ok_or(ImageError::InvalidPe)?;
        let headers = range(0, headers_size as usize, file.len()).ok_or(ImageError::InvalidPe)?;
        if headers.end > image_size {
            return Err(ImageError::InvalidPe);
        }
        let section_table = optional
            .checked_add(u16_at(file, nt + 20).ok_or(ImageError::InvalidPe)? as usize)
            .ok_or(ImageError::InvalidPe)?;
        let section_count = u16_at(file, nt + 6).ok_or(ImageError::InvalidPe)? as usize;
        range(section_table, section_count * SECTION_SIZE, file.len())
            .ok_or(ImageError::InvalidPe)?;
        let mut bytes = vec![0; image_size];
        bytes[..headers.end].copy_from_slice(&file[headers]);
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let offset = section_table + index * SECTION_SIZE;
            let section = Section {
                virtual_size: u32_at(file, offset + 8).unwrap(),
                address: u32_at(file, offset + 12).unwrap(),
                raw_size: u32_at(file, offset + 16).unwrap(),
                characteristics: u32_at(file, offset + 36).unwrap(),
            };
            if section.raw_size != 0 {
                let raw = u32_at(file, offset + 20).unwrap() as usize;
                let src = range(raw, section.raw_size as usize, file.len())
                    .ok_or(ImageError::SectionOutsideImage)?;
                let dst = range(
                    section.address as usize,
                    section.raw_size as usize,
                    image_size,
                )
                .ok_or(ImageError::SectionOutsideImage)?;
                bytes[dst].copy_from_slice(&file[src]);
            }
            sections.push(section);
        }
        let mut directories = [(0, 0); 16];
        for (index, directory) in directories.iter_mut().enumerate() {
            *directory = (
                u32_at(file, optional + 112 + index * 8).unwrap(),
                u32_at(file, optional + 116 + index * 8).unwrap(),
            );
        }
        Ok(Self {
            bytes,
            preferred_base: u64_at(file, optional + 24).unwrap(),
            entry_rva: u32_at(file, optional + 16).unwrap(),
            headers_size,
            sections,
            directories,
        })
    }

    pub fn directory(&self, index: usize) -> Option<(u32, u32)> {
        self.directories.get(index).copied()
    }

    /// Apply IMAGE_REL_BASED_DIR64 entries, ignoring other relocation types like the C++ loader.
    pub fn relocate(&mut self, actual_base: u64) -> Result<(), ImageError> {
        let delta = actual_base.wrapping_sub(self.preferred_base);
        if delta == 0 {
            return Ok(());
        }
        let (address, size) = self.directories[RELOCATION_DIRECTORY];
        if address == 0 {
            return Err(ImageError::MissingRelocations);
        }
        let area = range(address as usize, size as usize, self.bytes.len())
            .ok_or(ImageError::InvalidRelocations)?;
        let mut cursor = area.start;
        while cursor < area.end {
            let page = u32_at(&self.bytes, cursor).ok_or(ImageError::InvalidRelocations)?;
            let size =
                u32_at(&self.bytes, cursor + 4).ok_or(ImageError::InvalidRelocations)? as usize;
            if size < 8 {
                break;
            }
            let block = range(cursor, size, area.end).ok_or(ImageError::InvalidRelocations)?;
            for offset in (cursor + 8..block.end.saturating_sub(1)).step_by(2) {
                let entry = u16_at(&self.bytes, offset).ok_or(ImageError::InvalidRelocations)?;
                if entry >> 12 == DIR64 {
                    let target = (page as usize)
                        .checked_add((entry & 0xfff) as usize)
                        .ok_or(ImageError::InvalidRelocations)?;
                    let old = u64_at(&self.bytes, target).ok_or(ImageError::InvalidRelocations)?;
                    self.bytes[target..target + 8]
                        .copy_from_slice(&old.wrapping_add(delta).to_le_bytes());
                }
            }
            cursor = block.end;
        }
        Ok(())
    }
}
