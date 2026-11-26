//! Textures shaders write that the title's own code then reads on the CPU.
//! On the console the CPU and the GPU share memory, so what a compute pass
//! stores into a texture is in title memory for the CPU to read; here it
//! stays in the texture's host copy. The title's ocean is one: its wave
//! passes write a 256x256 height map (R32_FLOAT) and a displacement map
//! (R32G32_FLOAT) that its CPU code samples, which is what boats float on.
//! Without the texels there the physics sees no water: boats sink.
//!
//! Off by default; `VICTORY_OCEAN_READBACK=1` switches it on. A list in which
//! shaders write a texture of a kind in `VICTORY_TEXTURE_BACK` (comma-
//! separated `<width>x<height>f<DXGI format>`, by default the kinds the
//! title was seen reading: `256x256f41,256x256f16,64x64f16,128x128f36`)
//! copies it into a read-back buffer of ours when it closes; once the list
//! has run ([`crate::host`]'s `read_backs`, which the relay and the late
//! settles already call for every list) the texels are tiled into the
//! texture's title memory in the console's layout, XOR swizzle modes
//! included (`victory_formats::texture::Layout::for_gpu`).
//!
//! Ported from Geckomacabre/vx-layer (commit eee5667, the copy-back), with
//! the XOR addressing from MiamiGame/XTP (commit 00c60fb), both GPL-3.0.
//!
//! shortcut: by kind, not by texture: textures of those kinds the title
//! never reads are copied back too (about a megabyte per such list). The
//! copy shows what the list's last write left; a title read in between sees
//! the frame before, as it would before the console's GPU has run.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use victory_formats::texture::{Dimension, Layout, Swizzle};
use windows::Win32::Graphics::Direct3D12::*;

use crate::object::This;

#[cfg(test)]
mod tests;

/// Whether the copy-back is on (`VICTORY_OCEAN_READBACK=1`).
fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_OCEAN_READBACK").is_some_and(|v| v == "1"))
}

/// The kinds copied back: (width, height, format).
fn kinds() -> &'static [(u64, u32, u32)] {
    static KINDS: OnceLock<Vec<(u64, u32, u32)>> = OnceLock::new();
    KINDS.get_or_init(|| {
        if !on() {
            return Vec::new();
        }
        const DEFAULT: &str = "256x256f41,256x256f16,64x64f16,128x128f36";
        let list = std::env::var("VICTORY_TEXTURE_BACK").unwrap_or_else(|_| DEFAULT.to_owned());
        let kinds: Vec<(u64, u32, u32)> = list
            .split(',')
            .filter_map(|k| {
                let (size, format) = k.trim().split_once('f')?;
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?, format.parse().ok()?))
            })
            .collect();
        tracing::info!(?kinds, "textures of these kinds that shaders write are copied back into title memory (VICTORY_OCEAN_READBACK=1)");
        kinds
    })
}

/// The tile (swizzle) modes whose textures are copied back: `VICTORY_TEXTURE_BACK_MODES`, a
/// comma list of mode numbers, "0" (linear only) when unset.
fn modes_allowed() -> &'static [u32] {
    static MODES: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    MODES.get_or_init(|| {
        std::env::var("VICTORY_TEXTURE_BACK_MODES")
            .unwrap_or_else(|_| "0".into())
            .split(',')
            .filter_map(|m| m.trim().parse().ok())
            .collect()
    })
}

/// Whether title texture `resource` is of a kind copied back.
pub(crate) fn wanted(resource: This) -> bool {
    let kinds = kinds();
    !kinds.is_empty() && crate::resource::texture_kind(resource).is_some_and(|k| kinds.contains(&k))
}

/// A texture's copy into a read-back buffer, recorded into a list, and where
/// its texels go once the list has run.
pub(crate) struct Back {
    buffer: ID3D12Resource,
    buffer_bytes: usize,
    packed_linear: bool,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    mips: u32,
    layout: Layout,
    /// The texture's title memory and its layout's bytes.
    at: usize,
    size: usize,
}

/// Records into `list` the copy of `texture` (title resource `resource`'s
/// host copy, in the copy-source state) into a read-back buffer.
pub(crate) fn record(list: &ID3D12GraphicsCommandList, resource: This, texture: &ID3D12Resource) -> Option<Back> {
    let host = crate::host::host()?;
    let (layout, at, size, mode) = crate::resource::title_texture(resource)?;
    // SAFETY: a live texture.
    let desc = unsafe { texture.GetDesc() };
    if !layout.addressable() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(mode, format = desc.Format.0, "a texture to copy back has no addressing in its swizzle mode: not copied back"));
        return None;
    }
    // Only the tile modes allowed (`VICTORY_TEXTURE_BACK_MODES`, default "0": linear): the
    // ocean's height maps are linear; copies in modes 2 and 27 turned the water to foam with
    // black patches (2026-11-25), likely the base-swizzle assumption of the XOR addressing.
    if !modes_allowed().contains(&mode) {
        return None;
    }
    arm_log(resource, at, &desc, size, mode);
    let mips = u32::from(desc.MipLevels.max(1));
    let subresources = mips * u32::from(desc.DepthOrArraySize.max(1));
    let n = subresources as usize;
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
    let mut rows = vec![0u32; n];
    let mut row_bytes = 0u64;
    let mut total = 0u64;
    // SAFETY: out arrays of `n` entries. The single row size is requested
    // only for a texture with one subresource.
    unsafe {
        host.device.GetCopyableFootprints(
            &desc,
            0,
            subresources,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            (n == 1).then_some(&raw mut row_bytes),
            Some(&mut total),
        )
    };
    let packed_linear = total == size as u64 && packed_linear(&layout, &footprints, &rows, row_bytes, total);
    let buffer = host.read_back_buffer(total)?;
    for (i, footprint) in footprints.iter().enumerate() {
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer the location never releases.
            pResource: unsafe { std::mem::transmute_copy(&buffer) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *footprint },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: i as u32 },
        };
        // SAFETY: an open list; the texture is in the copy-source state.
        unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    }
    Some(Back {
        buffer,
        buffer_bytes: total as usize,
        packed_linear,
        footprints,
        rows,
        mips,
        layout,
        at,
        size,
    })
}

