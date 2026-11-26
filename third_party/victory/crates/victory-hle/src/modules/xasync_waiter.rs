//! Task queue waiters. A registration owns a duplicate of the caller's handle.
//! https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xtaskqueue/functions/xtaskqueueregisterwaiter
use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Weak;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, SetEvent, WaitForMultipleObjects,
};

pub(super) struct Waiter {
    watched: OwnedHandle,
    canceled: OwnedHandle,
    completed: OwnedHandle,
    active: AtomicBool,
    task: TaskCallback,
}

struct TaskCallback {
    context: usize,
    callback: Callback,
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}

fn error() -> HRESULT {
    // SAFETY: GetLastError reads this thread's error value.
    let error = unsafe { GetLastError() };
    (0x8007_0000 | (error & 0xffff)) as i32
}

fn event(manual: bool) -> Result<OwnedHandle, HRESULT> {
    // SAFETY: null security and name request an unnamed event.
    let event = unsafe { CreateEventW(std::ptr::null(), manual as i32, 0, std::ptr::null()) };
    if event.is_null() {
        Err(error())
    } else {
        // SAFETY: CreateEventW returned a new owned handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(event.cast()) })
    }
}

fn duplicate(handle: HANDLE) -> Result<OwnedHandle, HRESULT> {
    let mut duplicate: HANDLE = std::ptr::null_mut();
    // SAFETY: duplicate a caller-supplied handle into this process.
    let ok = unsafe {
        let process = GetCurrentProcess();
        DuplicateHandle(
            process,
            handle,
            process,
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        Err(error())
    } else {
        // SAFETY: DuplicateHandle returned a new owned handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(duplicate.cast()) })
    }
}

impl Waiter {
    pub(super) fn cancel(&self) {
        self.active.store(false, Ordering::Release);
        // SAFETY: the event remains owned by this waiter.
        unsafe { SetEvent(raw(&self.canceled)) };
    }

    pub(super) fn invoke(&self, canceled: bool) {
        if !canceled && self.active.load(Ordering::Acquire) {
            (self.task.callback)(self.task.context as *mut c_void, false);
        }
        // SAFETY: the event remains owned by this waiter.
        unsafe { SetEvent(raw(&self.completed)) };
    }

    fn wait(self: Arc<Self>, queue: Weak<Queue>, port: u32) {
        loop {
            let handles = [raw(&self.canceled), raw(&self.watched)];
            // SAFETY: both handles are owned for the duration of this wait.
            if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) }
                != WAIT_OBJECT_0 + 1
            {
                break;
            }
            let Some(queue) = queue.upgrade() else { break };
            let task = Task {
                context: 0,
                callback: 0,
                waiter: Some(self.clone()),
            };
            if queue.submit(port, 0, task) != S_OK {
                break;
            }
            drop(queue);
            let handles = [raw(&self.canceled), raw(&self.completed)];
            // A manual-reset event is rearmed only after the prior callback completes.
            if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) }
                != WAIT_OBJECT_0 + 1
            {
                break;
            }
        }
    }
}

pub(super) fn register(
    handle: *const Queue,
    port: u32,
    watched: HANDLE,
    context: *mut c_void,
    callback: Option<Callback>,
    token: *mut u64,
) -> HRESULT {
    let (Some(queue), Some(callback)) = (queue(handle), callback) else {
        return E_INVALIDARG;
    };
    if port > COMPLETION || watched.is_null() || token.is_null() {
        return E_INVALIDARG;
    }
    let result = (|| {
        Ok::<_, HRESULT>(Arc::new(Waiter {
            watched: duplicate(watched)?,
            canceled: event(true)?,
            completed: event(false)?,
            active: AtomicBool::new(true),
            task: TaskCallback {
                context: context as usize,
                callback,
            },
        }))
    })();
    let waiter = match result {
        Ok(waiter) => waiter,
        Err(hr) => return hr,
    };
    let id = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let mut waiters = lock(&queue.waiters);
    if queue.terminated.load(Ordering::Acquire) {
        return E_ABORT;
    }
    waiters.insert(id, waiter.clone());
    let weak = Arc::downgrade(&queue);
    if std::thread::Builder::new()
        .spawn(move || waiter.wait(weak, port))
        .is_err()
    {
        waiters.remove(&id);
        return 0x8007_000e_u32 as i32;
    }
    // SAFETY: the token pointer is non-null and writable by contract.
    unsafe { token.write(id) };
    S_OK
}

