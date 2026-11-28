//! Cross-process test of the sampler gate with a real D3D12 pipeline library.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use victory_shader::HwStage;
use victory_shader::emit::{HostShader, StaticSampler};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::core::{PCSTR, s};

use super::{FILE, NEW, line, parse};
use crate::pipeline::host::{GraphicsState, HostPipeline};
use crate::pipeline::{Shader, Translated, library};

const CHILD: &str = "VICTORY_TEST_SAMPLER_ORDER_PHASE";
const CACHE: &str = "VICTORY_TEST_SAMPLER_ORDER_CACHE";
const OVERRIDE: &str = "VICTORY_LIBRARY_STATIC_SAMPLERS";
const TEST: &str = "pipeline::sampler_order::library_tests::sampler_order_gates_host_library";
const POINT: [u32; 4] = [0; 4];
const LINEAR: [u32; 4] = [0, 0, (1 << 20) | (1 << 22), 0];

fn shader(
    stage: &'static str,
    hw: HwStage,
    source: &str,
    profile: PCSTR,
    samplers: &[[u32; 4]],
) -> Arc<Shader> {
    Arc::new(Shader {
        stage,
        hash: "sampler-order-probe".into(),
        container: Vec::new(),
        ambient: None,
        probe: None,
        translated: Some(Translated {
            host: HostShader {
                stage: hw,
                dxil: crate::host::compile(source, s!("main"), profile).expect("compile test shader"),
                user_data: Vec::new(),
                samplers: samplers.to_vec(),
                static_samplers: samplers.iter().copied().map(StaticSampler::from_words).collect(),
                root_cbvs: Vec::new(),
                root_srvs: Vec::new(),
                root_uavs: Vec::new(),
                root_indexed_views: Vec::new(),
            },
            threads: (hw == HwStage::Cs).then_some([1, 1, 1]),
            used: Vec::new(),
            tables: crate::draw::TablePlan::new(&[], &[]),
            stores: false,
            stores_by_address: false,
            picks: Default::default(),
        })
        .into(),
    })
}

fn probe(shaders: &[Arc<Shader>], graphics: Option<&GraphicsState>, hit: bool) {
    let host = crate::host::host().expect("D3D12 host for sampler order test");
    let before = (library::MADE.load(Relaxed), library::LOADED.load(Relaxed));
    let made = if host.adapter_vendor_id() == Some(0x10de) {
        HostPipeline::new(shaders, graphics)
    } else {
        // Exercise the same denied baseline policy on WARP and other GPUs.
        HostPipeline::new_with_sampler_policy(shaders, graphics, |_| false)
    };
    assert!(made.is_some(), "create synthetic {shaders:?} pipeline");
    assert_eq!(
        (library::MADE.load(Relaxed) - before.0, library::LOADED.load(Relaxed) - before.1),
        if hit { (0, 1) } else { (1, 0) },
        "{shaders:?}: expected {}",
        if hit { "a host library hit" } else { "pipeline creation" },
    );
}

fn run_phase(phase: &str) {
    assert!(std::env::var_os(OVERRIDE).is_none());
    library::use_pipeline_cache(PathBuf::from(std::env::var_os(CACHE).unwrap()));
    const CS: &str = "[numthreads(1, 1, 1)] void main() {}";
    let compute = [shader("compute", HwStage::Cs, CS, s!("cs_5_1"), &[POINT])];
    let graphics = [
        shader("vertex", HwStage::Vs, "float4 main() : SV_Position { return 0; }", s!("vs_5_1"), &[]),
        shader("pixel", HwStage::Ps, "float4 main() : SV_Target { return 1; }", s!("ps_5_1"), &[POINT]),
    ];
    let defaults = GraphicsState::default();
    let mut targets = D3D12_RT_FORMAT_ARRAY { NumRenderTargets: 1, ..Default::default() };
    targets.RTFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
    let state = GraphicsState {
        topology: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        targets,
        depth: D3D12_DEPTH_STENCIL_DESC { DepthEnable: false.into(), ..defaults.depth },
        ..defaults
    };
    let both = |hit| {
        probe(&compute, None, hit);
        probe(&graphics, Some(&state), hit);
    };
    match phase {
        "learn" => {
            both(false);
            both(false); // Learning must not make the keys settled in this process.
            assert_eq!(NEW.load(Relaxed), 2);
            library::save(true);
        }
        "replay" => {
            both(false); // Nothing was stored while the samplers were being learned.
            both(true);  // A clean replay permits storage and then reuse.
            assert_eq!(NEW.load(Relaxed), 0);
            library::save(true);
            assert!(library::SAVED.load(Relaxed) > 0, "persist the host PSOs");
        }
        "hit" => {
            both(true); // The first request for each PSO hits the on-disk library.
            let mixed = [shader("compute", HwStage::Cs, CS, s!("cs_5_1"), &[POINT, LINEAR])];
            probe(&mixed, None, false);
            probe(&mixed, None, false);
            assert_eq!(NEW.load(Relaxed), 1);
            both(true); // A new key does not disqualify pipelines that do not use it.
        }
        "refuse" => {
            both(false); // The same on-disk PSOs still exist but must not be used.
            both(false);
            assert_eq!(NEW.load(Relaxed), 0);
        }
        _ => panic!("unknown sampler order test phase: {phase}"),
    }
}

#[test]
fn sampler_order_gates_host_library() {
    if let Ok(phase) = std::env::var(CHILD) {
        run_phase(&phase);
        return;
    }

    let dir = tempfile::tempdir().expect("isolated pipeline cache");
    let samplers = dir.path().join(FILE);
    let run = |phase| {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, phase)
            .env(CACHE, dir.path())
            .env("VICTORY_SAMPLER_ORDER", "1")
            .env_remove(OVERRIDE)
            .status()
            .expect("launch isolated sampler order test process");
        assert!(status.success(), "sampler order phase {phase}: {status}");
    };
    run("learn");
    let learned = std::fs::read_to_string(&samplers).expect("persist the learned samplers");
    assert_eq!(learned.lines().count(), 2);
    run("replay");
    run("hit");

    // A parseable but invalid sampler before the saved keys means even the
    // successfully replayed keys cannot be trusted to have the right indices.
    let mut invalid = parse(learned.lines().next().unwrap()).unwrap();
    invalid[1] = 0; // Not a D3D12_TEXTURE_ADDRESS_MODE.
    std::fs::write(&samplers, format!("{}\n{learned}", line(&invalid))).unwrap();
    run("refuse");
}
