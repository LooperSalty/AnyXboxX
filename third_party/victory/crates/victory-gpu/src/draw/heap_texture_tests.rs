use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use victory_formats::texture::Layout;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

use super::*;
use crate::descriptor::{create_heap, create_placed_srv, heap};
use crate::host::{Host, host};
use crate::object::{S_OK, guid, slot};
use crate::resource::{self, Memory, ResourceDesc};

const SIDE: u32 = 256;
const MIPS: u32 = 4;
const TEXTURES: u32 = 8;
const VIEWS: u32 = 8;
const ROUNDS: u32 = 4;
const RGBA8: u32 = DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32;
const TEXTURE_DESC: [u32; 14] = [
    3,
    0,
    0,
    0,
    SIDE,
    0,
    SIDE,
    MIPS << 16 | 1,
    RGBA8,
    1,
    0,
    0x105,
    0,
    0,
];

struct TitleHeap(This);

impl TitleHeap {
    fn new() -> Self {
        Self::of(TEXTURES * VIEWS)
    }

    fn of(slots: u32) -> Self {
        // A shader-visible view heap with room for its optional placement fields.
        let desc = [
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV.0 as u32,
            slots,
            D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE.0 as u32,
            0,
            0,
            0,
            0,
            0,
        ];
        let i = ID3D12DescriptorHeap::IID;
        let iid = guid(i.data1, i.data2, i.data3, i.data4);
        let mut out = null();
        assert_eq!(
            create_heap(null(), desc.as_ptr().cast(), &iid, &mut out),
            S_OK
        );
        Self(out)
    }
}

impl Drop for TitleHeap {
    fn drop(&mut self) {
        let release = slot::<extern "system" fn(This) -> u32>(self.0, 0x10);
        release(self.0);
    }
}

struct Source(Memory);

impl Source {
    fn write(&mut self, layout: &Layout, round: u32, texture: u32) {
        // SAFETY: the source owns the allocation and the previous list finished.
        let tiled = unsafe {
            std::slice::from_raw_parts_mut(self.0.address() as *mut u8, layout.size as usize)
        };
        for mip in 0..MIPS {
            let side = (SIDE >> mip) as usize;
            let pitch = side * 4;
            let linear: Vec<_> = (0..pitch * side)
                .map(|byte| texel_byte(round, texture, mip, byte))
                .collect();
            layout
                .tile(tiled, mip, 0, &linear, pitch, linear.len())
                .unwrap();
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        resource::release_test_placed_texture(self.0.address(), TEXTURE_DESC);
    }
}

fn texel_byte(round: u32, texture: u32, mip: u32, byte: usize) -> u8 {
    (37 * round + 13 * texture + 17 * mip + 3 * byte as u32 + (byte as u32 >> 8)) as u8
}

struct Readback {
    buffer: ID3D12Resource,
    footprints: [D3D12_PLACED_SUBRESOURCE_FOOTPRINT; MIPS as usize],
    size: usize,
}

impl Readback {
    fn record(host: &Host, list: &HostList, source: &Source) -> Self {
        let resource = resource::placed_known(source.0.address(), &TEXTURE_DESC).unwrap();
        let texture = resource::host_resource(resource).unwrap();
        crate::list::ensure_state(list, resource, D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32);
        let mut footprints = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); MIPS as usize];
        let mut size = 0;
        // SAFETY: the texture is live and the output holds all its subresources.
        unsafe {
            host.device.GetCopyableFootprints(
                &texture.GetDesc(),
                0,
                MIPS,
                0,
                Some(footprints.as_mut_ptr()),
                None,
                None,
                Some(&mut size),
            );
        }
        let buffer = host.read_back_buffer(size).unwrap();
        for (mip, footprint) in footprints.iter().enumerate() {
            let from = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: the copy location borrows the texture without releasing it.
                pResource: unsafe { std::mem::transmute_copy(texture) },
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: mip as u32,
                },
            };
            let to = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: as above, for the readback buffer.
                pResource: unsafe { std::mem::transmute_copy(&buffer) },
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: *footprint,
                },
            };
            // SAFETY: the open list names the live source and destination.
            unsafe { list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
        }
        Self {
            buffer,
            footprints,
            size: size as usize,
        }
    }

    fn check(&self, round: u32, texture: u32) {
        let mut ptr = null_mut();
        // SAFETY: the list has completed and this buffer is live.
        unsafe { self.buffer.Map(0, None, Some(&mut ptr)).unwrap() };
        // SAFETY: Map succeeded and the buffer holds at least `size` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), self.size) };
        for (mip, footprint) in self.footprints.iter().enumerate() {
            let side = (SIDE >> mip) as usize;
            let width = side * 4;
            for y in 0..side {
                let at = footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize;
                for (x, &actual) in bytes[at..at + width].iter().enumerate() {
                    let expected = texel_byte(round, texture, mip as u32, y * width + x);
                    assert_eq!(
                        actual, expected,
                        "round {round}, texture {texture}, mip {mip}, row {y}, byte {x}"
                    );
                }
            }
        }
        // SAFETY: the mapped bytes are no longer used.
        unsafe { self.buffer.Unmap(0, None) };
    }
}

