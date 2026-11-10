//! Glint.exe: a worker when started as one, the app otherwise.
//!
//! The app is the main window, or, started as `Glint.exe --window <folder> [--at <x>,<y>] [--size <w>x<h>]
//! [--main <pid>]`, a second window of its own in secondary mode (Open in new window, a tab dragged out): the
//! same program, keeping nothing of its own. `glint_ui::run` reads the command line first
//! (`platform::launch::begin`, `platform::secondary`); a worker's command line never starts with `--window`.

// No console window in release builds on Windows (the debug build keeps one for its logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Microsoft's mimalloc for every allocation, as the web app's Glint.exe had: faster for the many small
/// strings that listing and sorting make and free (measured with `Glint.exe bench`).
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // A worker answers and exits before anything of the app starts: no window, no GPUI.
    if let Some(code) = glint_core::worker() {
        std::process::exit(code);
    }
    let code = glint_ui::run();
    if code != 0 {
        std::process::exit(code);
    }
}
