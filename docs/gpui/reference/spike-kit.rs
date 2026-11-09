//! Phase 0 on gpui-kit 0.9.1: gpui-component's custom title bar, a 100,000-row DataTable, and an input
//! holding an Arabic name. Prints marks in ms since process creation, like the raw spike.
//!
//! Env: SPIKE_QUIT_MS=n quits after n ms and prints memory; SPIKE_HWND=1 prints the window handle and
//! rectangle (for hit-testing the title bar from outside).

#[path = "../phase0.rs"]
mod phase0;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{h_flex, v_flex, TitleBar};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn since_start() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).unwrap();
        (ft(GetSystemTimePreciseAsFileTime()) - ft(c)) as f64 / 10_000.0
    }
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

fn cell(row: usize, col: usize) -> SharedString {
    match col {
        0 => NAMES[row % NAMES.len()].replace("{}", &row.to_string()).into(),
        1 => format!("{} KB", (row * 37) % 9000).into(),
        _ => "2026-11-09 21:14".into(),
    }
}

struct Files {
    columns: Vec<Column>,
}

thread_local! {
    static DRAG_FROM: std::cell::Cell<Option<(usize, Point<Pixels>)>> = const { std::cell::Cell::new(None) };
}

impl TableDelegate for Files {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        100_000
    }
    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }
    fn render_td(&mut self, row_ix: usize, col_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) -> impl IntoElement {
        cell(row_ix, col_ix)
    }
    fn render_tr(&mut self, row_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) -> Stateful<Div> {
        div()
            .id(("row", row_ix))
            .on_mouse_down(MouseButton::Right, move |_, _, _| phase0::post_at_cursor(phase0::WM_SHOW_MENU, row_ix))
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, _| DRAG_FROM.with(|d| d.set(Some((row_ix, e.position)))))
            .on_mouse_move(move |e: &MouseMoveEvent, _, _| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                if let Some((row, from)) = DRAG_FROM.with(|d| d.get()) {
                    let moved = (e.position.x - from.x).abs() + (e.position.y - from.y).abs();
                    if moved > px(6.) {
                        DRAG_FROM.with(|d| d.set(None));
                        phase0::post_at_cursor(phase0::WM_DRAG_OUT, row);
                    }
                }
            })
    }
    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(cell(row_ix, col_ix))
    }
}

struct Kit {
    table: Entity<TableState<Files>>,
    address: Entity<InputState>,
    marked: bool,
}

impl Kit {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let files = Files {
            columns: vec![
                Column::new("name", "Name").width(px(420.)).sortable(),
                Column::new("size", "Size").width(px(110.)),
                Column::new("modified", "Date modified").width(px(180.)),
            ],
        };
        let table = cx.new(|cx| TableState::new(files, window, cx));
        let address = cx.new(|cx| InputState::new(window, cx).default_value("C:\\Users\\shado\\المستندات\\تقرير الربع 1.pdf"));
        Self { table, address, marked: false }
    }
}

impl Render for Kit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.marked {
            self.marked = true;
            if let Ok(mode) = std::env::var("SPIKE_RTL") {
                rtl_input_test(self.address.clone(), mode, window, cx);
            }
            println!("first render: {:.1} ms", since_start());
            window.on_next_frame(|window, cx| {
                if std::env::var("SPIKE_QUICK").is_ok() {
                    quick_test(cx);
                }
                let (ws, private) = memory();
                println!("first frame drawn: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
                if let Ok(handle) = HasWindowHandle::window_handle(window) {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        phase0::HWND_VALUE.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
                        if std::env::var("SPIKE_HWND").is_ok() {
                            println!("hwnd: {}", h.hwnd.get());
                        }
                    }
                }
                integration_tests();
            });
        }
        v_flex()
            .size_full()
            .child(TitleBar::new().child("Glint — GPUI Kit spike"))
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .child(Input::new(&self.address).w(px(560.)))
                    .child(gpui_kit::component::button::Button::new("go").label("Go")),
            )
            .child(div().flex_grow(1.).size_full().child(DataTable::new(&self.table)))
    }
}

static QUICK_FRAME: std::sync::Mutex<Option<f64>> = std::sync::Mutex::new(None);

struct Quick {
    input: Entity<InputState>,
    hits: Vec<(SharedString, SharedString)>,
    status: SharedString,
    marked: bool,
    painted: bool,
    asked_at: f64,
}

