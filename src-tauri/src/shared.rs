//! Big answers handed to the page through WebView2's shared memory rather than the IPC channel: a
//! folder of tens of thousands of items is copied once into a buffer the page reads as an ArrayBuffer,
//! with nothing serialized or copied again on the way (src/lib/fs.ts listens for
//! `sharedbufferreceived`). Small answers, and any a WebView2 too old to share memory can't take, go the
//! usual way.

use tauri::WebviewWindow;

/// Answers at least this big go through shared memory: below it the IPC channel is as quick.
pub const MIN_SHARED: usize = 256 << 10;

/// Hands `bytes` to the page in `window` as a read-only shared buffer tagged `id` (in its additional
/// data, `{"glintId": id}`). False when WebView2 couldn't, for the caller to answer the usual way.
/// Called off the main thread: it waits for the main thread, which owns the WebView2.
#[cfg(windows)]
pub fn post(window: &WebviewWindow, id: u64, bytes: Vec<u8>) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment12, ICoreWebView2_17, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY};
    use windows::core::{Interface, PCWSTR};
    let (sent, done) = std::sync::mpsc::channel();
    let queued = window.with_webview(move |webview| unsafe {
        let posted = (|| -> windows::core::Result<()> {
            let environment = webview.environment().cast::<ICoreWebView2Environment12>()?;
            let core = webview.controller().CoreWebView2()?.cast::<ICoreWebView2_17>()?;
            let buffer = environment.CreateSharedBuffer(bytes.len().max(1) as u64)?;
            let mut at = std::ptr::null_mut();
            buffer.Buffer(&mut at)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len());
            let data: Vec<u16> = format!("{{\"glintId\":{id}}}").encode_utf16().chain([0]).collect();
            let result = core.PostSharedBufferToScript(&buffer, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY, PCWSTR(data.as_ptr()));
            // The page holds its own view of the memory; this side lets go of it now.
            let _ = buffer.Close();
            result
        })();
        let _ = sent.send(posted.is_ok());
    });
    queued.is_ok() && done.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn post(_window: &WebviewWindow, _id: u64, _bytes: Vec<u8>) -> bool {
    false
}
