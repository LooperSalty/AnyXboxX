use super::*;

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
    Fixed(ID3D12Resource, u32),
}

impl Source {
    pub(super) fn tagged(&self) -> abi::Resource {
        match self {
            Source::Fixed(t, state) => abi::Resource::texture(t.as_raw(), *state),
        }
    }
}

// SAFETY: D3D12 resources are free-threaded.
unsafe impl Send for Source {}

/// The latest upscaling submitted, and the one the last present tagged.
pub(super) static SUBMITTED: Mutex<Option<Noted>> = Mutex::new(None);
pub(super) static PRESENTED_ID: AtomicU64 = AtomicU64::new(0);
