fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Keep the usual executable image base free for the manually mapped image.
        println!("cargo:rustc-link-arg-bin=anyxboxx-native-launcher=/BASE:0x7FF700000000");
    }
}