#[test]
fn duplicate_texture_views_share_uploads_within_each_pass() {
    let Some(host) = host() else {
        eprintln!("Skipped streaming upload readback: no D3D12 device");
        return;
    };
    // SAFETY: the words have ResourceDesc's layout and contain only integer fields.
    let desc: ResourceDesc = unsafe { std::mem::transmute(TEXTURE_DESC) };
    let layout = resource::texture_layout(&desc)
        .unwrap()
        .planes
        .remove(0)
        .layout;
    let size = resource::placed_bytes(&TEXTURE_DESC).unwrap();
    let mut sources: Vec<_> = (0..TEXTURES)
        .map(|_| Source(Memory::zeroed(size, layout.alignment).unwrap()))
        .collect();
    let title_heap = TitleHeap::new();
    let heap = heap(title_heap.0);
    let host_heap = heap.host().unwrap();
    let state = State::default();
    let uploads_before = HEAP_TEXTURES.load(Ordering::Relaxed);
    let bytes_before = HEAP_TEXTURE_BYTES.load(Ordering::Relaxed);
    let mut peak_staging = 0;
    let mut recording = Duration::ZERO;
    let mut submitting = Duration::ZERO;

    for round in 0..ROUNDS {
        for (texture, source) in sources.iter_mut().enumerate() {
            source.write(&layout, round, texture as u32);
        }
        for index in 0..TEXTURES * VIEWS {
            let source = &sources[((index / VIEWS + round) % TEXTURES) as usize];
            let start = source.0.address();
            // Newest written slots are served first. Some views expose only smaller mips.
            let first = if index % VIEWS < VIEWS / 2 { 0 } else { 2 };
            let view = [
                RGBA8,
                D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32,
                D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                0,
                first,
                MIPS - first,
                0,
                0,
                0,
                0,
                start as u32,
                (start >> 32) as u32,
                0,
                0,
                0,
                0,
            ];
            create_placed_srv(null(), &TEXTURE_DESC, &view, heap.handle(index).unwrap());
        }

        let list = host.new_list().unwrap();
        let before = crate::texture::memory()[1];
        let started = Instant::now();
        PASS_BUDGET.set(Some((5000, 64 << 20, true)));
        state.heap_textures(&list, heap, host_heap);
        PASS_BUDGET.set(None);
        recording += started.elapsed();
        peak_staging = peak_staging.max(crate::texture::memory()[1].saturating_sub(before));
        assert!((0..TEXTURES * VIEWS).all(|index| host_heap.made(index)));
        assert!(
            host_heap
                .texture_slots_due(crate::frame::current(), 1)
                .0
                .is_empty()
        );
        let readbacks: Vec<_> = sources
            .iter()
            .map(|source| Readback::record(host, &list, source))
            .collect();
        list.close();
        let started = Instant::now();
        host.execute(&[&list]);
        submitting += started.elapsed();
        for (texture, readback) in readbacks.iter().enumerate() {
            readback.check(round, texture as u32);
        }
    }

    let uploads = HEAP_TEXTURES.load(Ordering::Relaxed) - uploads_before;
    let bytes = HEAP_TEXTURE_BYTES.load(Ordering::Relaxed) - bytes_before;
    eprintln!(
        "adapter={:?}, slots={}, staged_uploads={uploads}, staged_title_bytes={bytes}, peak_staging_bytes={peak_staging}, record_us={}, submit_and_wait_us={}",
        crate::host::adapter_summary(),
        ROUNDS * TEXTURES * VIEWS,
        recording.as_micros(),
        submitting.as_micros()
    );
    assert_eq!(uploads, u64::from(ROUNDS * TEXTURES * 2));
}

