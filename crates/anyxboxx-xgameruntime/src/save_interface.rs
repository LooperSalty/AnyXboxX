//! The XGameSave and XGameSaveFiles interface (C++ `save_interface.cpp`), on the PC's disk.
//!
//! Provider {704C3F58…}, interface {1BFFF3AF…}, methods 3 to 32 of `gdk_api_table.inc`. The
//! storage is in `save_store`; there is no cloud, so nothing is ever out of sync.
//!
//! Handles are heap records led by a magic word; a handle of another kind, or a closed one, is an
//! invalid argument, as in the C++.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use victory_hle::modules::xasync::AsyncBlock;

use crate::handle::{checked, close, new_handle};
use crate::object::{E_INVALIDARG, HResult, S_OK};
use crate::save_async::{self, Completion};
use crate::save_blobs::{BlobSet, GameSaveBlob, GameSaveBlobInfo};
use crate::save_store::{
    self, E_GS_INVALID_CONTAINER_NAME, E_GS_UPDATE_TOO_BIG, E_PATH_NOT_FOUND, MAX_BLOB_BYTES,
    MAX_BLOB_NAME, MAX_CONTAINER_NAME, MAX_DISPLAY_NAME, QUOTA_BYTES, UpdateOperation,
};

const PROVIDER_MAGIC: u32 = 0x5047_5841; // « AXGP »
const CONTAINER_MAGIC: u32 = 0x4347_5841; // « AXGC »
const UPDATE_MAGIC: u32 = 0x5547_5841; // « AXGU »
/// The longest configuration identifier the game may give.
const MAX_CONFIGURATION_ID: usize = 64;

/// XGameSaveContainerInfo, as the title reads it (its fields are read by the title, not by Rust).
#[repr(C)]
#[allow(dead_code)]
pub struct GameSaveContainerInfo {
    name: *const c_char,
    display_name: *const c_char,
    blob_count: u32,
    total_size: u64,
    last_modified: i64,
    needs_sync: bool,
}

type BlobCallback = Option<unsafe extern "system" fn(*const GameSaveBlobInfo, *mut c_void) -> bool>;
type ContainerCallback =
    Option<unsafe extern "system" fn(*const GameSaveContainerInfo, *mut c_void) -> bool>;

struct Provider {
    folder: PathBuf,
}

struct Container {
    provider: PathBuf,
    folder: PathBuf,
}

struct Update {
    provider: PathBuf,
    folder: PathBuf,
    display_name: Vec<u8>,
    operations: Vec<UpdateOperation>,
}

// Identities of the asynchronous calls, as the C++ static characters: XAsync matches a result to
// its call by address. The contents are distinct only so the compiler cannot merge them.
static INITIALIZE_IDENTITY: [u8; 1] = [1];
static QUOTA_IDENTITY: [u8; 1] = [2];
static EMPTY_IDENTITY: [u8; 1] = [3];
static READ_IDENTITY: [u8; 1] = [4];
static FOLDER_IDENTITY: [u8; 1] = [5];

thread_local! {
    /// Blobs laid out by the last asynchronous read copied on this thread (the C++ `t_blobs_laid_out`).
    static BLOBS_LAID_OUT: Cell<u32> = const { Cell::new(0) };
}

fn identity(id: &'static [u8; 1]) -> *const c_void {
    id.as_ptr().cast()
}

/// The text of a C string the title passed, or `None` for a null pointer.
///
/// # Safety
/// `text` is null or a NUL-terminated string.
unsafe fn c_bytes<'a>(text: *const c_char) -> Option<&'a [u8]> {
    if text.is_null() {
        return None;
    }
    // SAFETY: a non-null pointer is a NUL-terminated string, per the contract above.
    Some(unsafe { CStr::from_ptr(text) }.to_bytes())
}

