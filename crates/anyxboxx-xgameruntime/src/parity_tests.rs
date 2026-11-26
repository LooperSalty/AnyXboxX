//! Check implemented C++ interface slots against the Rust method tables.
use std::collections::BTreeSet;
use std::ffi::c_void;

fn cpp_slots(source: &str, array: &str) -> Vec<usize> {
    let table = source
        .split_once(&format!("{array}[] = {{"))
        .expect("C++ interface table")
        .1
        .split_once("};")
        .expect("end of C++ interface table")
        .0;
    let slots: Vec<_> = table
        .lines()
        .filter(|line| line.contains("ANYXBOXX_METHOD("))
        .map(|line| {
            let (_, comment) = line.split_once("//").expect("C++ slot comment");
            comment
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .expect("numeric C++ slot")
        })
        .collect();
    assert!(!slots.is_empty(), "{array} has no checked C++ methods");
    slots
}

fn check(source: &str, array: &str, methods: Vec<(usize, *const c_void)>, refusals: &[usize]) {
    let slots: BTreeSet<_> = methods.into_iter().map(|(slot, _)| slot).collect();
    for slot in cpp_slots(source, array) {
        assert!(
            slots.contains(&slot) || refusals.contains(&slot),
            "{array} slot {slot} is implemented in C++ but absent in Rust"
        );
    }
}

#[test]
fn cpp_task_queue_methods_do_not_point_to_rust_missing_stubs() {
    let source =
        include_str!("../../../third_party/victory/crates/victory-hle/src/modules/xasync.rs")
            .replace("\r\n", "\n");
    let table = source
        .split_once("static VTABLE: Methods<31> = Methods([")
        .unwrap()
        .1
        .split_once("]);")
        .unwrap()
        .0;
    let methods: Vec<_> = table
        .lines()
        .map(|line| line.split("//").next().unwrap().trim())
        .filter(|line| !line.is_empty())
        .flat_map(|line| {
            line.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
        })
        .collect();
    assert_eq!(methods.len(), 31);
    for slot in cpp_slots(
        include_str!("../../../runtime/xgameruntime/task_queue_interface.cpp"),
        "kMethods",
    ) {
        let method = methods
            .get(slot)
            .expect("C++ slot must fit the Rust vtable");
        assert!(
            !method.contains("missing::<"),
            "task queue slot {slot} is unimplemented"
        );
        if *method == "REGISTER_WAITER" || *method == "UNREGISTER_WAITER" {
            let declaration = format!("#[cfg(windows)]\nconst {method}: *const () = ");
            let target = source
                .split_once(&declaration)
                .expect("Windows waiter declaration")
                .1
                .split_once(';')
                .unwrap()
                .0;
            assert!(
                !target.contains("missing::<"),
                "task queue slot {slot} is unimplemented"
            );
        }
    }
}

#[test]
fn cpp_implemented_interface_slots_have_rust_methods_or_explicit_refusals() {
    check(
        include_str!("../../../runtime/xgameruntime/package_interface.cpp"),
        "kMethods",
        crate::package::methods(),
        &[19, 32],
    );
    check(
        include_str!("../../../runtime/xgameruntime/user_interface.cpp"),
        "kUserMethods",
        crate::user::methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/user_interface.cpp"),
        "kGamertagMethods",
        crate::user::gamertag_methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/save_interface.cpp"),
        "kMethods",
        crate::save_interface::methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/store_interface.cpp"),
        "kMethods",
        crate::store::methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/local_storage_interface.cpp"),
        "kMethods",
        crate::local_storage::methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/platform_interface.cpp"),
        "kNetworkingMethods",
        crate::networking::methods(),
        &[],
    );
    check(
        include_str!("../../../runtime/xgameruntime/platform_interface.cpp"),
        "kHdrMethods",
        crate::hdr_display::methods(),
        &[],
    );
    for (array, object) in [
        ("kInviteMethods", "InviteInterface"),
        ("kPendingInviteMethods", "PendingInviteInterface"),
        ("kActivationMethods", "ActivationInterface"),
        ("kProtocolMethods", "ProtocolInterface"),
        ("kErrorMethods", "ErrorInterface"),
    ] {
        check(
            include_str!("../../../runtime/xgameruntime/event_interface.cpp"),
            array,
            crate::events::methods(object),
            &[],
        );
    }
    for (array, object) in [
        ("kSystemMethods", "SystemInterface"),
        ("kGameMethods", "GameInterface"),
        ("kDisplayMethods", "DisplayInterface"),
    ] {
        check(
            include_str!("../../../runtime/xgameruntime/system_interface.cpp"),
            array,
            crate::system_interface::methods(object),
            &[],
        );
    }
    // These C++ tables are written on one line and have a single method at slot 3.
    for object in ["AnalyticsInterface", "FeatureInterface"] {
        assert!(
            crate::system_interface::methods(object)
                .iter()
                .any(|(slot, _)| *slot == 3),
            "{object} slot 3"
        );
    }
}
