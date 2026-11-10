//! Glint.exe's icon and version info (resources/glint.rc). The manifest comes from the vendored GPUI.
//!
//! The version numbers come from the workspace version (CARGO_PKG_VERSION), written to glint-version.h in
//! OUT_DIR for the resource compiler. Only Windows builds get resources; other targets skip this.
//!
//! It also puts rg.exe, fd.exe and pdu.exe next to the built Glint.exe, as tauri-build did for every dev build
//! and the installer does: glint-core runs the copy beside the app first (proc::locate), so a dev build never
//! falls back to whatever the PATH holds.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/glint.rc");
    println!("cargo:rerun-if-changed=resources/icon.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string()));
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));
    let resources = manifest_dir.join("resources");

    if let Err(err) = write_version_header(&out_dir, &version) {
        panic!("failed to write glint-version.h: {err}");
    }

    // The .rc names the icon and the manifest by relative path, so it compiles from the resources folder.
    bundle_tools(&manifest_dir, &out_dir);

    let rc = resources.join("glint.rc");
    embed_resource::compile(
        &rc,
        embed_resource::ParamsIncludeDirs([out_dir.as_path(), resources.as_path()]),
    )
    .manifest_optional()
    .expect("failed to compile resources/glint.rc");
}

/// Writes the FILEVERSION numbers and the version text the .rc file shows: "0.10.0" becomes 0,10,0,0.
fn write_version_header(out_dir: &Path, version: &str) -> std::io::Result<()> {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    let header =
        format!("#define GLINT_FILE_VERSION {major},{minor},{patch},0\n#define GLINT_VERSION_TEXT \"{version}\"\n");
    fs::write(out_dir.join("glint-version.h"), header)
}

/// Copies the bundled search tools (the official, unsigned rg, fd and pdu in crates/glint/binaries) next to the exe
/// being built (OUT_DIR is <target>/<profile>/build/<pkg>/out).
fn bundle_tools(manifest_dir: &Path, out_dir: &Path) {
    let binaries = manifest_dir.join("binaries");
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    for tool in ["rg", "fd", "pdu"] {
        let source = binaries.join(format!("{tool}-x86_64-pc-windows-msvc.exe"));
        println!("cargo:rerun-if-changed={}", source.display());
        let target = profile_dir.join(format!("{tool}.exe"));
        let fresh = match (fs::metadata(&source), fs::metadata(&target)) {
            (Ok(s), Ok(t)) => s.len() == t.len() && s.modified().ok() <= t.modified().ok(),
            (Ok(_), Err(_)) => false,
            (Err(_), _) => true,
        };
        if !fresh {
            if let Err(err) = fs::copy(&source, &target) {
                println!("cargo:warning=couldn't put {tool}.exe next to Glint.exe: {err}");
            }
        }
    }
}