/// A name the title gave, checked for length (`None` when null, empty or too long).
///
/// # Safety
/// As [`c_bytes`].
unsafe fn checked_name<'a>(text: *const c_char, max: usize) -> Option<&'a [u8]> {
    // SAFETY: forwarded to the caller's contract.
    unsafe { c_bytes(text) }.filter(|name| save_store::valid_name(name, max))
}

/// The XUID of a user handle, or `E_INVALIDARG`.
///
/// # Safety
/// As the user module's handle check.
unsafe fn xuid_of(user: *const c_void) -> Result<u64, HResult> {
    // SAFETY: forwarded to the caller's contract.
    unsafe { crate::user::local_user_xuid(user) }.ok_or(E_INVALIDARG)
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

fn used_by(folder: &Path) -> i64 {
    save_store::used_bytes(folder) as i64
}

fn remaining_quota(folder: &Path) -> i64 {
    QUOTA_BYTES as i64 - used_by(folder)
}

/// Opens the provider of a user and configuration, or says why not.
///
/// # Safety
/// `user` is the title's user handle; `configuration_id` is null or a C string.
unsafe fn open_provider(
    user: *const c_void,
    configuration_id: *const c_char,
) -> Result<*mut c_void, HResult> {
    // SAFETY: forwarded to the callers' contract.
    let xuid = unsafe { xuid_of(user) }?;
    let configuration =
        unsafe { checked_name(configuration_id, MAX_CONFIGURATION_ID) }.ok_or(E_INVALIDARG)?;
    let (root, title) = save_store::title_root().ok_or(E_PATH_NOT_FOUND)?;
    let folder =
        save_store::provider_folder(&root, title, configuration, xuid).ok_or(E_PATH_NOT_FOUND)?;
    Ok(new_handle(PROVIDER_MAGIC, Provider { folder }))
}

/// Calls the title's container callback. Returns whether enumeration goes on.
fn report_container(
    info: &save_store::ContainerInfo,
    context: *mut c_void,
    callback: unsafe extern "system" fn(*const GameSaveContainerInfo, *mut c_void) -> bool,
) -> bool {
    let name = c_string(&info.name);
    let display = c_string(&info.display_name);
    let out = GameSaveContainerInfo {
        name: name.as_ptr(),
        display_name: display.as_ptr(),
        blob_count: info.blob_count,
        total_size: info.total_size,
        last_modified: info.last_modified,
        needs_sync: false,
    };
    // SAFETY: the title's callback takes the record and its strings for the duration of the call.
    unsafe { callback(&out, context) }
}

/// A C string of `bytes`, cut at the first NUL as the C++ `c_str` users see it.
fn c_string(bytes: &[u8]) -> CString {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    CString::new(&bytes[..end]).unwrap_or_default()
}

/// The blobs a read asks for: the names given, or every blob of the container when there are none.
///
/// # Safety
/// `names` is null or points at `count` C strings (null entries are refused).
unsafe fn collect(
    folder: &Path,
    names: *const *const c_char,
    count: u32,
) -> Result<BlobSet, HResult> {
    let mut set = BlobSet::default();
    if names.is_null() || count == 0 {
        set.names = save_store::list_blobs(folder, None)
            .into_iter()
            .map(|blob| blob.name)
            .collect();
    } else {
        for index in 0..count as usize {
            // SAFETY: the title passed `count` entries, per the contract.
            let name = unsafe { *names.add(index) };
            // SAFETY: each entry is null or a C string.
            let name = unsafe { c_bytes(name) }.ok_or(E_INVALIDARG)?;
            set.names.push(name.to_vec());
        }
    }
    for name in &set.names {
        set.data.push(save_store::read_blob(folder, name)?);
    }
    Ok(set)
}

// --- Provider -----------------------------------------------------------------------------

unsafe extern "system" fn initialize_provider(
    _this: *mut c_void,
    user: *mut c_void,
    configuration_id: *const c_char,
    _sync: bool,
    provider: *mut *mut c_void,
) -> HResult {
    if provider.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `provider` is writable, per the title's contract.
    unsafe { *provider = std::ptr::null_mut() };
    // SAFETY: forwarded to the title's contract.
    match unsafe { open_provider(user, configuration_id) } {
        Ok(handle) => {
            // SAFETY: as above.
            unsafe { *provider = handle };
            S_OK
        }
        Err(error) => error,
    }
}

unsafe extern "system" fn initialize_provider_async(
    _this: *mut c_void,
    user: *mut c_void,
    configuration_id: *const c_char,
    _sync: bool,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let (result, handle) = match unsafe { open_provider(user, configuration_id) } {
        Ok(handle) => (S_OK, handle),
        Err(error) => (error, std::ptr::null_mut()),
    };
    let mut completion = Completion::value(result, handle);
    if !handle.is_null() {
        completion.discard = Some(Box::new(move || {
            // SAFETY: the provider was never read, so nothing else uses it.
            unsafe { close::<Provider>(handle, PROVIDER_MAGIC) };
        }));
    }
    save_async::begin(block, identity(&INITIALIZE_IDENTITY), completion)
}

unsafe extern "system" fn initialize_provider_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    provider: *mut *mut c_void,
) -> HResult {
    save_async::result(
        block,
        identity(&INITIALIZE_IDENTITY),
        size_of::<*mut c_void>(),
        provider.cast(),
    )
}

