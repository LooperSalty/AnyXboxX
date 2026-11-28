//! Bounded GPU readback for opt-in card-buffer demotion.

use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_NONE, ID3D12CommandList, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::Interface;

use super::{Host, Prelude, queue_lock};
use crate::shadow::{Shadow, card_tier::Queue};

const DEADLINE_MS: u32 = 250;

struct DemoteCopy {
    list: Prelude,
    fence: ID3D12Fence,
    event: HANDLE,
    sent: u64,
    pending: Option<Pending>,
}

struct Pending {
    _staging: ID3D12Resource,
    sources: Vec<(Arc<Shadow>, u64)>,
    event_armed: bool,
}

impl Drop for DemoteCopy {
    fn drop(&mut self) {
        // SAFETY: this copy owns its event and is not dropped while pending.
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

// SAFETY: only the owner of the mutex uses the list, fence, and event.
unsafe impl Send for DemoteCopy {}

static COPY: Mutex<Option<DemoteCopy>> = Mutex::new(None);

impl DemoteCopy {
    fn new(host: &Host) -> Option<Self> {
        let list = host.make_prelude()?;
        // SAFETY: a live device and a private auto-reset event.
        unsafe {
            let fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).inspect_err(|error| {
                tracing::warn!(%error, "could not create card-buffer readback fence");
            }).ok()?;
            let event = CreateEventW(None, false, false, None).inspect_err(|error| {
                tracing::warn!(%error, "could not create card-buffer readback event");
            }).ok()?;
            Some(Self {
                list,
                fence,
                event,
                sent: 0,
                pending: None,
            })
        }
    }

    fn completed(&self) -> bool {
        // SAFETY: the private fence is live. MAX indicates device removal.
        let done = unsafe { self.fence.GetCompletedValue() };
        done != u64::MAX && done >= self.sent
    }

    fn reap(&mut self) {
        if let Some(pending) = &self.pending
            && self.completed()
            // SAFETY: the private event is live. Drain the notification
            // before allowing reuse, so it cannot wake a later batch.
            && (!pending.event_armed || unsafe { WaitForSingleObject(self.event, 0) } == WAIT_OBJECT_0)
        {
            self.pending = None;
        }
    }
}

pub(super) fn reap() {
    if let Ok(mut slot) = COPY.try_lock()
        && let Some(copy) = slot.as_mut()
    {
        copy.reap();
    }
}

fn read_if_complete<T>(
    status: WAIT_EVENT,
    completed: impl FnOnce() -> bool,
    read: impl FnOnce() -> T,
) -> Option<T> {
    match status {
        WAIT_OBJECT_0 if completed() => Some(read()),
        WAIT_TIMEOUT => {
            tracing::warn!(
                deadline_ms = DEADLINE_MS,
                "card-buffer readback timed out; skipping demotion"
            );
            None
        }
        WAIT_FAILED => {
            tracing::warn!(error = %windows::core::Error::from_thread(), "card-buffer readback wait failed; skipping demotion");
            None
        }
        _ => {
            tracing::warn!(
                status = status.0,
                "card-buffer readback did not complete; skipping demotion"
            );
            None
        }
    }
}

fn demote_readback_inner(
    host: &Host,
    copy: &mut DemoteCopy,
    staging: ID3D12Resource,
    data: *const u8,
    sources: Vec<(Arc<Shadow>, u64)>,
) -> Option<Vec<Vec<u8>>> {
    let (allocator, list) = &copy.list;
    // SAFETY: `copy` is exclusive and idle. Both buffers in each range are
    // live and cover the requested bytes. The packed staging map is valid.
    let result = unsafe {
        (|| -> windows::core::Result<()> {
            allocator.Reset()?;
            list.Reset(allocator, None)?;
            let mut at = 0;
            for (shadow, len) in &sources {
                // Both are buffers created in COMMON: the device-local UAV
                // source and the custom L0 write-back staging destination.
                // D3D12 buffers decay to COMMON at the end of a separate
                // ExecuteCommandLists operation even after explicit legacy
                // transitions. This list is submitted alone, so COPY_SOURCE
                // and COPY_DEST promotion is implicit, and both decay again.
                list.CopyBufferRegion(&staging, at, &shadow.buffer, 0, *len);
                at += *len;
            }
            list.Close()?;
            let lists = [Some(list.cast::<ID3D12CommandList>()?)];
            let _queue = queue_lock();
            copy.sent += 1;
            for (shadow, _) in &sources {
                shadow.note_aux_fence(Queue::Direct, &copy.fence, copy.sent);
            }
            // Install ownership before executing, including if Signal fails.
            copy.pending = Some(Pending {
                _staging: staging,
                sources,
                event_armed: false,
            });
            host.queue.ExecuteCommandLists(&lists);
            host.card_sent_locked();
            host.queue.Signal(&copy.fence, copy.sent)
        })()
    };
    if let Err(error) = result {
        tracing::warn!(%error, "card-buffer readback submission failed; skipping demotion");
        return None;
    }
    // SAFETY: the private event and fence are kept alive until completion,
    // including if registration or waiting fails. No queue lock is held.
    let status = unsafe {
        if let Err(error) = copy.fence.SetEventOnCompletion(copy.sent, copy.event) {
            tracing::warn!(%error, "card-buffer readback event registration failed; skipping demotion");
            return None;
        }
        copy.pending.as_mut()?.event_armed = true;
        WaitForSingleObject(copy.event, DEADLINE_MS)
    };
    if status == WAIT_OBJECT_0 {
        copy.pending.as_mut()?.event_armed = false;
    }
    read_if_complete(
        status,
        || copy.completed(),
        || {
            let pending = copy.pending.as_ref().unwrap();
            let mut at = 0;
            pending
                .sources
                .iter()
                .map(|(_, len)| {
                    let len = *len as usize;
                    // SAFETY: the exact readback fence completed and the mapping's
                    // owner is still pinned; each packed range lies in its bounds.
                    let bytes = unsafe { std::slice::from_raw_parts(data.add(at), len) }.to_vec();
                    at += len;
                    bytes
                })
                .collect()
        },
    )
    .inspect(|_| {
        copy.pending = None;
    })
}

pub(super) fn run(host: &Host, sources: Vec<(Arc<Shadow>, u64)>) -> Option<Vec<Vec<u8>>> {
    let total = sources
        .iter()
        .try_fold(0u64, |sum, (_, size)| sum.checked_add(*size))?;
    if total == 0 {
        return Some(Vec::new());
    }
    let mut slot = COPY.try_lock().ok()?;
    if slot.is_none() {
        *slot = DemoteCopy::new(host);
    }
    let copy = slot.as_mut()?;
    copy.reap();
    if copy.pending.is_some() {
        // Cap outstanding storage at one batch. A failed queue Signal with
        // no proof of completion leaves this slot pinned for the session.
        return None;
    }
    let (staging, data) = host.shared_buffer(total.next_multiple_of(64 << 10))?;
    let result = demote_readback_inner(host, copy, staging, data.cast_const(), sources);
    if result.is_none() && copy.pending.is_none() {
        // Recording failed before execution. Start with a fresh list next time.
        *slot = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_skips_mapping_and_demotion() {
        // SAFETY: an initially unsignalled event owned by this test.
        let event = unsafe { CreateEventW(None, false, false, None) }.unwrap();
        let result: Option<Vec<u8>> = read_if_complete(
            // SAFETY: the event is live; an unsignalled event times out.
            unsafe { WaitForSingleObject(event, 0) },
            || panic!("timeout must not query completion"),
            || panic!("timeout must not read or publish bytes"),
        );
        // SAFETY: no wait or fence notification uses this event anymore.
        unsafe { CloseHandle(event) }.unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn unconfirmed_readback_keeps_staging_until_its_fence() {
        let host = crate::host::host().expect("D3D12 is needed for the fence test");
        let mut copy = DemoteCopy::new(host).unwrap();
        let (staging, _) = host.shared_buffer(64 << 10).unwrap();
        copy.sent = 2;
        copy.pending = Some(Pending { _staging: staging, sources: Vec::new(), event_armed: false });
        // SAFETY: a private fence, owned exclusively by this test. No GPU
        // work was queued; advancing the fence simulates completion.
        unsafe { copy.fence.Signal(1) }.unwrap();
        copy.reap();
        assert!(copy.pending.is_some());
        // SAFETY: as above; this is the exact value recorded by the batch.
        unsafe { copy.fence.Signal(2) }.unwrap();
        copy.reap();
        assert!(copy.pending.is_none());
    }

    #[test]
    fn wake_without_fence_completion_skips_demotion() {
        assert_eq!(read_if_complete(WAIT_OBJECT_0, || false, || 5), None);
        assert_eq!(read_if_complete(WAIT_OBJECT_0, || true, || 5), Some(5));
        assert_eq!(read_if_complete(WAIT_FAILED, || true, || 5), None);
    }
}
