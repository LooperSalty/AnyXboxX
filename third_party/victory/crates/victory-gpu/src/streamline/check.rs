use super::upscale::{Textures, record};
use super::*;

/// Exercise Streamline without loading a title. Release the synthetic GPU
/// resources and the swap chain before shutting down the SDK on every exit.
pub fn check(output: [u32; 2], device: bool, present: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Err(why) = check_run(output, device, present, &mut |line| lines.push(line), false) {
        lines.push(why);
    }
    lines
}

/// Command-line entry point signals failed or unavailable requested checks.
pub fn check_command(output: [u32; 2], device: bool, present: bool) -> Result<(), String> {
    check_run(output, device, present, &mut |line| println!("{line}"), true)
}

fn check_run(
    output: [u32; 2],
    device: bool,
    present: bool,
    report: &mut impl FnMut(String),
    standalone: bool,
) -> Result<(), String> {
    let max = D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION;
    if output.iter().any(|&dim| dim == 0 || dim > max) {
        return Err(format!("output dimensions must be between 1 and {max}"));
    }
    if SL.get().is_some() {
        return Err("run each upscaler-check in a fresh process".into());
    }
    let mut asked = Asked::from_env()?;
    asked.dlss = true;
    asked.framegen = !asked.present_hook;
    let available = load_and_init(&asked, false).map_err(|why| {
        tracing::warn!("Streamline unavailable: {why}; falling back to the title's own upscaling and plain present");
        format!("Streamline did not load from {}: {why}", asked.dir.display())
    })?;
    report(format!(
        "Streamline loaded from {}: DLSS {}, frame generation {} on this GPU",
        asked.dir.display(),
        if available.dlss { "supported" } else { "NOT supported" },
        if available.framegen {
            "supported"
        } else if asked.framegen {
            "NOT supported"
        } else {
            "not requested"
        }
    ));
    let result = if device || present {
        match crate::host::host() {
            Some(host) => {
                let result = exercise(host, &asked, output, present, report);
                if std::env::var("VICTORY_CHECK_TEARDOWN").is_ok_and(|s| s.trim() == "0") {
                    report("Streamline teardown skipped (VICTORY_CHECK_TEARDOWN=0)".into());
                    return result;
                }
                resizing();
                host.drop_presenter();
                *lock(&SUBMITTED) = None;
                lock(&HUD_SUBMITTED).clear();
                for flag in [&CHECKING, &FG_READY, &DLSS_READY, &SWAP_CHAIN_PROXIED, &HUDLESS_WANTED] {
                    flag.store(false, Release);
                }
                result
            }
            None => Err("no host D3D12 device".into()),
        }
    } else if available.dlss || available.framegen {
        Ok(())
    } else {
        Err("no requested feature is supported".into())
    };
    if let Err(why) = &result {
        tracing::warn!("upscaler-check failed: {why}");
    }
    let sl = SL.get().unwrap();
    let code = shutdown::shut_down(sl, standalone && (device || present))?;
    report(format!("Streamline shut down: {} ({code})", abi::result_name(code)));
    result?;
    if code == 0 { Ok(()) } else { Err(format!("slShutdown failed ({code})")) }
}

