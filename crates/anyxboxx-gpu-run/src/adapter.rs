use std::{ffi::c_void, mem::size_of};
use windows::{
    Win32::Graphics::{Direct3D::D3D_FEATURE_LEVEL_11_0, Direct3D12::*, Dxgi::*},
    core::{Interface, Result},
};

fn name(adapter: &IDXGIAdapter1) -> String {
    unsafe {
        adapter
            .GetDesc1()
            .map(|desc| {
                String::from_utf16_lossy(
                    &desc.Description[..desc
                        .Description
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.Description.len())],
                )
            })
            .unwrap_or_default()
    }
}

fn device(adapter: &IDXGIAdapter1) -> Result<ID3D12Device> {
    let mut device = None;
    unsafe {
        D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device)?;
    }
    Ok(device.expect("D3D12CreateDevice returned no device"))
}

fn hardware(factory: &IDXGIFactory4) -> Vec<IDXGIAdapter1> {
    let mut result = Vec::new();
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        if unsafe { adapter.GetDesc1() }
            .is_ok_and(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
        {
            result.push(adapter);
        }
    }
    result
}

pub fn chosen() -> Result<(IDXGIAdapter1, ID3D12Device, &'static str)> {
    let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let filter = std::env::var("ANYXBOXX_GPU").unwrap_or_default();
    let (adapter, kind) = if filter == "warp" {
        (unsafe { factory.EnumWarpAdapter()? }, "WARP")
    } else if !filter.is_empty() && !filter.eq_ignore_ascii_case("warp") {
        let adapters = hardware(&factory);
        let Some(adapter) = adapters
            .iter()
            .find(|a| name(a).to_lowercase().contains(&filter.to_lowercase()))
            .cloned()
        else {
            let names = adapters.iter().map(name).collect::<Vec<_>>().join(", ");
            eprintln!(
                "[gpu] aucun adaptateur matériel ne contient « {filter} » ; adaptateurs : {}",
                if names.is_empty() { "aucun" } else { &names }
            );
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80004005u32 as i32,
            )));
        };
        (adapter, "matériel")
    } else if let Ok(adapter) = unsafe { factory.EnumAdapters1(0) } {
        if device(&adapter).is_ok() {
            (adapter, "matériel")
        } else {
            (unsafe { factory.EnumWarpAdapter()? }, "WARP")
        }
    } else {
        (unsafe { factory.EnumWarpAdapter()? }, "WARP")
    };
    let device = match device(&adapter) {
        Ok(device) => device,
        Err(error) => {
            eprintln!("[gpu] pas de périphérique D3D12 sur « {} »", name(&adapter));
            return Err(error);
        }
    };
    Ok((adapter, device, kind))
}

// The execution path asks D3D12 for its default adapter directly. Unlike
// --adaptateur, it does not first select DXGI adapter zero.
pub fn create() -> Result<(ID3D12Device, &'static str)> {
    let filter = std::env::var("ANYXBOXX_GPU").unwrap_or_default();
    if !filter.is_empty() && !filter.eq_ignore_ascii_case("warp") {
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let adapters = hardware(&factory);
        let Some(adapter) = adapters
            .iter()
            .find(|a| name(a).to_lowercase().contains(&filter.to_lowercase()))
        else {
            let names = adapters.iter().map(name).collect::<Vec<_>>().join(", ");
            eprintln!(
                "[gpu] aucun adaptateur matériel ne contient « {filter} » ; adaptateurs : {}",
                if names.is_empty() { "aucun" } else { &names }
            );
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80004005u32 as i32,
            )));
        };
        return match device(adapter) {
            Ok(device) => Ok((device, "matériel")),
            Err(error) => {
                eprintln!(
                    "[gpu] pas de périphérique D3D12 sur « {} » (0x{:08x})",
                    name(adapter),
                    error.code().0 as u32
                );
                Err(error)
            }
        };
    }
    if filter != "warp" {
        let mut result = None;
        if unsafe {
            D3D12CreateDevice(
                None::<&windows::core::IUnknown>,
                D3D_FEATURE_LEVEL_11_0,
                &mut result,
            )
        }
        .is_ok()
        {
            return Ok((
                result.expect("D3D12CreateDevice returned no device"),
                "matériel",
            ));
        }
    }
    let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let warp: IDXGIAdapter1 = unsafe { factory.EnumWarpAdapter()? };
    Ok((device(&warp)?, "WARP"))
}

fn feature<T: Default>(device: &ID3D12Device, kind: D3D12_FEATURE) -> T {
    let mut data = T::default();
    let _ = unsafe {
        device.CheckFeatureSupport(
            kind,
            &mut data as *mut T as *mut c_void,
            size_of::<T>() as u32,
        )
    };
    data
}

pub fn print_adapter() -> Result<()> {
    let (adapter, device, _) = chosen()?;
    let desc = unsafe { adapter.GetDesc1()? };
    println!(
        "adapter={} vendor={:04x} device={:04x}",
        name(&adapter),
        desc.VendorId,
        desc.DeviceId
    );
    if let Ok(version) = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) } {
        let v = version as u64;
        println!(
            "driver={}.{}.{}.{}",
            (v >> 48) & 65535,
            (v >> 32) & 65535,
            (v >> 16) & 65535,
            v & 65535
        );
    } else {
        println!("driver=inconnu");
    }
    println!(
        "backend={}",
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            "warp"
        } else {
            "hardware"
        }
    );
    let lanes: D3D12_FEATURE_DATA_D3D12_OPTIONS1 = feature(&device, D3D12_FEATURE_D3D12_OPTIONS1);
    println!(
        "native_lane_range={}-{}",
        lanes.WaveLaneCountMin, lanes.WaveLaneCountMax
    );
    let rays: D3D12_FEATURE_DATA_D3D12_OPTIONS5 = feature(&device, D3D12_FEATURE_D3D12_OPTIONS5);
    let tier = rays.RaytracingTier.0;
    if tier == 0 {
        println!("raytracing_tier=aucun");
    } else {
        println!("raytracing_tier={}.{}", tier / 10, tier % 10);
    }
    let options: D3D12_FEATURE_DATA_D3D12_OPTIONS = feature(&device, D3D12_FEATURE_D3D12_OPTIONS);
    println!(
        "double_precision={}",
        if options.DoublePrecisionFloatShaderOps.as_bool() {
            "oui"
        } else {
            "non"
        }
    );
    Ok(())
}
