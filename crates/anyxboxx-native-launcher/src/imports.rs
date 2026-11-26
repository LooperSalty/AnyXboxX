//! Decode the native image's imports without loading any libraries.

use crate::image::Image;
use std::fmt;

const IMPORT_DIRECTORY: usize = 1;
const ORDINAL_FLAG: u64 = 0x8000_0000_0000_0000;

#[derive(Debug, PartialEq, Eq)]
pub enum Symbol {
    Name(Vec<u8>),
    Ordinal(u16),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Import {
    pub dll: Vec<u8>,
    pub symbol: Symbol,
    pub slot: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidImports;

impl fmt::Display for InvalidImports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("table d'imports invalide")
    }
}
impl std::error::Error for InvalidImports {}

fn read<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], InvalidImports> {
    data.get(offset..offset.checked_add(N).ok_or(InvalidImports)?)
        .ok_or(InvalidImports)?
        .try_into()
        .map_err(|_| InvalidImports)
}

fn text(data: &[u8], offset: usize) -> Result<Vec<u8>, InvalidImports> {
    let tail = data.get(offset..).ok_or(InvalidImports)?;
    let end = tail.iter().position(|&b| b == 0).ok_or(InvalidImports)?;
    Ok(tail[..end].to_vec())
}

pub fn imports(image: &Image) -> Result<Vec<Import>, InvalidImports> {
    let (address, _) = image.directory(IMPORT_DIRECTORY).ok_or(InvalidImports)?;
    if address == 0 {
        return Ok(Vec::new());
    }
    let data = &image.bytes;
    let mut entry = address as usize;
    let mut result = Vec::new();
    loop {
        let lookup = u32::from_le_bytes(read(data, entry)?) as usize;
        let name = u32::from_le_bytes(read(data, entry + 12)?) as usize;
        let slots = u32::from_le_bytes(read(data, entry + 16)?) as usize;
        if name == 0 {
            break;
        }
        let dll = text(data, name)?;
        let mut lookup = if lookup == 0 { slots } else { lookup };
        let mut slot = slots;
        loop {
            let value = u64::from_le_bytes(read(data, lookup)?);
            if value == 0 {
                break;
            }
            // Check the IAT even if the lookup table is elsewhere.
            let _: [u8; 8] = read(data, slot)?;
            let symbol = if value & ORDINAL_FLAG != 0 {
                Symbol::Ordinal(value as u16)
            } else {
                let name = usize::try_from(value).map_err(|_| InvalidImports)?;
                Symbol::Name(text(data, name.checked_add(2).ok_or(InvalidImports)?)?)
            };
            result.push(Import {
                dll: dll.clone(),
                symbol,
                slot,
            });
            lookup = lookup.checked_add(8).ok_or(InvalidImports)?;
            slot = slot.checked_add(8).ok_or(InvalidImports)?;
        }
        entry = entry.checked_add(20).ok_or(InvalidImports)?;
    }
    Ok(result)
}

#[derive(Debug, PartialEq, Eq)]
pub enum BindError {
    InvalidTable,
    Missing(Vec<Import>),
}

/// Resolve imports and write their IAT entries. Missing symbols are reported as an error.
pub fn bind(
    image: &mut Image,
    mut resolve: impl FnMut(&Import) -> Option<u64>,
) -> Result<(), BindError> {
    let imports = imports(image).map_err(|_| BindError::InvalidTable)?;
    let mut missing = Vec::new();
    for import in imports {
        if let Some(address) = resolve(&import).filter(|&value| value != 0) {
            image.bytes[import.slot..import.slot + 8].copy_from_slice(&address.to_le_bytes());
        } else {
            missing.push(import);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(BindError::Missing(missing))
    }
}