unsafe extern "system" fn close_provider(_this: *mut c_void, handle: *mut c_void) {
    // SAFETY: forwarded to the title's contract.
    unsafe { close::<Provider>(handle, PROVIDER_MAGIC) };
}

unsafe extern "system" fn get_remaining_quota(
    _this: *mut c_void,
    handle: *mut c_void,
    remaining: *mut i64,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    if remaining.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `remaining` is writable, per the title's contract.
    unsafe { *remaining = remaining_quota(&provider.folder) };
    S_OK
}

unsafe extern "system" fn get_remaining_quota_async(
    _this: *mut c_void,
    handle: *mut c_void,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    let value = remaining_quota(&provider.folder);
    save_async::begin(
        block,
        identity(&QUOTA_IDENTITY),
        Completion::value(S_OK, value),
    )
}

unsafe extern "system" fn get_remaining_quota_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    remaining: *mut i64,
) -> HResult {
    save_async::result(
        block,
        identity(&QUOTA_IDENTITY),
        size_of::<i64>(),
        remaining.cast(),
    )
}

/// Deletes a container of a provider.
///
/// # Safety
/// `handle` is the title's provider; `name` is null or a C string.
unsafe fn delete_container(handle: *mut c_void, name: *const c_char) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let Some(name) = (unsafe { checked_name(name, MAX_CONTAINER_NAME) }) else {
        return E_GS_INVALID_CONTAINER_NAME;
    };
    save_store::delete_container(&provider.folder, name)
}

unsafe extern "system" fn delete_container_method(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { delete_container(handle, name) }
}

unsafe extern "system" fn delete_container_async(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let result = unsafe { delete_container(handle, name) };
    save_async::begin(block, identity(&EMPTY_IDENTITY), Completion::status(result))
}

unsafe extern "system" fn empty_result(_this: *mut c_void, block: *mut AsyncBlock) -> HResult {
    save_async::status(block)
}

unsafe extern "system" fn get_container_info(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
    context: *mut c_void,
    callback: ContainerCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let (Some(name), Some(callback)) = (unsafe { c_bytes(name) }, callback) else {
        return E_INVALIDARG;
    };
    // A missing container is not reported: the title tests for it by whether the callback runs.
    if let Some(info) = save_store::find_container(&provider.folder, name) {
        report_container(&info, context, callback);
    }
    S_OK
}

/// Calls the callback for each container starting with `prefix` (all of them when null), until it says stop.
///
/// # Safety
/// `handle` is the title's provider; `prefix` is null or a C string.
unsafe fn enumerate_containers(
    handle: *mut c_void,
    prefix: *const c_char,
    context: *mut c_void,
    callback: ContainerCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    let Some(callback) = callback else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let prefix = unsafe { c_bytes(prefix) };
    for info in save_store::list_containers(&provider.folder, prefix) {
        if !report_container(&info, context, callback) {
            break;
        }
    }
    S_OK
}

