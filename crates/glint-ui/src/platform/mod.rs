//! Windows platform services the web app's Tauri shell provided, rebuilt for the GPUI app. Each module
//! exposes calls and events; `crate::startup` wires them into the app.

pub mod dragdrop;
pub mod launch;
pub mod preview_host;
pub mod tray;
pub mod window_state;
