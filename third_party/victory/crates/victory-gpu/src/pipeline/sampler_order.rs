//! Persistent static sampler ordering, off unless `VICTORY_SAMPLER_ORDER=1`
//! (also accepts `true` or `on`).
//!
//! Some drivers index static samplers in creation order and embed those
//! indices in cached pipelines. Workers can otherwise reach their root
//! signatures in a different order each run, invalidating driver cache hits.
//!
//! When enabled, replay `static-samplers.txt` from the pipeline cache folder
//! before creating the first root signature with static samplers. Keep one
//! root signature per listed sampler alive for the run, and append newly
//! encountered samplers under the same lock. On NVIDIA, the ordering opt-in
//! enables host library reuse only for pipelines whose samplers were all
//! replayed at startup, without any refusals.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;

/// The list, in the pipeline cache's folder: a line a sampler, thirteen
/// hexadecimal words ([`Key`]).
const FILE: &str = "static-samplers.txt";

/// Cap the number pre-created to leave room in the driver's sampler table.
const MOST: usize = 1024;

/// A static sampler as the host is told of it, whole: filter, the three
/// address modes, the bias' bits, the anisotropy, the comparison, the border
/// colour, the bits of both level bounds, register, space and visibility.
type Key = [u32; 13];

fn key(s: &D3D12_STATIC_SAMPLER_DESC) -> Key {
    [
        s.Filter.0 as u32,
        s.AddressU.0 as u32,
        s.AddressV.0 as u32,
        s.AddressW.0 as u32,
        s.MipLODBias.to_bits(),
        s.MaxAnisotropy,
        s.ComparisonFunc.0 as u32,
        s.BorderColor.0 as u32,
        s.MinLOD.to_bits(),
        s.MaxLOD.to_bits(),
        s.ShaderRegister,
        s.RegisterSpace,
        s.ShaderVisibility.0 as u32,
    ]
}

fn description(k: &Key) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER(k[0] as i32),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(k[1] as i32),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(k[2] as i32),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(k[3] as i32),
        MipLODBias: f32::from_bits(k[4]),
        MaxAnisotropy: k[5],
        ComparisonFunc: D3D12_COMPARISON_FUNC(k[6] as i32),
        BorderColor: D3D12_STATIC_BORDER_COLOR(k[7] as i32),
        MinLOD: f32::from_bits(k[8]),
        MaxLOD: f32::from_bits(k[9]),
        ShaderRegister: k[10],
        RegisterSpace: k[11],
        ShaderVisibility: D3D12_SHADER_VISIBILITY(k[12] as i32),
    }
}

fn line(k: &Key) -> String {
    k.iter().map(|w| format!("{w:x}")).collect::<Vec<_>>().join(" ")
}

/// A line of the file, or nothing for one that is not thirteen words (a line
/// cut short when the process ended).
fn parse(line: &str) -> Option<Key> {
    let mut k = [0u32; 13];
    let mut words = line.split_ascii_whitespace();
    for w in &mut k {
        *w = u32::from_str_radix(words.next()?, 16).ok()?;
    }
    words.next().is_none().then_some(k)
}

/// The samplers in their kept order.
#[derive(Default)]
struct Listed {
    order: Vec<Key>,
    known: HashSet<Key>,
}

impl Listed {
    /// The lines of the file, each sampler once, at most [`MOST`].
    fn of(text: &str) -> Listed {
        let mut listed = Listed::default();
        for k in text.lines().filter_map(parse) {
            if listed.order.len() >= MOST {
                break;
            }
            listed.place(k);
        }
        listed
    }

    /// The place `k` takes at the end if it has none yet (and the list has
    /// room).
    fn place(&mut self, k: Key) -> Option<usize> {
        if self.order.len() >= MOST || !self.known.insert(k) {
            return None;
        }
        self.order.push(k);
        Some(self.order.len() - 1)
    }
}

/// A root signature kept so that its sampler keeps its place.
struct Kept {
    _root: ID3D12RootSignature,
}
// SAFETY: D3D12 objects are free-threaded; this one is only held.
unsafe impl Send for Kept {}

struct Order {
    listed: Listed,
    /// Only the keys from a clean startup replay, never keys learned later.
    replayed: HashSet<Key>,
    kept: Vec<Kept>,
    file: Option<PathBuf>,
    /// Samplers met with the list full: counted, not made early.
    over: u64,
}

static ORDER: Mutex<Option<Order>> = Mutex::new(None);