unsafe extern "system" fn enumerate_container_info(
    _this: *mut c_void,
    handle: *mut c_void,
    context: *mut c_void,
    callback: ContainerCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { enumerate_containers(handle, std::ptr::null(), context, callback) }
}

unsafe extern "system" fn enumerate_container_info_by_name(
    _this: *mut c_void,
    handle: *mut c_void,
    prefix: *const c_char,
    context: *mut c_void,
    callback: ContainerCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { enumerate_containers(handle, prefix, context, callback) }
}

// --- Containers and blobs -----------------------------------------------------------------

unsafe extern "system" fn create_container(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
    container: *mut *mut c_void,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(provider) = (unsafe { checked::<Provider>(handle, PROVIDER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    if container.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `container` is writable, per the title's contract.
    unsafe { *container = std::ptr::null_mut() };
    // SAFETY: as above.
    let Some(name) = (unsafe { checked_name(name, MAX_CONTAINER_NAME) }) else {
        return E_GS_INVALID_CONTAINER_NAME;
    };
    let value = Container {
        provider: provider.folder.clone(),
        folder: save_store::container_folder(&provider.folder, name),
    };
    // SAFETY: as above.
    unsafe { *container = new_handle(CONTAINER_MAGIC, value) };
    S_OK
}

unsafe extern "system" fn close_container(_this: *mut c_void, handle: *mut c_void) {
    // SAFETY: forwarded to the title's contract.
    unsafe { close::<Container>(handle, CONTAINER_MAGIC) };
}

/// Calls the callback for each blob of a container starting with `prefix` (all when null).
///
/// # Safety
/// `handle` is the title's container; `prefix` is null or a C string.
unsafe fn enumerate_blobs(
    handle: *mut c_void,
    prefix: *const c_char,
    context: *mut c_void,
    callback: BlobCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(container) = (unsafe { checked::<Container>(handle, CONTAINER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    let Some(callback) = callback else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let prefix = unsafe { c_bytes(prefix) };
    for blob in save_store::list_blobs(&container.folder, prefix) {
        let name = c_string(&blob.name);
        let info = GameSaveBlobInfo {
            name: name.as_ptr(),
            size: blob.size,
        };
        // SAFETY: the callback takes the record and its name for the duration of the call.
        if !unsafe { callback(&info, context) } {
            break;
        }
    }
    S_OK
}

unsafe extern "system" fn enumerate_blob_info(
    _this: *mut c_void,
    handle: *mut c_void,
    context: *mut c_void,
    callback: BlobCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { enumerate_blobs(handle, std::ptr::null(), context, callback) }
}

unsafe extern "system" fn enumerate_blob_info_by_name(
    _this: *mut c_void,
    handle: *mut c_void,
    prefix: *const c_char,
    context: *mut c_void,
    callback: BlobCallback,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { enumerate_blobs(handle, prefix, context, callback) }
}

/// Reads blobs into the title's buffer; `count` gives the blob count back.
unsafe extern "system" fn read_blob_data(
    _this: *mut c_void,
    handle: *mut c_void,
    names: *const *const c_char,
    count: *mut u32,
    size: usize,
    blobs: *mut GameSaveBlob,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(container) = (unsafe { checked::<Container>(handle, CONTAINER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    if count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `count` is readable and writable, per the title's contract.
    let set = match unsafe { collect(&container.folder, names, *count) } {
        Ok(set) => set,
        Err(error) => return error,
    };
    if blobs.is_null() || size < set.required() {
        return save_store::E_GS_PROVIDED_BUFFER_TOO_SMALL;
    }
    // SAFETY: the buffer holds `required()` bytes, per the check above.
    unsafe { set.layout(blobs.cast()) };
    // SAFETY: as above.
    unsafe { *count = set.names.len() as u32 };
    S_OK
}

unsafe extern "system" fn read_blob_data_async(
    _this: *mut c_void,
    handle: *mut c_void,
    names: *const *const c_char,
    count: u32,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(container) = (unsafe { checked::<Container>(handle, CONTAINER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let completion = match unsafe { collect(&container.folder, names, count) } {
        Ok(set) => {
            let size = set.required();
            let laid_out = set.names.len() as u32;
            Completion {
                result: S_OK,
                size,
                write: Some(Box::new(move |buffer| {
                    // SAFETY: the title's buffer holds `size` bytes and is aligned for the records.
                    unsafe { set.layout(buffer.cast()) };
                    BLOBS_LAID_OUT.with(|cell| cell.set(laid_out));
                })),
                discard: None,
            }
        }
        Err(error) => Completion::status(error),
    };
    save_async::begin(block, identity(&READ_IDENTITY), completion)
}

unsafe extern "system" fn read_blob_data_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    size: usize,
    blobs: *mut GameSaveBlob,
    count: *mut u32,
) -> HResult {
    BLOBS_LAID_OUT.with(|cell| cell.set(0));
    let result = save_async::result(block, identity(&READ_IDENTITY), size, blobs.cast());
    if !count.is_null() {
        let laid_out = if result >= 0 {
            BLOBS_LAID_OUT.with(Cell::get)
        } else {
            0
        };
        // SAFETY: `count` is writable, per the title's contract.
        unsafe { *count = laid_out };
    }
    result
}

// --- Updates: applied in one go when submitted --------------------------------------------

unsafe extern "system" fn create_update(
    _this: *mut c_void,
    handle: *mut c_void,
    display_name: *const c_char,
    update: *mut *mut c_void,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(container) = (unsafe { checked::<Container>(handle, CONTAINER_MAGIC) }) else {
        return E_INVALIDARG;
    };
    if update.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `update` is writable, per the title's contract.
    unsafe { *update = std::ptr::null_mut() };
    // SAFETY: as above.
    let Some(display) =
        (unsafe { c_bytes(display_name) }).filter(|name| name.len() <= MAX_DISPLAY_NAME)
    else {
        return E_INVALIDARG;
    };
    let value = Update {
        provider: container.provider.clone(),
        folder: container.folder.clone(),
        display_name: display.to_vec(),
        operations: Vec::new(),
    };
    // SAFETY: as above.
    unsafe { *update = new_handle(UPDATE_MAGIC, value) };
    S_OK
}

unsafe extern "system" fn close_update(_this: *mut c_void, handle: *mut c_void) {
    // SAFETY: forwarded to the title's contract.
    unsafe { close::<Update>(handle, UPDATE_MAGIC) };
}

unsafe extern "system" fn submit_blob_write(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
    data: *const u8,
    size: usize,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(update) = (unsafe { checked::<Update>(handle, UPDATE_MAGIC) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let Some(name) = (unsafe { checked_name(name, MAX_BLOB_NAME) }) else {
        return E_INVALIDARG;
    };
    if data.is_null() && size > 0 {
        return E_INVALIDARG;
    }
    if size > MAX_BLOB_BYTES {
        return E_GS_UPDATE_TOO_BIG;
    }
    let bytes = if size == 0 {
        Vec::new()
    } else {
        // SAFETY: `data` is readable for `size` bytes, per the title's contract.
        unsafe { std::slice::from_raw_parts(data, size) }.to_vec()
    };
    update.operations.push(UpdateOperation {
        blob: name.to_vec(),
        erase: false,
        data: bytes,
    });
    S_OK
}

unsafe extern "system" fn submit_blob_delete(
    _this: *mut c_void,
    handle: *mut c_void,
    name: *const c_char,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(update) = (unsafe { checked::<Update>(handle, UPDATE_MAGIC) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: as above.
    let Some(name) = (unsafe { checked_name(name, MAX_BLOB_NAME) }) else {
        return E_INVALIDARG;
    };
    update.operations.push(UpdateOperation {
        blob: name.to_vec(),
        erase: true,
        data: Vec::new(),
    });
    S_OK
}

/// Applies a submitted update to its container.
///
/// # Safety
/// `handle` is the title's update.
unsafe fn submit_update(handle: *mut c_void) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let Some(update) = (unsafe { checked::<Update>(handle, UPDATE_MAGIC) }) else {
        return E_INVALIDARG;
    };
    save_store::apply_update(
        &update.provider,
        &update.folder,
        &update.display_name,
        &update.operations,
    )
}

unsafe extern "system" fn submit_update_method(_this: *mut c_void, handle: *mut c_void) -> HResult {
    // SAFETY: forwarded to the title's contract.
    unsafe { submit_update(handle) }
}

unsafe extern "system" fn submit_update_async(
    _this: *mut c_void,
    handle: *mut c_void,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let result = unsafe { submit_update(handle) };
    save_async::begin(block, identity(&EMPTY_IDENTITY), Completion::status(result))
}

// --- XGameSaveFiles: one plain folder per user --------------------------------------------

unsafe extern "system" fn files_folder_async(
    _this: *mut c_void,
    user: *mut c_void,
    configuration_id: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    // SAFETY: forwarded to the title's contract.
    let xuid = unsafe { xuid_of(user) };
    let configuration = unsafe { checked_name(configuration_id, MAX_CONFIGURATION_ID) };
    let (Ok(xuid), Some(configuration)) = (xuid, configuration) else {
        return E_INVALIDARG;
    };
    let folder = save_store::title_root()
        .and_then(|(root, title)| save_store::files_folder(&root, title, configuration, xuid));
    let (result, mut text) = match folder {
        Some(path) => (S_OK, path_bytes(&path)),
        None => (E_PATH_NOT_FOUND, Vec::new()),
    };
    let size = text.len() + 1;
    text.push(0);
    let completion = Completion {
        result,
        size,
        write: Some(Box::new(move |buffer| {
            // SAFETY: the title's buffer holds the `size` bytes, NUL included.
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast::<u8>(), text.len())
            };
        })),
        discard: None,
    };
    save_async::begin(block, identity(&FOLDER_IDENTITY), completion)
}

unsafe extern "system" fn files_folder_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    size: usize,
    folder: *mut c_char,
) -> HResult {
    save_async::result(block, identity(&FOLDER_IDENTITY), size, folder.cast())
}

unsafe extern "system" fn files_remaining_quota(
    _this: *mut c_void,
    user: *mut c_void,
    configuration_id: *const c_char,
    remaining: *mut i64,
) -> HResult {
    if remaining.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: forwarded to the title's contract.
    let xuid = unsafe { xuid_of(user) };
    let configuration = unsafe { checked_name(configuration_id, MAX_CONFIGURATION_ID) };
    let (Ok(xuid), Some(configuration)) = (xuid, configuration) else {
        return E_INVALIDARG;
    };
    let used = save_store::title_root()
        .and_then(|(root, title)| save_store::files_folder(&root, title, configuration, xuid))
        .map_or(0, |folder| used_by(&folder));
    // SAFETY: `remaining` is writable, per the title's contract.
    unsafe { *remaining = QUOTA_BYTES as i64 - used };
    S_OK
}

/// The method table of the GameSave object, slots 3 to 32 of `gdk_api_table.inc`.
pub fn methods() -> Vec<(usize, *const c_void)> {
    let table: [(usize, *const c_void); 30] = [
        (3, initialize_provider as *const c_void),
        (4, initialize_provider_async as *const c_void),
        (5, initialize_provider_result as *const c_void),
        (6, close_provider as *const c_void),
        (7, get_remaining_quota as *const c_void),
        (8, get_remaining_quota_async as *const c_void),
        (9, get_remaining_quota_result as *const c_void),
        (10, delete_container_method as *const c_void),
        (11, delete_container_async as *const c_void),
        (12, empty_result as *const c_void),
        (13, get_container_info as *const c_void),
        (14, enumerate_container_info as *const c_void),
        (15, enumerate_container_info_by_name as *const c_void),
        (16, create_container as *const c_void),
        (17, close_container as *const c_void),
        (18, enumerate_blob_info as *const c_void),
        (19, enumerate_blob_info_by_name as *const c_void),
        (20, read_blob_data as *const c_void),
        (21, read_blob_data_async as *const c_void),
        (22, read_blob_data_result as *const c_void),
        (23, create_update as *const c_void),
        (24, close_update as *const c_void),
        (25, submit_blob_write as *const c_void),
        (26, submit_blob_delete as *const c_void),
        (27, submit_update_method as *const c_void),
        (28, submit_update_async as *const c_void),
        (29, empty_result as *const c_void),
        (30, files_folder_async as *const c_void),
        (31, files_folder_result as *const c_void),
        (32, files_remaining_quota as *const c_void),
    ];
    table.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_covers_slots_3_to_32_once_each() {
        let slots: Vec<usize> = methods().iter().map(|(slot, _)| *slot).collect();
        assert_eq!(slots, (3..=32).collect::<Vec<usize>>());
    }

    #[test]
    fn a_closed_or_foreign_handle_is_refused() {
        let mut foreign = 0u64;
        let refused = std::ptr::from_mut(&mut foreign).cast::<c_void>();
        // SAFETY: the pointer is readable (a live local) and holds no magic word.
        assert!(unsafe { checked::<Provider>(refused, PROVIDER_MAGIC) }.is_none());
        // SAFETY: a null handle is refused without being read.
        let mut out = 1i64;
        assert_eq!(
            unsafe { get_remaining_quota(std::ptr::null_mut(), std::ptr::null_mut(), &mut out) },
            E_INVALIDARG
        );
    }

    #[test]
    fn a_saved_blob_is_read_back_through_the_handles() {
        let dir = std::env::temp_dir().join(format!("axsave-handles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let provider = new_handle(
            PROVIDER_MAGIC,
            Provider {
                folder: dir.join("provider"),
            },
        );
        let mut container: *mut c_void = std::ptr::null_mut();
        let name = c"slot";
        // SAFETY: the handles are ours, the strings are NUL-terminated, the out pointers are writable.
        unsafe {
            assert_eq!(
                create_container(
                    std::ptr::null_mut(),
                    provider,
                    name.as_ptr(),
                    &mut container
                ),
                S_OK
            );
            let mut update: *mut c_void = std::ptr::null_mut();
            assert_eq!(
                create_update(
                    std::ptr::null_mut(),
                    container,
                    c"Partie".as_ptr(),
                    &mut update
                ),
                S_OK
            );
            let data = b"hello";
            assert_eq!(
                submit_blob_write(
                    std::ptr::null_mut(),
                    update,
                    c"save".as_ptr(),
                    data.as_ptr(),
                    data.len()
                ),
                S_OK
            );
            assert_eq!(submit_update(update), S_OK);
            close_update(std::ptr::null_mut(), update);

            let mut count = 0u32;
            let size = 256;
            let mut buffer = vec![0u64; size / 8];
            let blobs = buffer.as_mut_ptr().cast::<GameSaveBlob>();
            assert_eq!(
                read_blob_data(
                    std::ptr::null_mut(),
                    container,
                    std::ptr::null(),
                    &mut count,
                    size,
                    blobs
                ),
                S_OK
            );
            assert_eq!(count, 1);
            let blob = &*blobs;
            assert_eq!(CStr::from_ptr(blob.info.name).to_bytes(), b"save");
            assert_eq!(std::slice::from_raw_parts(blob.data, 5), data);
            close_container(std::ptr::null_mut(), container);
            close_provider(std::ptr::null_mut(), provider);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
