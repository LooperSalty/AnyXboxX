//! Warm single-worker THIN translation on D3D12, without submission.
//! Run alone with `cargo test --release -p victory-gpu --lib thin_translation_bench -- --ignored --nocapture`.
//! Use `--features diagnostics` for the D3D12 wire comparison test. Run the
//! timing test with the default features and VICTORY_THIN_TIMES unset.
use super::*;
use crate::pipeline::list_bench::Scene;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16_UINT;

struct Frame {
    chunks: Vec<Vec<u64>>,
    draws: usize,
}

impl Frame {
    fn new(s: &Scene, draws: usize) -> Self {
        let mut frame = Self {
            chunks: Vec::new(),
            draws,
        };
        frame.put(RESET, &[s.draw[0] as u64]);
        frame.put(HEAPS, &[1, s.heap as u64]);
        frame.put(Plain::TOPOLOGY as u64, &[4]);
        frame.put(Plain::GRAPHICS_VIEW as u64, &[0, s.ring]);
        let index_bytes = 36 * size_of::<u16>() as u64;
        frame.put(
            INDEX_BUFFER,
            &[
                1,
                s.index,
                index_bytes | ((DXGI_FORMAT_R16_UINT.0 as u64) << 32),
            ],
        );
        for i in 0..draws as u64 {
            if i % 32 == 0 {
                frame.put(
                    Plain::SET_PIPELINE as u64,
                    &[s.draw[(i / 32 % 2) as usize] as u64],
                );
            }
            frame.put(
                Plain::GRAPHICS_TABLE as u64,
                &[2, s.heap_start + 32 * (i % 3)],
            );
            frame.put(Plain::GRAPHICS_VIEW as u64, &[1, s.ring + (i % 128) * 256]);
            frame.put(GRAPHICS_CONSTANTS, &[3, 4, i | (2 << 32), 3 | (4 << 32)]);
            frame.put(Plain::DRAW_INDEXED as u64, &[36, 1, 0, 0, i % 4]);
            if i % 64 == 0 {
                frame.put(BARRIER, &[1, 2, 0, 0, 0]); // global UAV
                frame.put(
                    Plain::COPY_RESOURCE as u64,
                    &[s.textures[7] as u64, s.textures[0] as u64],
                );
            }
        }
        frame.put(CLOSE, &[]);
        frame
    }

    fn put(&mut self, op: u64, args: &[u64]) {
        if self.chunks.last().is_none_or(|c| c.len() >= CHUNK) {
            self.chunks.push(Vec::new());
        }
        let chunk = self.chunks.last_mut().unwrap();
        chunk.push(op | (args.len() as u64) << 16);
        chunk.extend_from_slice(args);
    }

