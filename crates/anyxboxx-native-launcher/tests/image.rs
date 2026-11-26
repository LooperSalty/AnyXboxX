use anyxboxx_native_launcher::image::{Image, ImageError};

fn put16(data: &mut [u8], at: usize, n: u16) {
    data[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn put32(data: &mut [u8], at: usize, n: u32) {
    data[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn put64(data: &mut [u8], at: usize, n: u64) {
    data[at..at + 8].copy_from_slice(&n.to_le_bytes());
}

fn fixture() -> Vec<u8> {
    let mut file = vec![0; 0x400];
    put16(&mut file, 0, 0x5a4d);
    put32(&mut file, 60, 0x80);
    put32(&mut file, 0x80, 0x4550);
    put16(&mut file, 0x84, 0x8664);
    put16(&mut file, 0x86, 1);
    put16(&mut file, 0x94, 240);
    let optional = 0x98;
    put16(&mut file, optional, 0x20b);
    put32(&mut file, optional + 16, 0x1000);
    put64(&mut file, optional + 24, 0x140000000);
    put32(&mut file, optional + 56, 0x2000);
    put32(&mut file, optional + 60, 0x200);
    put32(&mut file, optional + 112 + 5 * 8, 0x1100);
    put32(&mut file, optional + 116 + 5 * 8, 12);
    let section = optional + 240;
    put32(&mut file, section + 8, 0x200);
    put32(&mut file, section + 12, 0x1000);
    put32(&mut file, section + 16, 0x200);
    put32(&mut file, section + 20, 0x200);
    put32(&mut file, section + 36, 0x60000020);
    put64(&mut file, 0x220, 0x140001234);
    put32(&mut file, 0x300, 0x1000);
    put32(&mut file, 0x304, 12);
    put16(&mut file, 0x308, 0xa020);
    file
}

#[test]
fn maps_sections_and_applies_dir64_relocation() {
    let mut image = Image::parse(&fixture()).unwrap();
    assert_eq!(image.preferred_base, 0x140000000);
    assert_eq!(image.entry_rva, 0x1000);
    assert_eq!(image.sections[0].characteristics, 0x60000020);
    assert_eq!(&image.bytes[0x1020..0x1028], &0x140001234u64.to_le_bytes());
    image.relocate(0x150000000).unwrap();
    assert_eq!(&image.bytes[0x1020..0x1028], &0x150001234u64.to_le_bytes());
}

#[test]
fn rejects_invalid_headers_and_sections() {
    assert!(matches!(
        Image::parse(b"not a PE"),
        Err(ImageError::InvalidPe)
    ));
    let mut file = fixture();
    put16(&mut file, 0x84, 0x14c);
    assert!(matches!(Image::parse(&file), Err(ImageError::NotPe32Plus)));
    let mut file = fixture();
    put32(&mut file, 0x98 + 240 + 12, 0x1fff);
    assert!(matches!(
        Image::parse(&file),
        Err(ImageError::SectionOutsideImage)
    ));
}

#[test]
fn refuses_missing_or_invalid_relocations() {
    let mut file = fixture();
    put32(&mut file, 0x98 + 112 + 5 * 8, 0);
    let mut image = Image::parse(&file).unwrap();
    assert_eq!(
        image.relocate(0x150000000),
        Err(ImageError::MissingRelocations)
    );
    let mut file = fixture();
    put32(&mut file, 0x304, 0x100);
    let mut image = Image::parse(&file).unwrap();
    assert_eq!(
        image.relocate(0x150000000),
        Err(ImageError::InvalidRelocations)
    );
}

#[cfg(windows)]
#[test]
fn allocates_relocates_and_protects_a_synthetic_image() {
    use anyxboxx_native_launcher::memory::{Allocation, MemoryError};
    let mut image = Image::parse(&fixture()).unwrap();
    let allocation = Allocation::map(&mut image).unwrap();
    let expected = (allocation.base() as u64)
        .wrapping_add(0x1234)
        .to_le_bytes();
    // SAFETY: the fixture maps this offset inside the live allocation.
    let actual = unsafe { std::slice::from_raw_parts(allocation.base().add(0x1020), 8) };
    assert_eq!(actual, expected);
    image.bytes[0x1028] = 0x5a;
    allocation.sync(&image).unwrap();
    // SAFETY: the address is within the live allocation.
    assert_eq!(unsafe { *allocation.base().add(0x1028) }, 0x5a);
    allocation.protect(&image).unwrap();
    assert!(matches!(
        allocation.sync(&image),
        Err(MemoryError::AlreadyProtected)
    ));
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn calls_a_synthetic_image_entry_point() {
    use anyxboxx_native_launcher::memory::{Allocation, MemoryError};
    let mut file = fixture();
    // mov eax, 42; ret -- generated test code, independent of any game.
    file[0x200..0x206].copy_from_slice(&[0xb8, 42, 0, 0, 0, 0xc3]);
    let mut image = Image::parse(&file).unwrap();
    let allocation = Allocation::map(&mut image).unwrap();
    // SAFETY: this test supplies its own six-byte x86-64 function.
    assert!(matches!(
        unsafe { allocation.call_entry(&image) },
        Err(MemoryError::InvalidEntry)
    ));
    allocation.protect(&image).unwrap();
    // SAFETY: the synthetic function uses the Windows x64 ABI and needs no imports or TLS.
    assert_eq!(unsafe { allocation.call_entry(&image) }.unwrap(), 42);
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn assembled_loader_runs_a_synthetic_image() {
    use anyxboxx_native_launcher::{
        execution,
        libraries::{LOAD_LIBRARY_SEARCH_SYSTEM32, Module},
    };
    use std::path::Path;
    let mut file = fixture();
    file[0x200..0x206].copy_from_slice(&[0xb8, 42, 0, 0, 0, 0xc3]);
    let system = Module::load(Path::new("kernel32.dll"), LOAD_LIBRARY_SEARCH_SYSTEM32).unwrap();
    let missing = Path::new(r"Z:\AnyXboxXMissingFolder");
    // SAFETY: the image contains only the synthetic function above; it has no imports or TLS.
    let code = unsafe {
        execution::launch(
            &file,
            Path::new(r"C:\synthetic.exe"),
            missing,
            missing,
            [system; 6],
            false,
        )
    };
    assert_eq!(code.unwrap(), 42);
}

#[cfg(windows)]
#[test]
fn tls_registration_rejects_missing_carrier_and_invalid_directory() {
    use anyxboxx_native_launcher::{
        libraries::{LOAD_LIBRARY_SEARCH_SYSTEM32, Module},
        memory::Allocation,
        registration::{RegistrationError, register},
    };
    use std::path::Path;
    let module = Module::load(Path::new("kernel32.dll"), LOAD_LIBRARY_SEARCH_SYSTEM32).unwrap();
    let mut file = fixture();
    put32(&mut file, 0x98 + 112 + 9 * 8, 0x1100);
    let mut image = Image::parse(&file).unwrap();
    let allocation = Allocation::map(&mut image).unwrap();
    // SAFETY: this fixture has no exception table and kernel32 has no TLS carrier export,
    // so no mapped code is executed or registered.
    assert_eq!(
        unsafe { register(&image, &allocation, module) },
        Err(RegistrationError::TlsInstallation)
    );
    put32(&mut file, 0x98 + 112 + 9 * 8, 0x1fff);
    let mut image = Image::parse(&file).unwrap();
    let allocation = Allocation::map(&mut image).unwrap();
    // SAFETY: the invalid directory is rejected before any mapped code can run.
    assert_eq!(
        unsafe { register(&image, &allocation, module) },
        Err(RegistrationError::InvalidTlsDirectory)
    );
}

#[test]
fn decodes_named_and_ordinal_imports_and_rejects_truncated_tables() {
    use anyxboxx_native_launcher::imports::{BindError, Import, Symbol, bind, imports};
    let mut file = fixture();
    put32(&mut file, 0x98 + 112 + 8, 0x1140);
    put32(&mut file, 0x340, 0x1180);
    put32(&mut file, 0x34c, 0x11c0);
    put32(&mut file, 0x350, 0x11a0);
    put64(&mut file, 0x380, 0x11d0);
    put64(&mut file, 0x388, 0x8000_0000_0000_0007);
    file[0x3c0..0x3c9].copy_from_slice(b"test.dll\0");
    file[0x3d2..0x3d7].copy_from_slice(b"Func\0");
    let image = Image::parse(&file).unwrap();
    assert_eq!(
        imports(&image).unwrap(),
        vec![
            Import {
                dll: b"test.dll".to_vec(),
                symbol: Symbol::Name(b"Func".to_vec()),
                slot: 0x11a0
            },
            Import {
                dll: b"test.dll".to_vec(),
                symbol: Symbol::Ordinal(7),
                slot: 0x11a8
            },
        ]
    );
    let mut bound = Image::parse(&file).unwrap();
    let missing = bind(&mut bound, |import| match import.symbol {
        Symbol::Name(_) => Some(0x1234),
        Symbol::Ordinal(_) => None,
    })
    .unwrap_err();
    assert_eq!(&bound.bytes[0x11a0..0x11a8], &0x1234u64.to_le_bytes());
    assert!(
        matches!(missing, BindError::Missing(entries) if entries[0].symbol == Symbol::Ordinal(7))
    );
    put64(&mut file, 0x380, 0x4000);
    assert!(imports(&Image::parse(&file).unwrap()).is_err());
}

#[cfg(windows)]
#[test]
fn resolves_system_import() {
    use anyxboxx_native_launcher::{
        libraries::{LOAD_LIBRARY_SEARCH_SYSTEM32, Module},
        resolver::bind,
    };
    use std::path::Path;
    let mut file = fixture();
    put32(&mut file, 0x98 + 112 + 8, 0x1140);
    put32(&mut file, 0x340, 0x1180);
    put32(&mut file, 0x34c, 0x11c0);
    put32(&mut file, 0x350, 0x11a0);
    put64(&mut file, 0x380, 0x11d0);
    file[0x3c0..0x3cd].copy_from_slice(b"kernel32.dll\0");
    file[0x3d2..0x3e6].copy_from_slice(b"GetCurrentProcessId\0");
    let system = Module::load(Path::new("kernel32.dll"), LOAD_LIBRARY_SEARCH_SYSTEM32).unwrap();
    let mut image = Image::parse(&file).unwrap();
    let absent = Path::new(r"Z:\AnyXboxXMissingFolder");
    bind(&mut image, &[system; 6], absent, absent, false).unwrap();
    assert_ne!(&image.bytes[0x11a0..0x11a8], &[0; 8]);
}

#[cfg(windows)]
#[test]
fn patches_named_import_in_a_synthetic_mapped_dll() {
    use anyxboxx_native_launcher::patch::patch_imports;
    let mut file = fixture();
    put32(&mut file, 0x98 + 112 + 8, 0x1140);
    put32(&mut file, 0x340, 0x1180);
    put32(&mut file, 0x34c, 0x11c0);
    put32(&mut file, 0x350, 0x11a0);
    put64(&mut file, 0x380, 0x11d0);
    put64(&mut file, 0x388, 0x8000_0000_0000_0007);
    file[0x3c0..0x3c9].copy_from_slice(b"test.dll\0");
    file[0x3d2..0x3d7].copy_from_slice(b"Func\0");
    let mut image = Image::parse(&file).unwrap();
    // SAFETY: this vector is a live writable synthetic image and no other thread accesses it.
    unsafe {
        patch_imports(
            image.bytes.as_mut_ptr(),
            image.bytes.len(),
            |dll, symbol| {
                assert_eq!((dll, symbol), (&b"test.dll"[..], &b"Func"[..]));
                Some(0x1234)
            },
        )
    }
    .unwrap();
    assert_eq!(&image.bytes[0x11a0..0x11a8], &0x1234u64.to_le_bytes());
    assert_eq!(&image.bytes[0x11a8..0x11b0], &[0; 8]);
}
