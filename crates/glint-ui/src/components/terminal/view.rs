//! One terminal: a shell's screen, its keyboard and mouse (`TerminalView` in TerminalPanel.tsx).
//!
//! A live terminal ([`TerminalView::live`]) starts its shell once the element has measured how many columns and
//! rows fit (the TypeScript fitted before spawning too), then:
//!
//! - **Output** arrives on core's reader thread and is parsed into the [`Screen`] right there; the view only hears
//!   "there is new output" through a channel, at most once a frame, and redraws. Folder reports and the shell's end
//!   come the same way and become [`TerminalEvent`]s for the pane.
//! - **Keys** are encoded to escape sequences by [`keys`](super::keys) and queued to the writer thread
//!   ([`Outbox`]); the pane's keystroke interceptor calls [`TerminalView::key`] before the app's own shortcuts
//!   can see them (every key but Ctrl+` is the shell's, as the TypeScript stopped propagation). Typed text and
//!   input methods come through GPUI's input handler ([`EntityInputHandler`]), so composition (Chinese, Japanese,
//!   Korean, dead keys) works.
//! - **The mouse** selects (drag, double click for a word, triple for a line, Alt for a block), scrolls the
//!   scrollback or the program (arrows in full-screen programs, wheel reports when it asked for the mouse),
//!   reports clicks to programs that ask, and Ctrl+click opens the link under the pointer.
//!
//! A playback terminal ([`TerminalView::playback`]) is the same screen and element with no shell: the cast
//! preview feeds it.

use super::element::{Fit, Geometry, TerminalElement};
use super::emu::{Hooks, LinkSpan, LinkTarget, Palette, Pick, Screen, Signal};
use super::keys::{self, Button, Mods, Mouse};
use super::session::{self, Backend, Msg, Outbox, SessionEvent, Spawn, Waker};
use crate::kit::prelude::*;
use crate::{t, theme};
use alacritty_terminal::term::TermMode;
use gpui_kit::{
    Bounds, ClipboardItem, EntityInputHandler, EventEmitter, KeyDownEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Role, ScrollEvent, Subscription, Task, UTF16Selection,
};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// A frame, in milliseconds: output redraws no oftener than this.
const FRAME: Duration = Duration::from_millis(16);
/// How long the cursor shows and hides when it blinks.
const BLINK: Duration = Duration::from_millis(530);
/// The font size in rems: 13 px at the usual 16 (`fontSize: 13`).
pub const FONT_REMS: f32 = 13. / 16.;

/// What a terminal tells the pane (and the pane's owner).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// Its prompt said it is in this folder.
    Cwd(String),
    /// The shell ended with this code.
    Exited(u32),
    /// The program set the title.
    Title(Option<String>),
    /// A path in the output was Ctrl+clicked: to be opened in Glint.
    OpenPath(String),
    /// Whether right-to-left text is on screen (the pane says terminals draw it letter by letter).
    Rtl(bool),
}

/// The live part of a terminal: its shell.
struct Live {
    backend: Backend,
    outbox: Outbox,
    /// The writer thread's end of the queue, until the session has an id to write to.
    commands: Option<std::sync::mpsc::Receiver<session::Command>>,
    shell: String,
    cwd: String,
    id: Option<u32>,
    started: bool,
    tx: async_channel::Sender<Msg>,
}

/// What a held mouse button is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    /// Extending the selection.
    Select,
    /// Dragging the scrollbar's thumb, holding it this far from the thumb's top.
    Scrollbar(f32),
    /// Telling the program about the held button.
    Report(Button),
}

