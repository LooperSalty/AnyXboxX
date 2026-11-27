use super::*;

/// Frame generation: what the present after this list's submission tags,
/// noted on the host list ([`submitted`] then makes it the latest).
pub(super) fn note(host: &crate::host::HostList, up: &Upscale, id: u64) {
    let (Some(depth), Some(motion)) =
        (crate::resource::host_copy(up.resources[DEPTH].0), crate::resource::host_copy(up.resources[MOTION].0))
    else {
        return;
    };
    let mut pending = pending();
    pending.retain(|n| id.saturating_sub(n.id) < 16);
    pending.push(Noted { id, depth: Source::Host(depth), motion: Source::Host(motion), camera: up.camera });
    drop(pending);
    host.mark_upscale(id);
}

/// The inputs of an upscaling for frame generation.
pub(super) struct Noted {
    pub(super) id: u64,
    pub(super) depth: Source,
    pub(super) motion: Source,
    pub(super) camera: plan::Camera,
}

/// A texture frame generation reads at the present, and the state it is in
/// then: a title texture's host copy (in the state the lists submitted so
/// far left it in), or one of the check's ([`check`]) in a fixed state.
#[derive(Clone)]
pub(super) enum Source {
    Host(Arc<crate::resource::HostTexture>),
    Fixed(ID3D12Resource, u32),
}

impl Source {
    pub(super) fn tagged(&self) -> abi::Resource {
        match self {
            Source::Host(t) => abi::Resource::texture(t.texture.as_raw(), t.state.load(Relaxed)),
            Source::Fixed(t, state) => abi::Resource::texture(t.as_raw(), *state),
        }
    }
}

// SAFETY: D3D12 resources are free-threaded.
unsafe impl Send for Source {}

static PENDING: Mutex<Vec<Noted>> = Mutex::new(Vec::new());
/// The latest upscaling submitted, and the one the last present tagged.
pub(super) static SUBMITTED: Mutex<Option<Noted>> = Mutex::new(None);
pub(super) static PRESENTED_ID: AtomicU64 = AtomicU64::new(0);

fn pending() -> std::sync::MutexGuard<'static, Vec<Noted>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// A host list that recorded upscaling `id` is submitted (in submission
/// order, before the present that follows it).
pub(crate) fn submitted(id: u64) {
    let noted = {
        let mut pending = pending();
        let at = pending.iter().position(|n| n.id == id);
        at.map(|i| pending.swap_remove(i))
    };
    if let Some(noted) = noted {
        let mut latest = SUBMITTED.lock().unwrap_or_else(|e| e.into_inner());
        if latest.as_ref().is_none_or(|l| l.id < noted.id) {
            *latest = Some(noted);
        }
    }
}
