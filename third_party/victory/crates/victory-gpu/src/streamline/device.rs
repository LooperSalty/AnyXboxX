use super::*;

pub(crate) fn device_made(device: &ID3D12Device) -> Option<ID3D12CommandQueue> {
    let plan = PLAN.get()?;
    let sl = SL.get()?;
    // SAFETY: the native device.
    let code = unsafe { (sl.set_d3d_device)(device.as_raw()) };
    if code != 0 {
        tracing::warn!(
            "Streamline: slSetD3DDevice failed: {} ({code}); DLSS and frame generation stay off",
            abi::result_name(code)
        );
        return None;
    }
    let function = |feature: u32, name: &CStr| -> Option<*mut c_void> {
        let mut f = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name and an out pointer.
        let code = unsafe { (sl.get_feature_function)(feature, name.as_ptr(), &mut f) };
        if code != 0 || f.is_null() {
            tracing::warn!(
                "Streamline: no {} ({}, {code})",
                name.to_string_lossy(),
                abi::result_name(code)
            );
            return None;
        }
        Some(f)
    };
    macro_rules! f {
        ($feature:expr, $name:literal, $t:ty) => {
            // SAFETY: the function of that name has the header's signature.
            function($feature, $name).map(|f| unsafe { std::mem::transmute::<*mut c_void, $t>(f) })
        };
    }
    let mut functions = Functions::default();
    if plan.dlss {
        functions.dlss_optimal = f!(abi::FEATURE_DLSS, c"slDLSSGetOptimalSettings", abi::SlDlssGetOptimalSettings);
        functions.dlss_set_options = f!(abi::FEATURE_DLSS, c"slDLSSSetOptions", abi::SlDlssSetOptions);
        let ready = functions.dlss_optimal.is_some() && functions.dlss_set_options.is_some();
        log_version(sl, abi::FEATURE_DLSS, "DLSS Super Resolution");
        DLSS_READY.store(ready, Release);
    }
    let _ = FUNCTIONS.set(functions);
    None
}

fn log_version(sl: &Sl, feature: u32, name: &str) {
    let mut version = abi::FeatureVersion {
        base: abi::BaseStructure::new(abi::FEATURE_VERSION, 1),
        version_sl: abi::Version::default(),
        version_ngx: abi::Version::default(),
    };
    // SAFETY: a structure to fill.
    let code = unsafe { (sl.get_feature_version)(feature, &mut version) };
    if code == 0 {
        tracing::info!(streamline = %version.version_sl, ngx = %version.version_ngx, "Streamline: {name} loaded");
    } else {
        tracing::warn!("Streamline: {name} has no version: {} ({code})", abi::result_name(code));
    }
}
