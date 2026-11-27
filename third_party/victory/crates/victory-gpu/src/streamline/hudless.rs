use super::*;

pub(crate) fn hudless_wanted() -> bool {
    HUDLESS_WANTED.load(Relaxed)
}

pub(super) static HUDLESS_WANTED: AtomicBool = AtomicBool::new(false);

pub(super) struct Copied {
    pub(super) id: u64,
    /// The host texture copied (its address).
    pub(super) target: usize,
    pub(super) texture: ID3D12Resource,
}

// SAFETY: D3D12 resources are free-threaded.
unsafe impl Send for Copied {}

pub(super) static HUD_SUBMITTED: Mutex<Vec<Copied>> = Mutex::new(Vec::new());
pub(super) static HUD_COUNTS: [AtomicU64; 1] = [AtomicU64::new(0)];
pub(super) const HUD_TAGGED: usize = 0;
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
pub(super) fn hudless_for(shown: &ID3D12Resource) -> Option<ID3D12Resource> {
    let mut copies = lock(&HUD_SUBMITTED);
    let target = shown.as_raw() as usize;
    let first = copies.iter().filter(|c| c.target == target).min_by_key(|c| c.id)?.texture.clone();
    copies.retain(|c| c.target != target);
    Some(first)
}
