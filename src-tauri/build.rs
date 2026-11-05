fn main() {
    tauri_build::build();

    // ONNX Runtime's static library (pulled in by fastembed) calls a clang runtime helper,
    // `__isPlatformVersionAtLeast`, that the release link (LTO) doesn't otherwise provide.
    // Debug builds link without this.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && let Ok(out) = std::process::Command::new("xcrun").args(["clang", "--print-resource-dir"]).output()
        && out.status.success()
    {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("cargo:rustc-link-arg-bins=-L{dir}/lib/darwin");
        println!("cargo:rustc-link-arg-bins=-lclang_rt.osx");
    }
}
