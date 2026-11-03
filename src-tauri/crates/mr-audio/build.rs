fn main() {
    // Swift-мост ScreenCaptureKit лежит в /usr/lib/swift, а не в dyld-кэше:
    // без rpath тестовый бинарник падает с «Library not loaded».
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
