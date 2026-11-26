use anyxboxx_native_launcher::redirect::{Redirect, prefer_tombox, redirect, runtime_for};

#[test]
fn runtime_mapping_matches_native_launcher() {
    assert_eq!(runtime_for(r"C:\runtime\XGameRuntime_141.dll"), Some(0));
    assert_eq!(runtime_for("XMEM"), Some(1));
    assert_eq!(runtime_for("d3d12_xs.dll"), Some(2));
    assert_eq!(runtime_for("kernelx.dll"), Some(3));
    assert_eq!(runtime_for("XCRDAPI.dll"), None);
    assert_eq!(
        (0..6).filter(|&i| prefer_tombox(i)).collect::<Vec<_>>(),
        [1, 2, 5]
    );
}

#[test]
fn registry_and_dxgi_redirect_to_runtime_exports() {
    assert_eq!(
        redirect("ADVAPI32.DLL", b"RegOpenKeyW", true, false),
        Some(Redirect::RuntimeExport(b"AnyXboxX_RegOpenKeyW".to_vec()))
    );
    assert_eq!(
        redirect("dxgi.dll", b"CreateDXGIFactory2", false, false),
        None
    );
    assert_eq!(
        redirect("dxgi.dll", b"CreateDXGIFactory2", false, true),
        Some(Redirect::RuntimeExport(
            b"AnyXboxX_CreateDXGIFactory2".to_vec()
        ))
    );
    assert_eq!(redirect("advapi32.dll", b"regopenkeyw", false, true), None);
}

#[test]
fn dll_imports_only_get_the_getprocaddress_shim() {
    assert_eq!(
        redirect("KERNEL32.DLL", b"LoadLibraryW", false, false),
        Some(Redirect::Shim)
    );
    assert_eq!(redirect("KERNEL32.DLL", b"LoadLibraryW", true, false), None);
    assert_eq!(
        redirect(
            "api-ms-win-core-libraryloader-l1-2-0.dll",
            b"GetProcAddress",
            true,
            false
        ),
        Some(Redirect::Shim)
    );
    assert_eq!(
        redirect("user32.dll", b"GetProcAddress", false, false),
        None
    );
}