impl Quick {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value("report"));
        Self { input, hits: Vec::new(), status: "".into(), marked: false, painted: true, asked_at: 0. }
    }

    fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.asked_at = since_start();
        let task = cx.background_spawn(async move {
            let request = shadui_index::proto::SearchRequest {
                query,
                roots: vec!["C:\\".into()],
                hidden: false,
                exclude: vec![],
                max_results: Some(50),
                fuzzy: false,
                rank: true,
            };
            let (mut hits, mut service_ms) = (Vec::new(), 0.0);
            let result = shadui_index::client::search(&request, |event| {
                match event {
                    shadui_index::client::Event::Hits(h) => hits.extend(h),
                    shadui_index::client::Event::Done { elapsed, .. } => service_ms = elapsed,
                    _ => {}
                }
                true
            });
            (result.map_err(|e| format!("{e:?}")), hits, service_ms)
        });
        cx.spawn(async move |this, cx| {
            let (result, hits, service_ms) = task.await;
            let _ = this.update(cx, |this, cx| {
                let waited = since_start() - this.asked_at;
                this.status = match result {
                    Ok(()) => format!("{} hits: the service took {service_ms:.1} ms, the answer was in this window's hands after {waited:.1} ms", hits.len()),
                    Err(e) => format!("index: {e}"),
                }
                .into();
                println!("quick: {}", this.status);
                this.hits = hits.into_iter().map(|h| (h.path.into(), format!("{} KB", h.size / 1024).into())).collect();
                this.painted = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Quick {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.marked {
            self.marked = true;
            window.on_next_frame(|_, _| *QUICK_FRAME.lock().unwrap() = Some(since_start()));
        }
        if !self.painted {
            self.painted = true;
            let asked = self.asked_at;
            window.on_next_frame(move |_, _| println!("quick: results painted {:.1} ms after the question", since_start() - asked));
        }
        v_flex()
            .size_full()
            .p_2()
            .gap_1()
            .child(Input::new(&self.input))
            .child(div().text_sm().child(self.status.clone()))
            .children(self.hits.iter().take(14).map(|(path, size)| {
                h_flex().gap_2().text_sm().child(div().flex_grow(1.).child(path.clone())).child(size.clone())
            }))
    }
}

fn quick_test(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut opens = Vec::new();
        for round in 0..6 {
            *QUICK_FRAME.lock().unwrap() = None;
            let t0 = since_start();
            let opened = cx.update(|cx| {
                let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
                gpui_kit::open_window(
                    WindowOptions {
                        kind: WindowKind::PopUp,
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: None,
                        ..Default::default()
                    },
                    cx,
                    |window, cx| cx.new(|cx| Quick::new(window, cx)),
                )
            });
            let Ok((window, quick)) = opened else {
                println!("quick: open failed");
                return;
            };
            loop {
                cx.background_executor().timer(Duration::from_millis(1)).await;
                if let Some(t) = *QUICK_FRAME.lock().unwrap() {
                    opens.push(((t - t0) * 10.).round() / 10.);
                    break;
                }
            }
            if round == 0 || round == 5 {
                cx.update(|cx| quick.update(cx, |q, cx| q.search("report".into(), cx)));
                cx.background_executor().timer(Duration::from_millis(900)).await;
            }
            cx.update(|cx| {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            });
            cx.background_executor().timer(Duration::from_millis(250)).await;
        }
        println!("quick: window opened to its first frame, ms per round: {opens:?}");
    })
    .detach();
}

fn rtl_input_test(address: Entity<InputState>, mode: String, window: &mut Window, cx: &mut Context<Kit>) {
    cx.spawn_in(window, async move |_, cx| {
        let executor = cx.background_executor().clone();
        let pause = |ms: u64| executor.timer(Duration::from_millis(ms));
        pause(500).await;
        let name = "تقرير الربع 1.pdf";
        let _ = address.update_in(cx, |input, window, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        pause(300).await;
        let Some(bounds) = address.read_with(cx, |input, _| input.text_bounds()) else {
            println!("rtl: no text bounds");
            return;
        };
        let y = bounds.origin.y + bounds.size.height * 0.5;
        println!("rtl: text bounds x {:.1}..{:.1}", bounds.origin.x.as_f32(), (bounds.origin.x + bounds.size.width).as_f32());
        // Byte offsets of the characters, to read the cursor back as "after which letter".
        let label = |cursor: usize| -> String {
            match name[..cursor.min(name.len())].chars().last() {
                Some(c) => format!("{cursor} (after '{c}')"),
                None => format!("{cursor} (start)"),
            }
        };
        let mut clicks = Vec::new();
        for k in 0..=16 {
            let x = bounds.origin.x + px(4.) + px(9.) * k as f32;
            let position = point(x, y);
            let _ = cx.update(|window, cx| {
                let modifiers = Modifiers::default();
                window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position, modifiers, click_count: 1, first_mouse: false }), cx);
                window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers, click_count: 1 }), cx);
            });
            pause(40).await;
            let cursor = address.read_with(cx, |input, _| input.cursor());
            clicks.push(format!("x+{:.0}: {}", (x - bounds.origin.x).as_f32(), label(cursor)));
        }
        println!("rtl: clicks across the text (logical px from its left) -> cursor: {}", clicks.join(" | "));
        let mut keys = Vec::new();
        for key in ["home", "right", "right", "right", "end", "left", "left"] {
            let _ = cx.update(|window, cx| {
                window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            });
            pause(40).await;
            keys.push(format!("{key}: {}", label(address.read_with(cx, |input, _| input.cursor()))));
        }
        println!("rtl: keys -> cursor: {}", keys.join(" | "));
        let _ = address.update_in(cx, |input, _, cx| {
            if mode == "select" {
                input.set_selected_range(0..10, cx);
            } else {
                input.set_selected_range(10..10, cx);
            }
        });
        println!("rtl: left {} for the screenshot", if mode == "select" { "the first word selected" } else { "the caret after the first word" });
    })
    .detach();
}

