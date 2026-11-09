//! Glint.exe's icon and version info (resources/glint.rc). The manifest comes from the vendored GPUI.
//!
//! The version numbers come from the workspace version (CARGO_PKG_VERSION), written to glint-version.h in
//! OUT_DIR for the resource compiler. Only Windows builds get resources; other targets skip this.

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
