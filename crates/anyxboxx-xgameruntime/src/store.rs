//! XStore (C++ `store_interface.cpp`): the Microsoft Store, offline.
//!
//! Provider {0DD112AC…}, four interface versions that share one table (methods 3 to 83 of
//! `gdk_api_table.inc`). Nothing is sold and no server is contacted:
//!  - a context is created and closed normally;
//!  - the game's licence is local, valid and complete (active, no trial, no disc);
//!  - product queries complete with an empty result set;
//!  - purchase, store windows, downloads and package licences fail with
//!    `E_GAMESTORE_NETWORK_ERROR`, the console's error when the store is unreachable;
//!  - subscribing to licence changes succeeds and the callback never runs;
//!  - every other method is `E_NOTIMPL` (not listed in the table, so the object answers it).

use std::collections::HashSet;
use std::ffi::{c_char, c_void};
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use victory_hle::modules::xasync::AsyncBlock;

use crate::events;
use crate::handle::{checked, close, new_handle};
use crate::object::{E_INVALIDARG, HResult, S_OK};
use crate::save_async::{self, Completion};
use crate::user;

const CONTEXT_MAGIC: u32 = 0x3153_5841; // « AXS1 »
const QUERY_MAGIC: u32 = 0x3253_5841; // « AXS2 »
/// The console's error when the store cannot be reached (XGameErr.h).
pub const E_GAMESTORE_NETWORK_ERROR: HResult = 0x8924_5301_u32 as i32;

static LICENSE_IDENTITY: [u8; 1] = [8];
static QUERY_IDENTITY: [u8; 1] = [9];
static OFFLINE_IDENTITY: [u8; 1] = [10];

/// XStoreGameLicense of XStore.h, laid out as the title reads it (read by the title, not by Rust).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct GameLicense {
    sku_store_id: [c_char; 18],
    is_active: bool,
    is_trial_owned_by_this_user: bool,
    is_disc_license: bool,
    is_trial: bool,
    trial_time_remaining_in_seconds: u32,
    trial_unique_id: [c_char; 64],
    expiration_date: i64,
}

/// The licence of the game: active and complete, with no trial, no disc and no expiry.
pub fn game_license() -> GameLicense {
    GameLicense {
        sku_store_id: [0; 18],
        is_active: true,
        is_trial_owned_by_this_user: false,
        is_disc_license: false,
        is_trial: false,
        trial_time_remaining_in_seconds: 0,
        trial_unique_id: [0; 64],
        expiration_date: 0,
    }
}

fn identity(id: &'static [u8; 1]) -> *const c_void {
    id.as_ptr().cast()
}

/// Whether `context` is a live store context.
///
/// # Safety
/// As [`checked`].
unsafe fn valid_context(context: *mut c_void) -> bool {
    // SAFETY: forwarded to the title's contract.
    unsafe { checked::<()>(context, CONTEXT_MAGIC) }.is_some()
}

// --- Context ---------------------------------------------------------------------------

unsafe extern "system" fn create_context(
    _this: *mut c_void,
    user: *mut c_void,
    context: *mut *mut c_void,
) -> HResult {
    if context.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a non-null user is the title's handle, checked by the user module.
    if !user.is_null() && unsafe { user::local_user_xuid(user) }.is_none() {
        return E_INVALIDARG;
    }
    // SAFETY: `context` is writable, per the title's contract.
    unsafe { *context = new_handle(CONTEXT_MAGIC, ()) };
    S_OK
}

unsafe extern "system" fn close_context(_this: *mut c_void, context: *mut c_void) {
    // SAFETY: forwarded to the title's contract.
    unsafe { close::<()>(context, CONTEXT_MAGIC) };
}

// --- Game licence: local and valid -------------------------------------------------------

unsafe extern "system" fn query_game_license_async(
    _this: *mut c_void,
    context: *mut c_void,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    if !unsafe { valid_context(context) } {
        return E_INVALIDARG;
    }
    save_async::begin(
        block,
        identity(&LICENSE_IDENTITY),
        Completion::value(S_OK, game_license()),
    )
}

unsafe extern "system" fn query_game_license_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    license: *mut GameLicense,
) -> HResult {
    save_async::result(
        block,
        identity(&LICENSE_IDENTITY),
        size_of::<GameLicense>(),
        license.cast(),
    )
}

