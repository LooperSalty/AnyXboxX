use super::*;

/// The title's interface composite where the interface is no plane of the
/// present (Series X): one triangle that blends the interface target
/// (3840x2160, premultiplied) over the frame's colour, with these vertex and
/// pixel shaders, the frame's last draw into the texture it presents. Seen
/// in every frame listing of 2026-11-26 (stock 3840x2160 output: once a
/// frame, after the scene's composite into the same texture) in VIctory-NVIDIA's measurements.
const INTERFACE_COMPOSITE: [&str; 2] = ["0602c2384acbb3b2", "a78d088fa55aa495"];

/// Whether draws are looked at for the colour without the interface: frame
/// generation runs on our swap chain and `VICTORY_FRAMEGEN_HUDLESS` is not 0.
#[inline]
pub(crate) fn hudless_wanted() -> bool {
    HUDLESS_WANTED.load(Acquire)
}

pub(super) static HUDLESS_WANTED: AtomicBool = AtomicBool::new(false);

/// Frame zero is never presented. Plain draws keep their usual THIN encoding.
pub(crate) fn hudless_frame(vertices: u32, instances: u32) -> u64 {
    draw_frame(hudless_wanted(), vertices, instances, crate::queue::presented)
}

fn draw_frame(wanted: bool, vertices: u32, instances: u32, presented: impl FnOnce() -> u64) -> u64 {
    if wanted && vertices == 3 && instances == 1 {
        presented() + 1
    } else {
        0
    }
}

/// The host textures of the last presents (their addresses): the textures
/// the title presents (it has two, in turn).
static SHOWN: [std::sync::atomic::AtomicUsize; 4] = [const { std::sync::atomic::AtomicUsize::new(0) }; 4];
static SHOWN_NEXT: AtomicU32 = AtomicU32::new(0);

/// A copy of a presented texture taken in front of the interface composite.
pub(super) struct Copied<T = ID3D12Resource> {
    /// Number of the title's present this draw was recorded for.
    pub(super) id: u64,
    /// The host texture copied (its address).
    pub(super) target: usize,
    pub(super) texture: T,
}

/// Each recording owns its texture here through its present. Only submitted
/// copies are eligible for tagging. Take HUD_PENDING first when using both.
static HUD_PENDING: Mutex<Pending> = Mutex::new(Pending { presented: 0, copies: Vec::new() });
pub(super) static HUD_SUBMITTED: Mutex<Vec<Copied>> = Mutex::new(Vec::new());

struct SceneCopy {
    /// Pins the COM identity used to match the presented target, even if
    /// the title releases it and resets its command list before Present.
    _target: ID3D12Resource,
    texture: Arc<ID3D12Resource>,
}

struct Pending<T = SceneCopy> {
    presented: u64,
    copies: Vec<Copied<T>>,
}

impl<T> Pending<T> {
    fn record(&mut self, id: u64, target: usize, copy: impl FnOnce() -> Option<T>) -> bool {
        if id <= self.presented || self.copies.iter().any(|c| c.id == id) {
            return false;
        }
        let Some(texture) = copy() else { return false };
        self.copies.push(Copied { id, target, texture });
        true
    }

    fn submit<U>(&self, id: u64, submitted: &mut Vec<Copied<U>>, texture: impl FnOnce(&T) -> U) {
        if id > self.presented && let Some(c) = self.copies.iter().find(|c| c.id == id) {
            submitted.push(Copied { id, target: c.target, texture: texture(&c.texture) });
        }
    }

    fn begin_present<U>(&mut self, frame: u64, submitted: &mut Vec<Copied<U>>) {
        self.presented = self.presented.max(frame);
        // Retain this frame's destination through Present even if its list
        // was already reset. Only recordings for later frames can still submit.
        self.copies.retain(|c| c.id >= self.presented);
        submitted.retain(|c| c.id >= self.presented);
    }

    fn take<U>(&self, target: usize, submitted: &mut Vec<Copied<U>>) -> Option<U> {
        let at = submitted.iter().position(|c| c.id == self.presented && c.target == target)?;
        Some(submitted.swap_remove(at).texture)
    }
}

/// The two textures the copies go to in turn (a present reads one while the
/// next frame's lists may already be recorded), with their size and format.
struct HudlessTextures {
    size: [u32; 2],
    format: i32,
    textures: [Arc<ID3D12Resource>; 2],
}

static HUD_TEXTURES: Mutex<Option<HudlessTextures>> = Mutex::new(None);

// Since the last line about it (every ten seconds): copies recorded, presents
// tagged with one, presents with frame generation on that had none, composite
// draws, those after a copy in their list, draws of other pipelines into a
// presented texture.
pub(super) static HUD_COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
const HUD_COPIES: usize = 0;
pub(super) const HUD_TAGGED: usize = 1;
pub(super) static HUD_TAGGED_ALL: AtomicU64 = AtomicU64::new(0);
pub(super) const HUD_MISSING: usize = 2;
const HUD_COMPOSITES: usize = 3;
const HUD_AGAIN: usize = 4;
const HUD_OTHERS: usize = 5;

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Before a draw of `pipeline` into `target` alone (no depth), with frame
/// generation: if `target` is a texture the title presents and the draw is
/// the title's interface composite, record a copy immediately before it.
/// Direct recording runs on the title thread. For THIN, the same stamp is
/// carried with the draw command to its translation thread.
pub(crate) fn before_target_draw(
    list: &crate::host::HostList,
    target: crate::object::This,
    pipeline: &crate::pipeline::host::HostPipeline,
    triangle: bool,
) {
    let frame = if triangle { crate::queue::presented() + 1 } else { 0 };
    before_target_draw_at(list, target, pipeline, frame);
}

