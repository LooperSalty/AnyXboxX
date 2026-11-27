use super::*;

pub(super) static FG_ON_ALL: AtomicU64 = AtomicU64::new(0);
pub(super) static FG_SHOWN_ALL: AtomicU64 = AtomicU64::new(0);
pub(super) static OCCLUDED: AtomicBool = AtomicBool::new(false);
pub(super) static FG_STATUS: AtomicU32 = AtomicU32::new(0);
/// The planes a present shows (Series S: the scene and the interface over
/// it), the host states they are in, and the back buffer's size.
pub(crate) struct Planes<'a> {
    pub base: (&'a ID3D12Resource, u32),
    pub overlay: Option<(&'a ID3D12Resource, u32)>,
    pub size: [u32; 2],
}

/// What [`present_begins`] began, for [`present_ends`].
pub(crate) struct Presenting {
    token: *mut abi::FrameToken,
    frame: u32,
}

/// Before our swap chain's Present of title frame `frame`: with frame
/// generation, the frame's latency markers, constants and tags, and whether
/// frame generation runs on it (it does when an upscaling was submitted
/// since the last present).
pub(crate) fn present_begins(frame: u64, planes: Planes<'_>) -> Option<Presenting> {
    if !FG_READY.load(Acquire) || !SWAP_CHAIN_PROXIED.load(Acquire) {
        return None;
    }
    if !CHECKING.load(Relaxed) {
        return None;
    }
    // Where the interface is no plane of its own (Series X): the copy of
    // this texture taken in front of the title's interface composite.
    let hudless = if planes.overlay.is_none() && hudless_wanted() { hudless_for(planes.base.0) } else { None };
    let (sl, functions) = (SL.get()?, FUNCTIONS.get()?);
    let (set_options, marker) = (functions.dlssg_set_options?, functions.pcl_set_marker?);
    let frame = frame as u32;
    let token = frame_token(sl, frame)?;
    let mark = |m: u32| {
        // SAFETY: a marker for a token of ours.
        unsafe { marker(m, token) };
    };
    let mut calls = calls();
    if calls.simulating != Some(frame) {
        mark(abi::SIMULATION_START);
    }
    calls.simulating = None;
    mark(abi::SIMULATION_END);
    mark(abi::RENDER_SUBMIT_START);
    mark(abi::RENDER_SUBMIT_END);
    let fresh = {
        let latest = SUBMITTED.lock().unwrap_or_else(|e| e.into_inner());
        latest
            .as_ref()
            .filter(|n| n.id > PRESENTED_ID.load(Relaxed))
            .map(|n| (n.id, n.depth.clone(), n.motion.clone(), n.camera))
    };
    let viewport = abi::ViewportHandle::new(VIEWPORT_FG);
    let on = if let Some((id, depth, motion, camera)) = fresh {
        PRESENTED_ID.store(id, Relaxed);
        let plan = PLAN.get()?;
        // (Frame generation reads the camera's planes and matrices: the
        // nearer plane is the near one, whichever way the title names them.)
        let camera = camera.planes_in_order();
        let constants = camera.constants(plan.asked.jitter, plan.asked.mvec);
        // SAFETY: constants for this frame and viewport.
        let code = unsafe { (sl.set_constants)(&constants, token, &viewport) };
        if code != 0 {
            once!(warn, "frame generation: slSetConstants: {} ({code})", abi::result_name(code));
        }
        let constants_ok = code == 0;
        let mut resources = vec![
            (depth.tagged(), abi::BUFFER_DEPTH, camera.render),
            (motion.tagged(), abi::BUFFER_MOTION_VECTORS, camera.motion_size()),
        ];
        // Two planes: the scene is the colour without the interface, the
        // interface plane its colour and alpha (premultiplied, as `compose`
        // blends it).
        if let Some((over, over_state)) = planes.overlay {
            resources.push((abi::Resource::texture(planes.base.0.as_raw(), planes.base.1), abi::BUFFER_HUDLESS_COLOR, planes.size));
            resources.push((abi::Resource::texture(over.as_raw(), over_state), abi::BUFFER_UI_COLOR_AND_ALPHA, planes.size));
        } else if let Some(copy) = &hudless {
            // One plane with the interface drawn into it: the copy taken in
            // front of the interface composite (ours, always a copy's
            // destination; Streamline holds it until the tags of the frame
            // go).
            resources.push((abi::Resource::texture(copy.as_raw(), COPY_DEST), abi::BUFFER_HUDLESS_COLOR, planes.size));
        }
        let tags: Vec<abi::ResourceTag> = resources
            .iter_mut()
            .map(|(r, kind, size)| abi::ResourceTag::new(r, *kind, abi::VALID_UNTIL_PRESENT, abi::Extent::sized(*size)))
            .collect();
        // SAFETY: tags of live textures in the states they are in at the
        // present (the line of submissions sends the present after them).
        let code = unsafe { (sl.set_tag_for_frame)(token, &viewport, tags.as_ptr(), tags.len() as u32, std::ptr::null_mut()) };
        if code != 0 {
            once!(warn, "frame generation: slSetTagForFrame: {} ({code})", abi::result_name(code));
        }
        once!(
            info,
            render = ?camera.render,
            output = ?camera.output,
            back_buffer = ?planes.size,
            interface_plane = planes.overlay.is_some(),
            interface_free_copy = hudless.is_some(),
            depth_state = format_args!("{:#x}", resources[0].0.state),
            motion_state = format_args!("{:#x}", resources[1].0.state),
            near = constants.camera_near,
            far = constants.camera_far,
            fov = constants.camera_fov,
            depth_inverted = camera.depth_inverted(),
            jitter = ?constants.jitter_offset,
            mvec_scale = ?constants.mvec_scale,
            "DLSS Frame Generation: first frame with depth and motion vectors tagged"
        );
        if code == 0 && hudless.is_some() {
            HUD_COUNTS[HUD_TAGGED].fetch_add(1, Relaxed);
        }
        constants_ok && code == 0
    } else {
        if calls.framegen_on {
            // No upscaling this frame (a menu, a load): what was tagged may
            // no longer hold what it did.
            let tags = [abi::BUFFER_DEPTH, abi::BUFFER_MOTION_VECTORS, abi::BUFFER_HUDLESS_COLOR, abi::BUFFER_UI_COLOR_AND_ALPHA]
                .map(|kind| abi::ResourceTag::new(std::ptr::null_mut(), kind, abi::VALID_UNTIL_PRESENT, abi::Extent::default()));
            // SAFETY: null tags.
            unsafe { (sl.set_tag_for_frame)(token, &viewport, tags.as_ptr(), tags.len() as u32, std::ptr::null_mut()) };
        }
        false
    };
    if on != calls.framegen_on {
        let mut options = abi::DlssgOptions::new(if on { abi::DLSSG_ON } else { abi::DLSSG_OFF });
        options.flags = abi::DLSSG_RETAIN_RESOURCES_WHEN_OFF;
        options.num_back_buffers = 2;
        options.color_width = planes.size[0];
        options.color_height = planes.size[1];
        options.on_error_callback = Some(present_error);
        // SAFETY: filled options for viewport 0, on the presenting thread.
        let code = unsafe { set_options(&viewport, &options) };
        tracing::info!(frame, on, code, result = abi::result_name(code), "DLSS Frame Generation switched");
        if code == 0 {
            calls.framegen_on = on;
        }
    }
    if calls.framegen_on {
        FG_ON_ALL.fetch_add(1, Relaxed);
    }
    drop(calls);
    mark(abi::PRESENT_START);
    Some(Presenting { token, frame })
}

unsafe extern "C" fn present_error(error: *const i32) {
    // SAFETY: the error for the call.
    let code = unsafe { error.as_ref() }.copied().unwrap_or(0);
    if code == windows::Win32::Foundation::DXGI_STATUS_OCCLUDED.0 {
        OCCLUDED.store(true, Relaxed);
        once!(warn, "DXGI_STATUS_OCCLUDED (0x087a0001): the presentation window is occluded");
        return;
    }
    if code < 0 { FG_STATUS.store(code as u32, Relaxed); }
    once!(warn, hresult = format_args!("{:#x}", code as u32), "DLSS Frame Generation: a present failed");
}

/// After our swap chain's Present: the frame's last marker, the next
/// frame's first, and what frame generation reports.
pub(crate) fn present_ends(presenting: Option<Presenting>) {
    let Some(Presenting { token, frame }) = presenting else { return };
    let (Some(sl), Some(functions)) = (SL.get(), FUNCTIONS.get()) else { return };
    let (Some(marker), Some(get_state)) = (functions.pcl_set_marker, functions.dlssg_get_state) else { return };
    // SAFETY: a marker for this frame's token.
    unsafe { marker(abi::PRESENT_END, token) };
    let mut calls = calls();
    if let Some(next) = frame_token(sl, frame.wrapping_add(1)) {
        // SAFETY: as above, for the next frame.
        unsafe { marker(abi::SIMULATION_START, next) };
        calls.simulating = Some(frame.wrapping_add(1));
    }
    if calls.framegen_on {
        let mut state = abi::DlssgState::default();
        let viewport = abi::ViewportHandle::new(VIEWPORT_FG);
        // SAFETY: a structure to fill, no options (no estimate asked).
        let code = unsafe { get_state(&viewport, &mut state, std::ptr::null()) };
        if code != 0 {
            tracing::warn!(code, "frame generation state unavailable");
            FG_STATUS.store(u32::MAX, Relaxed);
        } else {
            FG_SHOWN_ALL.fetch_add(u64::from(state.num_frames_actually_presented), Relaxed);
            let before = FG_STATUS.fetch_or(state.status, Relaxed);
            if state.status != before {
                tracing::warn!(
                    status = format_args!("{:#x}", state.status),
                    "DLSS Frame Generation status changed (0 ok, 1 output too small, 2 Reflex not seen, 4 HDR format, 8 constants invalid, 16 back buffer index not asked)"
                );
            }
        }
    }
}

/// Our swap chain's buffers are about to be made again: frame generation is
/// off until the next present with an upscaling (Streamline asks so).
pub(crate) fn resizing() {
    if !FG_READY.load(Acquire) || !SWAP_CHAIN_PROXIED.load(Acquire) {
        return;
    }
    let Some(set_options) = FUNCTIONS.get().and_then(|f| f.dlssg_set_options) else { return };
    let mut calls = calls();
    if calls.framegen_on {
        let options = abi::DlssgOptions::new(abi::DLSSG_OFF);
        let viewport = abi::ViewportHandle::new(VIEWPORT_FG);
        // SAFETY: filled options.
        unsafe { set_options(&viewport, &options) };
        calls.framegen_on = false;
    }
}
