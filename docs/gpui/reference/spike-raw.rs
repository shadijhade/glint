//! Spike: how fast a Glint-like GPUI window shows on this PC, what it costs in memory, whether Arabic
//! and Hebrew file names render in the right order, and how scrolling a 100,000-row list paces.
//! Prints marks in ms since process creation.
//!
//! Env: SPIKE_QUIT_MS=n quits n ms after start and prints memory; SPIKE_SCROLL=px scrolls the list by
//! px per frame for 4 s and prints frame pacing and CPU per frame; SPIKE_MAX=1 opens maximized.

use gpui::{
    div, point, prelude::*, px, rgb, size, uniform_list, App, Bounds, Context, SharedString,
    UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn times() -> (FILETIME, FILETIME, FILETIME) {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).unwrap();
        (c, k, u)
    }
}

/// Milliseconds since this process was created.
fn since_start() -> f64 {
    let (c, _, _) = times();
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    (ft(now) - ft(c)) as f64 / 10_000.0
}

/// CPU time used by the whole process (all threads), in ms.
fn cpu_ms() -> f64 {
    let (_, k, u) = times();
    (ft(k) + ft(u)) as f64 / 10_000.0
}

fn memory() -> (f64, f64) {
    unsafe {
        let mut m = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, size);
        (m.WorkingSetSize as f64 / 1048576.0, m.PrivateUsage as f64 / 1048576.0)
    }
}

const NAMES: [&str; 8] = [
    "Quarterly report {}.xlsx",
    "تقرير الربع {}.pdf",
    "IMG_{}.jpg",
    "مجلد الصور {}",
    "קובץ {}.txt",
    "notes-{}.md",
    "صلاة الفجر - {}.mp3",
    "Cargo {}.toml",
];

struct ScrollTest {
    px_per_frame: f32,
    y: f32,
    start: Option<(Instant, f64)>,
    frames: Vec<f64>,
    done: bool,
}

struct Files {
    rows: usize,
    marked: bool,
    scroll: UniformListScrollHandle,
    test: Option<ScrollTest>,
    marked_caret: bool,
}

