use std::cell::Cell;
use std::ptr::null;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32G32_FLOAT,
};
use windows::core::Interface;

use super::*;
use crate::host::{Host, HostList, host};
use crate::object::{S_OK, guid, slot};
use crate::resource::{self, ResourceDesc};

const GUARD: u8 = 0xa5;
const GUARD_BYTES: usize = 64;

thread_local! {
    static SCRATCH: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

pub(super) fn note_scratch(bytes: usize) {
    let (total, peak) = SCRATCH.get();
    SCRATCH.set((total + bytes, peak.max(bytes)));
}

/// A title texture backed by synthetic CPU memory and an ordinary host
/// D3D12 texture. The upload buffer holds a new pattern each round.
struct Image {
    resource: This,
    title: Vec<u8>,
    active_row: usize,
    title_pitch: usize,
    height: usize,
    upload: ID3D12Resource,
    upload_ptr: *mut u8,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

impl Image {
    fn new(host: &Host, width: u32, height: u32, format: DXGI_FORMAT) -> Self {
        let words = [
            D3D12_RESOURCE_DIMENSION_TEXTURE2D.0 as u32,
            0,
            0,
            0,
            width,
            0,
            height,
            0x1_0001,
            format.0 as u32,
            1,
            0,
            1, // Console linear layout.
            0,
            0,
        ];
        // SAFETY: ResourceDesc is the layout of these integer fields.
        let desc: ResourceDesc = unsafe { std::mem::transmute(words) };
        let layout = resource::texture_layout(&desc).unwrap();
        let plane = &layout.planes[0].layout;
        let mip = &plane.mips[0];
        let bpe = plane.desc.element_bytes as usize;
        let title_pitch = mip.pitch as usize * bpe;
        let active_row = mip.width as usize * bpe;
        let mut title = vec![GUARD; layout.size as usize + GUARD_BYTES];
        let mut resource = null();
        let i = ID3D12Resource::IID;
        let iid = guid(i.data1, i.data2, i.data3, i.data4);
        assert_eq!(
            resource::create_at_address(
                null(),
                title.as_mut_ptr() as u64,
                &desc,
                0,
                null(),
                &iid,
                &mut resource,
            ),
            S_OK,
        );
        let texture = resource::host_resource(resource).unwrap();
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0;
        // SAFETY: one live subresource and room for its footprint and size.
        unsafe {
            host.device.GetCopyableFootprints(
                &texture.GetDesc(),
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut total),
            );
        }
        let (upload, upload_ptr) = host.upload_buffer(total).unwrap();
        Self {
            resource,
            title,
            active_row,
            title_pitch,
            height: height as usize,
            upload,
            upload_ptr,
            footprint,
        }
    }

    fn record(&self, list: &HostList, seed: u8) -> Back {
        let texture = resource::host_resource(self.resource).unwrap();
        let row_pitch = self.footprint.Footprint.RowPitch as usize;
        for y in 0..self.height {
            // SAFETY: the D3D12 footprint leaves room for the active bytes of every row.
            unsafe {
                self.upload_ptr
                    .add(y * row_pitch)
                    .write_bytes(pattern(seed, y), self.active_row)
            };
        }
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the location borrows the upload buffer without releasing it.
            pResource: unsafe { std::mem::transmute_copy(&self.upload) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: self.footprint,
            },
        };
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the location borrows the texture without releasing it.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        crate::list::ensure_state(list, self.resource, D3D12_RESOURCE_STATE_COPY_DEST.0 as u32);
        // SAFETY: an open list with both resources alive and in the copy states.
        unsafe { list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
        crate::list::ensure_state(
            list,
            self.resource,
            D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32,
        );
        record(&list.list, self.resource, texture).unwrap()
    }

    fn check(&self, seed: u8) {
        let (rows, guard) = self.title.split_at(self.title_pitch * self.height);
        for (y, row) in rows.chunks_exact(self.title_pitch).enumerate() {
            let (active, padding) = row.split_at(self.active_row);
            let expected = pattern(seed, y);
            assert!(
                active.iter().all(|&byte| byte == expected),
                "texels changed at row {y}"
            );
            assert!(
                padding.iter().all(|&byte| byte == GUARD),
                "row padding changed at row {y}"
            );
        }
        assert_eq!(guard, &[GUARD; GUARD_BYTES]);
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let release = slot::<extern "system" fn(This) -> u32>(self.resource, 0x10);
        release(self.resource);
    }
}

fn pattern(seed: u8, row: usize) -> u8 {
    seed.wrapping_add((row as u8).wrapping_mul(13))
}

#[test]
fn packed_linear_readbacks_skip_cpu_scratch() {
    let Some(host) = host() else {
        eprintln!("Skipped packed linear texture readbacks because no D3D12 device was available");
        return;
    };
    const ROUNDS: u8 = 64;
    let images = [
        Image::new(host, 256, 256, DXGI_FORMAT_R32_FLOAT),
        Image::new(host, 256, 256, DXGI_FORMAT_R32G32_FLOAT),
    ];
    let active = images
        .iter()
        .map(|image| image.active_row * image.height)
        .sum::<usize>();
    let list = host.new_list().unwrap();
    let mut landing = Duration::ZERO;
    SCRATCH.set((0, 0));
    for round in 0..ROUNDS {
        let seed = round.wrapping_mul(17);
        let backs = [
            images[0].record(&list, seed),
            images[1].record(&list, seed ^ 0x7f),
        ];
        list.close();
        host.execute(&[&list]);

        let started = Instant::now();
        for back in &backs {
            land(back);
        }
        landing += started.elapsed();

        images[0].check(seed);
        images[1].check(seed ^ 0x7f);
        list.reset();
    }
    let (scratch, peak) = SCRATCH.get();
    let landed = active * usize::from(ROUNDS);
    eprintln!(
        "adapter={:?}, readbacks={}, texel_bytes={landed}, scratch_bytes={scratch}, peak_scratch_bytes={peak}, cpu_copy_bytes={}, land_us={}",
        crate::host::adapter_summary(),
        u32::from(ROUNDS) * 2,
        landed + 2 * scratch,
        landing.as_micros(),
    );
    assert_eq!(
        scratch, 0,
        "packed linear readbacks used CPU scratch after every GPU copy"
    );
}

#[test]
fn linear_readback_preserves_row_padding() {
    let Some(host) = host() else {
        eprintln!("Skipped padded linear texture readback because no D3D12 device was available");
        return;
    };
    let image = Image::new(host, 65, 64, DXGI_FORMAT_R32_FLOAT);
    assert!(image.active_row < image.title_pitch);
    let list = host.new_list().unwrap();
    let back = image.record(&list, 7);
    list.close();
    host.execute(&[&list]);
    land(&back);
    image.check(7);
}

#[test]
fn padded_linear_readback_reuses_capped_cpu_scratch() {
    let first = with_tiled_scratch(8192, |buf| {
        buf.fill(0x33);
        buf.as_ptr() as usize
    });
    let second = with_tiled_scratch(4096, |buf| buf.as_ptr() as usize);
    assert_eq!(first, second, "small tiled readbacks reuse thread-local scratch");

    with_tiled_scratch(TILED_SCRATCH_CAP + 4096, |huge| {
        huge[0] = 1;
        huge[TILED_SCRATCH_CAP] = 2;
    });
    let cap = TILED_SCRATCH.with(|cell| cell.borrow().capacity());
    assert!(cap <= TILED_SCRATCH_CAP);
}