// --- Product queries: an empty result set ---------------------------------------------------

fn queries() -> &'static Mutex<HashSet<usize>> {
    static QUERIES: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    QUERIES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn close_query(query: *mut c_void) {
    if queries().lock().unwrap().remove(&(query as usize)) {
        // SAFETY: removal proves this handle is live and only this call frees it.
        unsafe { close::<()>(query, QUERY_MAGIC) };
    }
}

/// Starts a query that completes with an empty query handle.
///
/// # Safety
/// `context` is the title's handle, `block` its XAsyncBlock.
unsafe fn begin_query(context: *mut c_void, block: *mut AsyncBlock) -> HResult {
    // SAFETY: forwarded to the caller's contract.
    if !unsafe { valid_context(context) } {
        return E_INVALIDARG;
    }
    let query = new_handle(QUERY_MAGIC, ());
    queries().lock().unwrap().insert(query as usize);
    let mut completion = Completion::value(S_OK, query);
    completion.discard = Some(Box::new(move || {
        close_query(query);
    }));
    save_async::begin(block, identity(&QUERY_IDENTITY), completion)
}

unsafe extern "system" fn query_associated_products_async(
    _this: *mut c_void,
    context: *mut c_void,
    _kind: u32,
    _count: u32,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

unsafe extern "system" fn query_products_async(
    _this: *mut c_void,
    context: *mut c_void,
    _kind: u32,
    ids: *const *const c_char,
    id_count: usize,
    _other: *const *const c_char,
    _other_count: usize,
    block: *mut AsyncBlock,
) -> HResult {
    if ids.is_null() && id_count > 0 {
        return E_INVALIDARG;
    }
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

unsafe extern "system" fn query_entitled_products_async(
    _this: *mut c_void,
    context: *mut c_void,
    _kind: u32,
    _count: u32,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

unsafe extern "system" fn query_product_for_current_game_async(
    _this: *mut c_void,
    context: *mut c_void,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

unsafe extern "system" fn query_product_for_package_async(
    _this: *mut c_void,
    context: *mut c_void,
    _kind: u32,
    package: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    if package.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

unsafe extern "system" fn query_associated_products_for_store_id_async(
    _this: *mut c_void,
    context: *mut c_void,
    store_id: *const c_char,
    _kind: u32,
    _count: u32,
    block: *mut AsyncBlock,
) -> HResult {
    if store_id.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_query(context, block) }
}

/// The result of the six query calls: the query handle.
unsafe extern "system" fn query_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    query: *mut *mut c_void,
) -> HResult {
    save_async::result(
        block,
        identity(&QUERY_IDENTITY),
        size_of::<*mut c_void>(),
        query.cast(),
    )
}

unsafe extern "system" fn enumerate_products_query(
    _this: *mut c_void,
    query: *mut c_void,
    _context: *mut c_void,
    callback: *mut c_void,
) -> HResult {
    let live = queries().lock().unwrap().contains(&(query as usize));
    if !live || callback.is_null() {
        return E_INVALIDARG;
    }
    // No product: the callback is never called.
    S_OK
}

unsafe extern "system" fn products_query_has_more_pages(
    _this: *mut c_void,
    _query: *mut c_void,
) -> bool {
    false
}

unsafe extern "system" fn close_products_query(_this: *mut c_void, query: *mut c_void) {
    close_query(query);
}

// --- Calls that need the store: the console's network error -----------------------------------

/// Starts a call that fails with `E_GAMESTORE_NETWORK_ERROR`.
///
/// # Safety
/// `context` is the title's handle, `block` its XAsyncBlock.
unsafe fn begin_offline(context: *mut c_void, block: *mut AsyncBlock) -> HResult {
    // SAFETY: forwarded to the caller's contract.
    if !unsafe { valid_context(context) } {
        return E_INVALIDARG;
    }
    save_async::begin(
        block,
        identity(&OFFLINE_IDENTITY),
        Completion::status(E_GAMESTORE_NETWORK_ERROR),
    )
}

unsafe extern "system" fn acquire_license_async(
    _this: *mut c_void,
    context: *mut c_void,
    _sku: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn product_page_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    _store_id: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn associated_products_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    _store_id: *const c_char,
    _kind: u32,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn purchase_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    _a: *const c_char,
    _b: *const c_char,
    _c: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn rate_and_review_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn redeem_token_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    _token: *const c_char,
    _tokens: *const *const c_char,
    _count: usize,
    _flag: bool,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn download_updates_async(
    _this: *mut c_void,
    context: *mut c_void,
    _ids: *const *const c_char,
    _count: usize,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

unsafe extern "system" fn gifting_ui_async(
    _this: *mut c_void,
    context: *mut c_void,
    _a: *const c_char,
    _b: *const c_char,
    _c: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { begin_offline(context, block) }
}

/// The result of a call that has no output: its status.
unsafe extern "system" fn offline_result(_this: *mut c_void, block: *mut AsyncBlock) -> HResult {
    save_async::status(block)
}

/// The result of a licence call: no licence handle, and the status.
unsafe extern "system" fn license_handle_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    license: *mut *mut c_void,
) -> HResult {
    if license.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `license` is writable, per the title's contract.
    unsafe { *license = std::ptr::null_mut() };
    save_async::status(block)
}

/// XStoreRateAndReviewResult: `wasUpdated` is false.
unsafe extern "system" fn rate_and_review_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    was_updated: *mut bool,
) -> HResult {
    if was_updated.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `was_updated` is writable, per the title's contract.
    unsafe { *was_updated = false };
    save_async::status(block)
}

unsafe extern "system" fn install_count_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    count: *mut u32,
) -> HResult {
    if count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `count` is writable, per the title's contract.
    unsafe { *count = 0 };
    save_async::status(block)
}

unsafe extern "system" fn install_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    _count: u32,
    _packages: *mut c_void,
) -> HResult {
    save_async::status(block)
}

// --- Licence changes: subscribed, never fired ----------------------------------------------------

unsafe extern "system" fn register_game_license_changed(
    _this: *mut c_void,
    context: *mut c_void,
    _a: *mut c_void,
    _b: *mut c_void,
    callback: *mut c_void,
    token: *mut u64,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    if !unsafe { valid_context(context) } {
        return E_INVALIDARG;
    }
    events::add_subscription(callback, token)
}

unsafe extern "system" fn unregister_game_license_changed(
    _this: *mut c_void,
    context: *mut c_void,
    token: u64,
    _wait: bool,
) -> bool {
    // SAFETY: forwarded to the title's contract.
    let live = unsafe { valid_context(context) };
    live && events::remove_subscription(token)
}

/// The method table of the store object, the slots of `gdk_api_table.inc` it writes.
pub fn methods() -> Vec<(usize, *const c_void)> {
    vec![
        (3, create_context as *const c_void),
        (4, close_context as *const c_void),
        (5, query_associated_products_async as *const c_void),
        (6, query_result as *const c_void),
        (7, query_products_async as *const c_void),
        (8, query_result as *const c_void),
        (9, query_entitled_products_async as *const c_void),
        (10, query_result as *const c_void),
        (11, query_product_for_current_game_async as *const c_void),
        (12, query_result as *const c_void),
        (13, query_product_for_package_async as *const c_void),
        (14, query_result as *const c_void),
        (15, enumerate_products_query as *const c_void),
        (16, products_query_has_more_pages as *const c_void),
        (19, close_products_query as *const c_void),
        (20, acquire_license_async as *const c_void),
        (21, license_handle_result as *const c_void),
        (28, query_game_license_async as *const c_void),
        (29, query_game_license_result as *const c_void),
        (49, purchase_ui_async as *const c_void),
        (50, offline_result as *const c_void),
        (51, rate_and_review_ui_async as *const c_void),
        (52, rate_and_review_result as *const c_void),
        (53, redeem_token_ui_async as *const c_void),
        (54, offline_result as *const c_void),
        (58, download_updates_async as *const c_void),
        (59, offline_result as *const c_void),
        (60, download_updates_async as *const c_void),
        (61, offline_result as *const c_void),
        (62, download_updates_async as *const c_void),
        (63, install_count_result as *const c_void),
        (64, install_result as *const c_void),
        (66, register_game_license_changed as *const c_void),
        (67, unregister_game_license_changed as *const c_void),
        (71, acquire_license_async as *const c_void),
        (72, license_handle_result as *const c_void),
        (73, associated_products_ui_async as *const c_void),
        (74, offline_result as *const c_void),
        (75, product_page_ui_async as *const c_void),
        (76, offline_result as *const c_void),
        (
            77,
            query_associated_products_for_store_id_async as *const c_void,
        ),
        (78, query_result as *const c_void),
        (82, gifting_ui_async as *const c_void),
        (83, offline_result as *const c_void),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_licence_is_active_and_complete() {
        let license = game_license();
        assert!(license.is_active);
        assert!(
            !license.is_trial && !license.is_disc_license && !license.is_trial_owned_by_this_user
        );
        assert_eq!(license.expiration_date, 0);
    }

    #[test]
    fn a_context_is_created_and_closed_and_another_kind_of_handle_is_refused() {
        let mut context: *mut c_void = std::ptr::null_mut();
        // SAFETY: the out pointer is writable; no user handle is given.
        assert_eq!(
            unsafe { create_context(std::ptr::null_mut(), std::ptr::null_mut(), &mut context) },
            S_OK
        );
        assert!(!context.is_null());
        let query = new_handle(QUERY_MAGIC, ());
        // SAFETY: the query handle is not a context, so the call is refused before the block is used.
        assert_eq!(
            unsafe { query_game_license_async(std::ptr::null_mut(), query, std::ptr::null_mut()) },
            E_INVALIDARG
        );
        // SAFETY: both handles are ours and closed once each.
        unsafe {
            close_context(std::ptr::null_mut(), context);
            close::<()>(query, QUERY_MAGIC);
        }
    }

    #[test]
    fn a_query_has_no_product_and_refuses_a_missing_callback() {
        let query = new_handle(QUERY_MAGIC, ());
        queries().lock().unwrap().insert(query as usize);
        // SAFETY: the query is ours; the callback is a placeholder that is never called.
        unsafe {
            assert_eq!(
                enumerate_products_query(
                    std::ptr::null_mut(),
                    query,
                    std::ptr::null_mut(),
                    std::ptr::dangling_mut::<c_void>()
                ),
                S_OK
            );
            assert_eq!(
                enumerate_products_query(
                    std::ptr::null_mut(),
                    query,
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                E_INVALIDARG
            );
            assert!(!products_query_has_more_pages(std::ptr::null_mut(), query));
            close_products_query(std::ptr::null_mut(), query);
            assert_eq!(
                enumerate_products_query(
                    std::ptr::null_mut(),
                    query,
                    std::ptr::null_mut(),
                    std::ptr::dangling_mut::<c_void>()
                ),
                E_INVALIDARG
            );
        }
    }

    #[test]
    fn a_licence_change_subscription_is_registered_and_removed_once() {
        let mut context: *mut c_void = std::ptr::null_mut();
        // SAFETY: the out pointer is writable.
        unsafe { create_context(std::ptr::null_mut(), std::ptr::null_mut(), &mut context) };
        let mut token = 0u64;
        // SAFETY: the context is ours; the callback is a placeholder that never runs.
        unsafe {
            assert_eq!(
                register_game_license_changed(
                    std::ptr::null_mut(),
                    context,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut token
                ),
                E_INVALIDARG
            );
            assert_eq!(
                register_game_license_changed(
                    std::ptr::null_mut(),
                    context,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::dangling_mut::<c_void>(),
                    &mut token
                ),
                S_OK
            );
            assert_ne!(token, 0);
            assert!(unregister_game_license_changed(
                std::ptr::null_mut(),
                context,
                token,
                false
            ));
            assert!(!unregister_game_license_changed(
                std::ptr::null_mut(),
                context,
                token,
                false
            ));
            assert!(!unregister_game_license_changed(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                token,
                false
            ));
            close_context(std::ptr::null_mut(), context);
        }
    }

    #[test]
    fn the_table_is_sorted_within_the_store_slots() {
        let slots: Vec<usize> = methods().iter().map(|(slot, _)| *slot).collect();
        assert!(slots.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(slots.iter().all(|slot| (3..=83).contains(slot)));
        assert_eq!(slots.len(), 44);
    }
}
