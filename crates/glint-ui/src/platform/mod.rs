//! Windows platform services the web app's Tauri shell provided, rebuilt for the GPUI app. Each module
//! exposes an `init`-style entry the coordinator wires into `run()`.

#[cfg(feature = "dragdrop")]
pub mod dragdrop;
#[cfg(feature = "launch")]
pub mod launch;
#[cfg(feature = "preview-host")]
pub mod preview_host;
#[cfg(feature = "tray")]
pub mod tray;
#[cfg(feature = "window-state")]
pub mod window_state;
