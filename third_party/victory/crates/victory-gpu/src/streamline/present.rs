use super::*;
use crate::frame_rate::FrameGeneration;

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
    // The tagged textures' states are those the submissions before this
    // present left: only the line of submissions orders the two.
    if !crate::queue::line_on() && !CHECKING.load(Relaxed) {
        HUDLESS_WANTED.store(false, Relaxed);
        once!(warn, "DLSS Frame Generation: the line of submissions did not start; frame generation stays off");
        return None;
    }
    // Where the interface is no plane of its own (Series X): the copy of
    // this texture taken in front of the title's interface composite.
    let hudless = if planes.overlay.is_none() && hudless_wanted() { hudless_for(planes.base.0) } else { None };
    let (sl, functions) = (SL.get()?, FUNCTIONS.get()?);
    let (set_options, marker) = (functions.dlssg_set_options?, functions.pcl_set_marker?);
    let began = std::time::Instant::now();
    let frame = frame as u32;
    let token = frame_token(sl, frame)?;
    let mark = |m: u32| {
        let began = std::time::Instant::now();
        // SAFETY: a marker for a token of ours.
        unsafe { marker(m, token) };
        spent(T_MARKERS, began);
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
        latest.as_ref().filter(|n| n.id > PRESENTED_ID.load(Relaxed)).map(|n| (n.id, n.depth.clone(), n.motion.clone(), n.camera))
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
        let constants_code = unsafe { (sl.set_constants)(&constants, token, &viewport) };
        if constants_code != 0 {
            once!(warn, "frame generation: slSetConstants: {} ({constants_code})", abi::result_name(constants_code));
        }
        let mut resources =
            vec![(depth.tagged(), abi::BUFFER_DEPTH, camera.render), (motion.tagged(), abi::BUFFER_MOTION_VECTORS, camera.motion_size())];
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
        } else if hudless_wanted() {
            HUD_COUNTS[HUD_MISSING].fetch_add(1, Relaxed);
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
        if code == 0 && constants_code == 0 && hudless.is_some() {
            HUD_COUNTS[HUD_TAGGED].fetch_add(1, Relaxed);
            HUD_TAGGED_ALL.fetch_add(1, Relaxed);
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
        constants_code == 0 && code == 0
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
            if on {
                // GetState counts since its previous read. Drain the unqueried
                // (FG off) period before Present so it cannot be counted twice.
                calls.fg_count_valid = functions.dlssg_get_state.is_some_and(|get_state| {
                    let mut state = abi::DlssgState::default();
                    // SAFETY: a live viewport and writable state, no estimate requested.
                    unsafe { get_state(&viewport, &mut state, std::ptr::null()) == 0 }
                });
            }
        }
    }
    if calls.framegen_on {
        FG_ON.fetch_add(1, Relaxed);
        // What it takes of the card's memory: before its first present and
        // some seconds of presents later.
        let presents = FG_ON_ALL.fetch_add(1, Relaxed) + 1;
        if (presents == 1 || presents == 240)
            && !CHECKING.load(Relaxed)
            && let Some(host) = crate::host::host()
        {
            host.log_video_memory(if presents == 1 {
                "right before the first present with frame generation on"
            } else {
                "240 presents with frame generation on later"
            });
        }
    } else {
        FG_OFF.fetch_add(1, Relaxed);
    }
    drop(calls);
    mark(abi::PRESENT_START);
    spent(T_BEFORE, began);
    Some(Presenting { token, frame })
}

unsafe extern "C" fn present_error(error: *const i32) {
    // SAFETY: the error for the call.
    let code = unsafe { error.as_ref() }.copied().unwrap_or(0);
    once!(warn, hresult = format_args!("{:#x}", code as u32), "DLSS Frame Generation: a present failed");
}

/// After our swap chain's Present: the frame's last marker, the next
/// frame's first, and what frame generation reports.
pub(crate) fn present_ends(presenting: Option<Presenting>) -> FrameGeneration {
    let Some(Presenting { token, frame }) = presenting else { return FrameGeneration::Off };
    let (Some(sl), Some(functions)) = (SL.get(), FUNCTIONS.get()) else { return FrameGeneration::Off };
    let (Some(marker), Some(get_state)) = (functions.pcl_set_marker, functions.dlssg_get_state) else { return FrameGeneration::Off };
    let began = std::time::Instant::now();
    // SAFETY: a marker for this frame's token.
    unsafe { marker(abi::PRESENT_END, token) };
    let mut calls = calls();
    if let Some(next) = frame_token(sl, frame.wrapping_add(1)) {
        // SAFETY: as above, for the next frame.
        unsafe { marker(abi::SIMULATION_START, next) };
        calls.simulating = Some(frame.wrapping_add(1));
    }
    spent(T_MARKERS, began);
    let mut generation = FrameGeneration::Off;
    if calls.framegen_on {
        let mut state = abi::DlssgState::default();
        let viewport = abi::ViewportHandle::new(VIEWPORT_FG);
        let asked = std::time::Instant::now();
        // SAFETY: a structure to fill, no options (no estimate asked).
        let code = unsafe { get_state(&viewport, &mut state, std::ptr::null()) };
        spent(T_STATE, asked);
        // A failed read may leave its count in the next report. Use the
        // configured multiplier until both ends of the interval are known.
        generation = FrameGeneration::Unavailable;
        if code == 0 {
            if calls.fg_count_valid {
                generation = FrameGeneration::Reported(state.num_frames_actually_presented);
            }
            FG_SHOWN.fetch_add(u64::from(state.num_frames_actually_presented), Relaxed);
            FG_SHOWN_ALL.fetch_add(u64::from(state.num_frames_actually_presented), Relaxed);
            let before = FG_STATUS.swap(state.status, Relaxed);
            if state.status != before {
                tracing::warn!(
                    status = format_args!("{:#x}", state.status),
                    "DLSS Frame Generation status changed (0 ok, 1 output too small, 2 Reflex not seen, 4 HDR format, 8 constants invalid, 16 back buffer index not asked)"
                );
            }
        }
        calls.fg_count_valid = code == 0;
    }
    spent(T_AFTER, began);
    generation
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
        let code = unsafe { set_options(&viewport, &options) };
        if code == 0 {
            calls.framegen_on = false;
        } else {
            tracing::warn!(code, "Streamline refused to disable frame generation before resize");
        }
    }
}
