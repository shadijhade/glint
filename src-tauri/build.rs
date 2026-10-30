fn main() {
    // Windows' Common Controls 6, which Tauri's dialogs need, is what Tauri's own manifest asks for. The
    // linker puts it in instead, so test binaries have it too: one whose code reaches a dialog then still
    // loads (it failed with STATUS_ENTRYPOINT_NOT_FOUND for TaskDialogIndirect without it).
    let msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut attributes = tauri_build::Attributes::new();
    if msvc {
        attributes = attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
        println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
    }
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