/// Samplers added to the list in this run.
pub(crate) static NEW: AtomicU64 = AtomicU64::new(0);

fn opt_in(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "on"))
}

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| opt_in(std::env::var("VICTORY_SAMPLER_ORDER").ok().as_deref()))
}

/// A root signature that holds the one static sampler `k` and nothing else.
fn root_of(device: &ID3D12Device, k: &Key) -> windows::core::Result<ID3D12RootSignature> {
    let sampler = description(k);
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumStaticSamplers: 1,
                pStaticSamplers: &sampler,
                ..Default::default()
            },
        },
    };
    // SAFETY: the description points at `sampler`, alive through the call;
    // the blob is read in full before it is released.
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes)
    }
}

impl Order {
    /// The file's samplers, each made in its turn.
    fn begin(device: &ID3D12Device) -> Order {
        let began = std::time::Instant::now();
        let file = super::library::folder().map(|d| d.join(FILE));
        let text = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let listed = Listed::of(&text);
        let kept: Vec<_> = listed
            .order
            .iter()
            .filter_map(|k| root_of(device, k).ok().map(|root| Kept { _root: root }))
            .collect();
        let refused = listed.order.len() - kept.len();
        tracing::info!(
            listed = listed.order.len(),
            refused,
            ms = began.elapsed().as_secs_f64() * 1e3,
            "static sampler order replayed before root signature creation"
        );
        // One refusal can shift the places of other samplers, too.
        let replayed = if refused == 0 { listed.known.clone() } else { HashSet::new() };
        Order { listed, replayed, kept, file, over: 0 }
    }

    /// `k`, met for the first time: made, kept and written to the end of the
    /// file.
    fn add(&mut self, device: &ID3D12Device, k: Key) {
        let Some(place) = self.listed.place(k) else {
            if self.listed.order.len() >= MOST && self.listed.known.insert(k) {
                self.over += 1;
                if self.over == 1 {
                    tracing::warn!(
                        most = MOST,
                        "more static samplers than are listed: the rest take their places as they come"
                    );
                }
            }
            return;
        };
        match root_of(device, &k) {
            Ok(root) => self.kept.push(Kept { _root: root }),
            Err(e) => tracing::warn!(error = %e, place, "a static sampler's own root signature was refused"),
        }
        if let Some(file) = &self.file {
            use std::io::Write;
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(file))
                .and_then(|mut f| writeln!(f, "{}", line(&k)));
            if let Err(e) = written {
                tracing::warn!(error = %e, "the list of static samplers was not written");
            }
        }
        NEW.fetch_add(1, Ordering::Relaxed);
        tracing::info!(place, sampler = %line(&k), "a static sampler met for the first time takes the next place");
    }
}

/// Reserve new samplers before creating the pipeline's root signature.
/// Return true only if ordering is enabled and all of its samplers already
/// held their places after a clean startup replay.
pub(crate) fn before(device: &ID3D12Device, samplers: &[D3D12_STATIC_SAMPLER_DESC]) -> bool {
    with_ordering(samplers, enabled, |samplers| {
        let mut order = ORDER.lock().unwrap_or_else(|e| e.into_inner());
        let order = order.get_or_insert_with(|| Order::begin(device));
        let mut settled = true;
        for k in samplers.iter().map(key) {
            if order.replayed.contains(&k) {
                continue;
            }
            settled = false;
            if !order.listed.known.contains(&k) {
                order.add(device, k);
            }
        }
        settled
    })
}

// Pass the switch and preparation separately so tests can exercise the gate
// without changing the process environment or initializing the global order.
fn with_ordering(
    samplers: &[D3D12_STATIC_SAMPLER_DESC],
    is_enabled: impl FnOnce() -> bool,
    prepare: impl FnOnce(&[D3D12_STATIC_SAMPLER_DESC]) -> bool,
) -> bool {
    !samplers.is_empty() && is_enabled() && prepare(samplers)
}

