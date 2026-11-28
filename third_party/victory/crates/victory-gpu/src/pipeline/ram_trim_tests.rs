//! Generated guest shaders only, with no title files or extracted code.

use super::*;
use crate::pipeline::{host::HostPipeline, of_builder, shader_with_mode, shader_with_stage_key};
use crate::ram_trim::GIB;
use victory_shader::HwStage;

fn guest_shader(stage: HwStage, padding_byte: u8) -> Vec<u8> {
    const COMPUTE_SLOT: usize = 0x60;
    const PIXEL_SLOT: usize = 0x30;
    const RECORD_SIZE: usize = 0x54;
    const PIXEL_PATCHES: usize = 0x14c;
    const PIXEL_PATCH_SIZE: usize = 4 + 10 * 10 * 4;
    const ABSENT: u64 = 0xDEAD_BEEF_DEAD_BEEF;
    let mut payload = vec![0u8; COMPUTE_SLOT + 8];
    payload[..8].copy_from_slice(&6u64.to_le_bytes());
    for slot in [0x08, 0x10, 0x18, 0x20, 0x28, PIXEL_SLOT, 0x48, COMPUTE_SLOT] {
        payload[slot..slot + 8].copy_from_slice(&ABSENT.to_le_bytes());
    }
    let (slot, mut io) = match stage {
        HwStage::Cs => (COMPUTE_SLOT, vec![0u8; 0x34]),
        HwStage::Ps => (PIXEL_SLOT, vec![0u8; PIXEL_PATCHES + 8 * PIXEL_PATCH_SIZE]),
        _ => panic!("no fixture for {stage:?}"),
    };
    if stage == HwStage::Cs {
        // One thread in each dimension; no LDS, views or constants.
        for word in io[0x28..].chunks_exact_mut(4) {
            word.copy_from_slice(&1u32.to_le_bytes());
        }
    } else {
        // No exports to patch.
        for mrt in 0..8 {
            let at = PIXEL_PATCHES + mrt * PIXEL_PATCH_SIZE;
            io[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    let record = payload.len();
    payload[slot..slot + 8].copy_from_slice(&(record as u64).to_le_bytes());
    payload.extend_from_slice(&((record + RECORD_SIZE + io.len()) as u64).to_le_bytes());
    payload.extend_from_slice(&4u64.to_le_bytes());
    payload.resize(record + RECORD_SIZE, 0);
    payload[record + 0x14] = 1 << 5; // wave32, independent of host wave size
    payload[record + 0x50] = 1; // one VGPR
    payload.extend_from_slice(&io);
    payload.extend_from_slice(&0xbf81_0000u32.to_le_bytes()); // s_endpgm

    let mut fssm = b"FSSM".to_vec();
    fssm.extend_from_slice(&0x101u32.to_le_bytes());
    fssm.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    fssm.extend_from_slice(&payload);

    let mut part = vec![0u8; 0x18];
    part.extend_from_slice(b"XDAT");
    const PADDING: usize = 64 * 1024;
    let index = 0x24;
    let fssm_at = index + 2 * 12;
    let pad_at = fssm_at + fssm.len();
    for field in [2, index, 0x0c, fssm_at, fssm.len(), 0x12, pad_at, PADDING] {
        part.extend_from_slice(&(field as u32).to_le_bytes());
    }
    part.extend_from_slice(&fssm);
    part.resize(pad_at + PADDING, padding_byte);

    // DXBC with one DXIL part; the translator only needs the native program.
    let mut out = b"DXBC".to_vec();
    out.extend_from_slice(&[0; 16]);
    for word in [1u32, (36 + 8 + part.len()) as u32, 1, 36] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(b"DXIL");
    out.extend_from_slice(&(part.len() as u32).to_le_bytes());
    out.extend_from_slice(&part);
    out
}

fn critical() -> TrimThresholds {
    *TrimPolicy::for_host(true, 32 * GIB, GIB, GIB).thresholds().unwrap()
}

fn dxil_state(shader: &Shader) -> &Dxil {
    shader.translated().expect("generated shader translates").dxil.as_deref().unwrap()
}

fn resident(dxil: &Dxil) -> bool {
    dxil.state.lock().unwrap().bytes.is_some()
}

fn packed(shader: &Shader) -> bool {
    matches!(shader.container.bytes(), super::super::shader_container::Bytes::Inflated(_))
}

#[test]
fn ram_trim_disabled_keeps_original_storage() {
    let guest = guest_shader(HwStage::Cs, 0x33);
    let shader = shader_with_mode("compute", &guest, true, false);
    assert!(!shader.container.trimmable());
    let mut build = shader.pso_bytecode().expect("baseline DXIL");
    let translated = shader.translated().unwrap();
    assert!(translated.dxil.is_none());
    let original = translated.host.dxil.as_ptr();
    assert!(!build.as_ref().is_empty());
    assert_eq!(build.as_ref().as_ptr(), original);
    build.pipeline_created();
    drop(build);
    assert_eq!(shader.pso_bytecode().unwrap().as_ref().as_ptr(), original);
    assert_eq!(shader.container.bytes().as_ref(), guest.as_slice());
}

#[test]
fn ram_trim_storage_selection_follows_process_switch() {
    let guest = guest_shader(HwStage::Cs, 0x41);
    let shader = shader_with_stage_key("compute", &guest, true);
    assert_eq!(shader.container.trimmable(), ram_trim::enabled());
}

#[test]
fn ram_trim_waits_for_success_and_all_builders_then_reloads_exact_bytes() {
    let guest = guest_shader(HwStage::Cs, 0x42);
    let shader = shader_with_mode("compute", &guest, true, true);
    let thresholds = critical();
    let dxil = dxil_state(&shader);
    let programs = shader.programs().expect("native program is readable");
    assert!(!of_builder(std::slice::from_ref(&shader)));

    let unsuccessful = shader.pso_bytecode().unwrap();
    let expected = unsuccessful.as_ref().to_vec();
    assert!(!expected.is_empty());
    drop(unsuccessful);
    dxil.trim_with(&shader, &thresholds, u64::MAX);
    assert!(resident(dxil), "a failed build cannot make DXIL cold");
    assert!(!packed(&shader));

    let raw_read = shader.container.bytes();
    let dropped_before = COUNTERS.shader_dxil_dropped_bytes.load(Relaxed);
    let mut made = shader.pso_bytecode().unwrap();
    let other = shader.pso_bytecode().unwrap();
    made.pipeline_created();
    drop(made);
    dxil.trim_with(&shader, &thresholds, u64::MAX);
    assert!(resident(dxil), "another build still uses this stage");
    assert!(!packed(&shader));
    assert_eq!(other.as_ref(), expected.as_slice());
    drop(other);

    dxil.trim_with(&shader, &thresholds, u64::MAX);
    assert!(!resident(dxil));
    assert!(COUNTERS.shader_dxil_dropped_bytes.load(Relaxed) >= dropped_before + expected.len() as u64);
    assert!(packed(&shader));
    assert_eq!(shader.container.hash(), blake3::hash(&guest));
    assert_eq!(raw_read.as_ref(), guest.as_slice(), "compression leaves existing readers valid");
    drop(raw_read);
    assert_eq!(shader.container.bytes().as_ref(), guest.as_slice());
    assert_eq!(shader.programs().unwrap(), programs);
    assert!(!of_builder(std::slice::from_ref(&shader)));
    let reloaded = shader.pso_bytecode().expect("reload from the packed guest container");
    assert_eq!(reloaded.as_ref(), expected.as_slice());
}

#[test]
fn ram_trim_packs_and_reloads_pixel_shader_without_builder_probe() {
    let guest = guest_shader(HwStage::Ps, 0x43);
    let shader = shader_with_mode("pixel", &guest, true, true);
    let mut build = shader.pso_bytecode().expect("generated pixel shader translates");
    let expected = build.as_ref().to_vec();
    build.pipeline_created();
    drop(build);
    let dxil = dxil_state(&shader);
    dxil.trim_with(&shader, &critical(), u64::MAX);
    assert!(!resident(dxil));
    assert!(packed(&shader));
    assert_eq!(shader.container.bytes().as_ref(), guest.as_slice());
    assert_eq!(shader.pso_bytecode().unwrap().as_ref(), expected.as_slice());
}

#[test]
fn ram_trim_rebuilds_real_pso() {
    let host = crate::host::host().expect("a D3D12 device is required for the RAM trim PSO check");
    let guest = guest_shader(HwStage::Cs, 0x44);
    let shader = shader_with_mode("compute", &guest, true, true);
    let stages = std::slice::from_ref(&shader);
    let first = HostPipeline::new(stages, None).expect("first compute PSO");
    let dxil = dxil_state(&shader);
    dxil.trim_with(&shader, &critical(), u64::MAX);
    assert!(!resident(dxil), "real PSO creation made DXIL eligible");
    assert!(packed(&shader));

    let before = COUNTERS.shader_dxil_reloaded_bytes.load(Relaxed);
    let second = HostPipeline::new(stages, None).expect("compute PSO after eviction and reload");
    assert!(first.compute && second.compute);
    assert!(COUNTERS.shader_dxil_reloaded_bytes.load(Relaxed) > before);
    // SAFETY: both PSOs and the device are still alive.
    unsafe { host.device.GetDeviceRemovedReason() }.expect("device remained usable after both builds");
}