pub(crate) fn before_target_draw_at(
    list: &crate::host::HostList,
    target: crate::object::This,
    pipeline: &crate::pipeline::host::HostPipeline,
    frame: u64,
) {
    let Some(texture) = crate::resource::host_resource(target) else { return };
    let shown = texture.as_raw() as usize;
    if !SHOWN.iter().any(|s| s.load(Relaxed) == shown) {
        return;
    }
    let triangle = frame != 0;
    let composite = interface_composite(triangle, pipeline.stages.iter().map(|s| s.shader.hash.as_str()));
    if !composite {
        HUD_COUNTS[HUD_OTHERS].fetch_add(1, Relaxed);
        static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
        let key = std::ptr::from_ref(pipeline) as usize;
        let mut seen = lock(&SEEN);
        if !seen.contains(&key) && seen.len() < 24 {
            seen.push(key);
            tracing::info!(
                pipeline = %pipeline.name,
                state = %pipeline.state,
                triangle,
                after_the_copy_in_its_list = list.hudless() != 0,
                "DLSS Frame Generation: this pipeline draws into the texture the title presents (the scene's composite does; an interface drawn here before the interface composite would be in the colour without the interface)"
            );
        }
        return;
    }
    HUD_COUNTS[HUD_COMPOSITES].fetch_add(1, Relaxed);
    if list.hudless() != 0 {
        HUD_COUNTS[HUD_AGAIN].fetch_add(1, Relaxed);
        return;
    }
    let began = std::time::Instant::now();
    let recorded = lock(&HUD_PENDING).record(frame, shown, || {
        let copy = hudless_texture(texture, frame)?;
        crate::list::ensure_state(list, target, D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32);
        // SAFETY: a prepared draw on an open list, matching textures in copy
        // states. Restore the source before the draw that follows this call.
        unsafe { list.list.CopyResource(copy.as_ref(), texture) };
        crate::list::ensure_state(list, target, D3D12_RESOURCE_STATE_RENDER_TARGET.0 as u32);
        list.keep(texture);
        list.keep_hudless(&copy);
        list.mark_hudless(frame);
        Some(SceneCopy { _target: texture.clone(), texture: copy })
    });
    if !recorded {
        return;
    }
    HUD_COUNTS[HUD_COPIES].fetch_add(1, Relaxed);
    spent(T_COPY, began);
    // SAFETY: a live texture.
    let desc = unsafe { texture.GetDesc() };
    once!(
        info,
        width = desc.Width,
        height = desc.Height,
        format = desc.Format.0,
        pipeline = %pipeline.name,
        state = %pipeline.state,
        "DLSS Frame Generation: the colour without the interface is copied in front of the title's interface composite (first time)"
    );
}

/// One of the two textures the colour without the interface is copied to
/// (copy `id`'s), made like `target` in the swap chain's format; `None` if
/// `target` is no plain 2D texture of a format that is shown.
fn hudless_texture(target: &ID3D12Resource, id: u64) -> Option<Arc<ID3D12Resource>> {
    // SAFETY: a live texture.
    let desc = unsafe { target.GetDesc() };
    let plain = desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.DepthOrArraySize == 1
        && desc.MipLevels == 1
        && desc.SampleDesc.Count == 1;
    let format = crate::host::swap_format(desc.Format).filter(|_| plain);
    let Some(format) = format else {
        once!(
            warn,
            format = desc.Format.0,
            levels = desc.MipLevels,
            samples = desc.SampleDesc.Count,
            "DLSS Frame Generation: the presented texture cannot be copied for the colour without the interface; none is tagged"
        );
        return None;
    };
    let size = [desc.Width as u32, desc.Height];
    let mut textures = lock(&HUD_TEXTURES);
    let device = &crate::host::host()?.device;
    let make = || match texture(device, size, format, D3D12_RESOURCE_FLAG_NONE, COPY_DEST) {
        Ok(resource) => Some(Arc::new(resource)),
        Err(e) => {
            once!(warn, error = %e, "DLSS Frame Generation: no texture for the colour without the interface; none is tagged");
            None
        }
    };
    if textures.as_ref().is_none_or(|t| t.size != size || t.format != format.0) {
        *textures = Some(HudlessTextures { size, format: format.0, textures: [make()?, make()?] });
    }
    let pair = &textures.as_ref()?.textures;
    let first = (id % 2) as usize;
    // A still-recorded list can be submitted late or replayed. Its destination
    // must stay private until reset, even after this frame has been presented.
    for slot in [first, first ^ 1] {
        if Arc::strong_count(&pair[slot]) == 1 {
            return Some(pair[slot].clone());
        }
    }
    make()
}

