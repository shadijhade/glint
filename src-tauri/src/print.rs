//! Export as PDF for Markdown: a hidden window of the app (print.html) draws the file exactly as the
//! preview does, math and Mermaid included, and says when it's ready; then WebView2's own PrintToPdf writes
//! it, light, without the browser's header and footer. Through actions.rs, so the PDF lands beside the
//! Markdown and Undo takes it back. Nothing leaves the PC.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder, Window};

static STOP: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
/// The print windows waiting to be ready, by their id.
static WAITING: LazyLock<Mutex<HashMap<u64, Sender<()>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The print page says it has drawn the file.
#[tauri::command]
pub fn print_ready(id: u64) {
    if let Some(ready) = WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        let _ = ready.send(());
    }
}

#[tauri::command]
pub async fn markdown_pdf(app: AppHandle, window: Window, files: Vec<String>, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        actions::run(owner, "pdf-export", &jobs, &output, 1, &STOP, &|p| drop(on_progress.send(p)), |job, part, work| print_one(&app, &job.inputs[0], part, work).map(|f| vec![f]))
    })
    .await
}

/// A path as a URL's query value.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn print_one(app: &AppHandle, file: &Path, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (ready_tx, ready_rx) = channel();
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).insert(id, ready_tx);
    let label = format!("glint-print-{id}");
    let url = format!("print.html?id={id}&path={}", query_value(&file.to_string_lossy()));
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .visible(false)
        .skip_taskbar(true)
        .inner_size(900.0, 1200.0)
        .build()
        .map_err(|e| e.to_string())?;
    part(0.2);
    let ready = ready_rx.recv_timeout(Duration::from_secs(40));
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    if ready.is_err() {
        let _ = window.destroy();
        return Err("The page didn't finish drawing it".into());
    }
    part(0.6);
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let out = work.join(format!("{stem}.pdf"));
    let printed = print_to_pdf(&window, &out);
    let _ = window.destroy();
    printed?;
    if out.is_file() {
        Ok(out)
    } else {
        Err("WebView2 made no PDF".into())
    }
}

/// WebView2's PrintToPdf of `window`'s page into `out`: no header or footer, backgrounds kept.
#[cfg(windows)]
fn print_to_pdf(window: &tauri::WebviewWindow, out: &Path) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment6, ICoreWebView2_7};
    use webview2_com::PrintToPdfCompletedHandler;
    use windows::core::{Interface, HSTRING};
    let (done_tx, done_rx) = channel::<Result<(), String>>();
    let target = HSTRING::from(out.as_os_str());
    window
        .with_webview(move |webview| unsafe {
            let started = (|| -> windows::core::Result<()> {
                let core = webview.controller().CoreWebView2()?.cast::<ICoreWebView2_7>()?;
                let settings = webview.environment().cast::<ICoreWebView2Environment6>()?.CreatePrintSettings()?;
                settings.SetShouldPrintHeaderAndFooter(false)?;
                settings.SetShouldPrintBackgrounds(true)?;
                let tx = done_tx.clone();
                let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, ok| {
                    let _ = tx.send(match result {
                        Ok(()) if ok => Ok(()),
                        Ok(()) => Err("WebView2 couldn't print it".into()),
                        Err(e) => Err(e.to_string()),
                    });
                    Ok(())
                }));
                core.PrintToPdf(&target, &settings, &handler)
            })();
            if let Err(e) = started {
                let _ = done_tx.send(Err(e.to_string()));
            }
        })
        .map_err(|e| e.to_string())?;
    done_rx.recv_timeout(Duration::from_secs(120)).map_err(|_| "Printing took too long".to_string())?
}

#[cfg(not(windows))]
fn print_to_pdf(_window: &tauri::WebviewWindow, _out: &Path) -> Result<(), String> {
    Err("Export as PDF needs Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puts_paths_in_a_query() {
        assert_eq!(query_value(r"C:\Docs\read me.md"), "C%3A%5CDocs%5Cread%20me.md");
        assert_eq!(query_value("ü.md"), "%C3%BC.md");
    }
}
