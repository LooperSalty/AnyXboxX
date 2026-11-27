//! NVIDIA Streamline 2.14.16, loaded from the user's signed runtime.
mod abi;
mod config;
mod library;
pub mod plan;
mod trust;

pub use config::{Asked, Available};
pub use plan::Mode;
use library::*;

use std::ffi::{CStr, c_char, c_void};
use std::path::Path;
use std::sync::OnceLock;
use windows::Win32::Graphics::Dxgi::*;

const INTERPOSER: &str = "sl.interposer.dll";
/// The project id NGX is given (engine "custom", no NVIDIA application id).
const PROJECT_ID: &CStr = c"6f0d7b2e-3a41-4c1e-9b57-2d8e4f1a9c30";
const ENGINE_VERSION: &CStr = c"VIctory 0.1";
static SL: OnceLock<Sl> = OnceLock::new();

/// What is on once the library is in: the switches and the features found.
struct Plan {
    asked: Asked,
    dlss: bool,
    framegen: bool,
}

static PLAN: OnceLock<Plan> = OnceLock::new();

/// Loads Streamline from `asked.dir` and asks for what `asked` names; call
/// once, before the host device is made (`crate::host::host`) and before the
/// title's imports are bound. Fails soft: what is not there is logged and
/// left off.
pub fn init(asked: &Asked) -> Available {
    if !asked.any() {
        return Available::default();
    }
    match load_and_init(asked, true) {
        Ok(available) => available,
        Err(why) => {
            tracing::warn!(
                folder = %asked.dir.display(),
                "Streamline: {why}; DLSS and frame generation stay off, the title's own upscaling and our present run"
            );
            Available::default()
        }
    }
}

fn load_and_init(asked: &Asked, require_line: bool) -> Result<Available, String> {
    let path = std::path::absolute(asked.dir.join(INTERPOSER)).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {INTERPOSER} in that folder (VICTORY_STREAMLINE_DIR)"));
    }
    let sl = load(&path)?;
    let sl = SL.get_or_init(|| sl);
    let dir_wide: Vec<u16> = wide(path.parent().expect("interposer has a parent"));
    let paths = [dir_wide.as_ptr()];
    let log_wide = asked.log_dir.as_deref().map(wide);
    // Frame generation presents from the line's thread, in order with the
    // submissions: without the line it is not loaded at all.
    let framegen_asked = asked.framegen && (!require_line || line_asked());
    if asked.framegen && !framegen_asked {
        tracing::warn!(
            "DLSS Frame Generation needs the line of submissions (VICTORY_THIN=1 or VICTORY_ASYNC_SUBMIT=1): frame generation stays off"
        );
    }
    let mut features = Vec::new();
    if asked.dlss {
        features.push(abi::FEATURE_DLSS);
    }
    if framegen_asked {
        features.extend([abi::FEATURE_DLSS_G, abi::FEATURE_REFLEX, abi::FEATURE_PCL]);
    }
    let preferences = abi::Preferences {
        base: abi::BaseStructure::new(abi::PREFERENCES, 1),
        show_console: false,
        log_level: if asked.log_dir.is_some() { abi::LOG_VERBOSE } else { abi::LOG_DEFAULT },
        paths_to_plugins: paths.as_ptr(),
        num_paths_to_plugins: 1,
        path_to_logs_and_data: log_wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr()),
        allocate_callback: std::ptr::null(),
        release_callback: std::ptr::null(),
        log_message_callback: Some(log_message),
        // No downloads (OTA): only what is in the folder runs.
        flags: abi::DISABLE_CL_STATE_TRACKING
            | abi::DISABLE_DEBUG_TEXT
            | abi::USE_MANUAL_HOOKING
            | abi::USE_DXGI_FACTORY_PROXY
            | abi::USE_FRAME_BASED_RESOURCE_TAGGING,
        features_to_load: features.as_ptr(),
        num_features_to_load: features.len() as u32,
        application_id: 0,
        engine: abi::ENGINE_CUSTOM,
        engine_version: ENGINE_VERSION.as_ptr(),
        project_id: PROJECT_ID.as_ptr(),
        render_api: abi::RENDER_API_D3D12,
    };
    // SAFETY: a filled preferences structure; the library copies what it keeps.
    let code = unsafe { (sl.init)(&preferences, abi::SDK_VERSION) };
    if code != 0 {
        return Err(format!("slInit failed: {} ({code})", abi::result_name(code)));
    }
    let adapter = default_adapter();
    let luid = adapter.as_ref().map(|a| a.1);
    let supported = |feature: u32| -> Result<(), String> {
        let Some(luid) = luid else { return Err("no adapter".into()) };
        let info = abi::AdapterInfo {
            base: abi::BaseStructure::new(abi::ADAPTER_INFO, 1),
            device_luid: std::ptr::from_ref(&luid).cast(),
            device_luid_size: size_of_val(&luid) as u32,
            vk_physical_device: std::ptr::null_mut(),
        };
        // SAFETY: an adapter description that lives through the call.
        let code = unsafe { (sl.is_feature_supported)(feature, &info) };
        if code == 0 { Ok(()) } else { Err(format!("{} ({code})", abi::result_name(code))) }
    };
    let dlss = asked.dlss && report("DLSS Super Resolution", abi::FEATURE_DLSS, supported(abi::FEATURE_DLSS), sl);
    let framegen = framegen_asked
        && report("DLSS Frame Generation", abi::FEATURE_DLSS_G, supported(abi::FEATURE_DLSS_G), sl)
        && report("Reflex", abi::FEATURE_REFLEX, supported(abi::FEATURE_REFLEX), sl);
    tracing::info!(
        library = %path.display(),
        sdk = "2.14.16",
        adapter = adapter.as_ref().map_or("?", |a| a.0.as_str()),
        dlss,
        framegen,
        mode = asked.mode.name(),
        preset = asked.preset,
        "Streamline loaded (manual hooking, no downloads)"
    );
    let _ = PLAN.set(Plan { asked: asked.clone(), dlss, framegen });
    Ok(Available { dlss, framegen })
}

