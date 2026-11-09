// The self-contained GNU toolchain ships no import library for shlwapi (needed by the
// web-link helper). GNU ld can link straight against a DLL, so we copy the system
// shlwapi.dll into the build folder and add that folder to the search path.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" || target_env != "gnu" {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    for dll in ["shlwapi.dll"] {
        let src = std::path::Path::new(&windir).join("System32").join(dll);
        let dst = out.join(dll);
        if !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
    println!("cargo:rustc-link-search=native={}", out.display());
}