/// A host list that recorded copy `id` of the colour without the interface
/// is submitted (in submission order, before the present that follows it).
pub(crate) fn hudless_submitted(id: u64) {
    lock(&HUD_PENDING).submit(id, &mut lock(&HUD_SUBMITTED), |copy| copy.texture.as_ref().clone());
}

pub(super) fn begin_hudless_present(frame: u64) {
    lock(&HUD_PENDING).begin_present(frame, &mut lock(&HUD_SUBMITTED));
}

/// Consume the submitted copy of this frame and single-plane target once.
pub(super) fn hudless_for(shown: &ID3D12Resource) -> Option<ID3D12Resource> {
    let shown = shown.as_raw() as usize;
    if !SHOWN.iter().any(|s| s.load(Relaxed) == shown) {
        SHOWN[SHOWN_NEXT.fetch_add(1, Relaxed) as usize % SHOWN.len()].store(shown, Relaxed);
    }
    lock(&HUD_PENDING).take(shown, &mut lock(&HUD_SUBMITTED))
}

fn interface_composite<'a>(triangle: bool, stages: impl Iterator<Item = &'a str>) -> bool {
    // HostPipeline stores the vertex stage first, followed by the pixel stage.
    triangle && stages.eq(INTERFACE_COMPOSITE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present<T>(pending: &mut Pending<T>, frame: u64, target: Option<usize>, submitted: &mut Vec<Copied<T>>) -> Option<T> {
        pending.begin_present(frame, submitted);
        target.and_then(|target| pending.take(target, submitted))
    }

    #[test]
    fn copies_only_the_single_triangle_with_both_build_shader_ids() {
        let (vertex, pixel) = ("0602c2384acbb3b2", "a78d088fa55aa495");
        assert!(interface_composite(true, [vertex, pixel].into_iter()));
        assert!(!interface_composite(false, [vertex, pixel].into_iter()));
        assert!(!interface_composite(true, [vertex, "another pixel shader"].into_iter()));
        assert!(!interface_composite(true, [pixel].into_iter()));
        assert!(!interface_composite(true, [pixel, vertex].into_iter()));
    }

    #[test]
    fn one_copy_across_lists_before_and_after_submission() {
        let mut pending = Pending { presented: 0, copies: Vec::new() };
        let mut submitted = Vec::new();
        let frame = draw_frame(true, 3, 1, || 40);
        assert_eq!(frame, 41);
        assert!(!pending.record(frame, 10, || None));
        assert!(pending.record(frame, 10, || Some("before the UI")));
        assert!(!pending.record(frame, 10, || panic!("second list copied")));
        pending.submit(frame, &mut submitted, |c| *c);
        assert!(!pending.record(frame, 20, || panic!("copied after submission")));
        assert_eq!(present(&mut pending, frame, Some(10), &mut submitted), Some("before the UI"));
        assert!(!pending.record(frame, 10, || panic!("late recording copied")));
        assert_eq!(present(&mut pending, frame + 1, Some(10), &mut submitted), None);
        assert!(pending.record(frame + 2, 10, || Some("new frame")));
    }

    #[test]
    fn tags_only_the_exact_submitted_frame_and_target() {
        let mut pending = Pending { presented: 0, copies: Vec::new() };
        let mut submitted = Vec::new();
        for (frame, target) in [(5, 10), (4, 20), (3, 10), (2, 20), (1, 10)] {
            assert!(pending.record(frame, target, || Some(frame)));
        }
        for frame in [5, 4, 2, 1] {
            pending.submit(frame, &mut submitted, |c| *c);
        }
        assert_eq!(present(&mut pending, 1, Some(10), &mut submitted), Some(1));
        assert_eq!(present(&mut pending, 2, Some(10), &mut submitted), None);
        assert_eq!(present(&mut pending, 3, Some(10), &mut submitted), None);
        pending.submit(3, &mut submitted, |c| *c); // List reached the queue too late.
        assert_eq!(present(&mut pending, 4, None, &mut submitted), None); // Two planes.
        assert_eq!(present(&mut pending, 5, Some(10), &mut submitted), Some(5));
        assert_eq!(pending.take(10, &mut submitted), None);
        pending.begin_present(6, &mut submitted);
        assert!(submitted.is_empty() && pending.copies.is_empty());
    }

    #[test]
    fn disabled_or_wrong_shape_draws_never_copy_or_tag() {
        let mut pending = Pending { presented: 0, copies: Vec::new() };
        let mut submitted = Vec::new();
        for (wanted, vertices, instances) in [(false, 3, 1), (true, 6, 1), (true, 3, 2)] {
            let frame = draw_frame(wanted, vertices, instances, || panic!("read the present number"));
            assert!(!interface_composite(frame != 0, INTERFACE_COMPOSITE.into_iter()));
            assert!(!pending.record(frame, 10, || -> Option<u64> { panic!("recorded a copy") }));
        }
        assert_eq!(present(&mut pending, 1, Some(10), &mut submitted), None);
    }
}
