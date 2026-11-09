// Tauri's build script set `desktop` (and `mobile` on phones); the moved modules use both cfgs.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(desktop)");
    println!("cargo::rustc-check-cfg=cfg(mobile)");
    println!("cargo::rustc-cfg=desktop");
}
