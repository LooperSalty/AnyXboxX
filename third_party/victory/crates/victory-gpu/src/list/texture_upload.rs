use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

use crate::host::{Host, HostList};
use crate::resource::PlacedFootprint;

use super::CopyLocation;

#[cfg(test)]
mod tests;

pub(super) fn stage(
    list: &HostList,
    host: &Host,
    footprint: PlacedFootprint,
    data: *const u8,
    bytes: u64,
    source_box: Option<&mut [u32; 6]>,
) -> Option<(CopyLocation, ID3D12Resource)> {
    let (mut footprint, start, bytes) = source_box
        .and_then(|region| footprint.crop_upload_rows(region, bytes))
        .unwrap_or((footprint, 0, bytes));
    let start = usize::try_from(start).ok()?;
    let len = usize::try_from(bytes).ok()?;
    let (buffer, staging) = list.upload(host, bytes)?;
    // Wine can report the mapped upload memory as MEM_FREE. The list owns
    // that mapping and fills it through the host-upload path on submission.
    list.defer_fill(staging, data.wrapping_add(start), len);
    footprint.offset = 0;
    let source = CopyLocation {
        resource: buffer.as_raw().cast_const().cast(),
        kind: 1, // D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT
        footprint,
    };
    Some((source, buffer))
}
