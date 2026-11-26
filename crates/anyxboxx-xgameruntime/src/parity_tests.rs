//! Check implemented C++ interface slots against the Rust method tables.
use std::collections::BTreeSet;
use std::ffi::c_void;

fn check(source: &str, array: &str, methods: Vec<(usize, *const c_void)>, refusals: &[usize]) {
    let table = source
        .split_once(&format!("{array}[] = {{"))
        .expect("C++ interface table")
        .1
        .split_once("};")
        .expect("end of C++ interface table")
        .0;
    let slots: BTreeSet<_> = methods.into_iter().map(|(slot, _)| slot).collect();
    let mut checked = 0;
    for line in table
        .lines()
        .filter(|line| line.contains("ANYXBOXX_METHOD("))
    {
        let (_, comment) = line.split_once("//").expect("C++ slot comment");
        let slot = comment
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .expect("numeric C++ slot");
        checked += 1;
        assert!(
            slots.contains(&slot) || refusals.contains(&slot),
            "{array} slot {slot} is implemented in C++ but absent in Rust: {line}"
        );
    }
    assert!(checked > 0, "{array} has no checked C++ methods");
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