pub(super) fn unregister(handle: *const Queue, token: u64) {
    if let Some(queue) = queue(handle) {
        if let Some(waiter) = lock(&queue.waiters).remove(&token) {
            waiter.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    extern "system" fn count(context: *mut c_void, canceled: bool) {
        assert!(!canceled);
        // SAFETY: each test passes a live AtomicU32 as context.
        unsafe { &*context.cast::<AtomicU32>() }.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn auto_reset_waiter_dispatches_and_unregisters() {
        let mut queue = std::ptr::null();
        assert_eq!(XTaskQueueCreate(0, 0, &mut queue), S_OK);
        let watched = event(false).unwrap();
        let count = AtomicU32::new(0);
        let mut token = 0;
        assert_eq!(
            register(
                queue,
                WORK,
                raw(&watched),
                (&raw const count).cast_mut().cast(),
                Some(count_callback),
                &mut token
            ),
            S_OK
        );
        assert_ne!(token, 0);
        for expected in 1..=2 {
            unsafe { SetEvent(raw(&watched)) };
            assert!(XTaskQueueDispatch(queue, WORK, 2000));
            assert_eq!(count.load(Ordering::SeqCst), expected);
        }
        unregister(queue, token);
        unsafe { SetEvent(raw(&watched)) };
        assert!(!XTaskQueueDispatch(queue, WORK, 100));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        XTaskQueueCloseHandle(queue);
    }

    extern "system" fn count_callback(context: *mut c_void, canceled: bool) {
        count(context, canceled)
    }

    #[test]
    fn invalid_arguments_are_refused() {
        let mut queue = std::ptr::null();
        assert_eq!(XTaskQueueCreate(0, 0, &mut queue), S_OK);
        let watched = event(false).unwrap();
        let mut token = 0;
        assert_eq!(
            register(
                queue,
                2,
                raw(&watched),
                std::ptr::null_mut(),
                Some(count),
                &mut token
            ),
            E_INVALIDARG
        );
        assert_eq!(
            register(
                queue,
                WORK,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                Some(count),
                &mut token
            ),
            E_INVALIDARG
        );
        assert_eq!(
            register(
                queue,
                WORK,
                raw(&watched),
                std::ptr::null_mut(),
                None,
                &mut token
            ),
            E_INVALIDARG
        );
        assert_eq!(
            register(
                queue,
                WORK,
                raw(&watched),
                std::ptr::null_mut(),
                Some(count),
                std::ptr::null_mut()
            ),
            E_INVALIDARG
        );
        XTaskQueueCloseHandle(queue);
    }

    #[test]
    fn manual_reset_waiter_repeats_and_a_queued_callback_is_canceled() {
        let mut queue = std::ptr::null();
        assert_eq!(XTaskQueueCreate(0, 0, &mut queue), S_OK);
        let watched = event(true).unwrap();
        let calls = AtomicU32::new(0);
        let mut token = 0;
        assert_eq!(
            register(
                queue,
                WORK,
                raw(&watched),
                (&raw const calls).cast_mut().cast(),
                Some(count),
                &mut token
            ),
            S_OK
        );
        unsafe { SetEvent(raw(&watched)) };
        assert!(XTaskQueueDispatch(queue, WORK, 2000));
        assert!(XTaskQueueDispatch(queue, WORK, 2000));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        unregister(queue, token);
        // A callback already queued before unregister must not call the client.
        XTaskQueueDispatch(queue, WORK, 100);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        XTaskQueueCloseHandle(queue);
    }

    #[test]
    fn terminate_cancels_a_waiter_before_dispatch() {
        let mut queue = std::ptr::null();
        assert_eq!(XTaskQueueCreate(0, 0, &mut queue), S_OK);
        let watched = event(false).unwrap();
        let calls = AtomicU32::new(0);
        let mut token = 0;
        assert_eq!(
            register(
                queue,
                WORK,
                raw(&watched),
                (&raw const calls).cast_mut().cast(),
                Some(count),
                &mut token
            ),
            S_OK
        );
        assert_eq!(
            XTaskQueueTerminate(queue, true, std::ptr::null_mut(), None),
            S_OK
        );
        unsafe { SetEvent(raw(&watched)) };
        assert!(!XTaskQueueDispatch(queue, WORK, 100));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        XTaskQueueCloseHandle(queue);
    }
}