const PREPARE_CHILD: &str = "VICTORY_TEST_TEXTURE_PREPARE";
const PREPARE_TEST: &str = "draw::heap_texture_tests::prepare_threads_make_the_textures_off_the_recording_thread";

/// With `VICTORY_TEXTURE_PREPARE_THREADS=2` the pass over textures picked
/// by index makes no host copy and no staging buffer on the thread that
/// records (the prepare threads make them, and detile), and the textures
/// still arrive whole: every slot made, none left due, every level of
/// every texture as the title wrote it, round after round of new contents.
/// Run in a child process: the switch is read once, and the queue is the
/// process's.
#[test]
fn prepare_threads_make_the_textures_off_the_recording_thread() {
    if std::env::var_os(PREPARE_CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([PREPARE_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(PREPARE_CHILD, "1")
            .env("VICTORY_TEXTURE_PREPARE_THREADS", "2")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "texture prepare probe: {}", output.status);
        return;
    }
    assert_eq!(crate::texture::prepare::threads(), 2);
    let Some(host) = host() else {
        eprintln!("Skipped texture prepare probe: no D3D12 device");
        return;
    };
    // SAFETY: the words have ResourceDesc's layout and contain only integer fields.
    let desc: ResourceDesc = unsafe { std::mem::transmute(TEXTURE_DESC) };
    let layout = resource::texture_layout(&desc)
        .unwrap()
        .planes
        .remove(0)
        .layout;
    let size = resource::placed_bytes(&TEXTURE_DESC).unwrap();
    let mut sources: Vec<_> = (0..TEXTURES)
        .map(|_| Source(Memory::zeroed(size, layout.alignment).unwrap()))
        .collect();
    let title_heap = TitleHeap::new();
    let heap = heap(title_heap.0);
    let host_heap = heap.host().unwrap();
    let state = State::default();
    let uploads_before = HEAP_TEXTURES.load(Ordering::Relaxed);
    let ready_before = crate::texture::prepare::STATS[0].load(Ordering::Relaxed);
    let made_by_threads_before = crate::texture::prepare::STATS[1].load(Ordering::Relaxed);
    let ([made_here_before, _], staged_here_before) = (resource::made_here(), crate::texture::staged_here());
    let mut passes_needed = 0;

    for round in 0..ROUNDS {
        for (texture, source) in sources.iter_mut().enumerate() {
            source.write(&layout, round, texture as u32);
        }
        for index in 0..TEXTURES * VIEWS {
            let source = &sources[((index / VIEWS + round) % TEXTURES) as usize];
            let start = source.0.address();
            let first = if index % VIEWS < VIEWS / 2 { 0 } else { 2 };
            let view = [
                RGBA8,
                D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32,
                D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                0,
                first,
                MIPS - first,
                0,
                0,
                0,
                0,
                start as u32,
                (start >> 32) as u32,
                0,
                0,
                0,
                0,
            ];
            create_placed_srv(null(), &TEXTURE_DESC, &view, heap.handle(index).unwrap());
        }

        let list = host.new_list().unwrap();
        // The first pass hands the textures out; the next ones record what
        // the threads made ready. (A slot whose texture another slot took
        // in the pass that handed it out hands it out again: three passes.)
        let mut passes = 0;
        loop {
            passes += 1;
            let parts = HEAP_TEXTURE_PARTS.each_ref().map(|c| c.load(Ordering::Relaxed));
            state.heap_textures(&list, heap, host_heap);
            let due = host_heap.texture_slots_due(crate::frame::current(), 4096).0;
            let after = HEAP_TEXTURE_PARTS.each_ref().map(|c| c.load(Ordering::Relaxed));
            eprintln!(
                "round {round} pass {passes}: looked {} recorded {} handed {} waited {}; {} due now; out {:?}",
                after[0] - parts[0],
                after[1] - parts[1],
                after[12] - parts[12],
                after[13] - parts[13],
                due.len(),
                crate::texture::prepare::census(),
            );
            if due.is_empty() || passes == 4 {
                break;
            }
            assert!(
                crate::texture::prepare::wait_idle(Duration::from_secs(30)),
                "the prepare threads did not finish"
            );
        }
        passes_needed = passes_needed.max(passes);
        assert!((0..TEXTURES * VIEWS).all(|index| host_heap.made(index)), "round {round}: a slot was not made");
        assert!(
            host_heap.texture_slots_due(crate::frame::current(), 1).0.is_empty(),
            "round {round}: a slot is still due after {passes} passes"
        );
        let readbacks: Vec<_> = sources
            .iter()
            .map(|source| Readback::record(host, &list, source))
            .collect();
        list.close();
        host.execute(&[&list]);
        for (texture, readback) in readbacks.iter().enumerate() {
            readback.check(round, texture as u32);
        }
    }

    let ([made_here, _], staged_here) = (resource::made_here(), crate::texture::staged_here());
    let uploads = HEAP_TEXTURES.load(Ordering::Relaxed) - uploads_before;
    let ready = crate::texture::prepare::STATS[0].load(Ordering::Relaxed) - ready_before;
    let made_by_threads = crate::texture::prepare::STATS[1].load(Ordering::Relaxed) - made_by_threads_before;
    eprintln!(
        "adapter={:?}, uploads={uploads}, made_ready={ready}, host_copies_by_threads={made_by_threads}, \
         host_copies_here={}, staging_buffers_here={}, passes_needed={passes_needed}",
        crate::host::adapter_summary(),
        made_here - made_here_before,
        staged_here[0] - staged_here_before[0],
    );
    assert_eq!(made_here - made_here_before, 0, "host copies made on the recording thread");
    assert_eq!(staged_here[0] - staged_here_before[0], 0, "staging buffers made on the recording thread");
    assert_eq!(made_by_threads, u64::from(TEXTURES), "each texture's host copy made once, by the threads");
    let least = u64::from(ROUNDS * TEXTURES * 2);
    assert!(uploads >= least && uploads <= 2 * least, "uploads: {uploads}");
    // At least one a texture a round (a view of the smaller mips may take
    // what was made for the view of all of them).
    assert!(ready >= u64::from(ROUNDS * TEXTURES), "made ready: {ready}");
    // Two passes a round: one hands out, the next records.
    assert!(passes_needed <= 3, "passes needed: {passes_needed}");
}

/// Where the pass's time goes on this machine's card, for 512 textures of
/// 256x256 RGBA8 with 4 mips (178 MB), two rounds: the first makes their
/// host copies and upload buffers, the second finds both (the pool).
/// Prints the recording thread's parts (`HEAP_TEXTURE_PARTS`) and the
/// submission's time. With `VICTORY_TEXTURE_PREPARE_THREADS` set, the
/// prepare threads' way. A benchmark, run by hand on an idle card:
/// `cargo test --release -p victory-gpu --lib heap_texture_pass_benchmark -- --ignored --exact --nocapture`.
#[test]
#[ignore = "a benchmark: run by hand, on an idle card"]
fn heap_texture_pass_benchmark() {
    const MANY: u32 = 512;
    let Some(host) = host() else {
        eprintln!("Skipped the texture pass benchmark: no D3D12 device");
        return;
    };
    // SAFETY: the words have ResourceDesc's layout and contain only integer fields.
    let desc: ResourceDesc = unsafe { std::mem::transmute(TEXTURE_DESC) };
    let layout = resource::texture_layout(&desc)
        .unwrap()
        .planes
        .remove(0)
        .layout;
    let size = resource::placed_bytes(&TEXTURE_DESC).unwrap();
    let mut sources: Vec<_> = (0..MANY)
        .map(|_| Source(Memory::zeroed(size, layout.alignment).unwrap()))
        .collect();
    let title_heap = TitleHeap::of(MANY);
    let heap = heap(title_heap.0);
    let host_heap = heap.host().unwrap();
    let state = State::default();
    let threads = crate::texture::prepare::threads();
    for round in 0..2 {
        for (texture, source) in sources.iter_mut().enumerate() {
            source.write(&layout, round, texture as u32);
        }
        for index in 0..MANY {
            // Another texture each round: every slot is written anew.
            let start = sources[((index + round) % MANY) as usize].0.address();
            let view = [
                RGBA8,
                D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32,
                D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                0,
                0,
                MIPS,
                0,
                0,
                0,
                0,
                start as u32,
                (start >> 32) as u32,
                0,
                0,
                0,
                0,
            ];
            create_placed_srv(null(), &TEXTURE_DESC, &view, heap.handle(index).unwrap());
        }
        let list = host.new_list().unwrap();
        let parts_before = HEAP_TEXTURE_PARTS.each_ref().map(|c| c.load(Ordering::Relaxed));
        let stats_before = crate::texture::prepare::STATS.each_ref().map(|c| c.load(Ordering::Relaxed));
        let started = Instant::now();
        let mut passes_ms = Vec::new();
        if threads == 0 {
            PASS_BUDGET.set(Some((60_000, 4 << 30, true)));
            state.heap_textures(&list, heap, host_heap);
            PASS_BUDGET.set(None);
            passes_ms.push(started.elapsed().as_secs_f64() * 1e3);
        } else {
            for _ in 0..4 {
                let pass = Instant::now();
                state.heap_textures(&list, heap, host_heap);
                passes_ms.push(pass.elapsed().as_secs_f64() * 1e3);
                if host_heap.texture_slots_due(crate::frame::current(), 1).0.is_empty() {
                    break;
                }
                assert!(crate::texture::prepare::wait_idle(Duration::from_secs(60)));
            }
        }
        let wall = started.elapsed().as_secs_f64() * 1e3;
        let p: Vec<u64> = HEAP_TEXTURE_PARTS
            .iter()
            .zip(parts_before)
            .map(|(c, before)| c.load(Ordering::Relaxed) - before)
            .collect();
        let s: Vec<u64> = crate::texture::prepare::STATS
            .iter()
            .zip(stats_before)
            .map(|(c, before)| c.load(Ordering::Relaxed) - before)
            .collect();
        list.close();
        let submitting = Instant::now();
        host.execute(&[&list]);
        let submit_ms = submitting.elapsed().as_secs_f64() * 1e3;
        let ms = |us: u64| us as f64 / 1e3;
        eprintln!(
            "round {round} (threads {threads}): {} uploads of {MANY} textures ({} MB), passes {passes_ms:.1?} ms, \
             wall {wall:.1} ms; recording thread: look {:.1}, make {:.1} ({} host copies in {:.1}), stage {:.1} \
             ({} staging buffers in {:.1}, {} upload buffers new in {:.1}), rest {:.1}, before {:.1} ms; \
             threads: {} made ready, {} host copies in {:.1} ms, staging {:.1} ms, detile {:.1} ms, all {:.1} ms; \
             submission (detile, copies, wait) {submit_ms:.1} ms",
            p[1],
            (u64::from(MANY) * size) >> 20,
            ms(p[2]),
            ms(p[3]),
            p[6],
            ms(p[7]),
            ms(p[4]),
            p[8],
            ms(p[9]),
            p[10],
            ms(p[11]),
            ms(p[5]),
            ms(p[14]),
            s[0],
            s[1],
            ms(s[4]),
            ms(s[5]),
            ms(s[6]),
            ms(s[3]),
        );
        // The frame never changes here: with the threads, what a frame may
        // hand out (`VICTORY_TEXTURE_STAGE_MB`) is spent by the first round.
        let due = host_heap.texture_slots_due(crate::frame::current(), 4096).0.len();
        if threads == 0 || round == 0 {
            assert_eq!(due, 0);
        } else if due > 0 {
            eprintln!("round {round}: {due} slots wait for the next frame's budget");
        }
    }
}