impl Files {
    fn step_scroll(&mut self, window: &mut Window) {
        let Some(t) = self.test.as_mut() else { return };
        if t.done {
            return;
        }
        let now = Instant::now();
        let (start, cpu_start) = *t.start.get_or_insert((now, cpu_ms()));
        let elapsed = now.duration_since(start).as_secs_f64() * 1000.0;
        t.frames.push(elapsed);
        if elapsed < 4000.0 {
            t.y += t.px_per_frame;
            self.scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-t.y)));
            window.request_animation_frame();
            return;
        }
        t.done = true;
        let cpu = cpu_ms() - cpu_start;
        let mut gaps: Vec<f64> = t.frames.windows(2).map(|w| w[1] - w[0]).collect();
        let n = gaps.len().max(1) as f64;
        let mean = gaps.iter().sum::<f64>() / n;
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| gaps.get(((gaps.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
        let median = pct(0.5);
        let long = gaps.iter().filter(|g| **g > median * 1.5).count();
        println!(
            "scroll {} px/frame: {} frames in {:.0} ms ({:.1} fps), interval median {:.2} ms, p95 {:.2}, p99 {:.2}, max {:.2}, {} frames over 1.5x median, CPU {:.2} ms per frame, rows passed {}",
            t.px_per_frame,
            t.frames.len(),
            elapsed,
            t.frames.len() as f64 * 1000.0 / elapsed,
            median,
            pct(0.95),
            pct(0.99),
            gaps.last().copied().unwrap_or(0.0),
            long,
            cpu / t.frames.len() as f64,
            (t.y / 24.0) as usize,
        );
        let _ = mean;
    }
}

impl Render for Files {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.marked {
            self.marked = true;
            println!("first render: {:.1} ms", since_start());
            window.on_next_frame(|window, _| {
                let (ws, private) = memory();
                println!("first frame drawn: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
                window.on_next_frame(|_, _| println!("second frame: {:.1} ms", since_start()));
            });
        }
        if std::env::var("SPIKE_CARET").is_ok() && !self.marked_caret {
            self.marked_caret = true;
            // Caret positions GPUI computes for an Arabic name (with the RTL display fix in place).
            let text: SharedString = "تقرير الربع 1.pdf".into();
            let run = window.text_style().to_run(text.len());
            let line = window.text_system().shape_line(text.clone(), px(20.), &[run], None);
            let glyphs: Vec<(usize, i32)> = line.runs.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.index, g.position.x.as_f32().round() as i32))).collect();
            println!("glyphs (byte index, x) in layout order: {glyphs:?}");
            let carets: Vec<String> = text.char_indices().map(|(i, c)| format!("{c}@{}", line.x_for_index(i).as_f32().round())).collect();
            println!("x_for_index per character: {}", carets.join(" "));
            let width = line.width.as_f32();
            let hits: Vec<String> = (0..=10).map(|k| { let x = width * k as f32 / 10.; format!("{:.0}->{:?}", x, line.index_for_x(px(x))) }).collect();
            println!("index_for_x across the line (width {width:.0}): {}", hits.join(" "));
            let near: Vec<String> = (0..=20).map(|k| { let x = width * k as f32 / 20.; format!("{:.0}->{}@{:.0}", x, line.closest_index_for_x(px(x)), line.x_for_index(line.closest_index_for_x(px(x))).as_f32()) }).collect();
            println!("closest_index_for_x (click -> index @ caret x): {}", near.join(" "));
            let sel: Vec<String> = line.x_ranges_for_index_range(0..10).iter().map(|r| format!("{:.0}..{:.0}", r.start.as_f32(), r.end.as_f32())).collect();
            println!("first word (bytes 0..10) shows at: {}", sel.join(", "));
        }
        self.step_scroll(window);
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family("Segoe UI")
            .text_size(px(if std::env::var("SPIKE_BIG").is_ok() { 34. } else { 13. }))
            .text_color(rgb(0xe8e8e8))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgb(0x202020))
                    .child("C:\\Users\\shado\\المستندات\\Projects")
                    .child(div().text_color(rgb(0x9a9a9a)).child(format!("{} items", self.rows))),
            )
            .child(
                uniform_list(
                    "files",
                    self.rows,
                    cx.processor(|_this, range: std::ops::Range<usize>, _window, _cx| {
                        range
                            .map(|ix| {
                                let name: SharedString = NAMES[ix % NAMES.len()].replace("{}", &ix.to_string()).into();
                                div()
                                    .id(ix)
                                    .flex()
                                    .h(px(if std::env::var("SPIKE_BIG").is_ok() { 52. } else { 24. }))
                                    .px_3()
                                    .items_center()
                                    .hover(|s| s.bg(rgb(0x2d2d2d)))
                                    .child(div().w(px(420.)).child(name))
                                    .child(div().w(px(110.)).text_color(rgb(0x9a9a9a)).child(format!("{} KB", (ix * 37) % 9000)))
                                    .child(div().text_color(rgb(0x9a9a9a)).child("2026-11-09 21:14"))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_grow(1.),
            )
    }
}

/// Loads the GPU driver on a thread while the rest of startup runs: creating the first D3D11 device in a
/// process is what costs (~150 ms on this PC); GPUI's own device, made later, then finds the driver loaded.
fn prewarm_gpu() {
    std::thread::spawn(|| unsafe {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::*;
        use windows::Win32::Graphics::Direct3D11::*;
        let t = Instant::now();
        let mut dev: Option<ID3D11Device> = None;
        let r = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1]),
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        );
        println!("prewarm device: {:.1} ms on its thread (ok: {})", t.elapsed().as_secs_f64() * 1000.0, r.is_ok());
        std::mem::forget(dev);
    });
}

fn main() {
    let (ws, private) = memory();
    println!("main: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
    if std::env::var("SPIKE_OLETEST").is_ok() {
        let t = Instant::now();
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) };
        let (ws, private) = memory();
        println!("after CoInitializeEx: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", t.elapsed().as_secs_f64() * 1000.0);
        let _ = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
        let (ws, private) = memory();
        println!("after OleInitialize: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", t.elapsed().as_secs_f64() * 1000.0);
    }
    if std::env::var("SPIKE_PREWARM").is_ok() {
        prewarm_gpu();
    }
    let quit_after = std::env::var("SPIKE_QUIT_MS").ok().and_then(|v| v.parse::<u64>().ok());
    let scroll = std::env::var("SPIKE_SCROLL").ok().and_then(|v| v.parse::<f32>().ok());
    let maximized = std::env::var("SPIKE_MAX").is_ok();
    let app = application();
    let (ws, private) = memory();
    println!("platform created: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
    app.run(move |cx: &mut App| {
        let (ws, private) = memory();
        println!("app ready: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(if maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) }),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                titlebar: Some(gpui::TitlebarOptions { title: Some("GPUI spike".into()), ..Default::default() }),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Files {
                    rows: 100_000,
                    marked: false,
                    scroll: UniformListScrollHandle::new(),
                    test: scroll.map(|px_per_frame| ScrollTest { px_per_frame, y: 0., start: None, frames: Vec::new(), done: false }),
                    marked_caret: false,
                })
            },
        )
        .unwrap();
        println!("window opened: {:.1} ms", since_start());
        cx.activate(true);
        if let Some(ms) = quit_after {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(ms)).await;
                let (ws, private) = memory();
                println!("memory after {ms} ms: working set {ws:.1} MB, private {private:.1} MB");
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}
