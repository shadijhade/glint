//! Windows platform services the web app's Tauri shell provided, rebuilt for the GPUI app. Each module
//! exposes calls and events; `crate::startup` wires them into the app.

pub mod dragdrop;
pub mod launch;
/// Windows' open and save pickers, with file-type filters, for every view.
pub mod pickers;
pub mod preview_host;
/// Second windows: Glint.exe in secondary mode, started with `--window <folder>`.
pub mod secondary;
/// Read aloud: Windows' speech synthesis behind a small `Speaker` trait.
pub mod speech;
pub mod tray;
pub mod window_state;