/// A terminal.
pub struct TerminalView {
    screen: Screen,
    live: Option<Live>,
    name: SharedString,
    shell_name: SharedString,
    focus: FocusHandle,
    geometry: Rc<Cell<Option<Geometry>>>,
    palette: Palette,
    palette_id: u64,
    waker: Waker,
    drag: Drag,
    hot: bool,
    link: Option<LinkSpan>,
    wheel_rest: f32,
    composition: Option<SharedString>,
    cursor_on: bool,
    blink: Option<Task<()>>,
    idle: bool,
    exited: Option<u32>,
    title: Option<String>,
    rtl: bool,
    _task: Task<()>,
    _start: Task<()>,
    _sync: Option<Task<()>>,
    _focus_in: Subscription,
    _focus_out: Subscription,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

/// The palette of the app's theme (`terminalTheme()`): the card as background, the text color, the primary color
/// for the cursor.
pub fn palette_of(t: &theme::Tokens) -> Palette {
    let hex = |color: gpui_kit::Hsla| {
        let c: gpui_kit::Rgba = color.into();
        let channel = |v: f32| (v * 255.).round().clamp(0., 255.) as u32;
        (channel(c.r) << 16) | (channel(c.g) << 8) | channel(c.b)
    };
    Palette::new(t.dark, hex(t.card), hex(t.foreground), hex(t.primary))
}

impl TerminalView {
    /// A terminal running `shell` (an id from `term_shells`; `shell_name` is how it is named to the person) in
    /// `cwd`. The shell starts when the view is first drawn and its size is known.
    pub fn live(
        shell: &str,
        shell_name: impl Into<SharedString>,
        cwd: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let backend = session::backend(cx);
        let (tx, rx) = async_channel::unbounded();
        let waker = Waker::new(tx.clone());
        let (outbox, commands) = Outbox::new();
        let hooks = {
            let (write, signal, wake) = (outbox.clone(), tx.clone(), waker.clone());
            Hooks {
                write: Arc::new(move |text| write.write(text)),
                signal: Arc::new(move |s| {
                    let _ = signal.try_send(Msg::Signal(s));
                }),
                wake: Arc::new(move || wake.wake()),
            }
        };
        let screen = Screen::new(80, 24, hooks);
        let live = Live {
            backend,
            outbox,
            commands: Some(commands),
            shell: shell.to_string(),
            cwd: cwd.to_string(),
            id: None,
            started: false,
            tx: tx.clone(),
        };
        Self::build(screen, Some(live), shell_name.into(), waker, tx, rx, window, cx)
    }

    /// A terminal with no shell, showing a screen something else feeds: `cols` x `rows`, no scrollback. A
    /// recording plays in one.
    pub fn playback(
        cols: usize,
        rows: usize,
        name: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let waker = Waker::new(tx.clone());
        let wake = waker.clone();
        let hooks = Hooks {
            wake: Arc::new(move || wake.wake()),
            ..Hooks::none()
        };
        let screen = Screen::fixed(cols, rows, hooks);
        Self::build(screen, None, name.into(), waker, tx, rx, window, cx)
    }

