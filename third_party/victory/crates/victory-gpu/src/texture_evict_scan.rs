//! Resource-side LRU scan and detachment. Only idle submits may supply a heap scan.

use super::super::{HostTexture, KINDS, Resource, fills, textures};
use super::{EvictOutcome, EvictPolicy, STATS, TextureInfo};
use crate::texture_heap_evict::{Batch, IdleWindow, Scan};
use crate::{frame, host, object::This, texture};
use std::cmp::Reverse;
use std::sync::Arc;
use std::sync::atomic::{Ordering::Acquire, Ordering::Relaxed};

pub(crate) type Candidate = (u64, u64, Reverse<u64>, usize, u64);

/// Evicted bytes count detached copies, not bytes already returned to the adapter.
pub(super) fn evict(target: u64, completed: u64, policy: EvictPolicy, use_heap: bool) -> EvictOutcome {
    if target == 0 || policy.max_victims == 0 || completed == u64::MAX {
        return EvictOutcome::default();
    }
    let mut outcome = EvictOutcome::default();
    let mut candidates = Vec::new();
    let mut heap = use_heap.then(Scan::new);
    {
        let all = textures();
        for &at in all.iter() {
            // SAFETY: a resource leaves the table under this lock before being freed.
            let r = unsafe { &*(at as *const Resource) };
            if !fills(&r.desc) || r.host.host_written.load(Acquire) {
                continue;
            }
            // Allocation can request LRU under a descriptor slot lock. Do not
            // wait on another texture being made during an opportunistic scan.
            let Ok(made) = r.host.made.try_lock() else {
                continue;
            };
            let Some((copy, _)) = made.as_ref() else { continue };
            let bytes = copy.card_bytes();
            outcome
                .texture
                .get_or_insert_with(|| TextureInfo::new(at, &r.desc, Some(bytes)));
            if !copy.fence_complete(completed) || Arc::strong_count(copy) != 1 {
                outcome.skipped_in_flight += 1;
            } else if bytes != 0 && idle(r, copy, policy) {
                let last = copy.last_used_frame.load(Relaxed);
                let used = r.host.used.load(Relaxed) & 0xffff_ffff;
                let candidate = (last, used, Reverse(bytes), at, copy.serial);
                let refs = host::references(&copy.texture);
                if refs == 1 {
                    candidates.push(candidate);
                } else if let Some(heap) = heap.as_mut() {
                    heap.consider(&copy.texture, candidate, refs);
                }
            }
        }
    }
    candidates.sort_unstable();
    detach(candidates, target, completed, policy, None, &mut outcome);
    if let Some(heap) = heap
        && outcome.evicted_bytes < target
        && (outcome.evicted_count as usize) < policy.max_victims
    {
        let (selected, mut batch) = {
            let all = textures();
            heap.select(
                target - outcome.evicted_bytes,
                policy.max_victims - outcome.evicted_count as usize,
                |candidate, refs, window| {
                    let (_, _, _, at, serial) = candidate;
                    if !all.contains(&at) {
                        return None;
                    }
                    // SAFETY: the texture is live while the table is locked.
                    let r = unsafe { &*(at as *const Resource) };
                    pin(r, serial, completed, policy, refs, window)
                },
            )
        };
        if !selected.is_empty() {
            batch.unmake();
            detach(
                selected,
                target,
                completed,
                policy,
                Some(&mut batch),
                &mut outcome,
            );
            let all = textures();
            batch.wake_resident(|at| {
                if !all.contains(&at) {
                    return false;
                }
                // SAFETY: the texture is live while the table is locked.
                let r = unsafe { &*(at as *const Resource) };
                r.host.made.try_lock().map_or(true, |made| made.is_some())
            });
        }
    }
    KINDS[2].fetch_add(outcome.evicted_count, Relaxed);
    KINDS[3].fetch_add(outcome.evicted_bytes, Relaxed);
    STATS[0].fetch_add(outcome.evicted_count, Relaxed);
    STATS[1].fetch_add(outcome.evicted_bytes, Relaxed);
    STATS[2].fetch_add(outcome.skipped_in_flight, Relaxed);
    outcome
}

fn pin(
    r: &Resource,
    serial: u64,
    completed: u64,
    policy: EvictPolicy,
    refs: u32,
    window: IdleWindow,
) -> Option<(Arc<HostTexture>, u64)> {
    let made = r.host.made.try_lock().ok()?;
    let (copy, _) = made.as_ref()?;
    if !can_detach(r, copy, serial, completed, policy, 1)
        || !window.is_cold(copy.last_used_frame.load(Relaxed))
        || host::references(&copy.texture) != refs
    {
        return None;
    }
    let bytes = copy.card_bytes();
    (bytes != 0).then(|| (Arc::clone(copy), bytes))
}

fn detach(
    candidates: Vec<Candidate>,
    target: u64,
    completed: u64,
    policy: EvictPolicy,
    mut heap: Option<&mut Batch>,
    outcome: &mut EvictOutcome,
) {
    let mut retired = Vec::new();
    let mut staged = Vec::new();
    // The heap pass still owns one temporary Arc per selected copy.
    let owners = 1 + usize::from(heap.is_some());
    {
        let all = textures();
        for (_, _, _, at, serial) in candidates {
            // Heap candidates have already had their slots unmade; finish
            // the whole selected batch even if its byte estimates changed.
            if outcome.evicted_count as usize == policy.max_victims
                || (heap.is_none() && outcome.evicted_bytes >= target)
            {
                break;
            }
            if !all.contains(&at) {
                continue;
            }
            // SAFETY: checked live under the table lock; serial guards address reuse.
            let r = unsafe { &*(at as *const Resource) };
            let Ok(mut made) = r.host.made.try_lock() else {
                continue;
            };
            let Some((copy, _)) = made.as_ref() else { continue };
            if !can_detach(r, copy, serial, completed, policy, owners)
                || heap
                    .as_ref()
                    .is_some_and(|batch| !batch.still_cold(copy.last_used_frame.load(Relaxed)))
                || host::references(&copy.texture) != 1
            {
                continue;
            }
            let bytes = copy.card_bytes();
            if bytes == 0 {
                continue;
            }
            copy.replaced.store(true, Relaxed);
            r.host.wanted.store(u32::MAX, Relaxed);
            r.host.gone.store(true, Relaxed);
            retired.push(made.take().unwrap());
            if let Some(batch) = heap.as_mut() {
                batch.detached(at);
            }
            drop(made);
            staged.extend(texture::take_staged(at as This, serial));
            if outcome.evicted_count == 0 {
                outcome.texture = Some(TextureInfo::new(at, &r.desc, Some(bytes)));
            }
            outcome.evicted_count += 1;
            outcome.evicted_bytes += bytes;
        }
    }
    drop(staged);
    for made in retired {
        host::retire(made);
    }
}

fn can_detach(
    r: &Resource,
    copy: &Arc<HostTexture>,
    serial: u64,
    completed: u64,
    policy: EvictPolicy,
    owners: usize,
) -> bool {
    copy.serial == serial
        && !r.host.host_written.load(Acquire)
        && copy.fence_complete(completed)
        && Arc::strong_count(copy) == owners
        && idle(r, copy, policy)
}

fn idle(r: &Resource, copy: &HostTexture, policy: EvictPolicy) -> bool {
    frame::current().saturating_sub(copy.last_used_frame.load(Relaxed)) >= policy.min_idle_frames
        && host::long_ago(
            r.host.used.load(Relaxed),
            policy.min_idle_ms,
            policy.min_idle_frames,
        )
}
