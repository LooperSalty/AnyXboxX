//! The asynchronous XUser calls: each one completes on the first turn of the task queue, with a
//! result fixed when it starts (C++ `user_interface.cpp`, `BeginCompleted`). The completion goes
//! through VIctory's XAsync (`XAsyncBegin`, `XAsyncSchedule`, `XAsyncComplete`, `XAsyncGetResult`).

use std::ffi::c_void;

use std::mem::size_of;

use victory_hle::modules::xasync::{self, AsyncBlock, ProviderData};

use crate::object::{E_INVALIDARG, HResult, S_OK};

/// XAsyncOp values, as VIctory's provider receives them.
const OP_BEGIN: u32 = 0;
const OP_DO_WORK: u32 = 1;
const OP_GET_RESULT: u32 = 2;
const OP_CLEANUP: u32 = 4;

/// What a completed call hands back: its result and, if it returns one, a user handle.
struct Completed {
    result: HResult,
    returns_user: bool,
    user: *mut c_void,
}

/// The provider of a completed call. `context` is the leaked `Completed`, freed on cleanup.
extern "system" fn completed_provider(op: u32, data: *const ProviderData) -> HResult {
    // SAFETY: XAsync passes its own provider data, with the context we gave it to `begin`.
    let data = unsafe { &*data };
    let state = data.context.cast::<Completed>();
    match op {
        OP_BEGIN => xasync::XAsyncSchedule(data.block, 0),
        OP_DO_WORK => {
            // SAFETY: the state lives until cleanup, which XAsync calls after completion.
            let state = unsafe { &*state };
            let size = if state.returns_user {
                size_of::<*mut c_void>()
            } else {
                0
            };
            xasync::XAsyncComplete(data.block, state.result, size);
            S_OK
        }
        OP_GET_RESULT => {
            // SAFETY: the buffer is the caller's, at least one pointer wide (checked by XAsync).
            let state = unsafe { &mut *state };
            // SAFETY: as above.
            unsafe { data.buffer.cast::<*mut c_void>().write(state.user) };
            state.user = std::ptr::null_mut();
            S_OK
        }
        OP_CLEANUP => {
            // SAFETY: the state was leaked by `begin_completed` and is freed exactly once here.
            let state = unsafe { Box::from_raw(state) };
            if !state.user.is_null() {
                crate::user::close_handle(state.user);
            }
            S_OK
        }
        _ => S_OK,
    }
}

/// Starts a call that completes at once with `result`; `returns_user` gives it a user handle.
///
/// # Safety
/// `block` is the caller's XAsyncBlock, as the title passes it.
pub unsafe fn begin_completed(
    block: *mut AsyncBlock,
    identity: *const c_void,
    result: HResult,
    returns_user: bool,
) -> HResult {
    let user = if returns_user {
        crate::user::new_handle()
    } else {
        std::ptr::null_mut()
    };
    if returns_user && user.is_null() {
        return crate::object::E_OUTOFMEMORY;
    }
    let state = Box::into_raw(Box::new(Completed {
        result,
        returns_user,
        user,
    }));
    // SAFETY: `block` is the title's block; `state` is ours until the provider frees it.
    let begun = {
        xasync::XAsyncBegin(
            block,
            state.cast(),
            identity,
            std::ptr::null(),
            Some(completed_provider),
        )
    };
    if begun < 0 {
        // SAFETY: XAsync did not take the state; we free it ourselves.
        let state = unsafe { Box::from_raw(state) };
        if !state.user.is_null() {
            crate::user::close_handle(state.user);
        }
    }
    begun
}

/// The user handle a completed `begin_completed` call returns, through XAsync's result.
///
/// # Safety
/// `block` is the title's block; `out` is writable for one handle.
pub unsafe fn result_user(
    block: *mut AsyncBlock,
    identity: *const c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `out` is writable; XAsync checks the block and the result size.
    {
        xasync::XAsyncGetResult(
            block,
            identity,
            size_of::<*mut c_void>(),
            out.cast(),
            std::ptr::null_mut(),
        )
    }
}
