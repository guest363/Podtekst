fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // mr-audio тянет Swift-мост ScreenCaptureKit, его рантайм лежит в /usr/lib/swift.
    // Link-arg из зависимости не переходит в тестовый бинарник этого крейта.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