    #[allow(clippy::too_many_arguments)] // The pieces `live` and `playback` have each made, put together.
    fn build(
        screen: Screen,
        live: Option<Live>,
        name: SharedString,
        waker: Waker,
        _tx: async_channel::Sender<Msg>,
        rx: async_channel::Receiver<Msg>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus, window, |this, _, cx| this.focus_changed(true, cx));
        let focus_out = cx.on_focus_out(&focus, window, |this, _, _, cx| this.focus_changed(false, cx));
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |this, window, cx| this.apply(batch, window, cx))
                    .is_err()
                {
                    break;
                }
                // No more than a redraw a frame, however fast the shell prints.
                cx.background_executor().timer(FRAME).await;
            }
        });
        let palette = Palette::new(true, 0x242424, 0xf2f2f2, 0xec4899);
        TerminalView {
            screen,
            live,
            shell_name: name.clone(),
            name,
            focus,
            geometry: Rc::new(Cell::new(None)),
            palette,
            palette_id: 1,
            waker,
            drag: Drag::None,
            hot: false,
            link: None,
            wheel_rest: 0.,
            composition: None,
            cursor_on: true,
            blink: None,
            idle: false,
            exited: None,
            title: None,
            rtl: false,
            _task: task,
            _start: Task::ready(()),
            _sync: None,
            _focus_in: focus_in,
            _focus_out: focus_out,
        }
    }

    // ---- What the pane reads and does

    /// The screen: a recording feeds it.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// The focus handle: focus it to type into the terminal.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Focuses the terminal.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// The shell's id, if this is a live terminal.
    pub fn shell(&self) -> Option<&str> {
        self.live.as_ref().map(|live| live.shell.as_str())
    }

    /// The title the program set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// At its prompt with nothing typed since: a `cd` can go in (`Live.idle`).
    pub fn is_idle(&self) -> bool {
        self.idle && self.exited.is_none()
    }

    /// How the shell ended (`None`: still running).
    pub fn exited(&self) -> Option<u32> {
        self.exited
    }

    /// The text on screen, a row each (tests, and screen readers' summary).
    #[cfg(test)]
    pub fn visible_text(&self) -> Vec<String> {
        self.screen.with(|emu| emu.visible_text())
    }

    /// Types `text` at the shell's prompt as if the person had (a `cd`, a task to run): the terminal is busy
    /// until its prompt reports back.
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.idle = false;
        self.write(text.to_string());
        if self.screen.with(|emu| emu.scroll_to_bottom()) {
            cx.notify();
        }
    }

    fn write(&self, text: String) {
        if let Some(live) = &self.live {
            if self.exited.is_none() {
                live.outbox.write(text);
            }
        }
    }

    fn mode(&self) -> TermMode {
        self.screen.with(|emu| emu.mode())
    }

    // ---- Messages from the shell's side

    fn apply(&mut self, batch: Vec<Msg>, window: &mut Window, cx: &mut Context<Self>) {
        let mut redraw = false;
        for msg in batch {
            match msg {
                Msg::Wake => {
                    // Rearmed before drawing: output that arrives meanwhile wakes the next redraw.
                    self.waker.rearm();
                    redraw = true;
                }
                Msg::Signal(Signal::Title(title)) => {
                    self.title = title.clone();
                    cx.emit(TerminalEvent::Title(title));
                }
                Msg::Signal(Signal::Clipboard(text)) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Msg::Signal(Signal::Bell) => {}
                Msg::Session(SessionEvent::Cwd(path)) => {
                    self.idle = true;
                    cx.emit(TerminalEvent::Cwd(path));
                }
                Msg::Session(SessionEvent::Exit(code)) => self.ended(code, cx),
                Msg::Resized(cols, rows) => self.resized(cols, rows, window, cx),
            }
        }
        if redraw {
            self.check_rtl(cx);
            self.arm_sync(cx);
            cx.notify();
        }
    }

    /// The shell ended: said dimmed, and no cursor left blinking (`[name ended with code.]`).
    fn ended(&mut self, code: u32, cx: &mut Context<Self>) {
        if self.exited.is_some() {
            return;
        }
        self.exited = Some(code);
        let with = if code == 0 {
            String::new()
        } else {
            format!(" with {code}")
        };
        self.screen
            .feed(format!("\r\n\x1b[2m[{} ended{with}.]\x1b[0m\r\n\x1b[?25l", self.shell_name).as_bytes());
        cx.emit(TerminalEvent::Exited(code));
        cx.notify();
    }

    fn check_rtl(&mut self, cx: &mut Context<Self>) {
        let rtl = self.screen.with(|emu| emu.snapshot().rtl);
        if rtl != self.rtl {
            self.rtl = rtl;
            cx.emit(TerminalEvent::Rtl(rtl));
        }
    }

    /// A program began a synchronized update and hasn't ended it: shows what it printed when its time is up.
    fn arm_sync(&mut self, cx: &mut Context<Self>) {
        let Some(wait) = self.screen.with(|emu| emu.sync_wait()) else {
            return;
        };
        if self._sync.is_some() {
            return;
        }
        let screen = self.screen.clone();
        self._sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait + Duration::from_millis(5)).await;
            if screen.with(|emu| emu.flush_sync()) {
                screen.wake();
            }
            this.update(cx, |this, _| this._sync = None).ok();
        }));
    }

    /// The element measured the grid: the first time starts the shell at that size, later ones resize it.
    fn resized(&mut self, cols: u16, rows: u16, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_mut() else { return };
        if live.started {
            live.outbox.resize(cols, rows);
            return;
        }
        live.started = true;
        let channel = session::output_channel(self.screen.clone(), live.tx.clone());
        let starting = (live.backend.spawn)(Spawn {
            shell: live.shell.clone(),
            cwd: live.cwd.clone(),
            cols,
            rows,
            output: channel,
        });
        let backend = live.backend.clone();
        let commands = live.commands.take();
        let name = self.shell_name.clone();
        self._start = cx.spawn(async move |this, cx| match starting.await {
            Ok(id) => {
                let kept = this
                    .update(cx, |this, _| match this.live.as_mut() {
                        Some(live) => {
                            live.id = Some(id);
                            true
                        }
                        None => false,
                    })
                    .unwrap_or(false);
                match commands {
                    Some(commands) if kept => session::start_writer(id, commands, backend),
                    // The tab was closed while the shell was starting.
                    _ => {
                        let close = backend.close.clone();
                        cx.background_spawn(async move { close(id) }).detach();
                    }
                }
            }
            Err(error) => {
                this.update(cx, |this, cx| {
                    this.screen
                        .feed(format!("\r\nCouldn't start {name}: {error}\r\n").as_bytes());
                    cx.notify();
                })
                .ok();
            }
        });
    }

    // ---- Focus and the cursor's blink

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.screen.with(|emu| emu.set_focused(focused));
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.write(if focused { "\x1b[I" } else { "\x1b[O" }.to_string());
        }
        if focused {
            self.restart_blink(cx);
        } else {
            self.blink = None;
            self.cursor_on = true;
        }
        cx.notify();
    }

    /// The cursor shows solid, and blinks again after the next pause.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_on = true;
        self.blink = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BLINK).await;
            if this
                .update(cx, |this, cx| {
                    this.cursor_on = !this.cursor_on;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        }));
    }

    // ---- The keyboard

    /// A key pressed while this terminal has the keyboard. Returns whether the terminal took it (the pane then
    /// stops the app's shortcuts from seeing it). Text a person types is left to the input handler, which is how
    /// input methods get it; everything else is encoded here.
    pub fn key(&mut self, keystroke: &Keystroke, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.live.is_none() {
            return false;
        }
        let m = &keystroke.modifiers;
        if m.platform
            || m.function
            || matches!(
                keystroke.key.as_str(),
                "shift" | "control" | "alt" | "platform" | "function"
            )
        {
            return false;
        }
        let mods = Mods {
            shift: m.shift,
            ctrl: m.control,
            alt: m.alt,
        };
        let key = keystroke.key.as_str();
        // Ctrl+` hides the pane: the app's.
        if mods.ctrl && !mods.alt && key == "`" {
            return false;
        }
        // Ctrl+Shift+C and Ctrl+Insert copy; Ctrl+C copies when something is selected (else it is ^C).
        let has_selection = self.screen.with(|emu| emu.selection_text().is_some());
        if mods.ctrl && !mods.alt && ((mods.shift && key == "c") || key == "insert" || (key == "c" && has_selection)) {
            self.copy_selection(cx);
            return true;
        }
        // Ctrl+V, Ctrl+Shift+V and Shift+Insert paste.
        if (mods.ctrl && !mods.alt && key == "v") || (mods.shift && !mods.ctrl && !mods.alt && key == "insert") {
            self.paste(cx);
            return true;
        }
        // Shift+PageUp, PageDown, Home and End move through the scrollback (not in a full-screen program).
        if mods.shift && !mods.ctrl && !mods.alt && !self.mode().contains(TermMode::ALT_SCREEN) {
            let moved = match key {
                "pageup" => Some(self.screen.with(|emu| emu.scroll_page(true))),
                "pagedown" => Some(self.screen.with(|emu| emu.scroll_page(false))),
                "home" => Some(self.screen.with(|emu| emu.scroll_to_top())),
                "end" => Some(self.screen.with(|emu| emu.scroll_to_bottom())),
                _ => None,
            };
            if let Some(moved) = moved {
                if moved {
                    cx.notify();
                }
                return true;
            }
        }
        // Typed text goes through the input handler (and its input method).
        if keys::is_text(keystroke.key_char.as_deref(), mods) {
            return false;
        }
        if let Some(sequence) = keys::encode_key(key, keystroke.key_char.as_deref(), mods, self.mode()) {
            self.send(sequence, cx);
        }
        true
    }

    /// Sends what the person typed: the view goes back to the bottom, and the terminal is busy.
    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        if self.live.is_none() {
            return;
        }
        if glint_model::cd::is_typing(&text) {
            self.idle = false;
        }
        let changed = self.screen.with(|emu| {
            let scrolled = emu.scroll_to_bottom();
            let cleared = emu.clear_selection();
            scrolled || cleared
        });
        self.write(text);
        self.restart_blink(cx);
        if changed {
            cx.notify();
        }
    }

    /// Copies the selection to the clipboard and clears it (`copy()`). Returns whether there was one.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.screen.with(|emu| {
            let text = emu.selection_text();
            if text.is_some() {
                emu.clear_selection();
            }
            text
        });
        match text {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Pastes the clipboard's text (bracketed when the program asked for that).
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let sequence = keys::paste_text(&text, self.mode());
        self.send(sequence, cx);
    }

    /// Ctrl+C copies a selection in a recording's screen.
    fn playback_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control && (k.key == "c" || k.key == "insert") && self.copy_selection(cx) {
            cx.stop_propagation();
        }
    }

    // ---- The mouse

    fn mods_of(modifiers: &Modifiers) -> Mods {
        Mods {
            shift: modifiers.shift,
            ctrl: modifiers.control,
            alt: modifiers.alt,
        }
    }

    fn report(&mut self, event: Mouse, col: usize, row: usize, modifiers: &Modifiers) {
        let mode = self.mode();
        if let Some(sequence) = keys::mouse_report(event, col, row, Self::mods_of(modifiers), mode) {
            self.write(sequence);
        }
    }

    fn button_of(button: MouseButton) -> Option<Button> {
        match button {
            MouseButton::Left => Some(Button::Left),
            MouseButton::Middle => Some(Button::Middle),
            MouseButton::Right => Some(Button::Right),
            _ => None,
        }
    }

    /// A button went down over the terminal.
    pub fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(geometry) = self.geometry.get() else { return };
        // The scrollbar.
        if event.button == MouseButton::Left {
            if let (Some(track), Some(thumb)) = (geometry.track, geometry.thumb) {
                if track.contains(&event.position) {
                    let grab = if thumb.contains(&event.position) {
                        f32::from(event.position.y - thumb.origin.y)
                    } else {
                        f32::from(thumb.size.height) / 2.
                    };
                    self.drag = Drag::Scrollbar(grab);
                    self.drag_scrollbar(event.position, &geometry, cx);
                    return;
                }
            }
        }
        let (col, row, right) = geometry.cell_at(event.position);
        // Ctrl+click opens the link under the pointer.
        if event.button == MouseButton::Left && event.modifiers.control {
            if let Some(link) = self.screen.with(|emu| emu.link_at(row, col)) {
                self.open_link(link.target, cx);
                return;
            }
        }
        let reporting = self.live.is_some() && self.mode().intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift;
        if reporting {
            if let Some(button) = Self::button_of(event.button) {
                self.report(Mouse::Press(button), col, row, &event.modifiers);
                self.drag = Drag::Report(button);
            }
            return;
        }
        match event.button {
            MouseButton::Left => {
                let pick = match event.click_count {
                    0 | 1 if event.modifiers.alt => Pick::Block,
                    0 | 1 => Pick::Cells,
                    2 => Pick::Word,
                    _ => Pick::Lines,
                };
                self.screen.with(|emu| emu.start_selection(pick, col, row, right));
                self.drag = Drag::Select;
                cx.notify();
            }
            // As Windows Terminal does: right-click copies a selection, else pastes.
            MouseButton::Right if self.live.is_some() => {
                if !self.copy_selection(cx) {
                    self.paste(cx);
                }
            }
            MouseButton::Middle if self.live.is_some() => self.paste(cx),
            _ => {}
        }
    }

    /// The pointer moved (anywhere in the window: a drag leaves the terminal).
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry.get() else { return };
        let inside = geometry.bounds.contains(&event.position);
        if inside != self.hot {
            self.hot = inside;
            cx.notify();
        }
        match self.drag {
            Drag::Select => {
                // Held above or below the text, the view scrolls on towards that side.
                if event.position.y < geometry.origin.y {
                    self.screen.with(|emu| emu.scroll_by(1));
                } else if event.position.y > geometry.origin.y + geometry.cell.height * geometry.rows as f32 {
                    self.screen.with(|emu| emu.scroll_by(-1));
                }
                let (col, row, right) = geometry.cell_at(event.position);
                self.screen.with(|emu| emu.update_selection(col, row, right));
                cx.notify();
            }
            Drag::Scrollbar(_) => self.drag_scrollbar(event.position, &geometry, cx),
            Drag::Report(button) => {
                let (col, row, _) = geometry.cell_at(event.position);
                self.report(Mouse::Move(Some(button)), col, row, &event.modifiers);
            }
            Drag::None => {
                if !inside {
                    self.set_link(None, cx);
                    return;
                }
                let (col, row, _) = geometry.cell_at(event.position);
                if self.live.is_some() && self.mode().contains(TermMode::MOUSE_MOTION) && !event.modifiers.shift {
                    self.report(Mouse::Move(None), col, row, &event.modifiers);
                }
                // The link under the pointer lights while Ctrl is held.
                let link = if event.modifiers.control {
                    self.screen.with(|emu| emu.link_at(row, col))
                } else {
                    None
                };
                self.set_link(link, cx);
            }
        }
    }

    fn set_link(&mut self, link: Option<LinkSpan>, cx: &mut Context<Self>) {
        if self.link != link {
            self.link = link;
            cx.notify();
        }
    }

    /// A button went up.
    pub fn mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        if let (Drag::Report(button), Some(geometry)) = (drag, self.geometry.get()) {
            let (col, row, _) = geometry.cell_at(event.position);
            self.report(Mouse::Release(button), col, row, &event.modifiers);
        }
        if drag != Drag::None {
            cx.notify();
        }
    }

    /// The wheel turned over the terminal.
    pub fn wheel(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry.get() else { return };
        let lines = event.delta.pixel_delta(geometry.cell.height).y / geometry.cell.height;
        self.wheel_rest += lines;
        let whole = self.wheel_rest.trunc();
        self.wheel_rest -= whole;
        let notches = whole as i32;
        if notches == 0 {
            return;
        }
        cx.stop_propagation();
        let mode = self.mode();
        let up = notches > 0;
        let count = notches.unsigned_abs() as usize;
        if self.live.is_some() && mode.intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift {
            let (col, row, _) = geometry.cell_at(event.position);
            for _ in 0..count {
                self.report(Mouse::Wheel { up }, col, row, &event.modifiers);
            }
        } else if self.live.is_some()
            && mode.contains(TermMode::ALT_SCREEN)
            && mode.contains(TermMode::ALTERNATE_SCROLL)
        {
            self.write(keys::wheel_as_arrows(up, count, mode));
        } else if self.screen.with(|emu| emu.scroll_by(notches)) {
            cx.notify();
        }
    }

    /// Moves the scrollbar's thumb to the pointer.
    fn drag_scrollbar(&mut self, at: Point<Pixels>, geometry: &Geometry, cx: &mut Context<Self>) {
        let (Some(track), Some(thumb), Drag::Scrollbar(grab)) = (geometry.track, geometry.thumb, self.drag) else {
            return;
        };
        let room = (f32::from(track.size.height) - f32::from(thumb.size.height)).max(1.);
        let top = (f32::from(at.y - track.origin.y) - grab).clamp(0., room);
        let moved = self.screen.with(|emu| {
            let (_, history) = emu.scroll_position();
            // Everything above the view, then how much of it is hidden below.
            let above = ((top / room) * history as f32).round() as usize;
            emu.scroll_to_offset(history.saturating_sub(above))
        });
        if moved {
            cx.notify();
        }
    }

    fn open_link(&mut self, target: LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Url(url) => cx.open_url(&url),
            LinkTarget::Path(path) => cx.emit(TerminalEvent::OpenPath(path)),
        }
    }

    // ---- For tests

    #[cfg(test)]
    pub(crate) fn geometry(&self) -> Option<Geometry> {
        self.geometry.get()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        // The shell ends with its tab.
        if let Some(live) = &self.live {
            live.outbox.close();
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.composition.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.composition.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        if !text.is_empty() {
            self.send(text.to_string(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition = (!new_text.is_empty()).then(|| SharedString::from(new_text.to_string()));
        cx.notify();
    }

    /// Where the input method puts its candidate window: under the cursor's cell.
    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.get()?;
        let (col, row) = self.screen.with(|emu| emu.cursor_cell())?;
        Some(geometry.cell_bounds(col, row, 1))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.live.is_some() && self.exited.is_none()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let palette = palette_of(&tokens);
        if palette != self.palette {
            self.palette = palette.clone();
            self.palette_id += 1;
        }
        let focused = self.focus.is_focused(window);
        let on_size = self.live.is_some().then(|| {
            let tx = self.live.as_ref().map(|live| live.tx.clone());
            Rc::new(move |cols: u16, rows: u16| {
                if let Some(tx) = &tx {
                    let _ = tx.try_send(Msg::Resized(cols, rows));
                }
            }) as Rc<dyn Fn(u16, u16)>
        });
        let fit = match self.live {
            Some(_) => Fit::Fill,
            None => {
                let (cols, rows) = self.screen.with(|emu| emu.size());
                Fit::Fixed { cols, rows }
            }
        };
        let element = TerminalElement {
            screen: self.screen.clone(),
            palette,
            palette_id: self.palette_id,
            family: theme::mono_font(cx),
            rems: FONT_REMS,
            fit,
            geometry: self.geometry.clone(),
            on_size,
            focus: Some(self.focus.clone()),
            focused,
            cursor_on: self.cursor_on,
            composition: self.composition.clone(),
            link: self.link.clone(),
            scrollbar_hot: self.hot,
            view: Some(cx.entity()),
        };
        let root = div()
            .id("terminal")
            .role(Role::Terminal)
            .aria_label(self.name.clone())
            .test_support()
            .aria_description(t!(
                "The terminal draws one letter per cell, so right-to-left text can look out of order."
            ))
            .key_context("Terminal")
            .track_focus(&self.focus)
            .tab_stop(true)
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(gpui_kit::rgb(self.palette.background));
        if self.live.is_some() {
            root.child(element)
        } else {
            root.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.playback_key(event, cx)))
                .child(element)
        }
    }
}