fn exercise(
    host: &crate::host::Host,
    asked: &Asked,
    output: [u32; 2],
    present: bool,
    report: &mut impl FnMut(String),
) -> Result<(), String> {
    let framegen = FG_READY.load(Acquire);
    let dlss = DLSS_READY.load(Acquire);
    report(format!("device: DLSS ready {dlss}, frame generation ready {framegen}"));
    if !dlss {
        return Err("DLSS device hand-over failed".into());
    }
    let render = asked.mode.render_size(output);
    let camera = plan::Camera {
        jitter: [0.0, 0.0],
        motion_scale: [render[0] as f32, render[1] as f32],
        render,
        output,
        // (The far plane first, as the title names them.)
        near: 10_000.0,
        far: 0.15,
        fov_y: 1.0,
        reset: true,
        flags: plan::FFX_HIGH_DYNAMIC_RANGE | plan::FFX_DEPTH_INVERTED | plan::FFX_AUTO_EXPOSURE,
    };
    let device = &host.device;
    let made = (|| -> windows::core::Result<_> {
        use windows::Win32::Graphics::Dxgi::Common::*;
        let depth =
            texture(device, render, DXGI_FORMAT_D32_FLOAT, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, SHADER_READ)?;
        let motion = texture(device, render, DXGI_FORMAT_R16G16_FLOAT, D3D12_RESOURCE_FLAG_NONE, SHADER_READ)?;
        let colour = texture(device, render, DXGI_FORMAT_R16G16B16A16_FLOAT, D3D12_RESOURCE_FLAG_NONE, SHADER_READ)?;
        let out = texture(
            device,
            output,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            UNORDERED,
        )?;
        let base = texture(device, output, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_NONE, 0)?;
        Ok((depth, motion, colour, out, base))
    })();
    let (depth, motion, colour, out, base) = made.map_err(|e| format!("stand-in textures not made: {e}"))?;
    {
        // (Known once the first evaluation asked for them.)
        let sizes = || -> String {
            let calls = calls();
            let sizes: Vec<String> = calls
                .optimal
                .iter()
                .filter(|(o, _, _)| *o == output)
                .map(|&(_, m, s)| format!("{} {:?}", plan::sl_mode_name(m), s))
                .collect();
            if sizes.is_empty() { "?".into() } else { sizes.join(", ") }
        };
        let mut textures: Textures = Default::default();
        textures[COLOUR] = Some((colour, SHADER_READ));
        textures[DEPTH] = Some((depth.clone(), SHADER_READ));
        textures[MOTION] = Some((motion.clone(), SHADER_READ));
        textures[OUTPUT] = Some((out, UNORDERED));
        // (`VICTORY_STREAMLINE_PRESENT=1`: DLSS alone, with a present through
        // Streamline's proxy of our swap chain after each upscaling, more of
        // them than Streamline waits for a present.)
        let hooked = present && asked.present_hook && !framegen;
        for round in 0..if hooked { 120 } else { 3 } {
            let id = NEXT_ID.fetch_add(1, Relaxed) + 1;
            let list = host.new_list().map_err(|e| format!("no command list: {e}"))?;
            let recorded = record(list.list.as_raw(), &textures, &camera, 1.0, id)
                .map_err(|why| format!("DLSS did not run: {why}"))?;
            // SAFETY: only the SDK recorded into this open synthetic list.
            unsafe { list.list.Close() }.map_err(|e| format!("closing DLSS list: {e}"))?;
            host.execute_sync_internal(&list);
            // SAFETY: device and list survive the synchronous submission.
            unsafe { device.GetDeviceRemovedReason() }.map_err(|e| format!("device removed after DLSS: {e}"))?;
            if round == 0 {
                report(format!(
                    "DLSS ran on the GPU: {render:?} -> {output:?} as {} (optimal {:?}); render sizes for {output:?}: {}",
                    plan::sl_mode_name(recorded.mode),
                    recorded.optimal,
                    sizes()
                ));
            }
            if hooked {
                host.present_planes(&base, 0, None, round + 1).ok_or("synthetic present failed")?;
            }
        }
        if hooked && !PRESENT_HOOKED.load(Relaxed) {
            return Err("DLSS swap chain proxy unavailable".into());
        }
        report(if hooked {
            "DLSS ran 120 times on stand-in textures with a present after each through Streamline's proxy".into()
        } else {
            "DLSS ran three times on stand-in textures; the device is fine".into()
        });
    }
    if present && framegen {
        CHECKING.store(true, Relaxed);
        let (on, shown) = (FG_ON_ALL.load(Relaxed), FG_SHOWN_ALL.load(Relaxed));
        // A stand-in for the colour without the interface, tagged as the
        // copies in front of the title's interface composite are: ours, a
        // copy's destination, of the back buffer's size and format.
        let copy = if asked.hudless {
            use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
            Some(
                texture(device, output, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_NONE, COPY_DEST)
                    .map_err(|e| format!("stand-in HUD-less colour: {e}"))?,
            )
        } else {
            None
        };
        let tagged = HUD_COUNTS[HUD_TAGGED].load(Relaxed);
        // Cap the synthetic source at 60 Hz for three seconds. GPU work and
        // Present consume part of that budget, especially on a busy desktop.
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let duration = Duration::from_secs(3);
        let mut presents = 0;
        while start.elapsed() < duration && presents < 180 {
            let frame = presents + 1;
            let id = NEXT_ID.fetch_add(1, Relaxed) + 1;
            *SUBMITTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Noted {
                id,
                depth: Source::Fixed(depth.clone(), SHADER_READ),
                motion: Source::Fixed(motion.clone(), SHADER_READ),
                camera,
            });
            if let Some(copy) = &copy {
                lock(&HUD_SUBMITTED).push(Copied { id: frame, target: base.as_raw() as usize, texture: copy.clone() });
            }
            host.present_planes(&base, 0, None, frame).ok_or("synthetic present failed")?;
            presents = frame;
            let next = start + duration.mul_f64(presents as f64 / 180.0);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        crate::host::settle_for_fence();
        report(format!(
            "frame generation: {presents} presents, {} with it on, {} frames shown by it, {} presents tagged with a colour without the interface, status {:#x}{}{}",
            FG_ON_ALL.load(Relaxed) - on,
            FG_SHOWN_ALL.load(Relaxed) - shown,
            HUD_COUNTS[HUD_TAGGED].load(Relaxed) - tagged,
            FG_STATUS.load(Relaxed),
            if present::OCCLUDED.load(Relaxed) { "; DXGI_STATUS_OCCLUDED (0x087a0001)" } else { "" },
            if SWAP_CHAIN_PROXIED.load(Relaxed) { "" } else { "; the swap chain is NOT Streamline's proxy" }
        ));
        if !framegen_passed(
            presents,
            FG_ON_ALL.load(Relaxed) - on,
            FG_SHOWN_ALL.load(Relaxed) - shown,
            HUD_COUNTS[HUD_TAGGED].load(Relaxed) - tagged,
            asked.hudless,
            FG_STATUS.load(Relaxed),
        ) {
            if present::OCCLUDED.load(Relaxed) {
                return Err("DXGI_STATUS_OCCLUDED (0x087a0001): the check window was occluded; generated frames could not be verified".into());
            }
            return Err("synthetic frame generation check failed (see counts above)".into());
        }
        // SAFETY: a plain query.
        if let Err(e) = unsafe { device.GetDeviceRemovedReason() } {
            return Err(format!("the device was removed while presenting: {e}"));
        }
    } else if present && !asked.present_hook {
        return Err("frame generation is not ready".into());
    }
    Ok(())
}
/// Count frames reported by the SDK, including dropped interpolated frames
/// under load. Passing requires at least one generated frame and valid tags.
fn framegen_passed(presents: u64, on: u64, shown: u64, tagged: u64, hudless: bool, status: u32) -> bool {
    presents > 0 && on == presents && shown > presents && (!hudless || tagged == presents) && status == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_generated_frames_and_requested_tags() {
        assert!(framegen_passed(180, 180, 291, 180, true, 0));
        assert!(framegen_passed(90, 90, 180, 0, false, 0));
        for (on, shown, tagged, status) in [(180, 180, 180, 0), (0, 360, 180, 0), (180, 360, 0, 0), (180, 360, 180, 2)]
        {
            assert!(!framegen_passed(180, on, shown, tagged, true, status));
        }
    }
}

const SHADER_READ: u32 = 0xc0;
const UNORDERED: u32 = 0x8;
pub(super) const COPY_DEST: u32 = 0x400;

/// A stand-in texture for [`check`] (its contents are whatever the memory held).
pub(super) fn texture(
    device: &ID3D12Device,
    size: [u32; 2],
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    state: u32,
) -> windows::core::Result<ID3D12Resource> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(size[0]),
        Height: size[1],
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: flags,
    };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain texture creation.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATES(state as i32),
            None,
            &mut made,
        )?
    };
    made.ok_or_else(windows::core::Error::empty)
}