#[cfg(test)]
#[path = "sampler_order_library_tests.rs"]
mod library_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn sampler(filter: i32, register: u32) -> D3D12_STATIC_SAMPLER_DESC {
        D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER(filter),
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
            MipLODBias: -0.75,
            MaxAnisotropy: 8,
            ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
            BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
            MinLOD: 0.5,
            MaxLOD: f32::MAX,
            ShaderRegister: register,
            RegisterSpace: 3,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        }
    }

    fn check_enabled(samplers: &[D3D12_STATIC_SAMPLER_DESC], check: impl FnOnce(&[D3D12_STATIC_SAMPLER_DESC])) {
        assert!(with_ordering(
            samplers,
            || opt_in(Some("1")),
            |samplers| {
                check(samplers);
                true
            },
        ));
    }

    #[test]
    fn sampler_order_is_disabled_by_default() {
        for value in [None, Some("0"), Some(""), Some("false"), Some("off"), Some("2")] {
            assert!(!opt_in(value), "{value:?}");
            assert!(!with_ordering(
                &[sampler(0, 0)],
                || opt_in(value),
                |_| panic!("disabled ordering must not prepare samplers"),
            ));
        }
        for value in ["1", "true", "on"] {
            assert!(opt_in(Some(value)), "{value:?}");
        }
        assert!(!with_ordering(&[], || panic!("no samplers"), |_| panic!("no samplers")));
    }

    #[test]
    fn a_sampler_comes_back_from_its_line_whole() {
        check_enabled(&[sampler(0x15, 4)], |samplers| {
            let k = key(&samplers[0]);
            let parsed = parse(&line(&k)).expect("a complete sampler line");
            let back = description(&parsed);
            assert_eq!(key(&back), k);
            assert_eq!(back.MipLODBias, -0.75);
            assert_eq!(back.MaxLOD, f32::MAX);
            assert_eq!(back.ShaderVisibility, D3D12_SHADER_VISIBILITY_PIXEL);
        });
    }

    #[test]
    fn lines_cut_short_or_damaged_are_left_out() {
        check_enabled(&[sampler(0, 0)], |samplers| {
            let whole = line(&key(&samplers[0]));
            let (short, _) = whole.rsplit_once(' ').expect("the last word");
            assert_eq!(parse(short), None);
            assert_eq!(parse("1 2 3"), None);
            assert_eq!(parse(&format!("{whole} 7")), None);
            assert_eq!(parse("zz 0 0 0 0 0 0 0 0 0 0 0 0"), None);
            assert_eq!(parse(""), None);
        });
    }

    #[test]
    fn places_are_the_order_of_first_meeting_and_never_change() {
        check_enabled(&[sampler(0, 0), sampler(0x15, 0), sampler(0x15, 1)], |samplers| {
            let (a, b, c) = (key(&samplers[0]), key(&samplers[1]), key(&samplers[2]));
            let mut listed = Listed::default();
            assert_eq!(listed.place(a), Some(0));
            assert_eq!(listed.place(b), Some(1));
            assert_eq!(listed.place(a), None);
            // The same sampler at another register is one of its own.
            assert_eq!(listed.place(c), Some(2));
            let text: String = listed.order.iter().map(|k| line(k) + "\n").collect();
            // A later run: the file's order, a line twice and a cut line ignored.
            let mut again = Listed::of(&format!("{text}{}\n1 2\n", line(&a)));
            assert_eq!(again.order, [a, b, c]);
            assert_eq!(again.place(b), None);
            assert_eq!(again.place(key(&sampler(1, 2))), Some(3));
        });
    }

    /// On the software adapter, create the sampler-only root signatures
    /// used to reserve places, including comparison and stage-visible samplers.
    #[test]
    fn each_sampler_gets_a_root_signature_of_its_own() {
        let _turn = crate::indirect::tests::warp_turn();
        let Some(device) = crate::inplace::tests::warp() else {
            return;
        };
        let mut compare = sampler(0x95, 2);
        compare.ComparisonFunc = D3D12_COMPARISON_FUNC_GREATER;
        let mut all = sampler(0x55, 7);
        all.ShaderVisibility = D3D12_SHADER_VISIBILITY_ALL;
        check_enabled(&[sampler(0, 0), sampler(0x15, 1), compare, all], |samplers| {
            for s in samplers {
                let k = key(s);
                assert!(root_of(&device, &k).is_ok(), "{}", line(&k));
            }
        });
    }

    #[test]
    fn the_list_takes_no_more_than_its_most() {
        let samplers: Vec<_> = (0..MOST as u32 + 5).map(|n| sampler(0, n)).collect();
        check_enabled(&samplers, |samplers| {
            let mut listed = Listed::default();
            for k in samplers.iter().map(key) {
                listed.place(k);
            }
            assert_eq!(listed.order.len(), MOST);
            let text: String = samplers.iter().map(|s| line(&key(s)) + "\n").collect();
            assert_eq!(Listed::of(&text).order.len(), MOST);
        });
    }
}