/// Files the rows stand for (the shell menu and drag-out need real ones).
fn spike_files() -> Vec<String> {
    let dir = std::env::temp_dir().join("glint-gpui-spike-files");
    let _ = std::fs::create_dir_all(&dir);
    ["report.txt", "تقرير الربع 1.txt", "notes.md"]
        .iter()
        .map(|name| {
            let path = dir.join(name);
            if !path.exists() {
                let _ = std::fs::write(&path, "GPUI spike test file");
            }
            path.to_string_lossy().into_owned()
        })
        .collect()
}

fn integration_tests() {
    let window = phase0::hwnd();
    let files = spike_files();
    phase0::set_files(files.clone());
    phase0::subclass(window);
    let target = phase0::replace_drop_target(window);
    std::mem::forget(target.clone());
    if std::env::var("SPIKE_ISLAND").is_ok() {
        use windows::Win32::Foundation::RECT;
        phase0::child_window(window, RECT { left: 620, top: 160, right: 1080, bottom: 280 });
        if let (Ok(path), Ok(clsid)) = (std::env::var("SPIKE_PREVIEW"), std::env::var("SPIKE_PREVIEW_CLSID")) {
            phase0::preview_handler(window, RECT { left: 40, top: 220, right: 580, bottom: 700 }, &path, &clsid);
        }
        let data = std::env::temp_dir().join("glint-gpui-spike-webview2").to_string_lossy().into_owned();
        phase0::webview(window, RECT { left: 620, top: 300, right: 1080, bottom: 520 }, data);
    }
    if std::env::var("SPIKE_SHELL").is_ok() {
        if let Some(target) = &target {
            phase0::synthetic_drop(target, &files[..2]);
        }
        let raw = phase0::HWND_VALUE.load(std::sync::atomic::Ordering::Relaxed);
        std::thread::spawn(move || {
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CANCELMODE};
            let window = windows::Win32::Foundation::HWND(raw as *mut _);
            std::thread::sleep(Duration::from_millis(700));
            let at = phase0::screen_point(window, 300, 300);
            phase0::post(window, phase0::WM_SHOW_MENU, 1, at.x, at.y);
            std::thread::sleep(Duration::from_millis(1800));
            unsafe {
                let _ = PostMessageW(Some(window), WM_CANCELMODE, Default::default(), Default::default());
            }
            std::thread::sleep(Duration::from_millis(400));
            phase0::post(window, phase0::WM_DRAG_OUT, 0, 0, 0);
        });
    }
}

fn main() {
    let (ws, private) = memory();
    println!("main: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
    let quit_after = std::env::var("SPIKE_QUIT_MS").ok().and_then(|v| v.parse::<u64>().ok());
    let app = application().with_assets(gpui_kit::assets::Assets);
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        let (ws, private) = memory();
        println!("app ready + kit init: {:.1} ms ({ws:.1} MB ws, {private:.1} MB private)", since_start());
        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Kit::new(window, cx)),
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
