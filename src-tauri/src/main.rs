// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Glint started by itself to read a JSON, YAML or TOML file for its tree: it answers and exits
    // before anything of the app runs.
    if let Some(code) = shadui_ripgrep_lib::worker() {
        std::process::exit(code);
    }
    shadui_ripgrep_lib::run()
}
