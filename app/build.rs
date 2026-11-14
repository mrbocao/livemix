// The self-contained GNU toolchain ships no import library for shlwapi (needed by the
// web-link helper). GNU ld can link straight against a DLL, so we copy the system
// shlwapi.dll into the build folder and add that folder to the search path.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        embed_icon(&target_env);
    }
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

fn embed_icon(target_env: &str) {
    use std::{env, fs, path::PathBuf, process::Command};
    println!("cargo:rerun-if-changed=assets/livemix.ico");
    println!("cargo:rerun-if-env-changed=RC");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = out.join("livemix.rc");
    let icon = manifest.join("assets/livemix.ico");
    fs::write(&source, format!("1 ICON \"{}\"\n", icon.display().to_string().replace('\\', "/")))
        .expect("write LiveMix icon resource");
    let gnu = target_env == "gnu";
    let object = out.join(if gnu { "livemix-icon.o" } else { "livemix-icon.res" });
    let compiler = env::var_os("RC").map(PathBuf::from).unwrap_or_else(|| {
        if gnu { return PathBuf::from("windres.exe"); }
        let sdk = env::var_os("ProgramFiles(x86)").map(PathBuf::from);
        sdk.and_then(|root| {
            let arch = if env::var("HOST").unwrap_or_default().starts_with("aarch64") { "arm64" } else { "x64" };
            fs::read_dir(root.join("Windows Kits/10/bin")).ok()?.flatten()
                .map(|entry| entry.path().join(arch).join("rc.exe"))
                .filter(|path| path.is_file()).max()
        }).unwrap_or_else(|| PathBuf::from("rc.exe"))
    });
    let mut command = Command::new(compiler);
    let status = if gnu {
        command.arg("-i").arg(source).arg("-o").arg(&object).status()
    } else {
        command.arg("/nologo").arg("/fo").arg(&object).arg(source).status()
    };
    if status.is_ok_and(|status| status.success()) {
        println!("cargo:rustc-link-arg-bins={}", object.display());
    } else {
        println!("cargo:warning=Could not embed the Windows executable icon. Set RC to the resource compiler path.");
    }
}
