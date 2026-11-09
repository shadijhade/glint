// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Microsoft's mimalloc (v3) for every allocation: faster for the many small strings listing, sorting
/// and searching make (`npm run bench`).
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // Glint started by itself to read a JSON, YAML or TOML file for its tree: it answers and exits
    // before anything of the app runs.
    if let Some(code) = shadui_ripgrep_lib::worker() {
        std::process::exit(code);
    }
    shadui_ripgrep_lib::run()
}