/// A copy of the whole readback can land directly if one subresource
/// covers both buffers, with identical rows and no padding on either side.
fn packed_linear(
    layout: &Layout,
    footprints: &[D3D12_PLACED_SUBRESOURCE_FOOTPRINT],
    rows: &[u32],
    row_bytes: u64,
    total: u64,
) -> bool {
    let ([mip], [footprint], [height]) = (layout.mips.as_slice(), footprints, rows) else {
        return false;
    };
    let title_pitch = u64::from(mip.pitch) * u64::from(layout.desc.element_bytes);
    layout.desc.swizzle == Swizzle::LINEAR
        && layout.desc.dimension == Dimension::Tex2d
        && layout.slices == 1
        && mip.offset == 0
        && footprint.Offset == 0
        && footprint.Footprint.Depth == 1
        && mip.pitch == mip.width
        && mip.height == *height
        && title_pitch == row_bytes
        && row_bytes == u64::from(footprint.Footprint.RowPitch)
        && total == row_bytes * u64::from(*height)
}

/// Logs the first copy of each texture: where it lies, its format and size,
/// and the tile mode its texels go in.
fn arm_log(resource: This, at: usize, desc: &D3D12_RESOURCE_DESC, size: usize, mode: u32) {
    static SEEN: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 32 && !seen.contains(&at) {
        seen.push(at);
        tracing::info!(
            resource = ?resource,
            address = format_args!("{at:#x}"),
            format = desc.Format.0,
            width = desc.Width,
            height = desc.Height,
            title_bytes = size,
            swizzle_mode = mode,
            "a texture shaders write is armed for copy-back into title memory (VICTORY_OCEAN_READBACK=1)"
        );
    }
}

/// Texture copy-backs landed in title memory since the last frame line.
pub(crate) static LANDED: AtomicU64 = AtomicU64::new(0);

/// Once the list that recorded `back` has run: its texels, tiled into the
/// title's layout, into the texture's title memory.
pub(crate) fn land(back: &Back) {
    let mut data = std::ptr::null_mut();
    // SAFETY: our read-back buffer, written by the list that has run.
    unsafe {
        if back.buffer.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
            return;
        }
    }
    // SAFETY: the mapped byte length is the width of the read-back buffer.
    let data = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), back.buffer_bytes) };
    let landed = if back.packed_linear {
        crate::host::copy_mapped(back.at as *mut u8, data.as_ptr(), back.size, true);
        true
    } else {
        land_tiled(back, data)
    };
    // SAFETY: mapped above, and its contents are no longer accessed.
    unsafe { back.buffer.Unmap(0, None) };
    if landed {
        LANDED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Maximum retained capacity of the per-thread scratch buffer used by
/// [`land_tiled`]; larger readbacks drop their buffer on exit.
const TILED_SCRATCH_CAP: usize = 4 << 20;

thread_local! {
    static TILED_SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn with_tiled_scratch<R>(size: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
    struct Guard(Option<Vec<u8>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(buf) = self.0.take()
                && buf.capacity() <= TILED_SCRATCH_CAP
            {
                let _ = TILED_SCRATCH.try_with(|cell| {
                    if let Ok(mut slot) = cell.try_borrow_mut() {
                        *slot = buf;
                    }
                });
            }
        }
    }
    let mut buf = TILED_SCRATCH
        .try_with(|cell| cell.try_borrow_mut().ok().map(|mut s| std::mem::take(&mut *s)))
        .ok()
        .flatten()
        .unwrap_or_default();
    if buf.len() < size {
        buf.resize(size, 0);
    }
    let mut guard = Guard(Some(buf));
    let slice = &mut guard.0.as_mut().unwrap()[..size];
    f(slice)
}

fn land_tiled(back: &Back, data: &[u8]) -> bool {
    #[cfg(test)]
    tests::note_scratch(back.size);
    with_tiled_scratch(back.size, |tiled| {
        // Bytes of title memory the subresources do not cover stay as they are.
        crate::host::copy_mapped(tiled.as_mut_ptr(), back.at as *const u8, back.size, false);
        for (i, (footprint, &rows)) in back.footprints.iter().zip(&back.rows).enumerate() {
            let (mip, slice) = (i as u32 % back.mips, i as u32 / back.mips);
            let pitch = footprint.Footprint.RowPitch as usize;
            // The last row's padding may be outside the mapped byte length.
            let linear = &data[footprint.Offset as usize..];
            if let Err(e) = back.layout.tile(tiled, mip, slice, linear, pitch, pitch * rows as usize) {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| tracing::warn!(?e, mip, slice, "a texture copied back could not be tiled"));
                return false;
            }
        }
        crate::host::copy_mapped(back.at as *mut u8, tiled.as_ptr(), back.size, true);
        true
    })
}