    fn replay(&self, list: This, profile: bool, fast: bool) -> Duration {
        let before = crate::draw::drawn();
        let mut rows = [(0u64, Duration::ZERO); COPY_TEXTURE as usize + 1];
        let start = Instant::now();
        for chunk in &self.chunks {
            if !profile {
                translate_with(list, chunk, || false, fast);
                continue;
            }
            let mut rest = &chunk[..];
            while let Some(&header) = rest.first() {
                let (command, next) = rest.split_at(1 + (header >> 16) as usize);
                let row = &mut rows[(header & 0xffff) as usize];
                let at = Instant::now();
                translate_with(list, command, || false, fast);
                row.1 += at.elapsed();
                row.0 += 1;
                rest = next;
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(crate::draw::drawn() - before, self.draws as u64);
        if profile {
            for (op, name) in [
                (RESET, "reset"),
                (HEAPS, "heaps"),
                (INDEX_BUFFER, "index buffer"),
                (Plain::TOPOLOGY as u64, "topology"),
                (Plain::SET_PIPELINE as u64, "pipeline"),
                (Plain::GRAPHICS_VIEW as u64, "root view"),
                (Plain::GRAPHICS_TABLE as u64, "table"),
                (GRAPHICS_CONSTANTS, "constants"),
                (Plain::DRAW_INDEXED as u64, "indexed draw"),
                (BARRIER, "barrier"),
                (Plain::COPY_RESOURCE as u64, "copy"),
                (CLOSE, "close"),
            ] {
                let (calls, time) = rows[op as usize];
                eprintln!(
                    "{name:14} {calls:5} calls {:8.0} ns/call",
                    time.as_nanos() as f64 / calls as f64
                );
            }
        }
        elapsed
    }
}

#[test]
#[ignore = "CPU timing; run alone in release with --ignored --nocapture"]
fn thin_translation_bench() {
    assert!(
        !timing::on(),
        "unset VICTORY_THIN_TIMES for the wall-time comparison"
    );
    crate::host::host().expect("benchmark requires D3D12");
    let scene = Scene::new();
    let back = boxed(0, std::ptr::null(), false);
    let list = std::ptr::from_ref(&*back).cast();
    super::super::close(list);
    for draws in [2_000, 5_000, 9_000] {
        let frame = Frame::new(&scene, draws);
        for fast in [false, true] {
            frame.replay(list, false, fast);
        }
        let mut times = [[0.0; 9]; 2];
        for round in 0..9 {
            for side in 0..2 {
                let mode = (round + side) % 2;
                times[mode][round] = frame.replay(list, false, mode == 1).as_secs_f64() * 1000.0;
            }
        }
        for (mode, mut samples) in times.into_iter().enumerate() {
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{draws} draws, {} chunks, FAST={mode}, warm median of 9: {:.3} ms",
                frame.chunks.len(),
                samples[4]
            );
        }
        for fast in [false, true] {
            eprintln!("per-op profile FAST={}", u8::from(fast));
            frame.replay(list, true, fast);
        }
        if draws == 9_000 {
            probes(&scene, list);
        }
    }
}

// Isolate the repeated CPU operations of the six texture bindings and
// three root CBVs. All resources and descriptors are already warm.
fn probes(s: &Scene, back: This) {
    use std::hint::black_box;
    let mut warm = Frame::new(s, 8);
    assert_eq!(warm.chunks.last_mut().unwrap().pop(), Some(CLOSE));
    warm.replay(back, false, false);
    let list = host_list(back).unwrap();
    let heap = crate::descriptor::heap(s.heap).host().unwrap();
    let resource = s.textures[0];
    let shadow = crate::shadow::find(s.ring, 1).unwrap();
    let each = |name: &str, work: &mut dyn FnMut()| {
        let mut ns = [0u128; 5];
        for t in &mut ns {
            let start = Instant::now();
            for _ in 0..30_000 {
                work();
            }
            *t = start.elapsed().as_nanos();
        }
        ns.sort();
        eprintln!("warm {name:22} {:6.1} ns", ns[2] as f64 / 30_000.0);
    };
    each("descriptor mirror", &mut || {
        black_box(heap.mirror(0, s.heap_start, true));
    });
    each("host copy", &mut || {
        black_box(crate::resource::host_copy(resource));
    });
    each("twin lookup", &mut || {
        black_box(crate::resource::twin(resource));
    });
    each("texture upload check", &mut || {
        crate::texture::upload_from(list, resource, 0)
    });
    each("texture state check", &mut || {
        crate::list::ensure_state(list, resource, crate::draw::SHADER_RESOURCE)
    });
    each("shadow lookup", &mut || {
        black_box(crate::shadow::find(s.ring, 1));
    });
    each("shadow refresh", &mut || shadow.refresh(list, s.ring, 256));
    super::super::close(back);
}

/// A process of its own keeps the draw counters, synthetic resources and
/// (with diagnostics) D3D12 recorder independent of parallel GPU tests.
#[test]
fn thin_repeated_reads_and_command_equivalence() {
    const CHILD: &str = "VICTORY_TEST_THIN_REPLAY";
    if std::env::var_os(CHILD).is_none() {
        let folder = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        let thread = std::thread::current();
        child
            .args(["--exact", thread.name().expect("test name"), "--nocapture"])
            .env(CHILD, "1")
            .env("VICTORY_THIN_FAST", "1");
        if cfg!(feature = "diagnostics") {
            child
                .env("VICTORY_RECORD", folder.path())
                .env("VICTORY_RECORD_ALWAYS", "1");
        }
        let out = child.output().unwrap();
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 passed"),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    crate::host::host().expect("THIN command equivalence requires D3D12");
    let scene = Scene::new();
    let back = boxed(0, std::ptr::null(), false);
    let list = std::ptr::from_ref(&*back).cast();
    super::super::close(list);
    check_state_after_copy(&scene);
    for draws in [2000, 5000, 9000] {
        let frame = Frame::new(&scene, draws);
        frame.replay(list, false, false); // allocate and warm the shared host objects
        crate::resource::thin_read::HITS.set(0);
        frame.replay(list, false, false);
        assert_eq!(crate::resource::thin_read::HITS.get(), 0);
        #[cfg(feature = "diagnostics")]
        let commands = || crate::replay::test_commands(&host_list(list).unwrap().list, draws);
        #[cfg(feature = "diagnostics")]
        let reference = commands();
        assert!(fast::wanted());
        frame.replay(list, false, fast::wanted());
        assert!(
            crate::resource::thin_read::HITS.get() > draws as u64 * 3,
            "repeated reads took the fast path"
        );
        #[cfg(feature = "diagnostics")]
        {
            let fast = commands();
            assert_eq!(reference.len(), fast.len(), "D3D12 command byte count");
            assert_eq!(
                reference.iter().zip(fast.iter()).position(|(a, b)| a != b),
                None,
                "first different D3D12 command byte"
            );
        }
    }
}

fn check_state_after_copy(scene: &Scene) {
    use crate::resource::thin_read::{HITS, prepare};
    let back = boxed(0, std::ptr::null(), false);
    let list = back.host.as_ref().unwrap();
    let texture = scene.textures[0];
    assert!(!prepare(list, scene.args));
    assert!(prepare(list, texture));
    HITS.set(0);
    assert!(prepare(list, texture));
    assert_eq!(HITS.get(), 1);
    crate::list::ensure_state(list, texture, crate::list::COPY_DEST);
    assert!(prepare(list, texture));
    assert_eq!(HITS.get(), 1, "copy invalidated the earlier read");
    assert!(
        list.transition(texture, crate::draw::SHADER_RESOURCE)
            .is_none(),
        "back in shader-read state"
    );
    list.close();
}