/// Logs whether a feature is supported, with its requirements; whether it is.
fn report(name: &str, feature: u32, supported: Result<(), String>, sl: &Sl) -> bool {
    let mut requirements: abi::FeatureRequirements = unsafe_zeroed_requirements();
    // SAFETY: a structure to fill.
    let asked = unsafe { (sl.get_feature_requirements)(feature, &mut requirements) };
    let needs = (asked == 0).then(|| {
        format!(
            "OS {} (needs {}), driver {} (needs {}){}{}",
            requirements.os_version_detected,
            requirements.os_version_required,
            requirements.driver_version_detected,
            requirements.driver_version_required,
            if requirements.flags & abi::REQUIRES_HARDWARE_SCHEDULING != 0 { ", hardware GPU scheduling" } else { "" },
            if requirements.flags & abi::REQUIRES_VSYNC_OFF != 0 { ", v-sync off" } else { "" },
        )
    });
    match supported {
        Ok(()) => {
            tracing::info!(requirements = needs.as_deref().unwrap_or("?"), "Streamline: {name} is supported on this GPU");
            true
        }
        Err(why) => {
            tracing::warn!(requirements = needs.as_deref().unwrap_or("?"), "Streamline: {name} is not supported here: {why}");
            false
        }
    }
}

fn unsafe_zeroed_requirements() -> abi::FeatureRequirements {
    // SAFETY: plain data; pointers null, counts zero.
    let mut r: abi::FeatureRequirements = unsafe { std::mem::zeroed() };
    r.base = abi::BaseStructure::new(abi::FEATURE_REQUIREMENTS, 2);
    r
}

/// Whether the switches turn the line of submissions on (frame generation
/// presents from its thread, in order with the submissions).
fn line_asked() -> bool {
    let on = |name: &str| std::env::var_os(name).is_some_and(|v| v == "1");
    on("VICTORY_THIN") || on("VICTORY_ASYNC_SUBMIT")
}

unsafe extern "C" fn log_message(kind: u32, message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: a NUL-terminated message for the call.
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    let text = text.trim_end();
    // (Every plugin in the folder that was not asked for says so.)
    let noise = text.contains("since it is was not requested by the host");
    match kind {
        abi::LOG_ERROR => tracing::error!(target: "streamline", "{text}"),
        abi::LOG_WARN if !noise => tracing::warn!(target: "streamline", "{text}"),
        _ => tracing::debug!(target: "streamline", "{text}"),
    }
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The adapter a device made with no adapter named is on (the first), its
/// name and LUID.
fn default_adapter() -> Option<(String, windows::Win32::Foundation::LUID)> {
    // SAFETY: plain DXGI queries.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter = factory.EnumAdapters1(0).ok()?;
        let desc = adapter.GetDesc1().ok()?;
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        Some((String::from_utf16_lossy(&desc.Description[..len]), desc.AdapterLuid))
    }
}


/// Probe the driver without creating a title process.
pub fn check() -> Result<(), String> {
    let mut asked = Asked::from_env()?;
    asked.dlss = true;
    asked.framegen = true;
    let available = load_and_init(&asked, false)
        .map_err(|why| format!("Streamline at {}: {why}", asked.dir.display()))?;
    println!("DLSS Super Resolution: {}. Frame Generation with Reflex: {}.",
        available.dlss, available.framegen);
    // SAFETY: this command owns the only session; no device or frame exists.
    let code = unsafe { (SL.get().unwrap().shutdown)() };
    if code != 0 { return Err(format!("slShutdown: {} ({code})", abi::result_name(code))); }
    if !available.dlss && !available.framegen { return Err("no requested feature is supported".into()); }
    Ok(())
}
