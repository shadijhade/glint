//! A terminal's screen: `alacritty_terminal`'s `Term` fed by vte's parser, behind one lock.
//!
//! Replaces what xterm.js (or ghostty-web) kept inside the TypeScript's `TermLike` (src/lib/terminal.ts): the
//! grid of cells, the scrollback, the selection, the modes a program sets (application cursor keys, bracketed
//! paste, mouse reporting) and the colors. Nothing here knows GPUI, so a screen is tested by feeding it bytes.
//!
//! **Who touches it.** The thread that reads the shell's output ([`Screen::feed`]) parses into the grid under
//! the lock, so the UI thread never parses. The UI thread takes the same lock for a moment to read a
//! [`Snapshot`] (one draw), to change the selection or the scroll, or to resize. A flood of output therefore
//! costs the UI thread one short lock per frame, never the parse.
//!
//! **The snapshot** is what a draw needs and nothing more: for each visible row its background fills and its
//! text in runs of one style, colors already resolved against the [`Palette`] (inverse video, bold-as-bright and
//! dim applied), the selection's columns, the cursor. It is cached against the screen's generation, so a frame
//! with no new output reuses it.

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, CursorStyle, Processor, Rgb};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The scrollback kept (`scrollback: 5000` in TerminalPanel.tsx).
pub const SCROLLBACK: usize = 5000;
/// The smallest grid a terminal is resized to (alacritty's own minimums, and what term.rs allows).
pub const MIN_COLS: usize = 10;
pub const MIN_ROWS: usize = 2;

// ---- Colors -------------------------------------------------------------------------------------------------

/// Windows Terminal's Campbell colors (`DARK` in TerminalPanel.tsx).
const DARK: [u32; 16] = [
    0x0c0c0c, 0xc50f1f, 0x13a10e, 0xc19c00, 0x0037da, 0x881798, 0x3a96dd, 0xcccccc, 0x767676, 0xe74856, 0x16c60c,
    0xf9f1a5, 0x3b78ff, 0xb4009e, 0x61d6d6, 0xf2f2f2,
];
/// One Half Light, for the light theme (`LIGHT` in TerminalPanel.tsx).
const LIGHT: [u32; 16] = [
    0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x0184bc, 0xa626a4, 0x0997b3, 0xfafafa, 0x4f525e, 0xe06c75, 0x98c379,
    0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
];

/// The terminal's colors (`terminalTheme()`): the app's card as its background, its text color, the primary
/// color for the cursor, and the ANSI sixteen. All are `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Whether this is the dark set.
    pub dark: bool,
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    /// The text under a block cursor (`cursorAccent`).
    pub cursor_accent: u32,
    pub selection: u32,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their bright versions.
    pub ansi: [u32; 16],
}

impl Palette {
    /// The theme's colors with the ANSI set that goes with light or dark.
    pub fn new(dark: bool, background: u32, foreground: u32, cursor: u32) -> Self {
        Palette {
            dark,
            background,
            foreground,
            cursor,
            cursor_accent: background,
            selection: if dark { 0x3b4b6b } else { 0xc8d7f5 },
            ansi: if dark { DARK } else { LIGHT },
        }
    }

    /// Color `index` of the 256-color table: the sixteen, the 6x6x6 cube, then 24 grays (xterm's).
    pub fn indexed(&self, index: usize) -> u32 {
        const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
        match index {
            0..=15 => self.ansi.get(index).copied().unwrap_or(self.foreground),
            16..=231 => {
                let n = index - 16;
                let level = |i: usize| LEVELS.get(i).copied().unwrap_or(0);
                (level(n / 36) << 16) | (level((n / 6) % 6) << 8) | level(n % 6)
            }
            232..=255 => {
                let gray = 8 + 10 * (index as u32 - 232);
                (gray << 16) | (gray << 8) | gray
            }
            _ => self.foreground,
        }
    }
}

/// `a` mixed towards `b`: `amount` of `a` (0..=1), the rest `b`. Dim text is two thirds of its color.
pub fn mix(a: u32, b: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x * amount + y * (1. - amount)).round().clamp(0., 255.) as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Columns and rows in one word (each clamped to 16 bits).
fn pack(cols: usize, rows: usize) -> u32 {
    (u32::from(cols.min(65535) as u16) << 16) | u32::from(rows.min(65535) as u16)
}

fn rgb_u32(color: Rgb) -> u32 {
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}

fn u32_rgb(color: u32) -> Rgb {
    Rgb {
        r: (color >> 16) as u8,
        g: (color >> 8) as u8,
        b: color as u8,
    }
}

/// How many entries alacritty's dynamic color table has (`term::color::COUNT`).
const COLOR_SLOTS: usize = 269;

/// The color a cell asks for, as `0xRRGGBB`. A color the program set itself (OSC 4, 10, 11) wins over the
/// palette. `bright` draws the eight dark ANSI colors as their bright twins (bold text).
fn resolve(color: Color, colors: &Colors, palette: &Palette, bright: bool) -> u32 {
    let slot = match color {
        Color::Spec(rgb) => return rgb_u32(rgb),
        Color::Indexed(i) => usize::from(i),
        Color::Named(name) => name as usize,
    };
    let slot = if bright && slot < 8 { slot + 8 } else { slot };
    if slot < COLOR_SLOTS {
        if let Some(rgb) = colors[slot] {
            return rgb_u32(rgb);
        }
    }
    match slot {
        0..=255 => palette.indexed(slot),
        // Foreground and its bright form, background, cursor.
        256 | 267 => palette.foreground,
        257 => palette.background,
        258 => palette.cursor,
        // Dim black..white.
        259..=266 => mix(palette.indexed(slot - 259), palette.background, 2. / 3.),
        268 => mix(palette.foreground, palette.background, 2. / 3.),
        _ => palette.foreground,
    }
}

// ---- The snapshot -------------------------------------------------------------------------------------------

/// How a run is underlined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// A rectangle of one color behind some cells (only where it isn't the terminal's background).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub col: usize,
    pub cells: usize,
    pub color: u32,
}

/// Cells drawn as one piece of text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub col: usize,
    /// How many columns it covers (a wide character covers two).
    pub cells: usize,
    pub text: String,
    pub fg: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub underline_color: Option<u32>,
    pub strike: bool,
    /// Every character in the run is two cells wide, so each glyph is given two columns.
    pub wide: bool,
}

/// One visible row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub fills: Vec<Fill>,
    pub runs: Vec<Run>,
    /// The selected columns, first and last (inclusive).
    pub selected: Option<(usize, usize)>,
}

/// The cursor, in viewport cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
    /// The cursor is on a wide character: two cells.
    pub wide: bool,
    /// The character under it, drawn again in the accent color over a block.
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// The program asked for it to blink (DECSCUSR).
    pub blinking: bool,
}

/// Everything one draw needs of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: Vec<Row>,
    pub cursor: Option<Cursor>,
    /// Lines scrolled up from the bottom.
    pub offset: usize,
    /// Lines of scrollback kept.
    pub history: usize,
    pub mode: TermMode,
    /// Some right-to-left letter is on screen: terminals draw a cell a letter, so it may look out of order.
    pub rtl: bool,
}

impl Snapshot {
    /// The text of row `row`, trailing spaces trimmed (tests read the screen with this).
    pub fn line_text(&self, row: usize) -> String {
        let mut runs: Vec<&Run> = self.rows.get(row).map(|r| r.runs.iter().collect()).unwrap_or_default();
        runs.sort_by_key(|run| run.col);
        let mut out = String::new();
        let mut at = 0;
        for run in runs {
            while at < run.col {
                out.push(' ');
                at += 1;
            }
            out.push_str(&run.text);
            at = run.col + run.cells;
        }
        out.trim_end().to_string()
    }
}

/// Whether `c` is a letter of a right-to-left script (Hebrew, Arabic, Syriac, Thaana, NKo and presentation forms).
pub fn is_rtl_letter(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff | 0x10800..=0x10fff | 0x1e800..=0x1efff)
        && c.is_alphabetic()
}

// ---- Events from the parser ---------------------------------------------------------------------------------

/// What the terminal tells the UI besides new cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The program set the window title (OSC 0/2); `None` resets it.
    Title(Option<String>),
    /// The program asked to copy text (OSC 52).
    Clipboard(String),
    /// The bell.
    Bell,
}

/// How a screen reaches the outside. A cast player's screen uses [`Hooks::none`].
#[derive(Clone)]
pub struct Hooks {
    /// Bytes the terminal answers a program with (cursor position reports, device attributes, color queries):
    /// they go to the shell's input.
    pub write: Arc<dyn Fn(String) + Send + Sync>,
    pub signal: Arc<dyn Fn(Signal) + Send + Sync>,
    /// New cells are there to draw. Called from whichever thread fed the screen, so it must be cheap and
    /// must not block: the live terminal's coalesces them to one redraw per frame.
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Hooks {
    /// A screen nobody listens to (a recording, a test).
    pub fn none() -> Self {
        Hooks {
            write: Arc::new(|_| {}),
            signal: Arc::new(|_| {}),
            wake: Arc::new(|| {}),
        }
    }
}

/// A color a program asked about (OSC 4/10/11 with `?`), answered once the parser lock is held.
type ColorAnswer = (usize, Arc<dyn Fn(Rgb) -> String + Send + Sync>);

/// alacritty's event sink: answers what can be answered at once, queues what needs the grid.
#[derive(Clone)]
struct Listener {
    hooks: Hooks,
    colors: Arc<Mutex<Vec<ColorAnswer>>>,
    /// The size of a cell in pixels, packed `width << 16 | height` (for the text area size report).
    cell: Arc<AtomicU32>,
    /// The grid's size, packed `columns << 16 | rows`.
    grid: Arc<AtomicU32>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => (self.hooks.write)(text),
            Event::Title(title) => (self.hooks.signal)(Signal::Title(Some(title))),
            Event::ResetTitle => (self.hooks.signal)(Signal::Title(None)),
            Event::ClipboardStore(_, text) => (self.hooks.signal)(Signal::Clipboard(text)),
            Event::Bell => (self.hooks.signal)(Signal::Bell),
            Event::ColorRequest(index, answer) => {
                if let Ok(mut queue) = self.colors.lock() {
                    queue.push((index, answer));
                }
            }
            Event::TextAreaSizeRequest(answer) => {
                let (cell, grid) = (self.cell.load(Ordering::Relaxed), self.grid.load(Ordering::Relaxed));
                let size = WindowSize {
                    num_lines: grid as u16,
                    num_cols: (grid >> 16) as u16,
                    cell_width: (cell >> 16) as u16,
                    cell_height: cell as u16,
                };
                (self.hooks.write)(answer(size));
            }
            // New cells wake the UI from `Screen::feed`, once per chunk, not once per parser event.
            Event::Wakeup
            | Event::CursorBlinkingChange
            | Event::MouseCursorDirty
            | Event::ClipboardLoad(..)
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

/// A grid size for alacritty (it has no public plain one outside its test module).
#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// How a mouse selection starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Cell to cell (a drag).
    Cells,
    /// A word (double click).
    Word,
    /// Whole lines (triple click).
    Lines,
    /// A rectangle (Alt+drag).
    Block,
}

// ---- Links --------------------------------------------------------------------------------------------------

/// What a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A web address or mail address: opened outside, in the default program.
    Url(String),
    /// A file or folder on this PC: opened in Glint.
    Path(String),
}

/// A link on screen: where it is (viewport cells, both ends inclusive) and what it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub target: LinkTarget,
}

impl LinkSpan {
    /// The columns of the link on row `row`, if it reaches it.
    pub fn on_row(&self, row: usize, cols: usize) -> Option<(usize, usize)> {
        if row < self.start.0 || row > self.end.0 || cols == 0 {
            return None;
        }
        let a = if row == self.start.0 { self.start.1 } else { 0 };
        let b = if row == self.end.0 { self.end.1 } else { cols - 1 };
        (a <= b).then_some((a, b))
    }
}

// ---- The screen ---------------------------------------------------------------------------------------------

struct Cached {
    generation: u64,
    palette: u64,
    snapshot: Arc<Snapshot>,
}

/// The terminal state behind the lock.
pub struct Emu {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    size: Size,
    history: usize,
    generation: u64,
    palette: Palette,
    palette_id: u64,
    cached: Option<Cached>,
}

fn config(history: usize) -> Config {
    Config {
        scrolling_history: history,
        // The cursor blinks (`cursorBlink: true`); a program can change that.
        default_cursor_style: CursorStyle {
            shape: CursorShape::Block,
            blinking: true,
        },
        // A program may put text on the clipboard; it may not read it.
        osc52: Osc52::OnlyCopy,
        ..Config::default()
    }
}

impl Emu {
    fn new(cols: usize, rows: usize, history: usize, hooks: Hooks) -> Self {
        let size = Size {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
        };
        let listener = Listener {
            hooks,
            colors: Arc::new(Mutex::new(Vec::new())),
            cell: Arc::new(AtomicU32::new(0)),
            grid: Arc::new(AtomicU32::new(pack(size.cols, size.rows))),
        };
        Emu {
            term: Term::new(config(history), &size, listener.clone()),
            parser: Processor::new(),
            listener,
            size,
            history,
            generation: 1,
            palette: Palette::new(true, 0x242424, 0xf2f2f2, 0xec4899),
            palette_id: 0,
            cached: None,
        }
    }

    /// The columns and rows.
    pub fn size(&self) -> (usize, usize) {
        (self.size.cols, self.size.rows)
    }

    /// Changes whenever what is drawn could have changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Parses `bytes` into the grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.generation += 1;
        self.answer_colors();
    }

    /// Answers the color questions the parser queued (`ESC ] 11 ; ? BEL`: what's your background?).
    fn answer_colors(&mut self) {
        let queued: Vec<ColorAnswer> = match self.listener.colors.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => return,
        };
        for (slot, answer) in queued {
            let rgb = (slot < COLOR_SLOTS)
                .then(|| self.term.colors()[slot])
                .flatten()
                .unwrap_or_else(|| {
                    u32_rgb(match slot {
                        0..=255 => self.palette.indexed(slot),
                        257 => self.palette.background,
                        258 => self.palette.cursor,
                        _ => self.palette.foreground,
                    })
                });
            (self.listener.hooks.write)(answer(rgb));
        }
    }

    /// A program began a synchronized update (DECSET 2026) and hasn't ended it: how long until the screen
    /// shows what it has printed anyway. `None` when none is pending.
    pub fn sync_wait(&self) -> Option<Duration> {
        let deadline = self.parser.sync_timeout().sync_timeout()?;
        Some(deadline.saturating_duration_since(Instant::now()))
    }

    /// Ends a synchronized update whose time is up. Returns whether it did.
    pub fn flush_sync(&mut self) -> bool {
        match self.parser.sync_timeout().sync_timeout() {
            Some(deadline) if Instant::now() >= deadline => {
                self.parser.stop_sync(&mut self.term);
                self.generation += 1;
                true
            }
            _ => false,
        }
    }

    /// Resizes the grid (the text reflows). Returns whether the size changed.
    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        let size = Size {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
        };
        if size.cols == self.size.cols && size.rows == self.size.rows {
            return false;
        }
        self.term.resize(size);
        self.size = size;
        self.listener.grid.store(pack(size.cols, size.rows), Ordering::Relaxed);
        self.generation += 1;
        true
    }

    /// Tells the terminal how big a cell is in pixels, for the program that asks (CSI 16 t, CSI 14 t).
    pub fn set_cell_pixels(&self, width: f32, height: f32) {
        let packed = pack(
            width.round().clamp(0., 65535.) as usize,
            height.round().clamp(0., 65535.) as usize,
        );
        self.listener.cell.store(packed, Ordering::Relaxed);
    }

    /// Uses these colors from now on.
    pub fn set_palette(&mut self, palette: &Palette, id: u64) {
        if self.palette_id != id || self.palette != *palette {
            self.palette = palette.clone();
            self.palette_id = id;
            self.generation += 1;
        }
    }

    /// The modes the program has set.
    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Whether the terminal is told it has the keyboard (the cursor's shape; focus reports).
    pub fn set_focused(&mut self, focused: bool) {
        self.term.is_focused = focused;
    }

    // ---- Scrolling

    /// Scrolls the view `lines` up (negative: down). Returns whether it moved.
    pub fn scroll_by(&mut self, lines: i32) -> bool {
        self.scroll(Scroll::Delta(lines))
    }

    fn scroll(&mut self, scroll: Scroll) -> bool {
        let before = self.term.grid().display_offset();
        self.term.scroll_display(scroll);
        let moved = self.term.grid().display_offset() != before;
        if moved {
            self.generation += 1;
        }
        moved
    }

    /// Back to the newest output.
    pub fn scroll_to_bottom(&mut self) -> bool {
        self.scroll(Scroll::Bottom)
    }

    /// To the oldest line kept.
    pub fn scroll_to_top(&mut self) -> bool {
        self.scroll(Scroll::Top)
    }

    /// A page up (`up`) or down.
    pub fn scroll_page(&mut self, up: bool) -> bool {
        self.scroll(if up { Scroll::PageUp } else { Scroll::PageDown })
    }

    /// Scrolls so `offset` lines of history are above the view (the scrollbar's thumb).
    pub fn scroll_to_offset(&mut self, offset: usize) -> bool {
        let now = self.term.grid().display_offset();
        let wanted = offset.min(self.term.grid().history_size());
        self.scroll(Scroll::Delta(wanted as i32 - now as i32))
    }

    /// Lines scrolled up, and lines of history.
    pub fn scroll_position(&self) -> (usize, usize) {
        (self.term.grid().display_offset(), self.term.grid().history_size())
    }

    // ---- Selection

    fn point_at(&self, col: usize, row: usize) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        let col = col.min(self.size.cols.saturating_sub(1));
        let row = row.min(self.size.rows.saturating_sub(1));
        Point::new(Line(row as i32 - offset), Column(col))
    }

    /// Starts a selection at a cell; `right` is the half of the cell the pointer was in.
    pub fn start_selection(&mut self, pick: Pick, col: usize, row: usize, right: bool) {
        let ty = match pick {
            Pick::Cells => SelectionType::Simple,
            Pick::Word => SelectionType::Semantic,
            Pick::Lines => SelectionType::Lines,
            Pick::Block => SelectionType::Block,
        };
        let side = if right { Side::Right } else { Side::Left };
        self.term.selection = Some(Selection::new(ty, self.point_at(col, row), side));
        self.generation += 1;
    }

    /// Drags the selection's end to a cell.
    pub fn update_selection(&mut self, col: usize, row: usize, right: bool) {
        let point = self.point_at(col, row);
        let side = if right { Side::Right } else { Side::Left };
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
            self.generation += 1;
        }
    }

    /// Drops the selection. Returns whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        let had = self.term.selection.take().is_some();
        if had {
            self.generation += 1;
        }
        had
    }

    /// Selects everything, scrollback included.
    pub fn select_all(&mut self) {
        let top = Point::new(self.term.topmost_line(), Column(0));
        let bottom = Point::new(self.term.bottommost_line(), Column(self.size.cols.saturating_sub(1)));
        let mut selection = Selection::new(SelectionType::Simple, top, Side::Left);
        selection.update(bottom, Side::Right);
        self.term.selection = Some(selection);
        self.generation += 1;
    }

    /// The selected text, if anything is selected.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    // ---- Reading

    /// The characters of the screen in rows, as text (trailing spaces cut), without styles. For tests and for
    /// finding links.
    pub fn visible_text(&self) -> Vec<String> {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        (0..self.size.rows)
            .map(|row| {
                let line = &grid[Line(row as i32 - offset)];
                let mut text = String::new();
                for col in 0..self.size.cols {
                    let cell = &line[Column(col)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    text.push(cell.c);
                    for extra in cell.zerowidth().into_iter().flatten() {
                        text.push(*extra);
                    }
                }
                text.trim_end().to_string()
            })
            .collect()
    }

    /// The cursor's cell in the viewport, if it is in view.
    pub fn cursor_cell(&self) -> Option<(usize, usize)> {
        let point = self.term.grid().cursor.point;
        let row = point.line.0 + self.term.grid().display_offset() as i32;
        (row >= 0 && (row as usize) < self.size.rows).then_some((point.column.0, row as usize))
    }

    /// What the screen needs drawn now. Cached until the screen or the palette changes.
    pub fn snapshot(&mut self) -> Arc<Snapshot> {
        if let Some(cached) = &self.cached {
            if cached.generation == self.generation && cached.palette == self.palette_id {
                return cached.snapshot.clone();
            }
        }
        let snapshot = Arc::new(self.build());
        self.cached = Some(Cached {
            generation: self.generation,
            palette: self.palette_id,
            snapshot: snapshot.clone(),
        });
        snapshot
    }

    fn build(&self) -> Snapshot {
        let grid = self.term.grid();
        let (cols, rows) = (self.size.cols, self.size.rows);
        let offset = grid.display_offset();
        let colors = self.term.colors();
        let palette = &self.palette;
        let selection = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term));
        let mut lines = Vec::with_capacity(rows);
        let mut rtl = false;
        for row in 0..rows {
            let line = Line(row as i32 - offset as i32);
            let cells = &grid[line];
            let mut out = Row {
                selected: selection.and_then(|range| selected_span(&range, line, cols)),
                ..Row::default()
            };
            let mut open: Option<Run> = None;
            for col in 0..cols {
                let cell = &cells[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let width = if wide { 2 } else { 1 };
                let style = style_of(cell, colors, palette);
                if style.bg != palette.background {
                    match out.fills.last_mut() {
                        Some(last) if last.color == style.bg && last.col + last.cells == col => last.cells += width,
                        _ => out.fills.push(Fill {
                            col,
                            cells: width,
                            color: style.bg,
                        }),
                    }
                }
                let hidden = cell.flags.contains(Flags::HIDDEN);
                let ch = if cell.c == '\0' || hidden { ' ' } else { cell.c };
                rtl |= is_rtl_letter(ch);
                let decorated = style.underline != Underline::None || style.strike;
                let blank = ch == ' ' && !decorated;
                let same = open.as_ref().is_some_and(|run| {
                    run.fg == style.fg
                        && run.bold == style.bold
                        && run.italic == style.italic
                        && run.underline == style.underline
                        && run.underline_color == style.underline_color
                        && run.strike == style.strike
                        && run.wide == wide
                        && run.col + run.cells == col
                });
                if !same {
                    finish(&mut open, &mut out);
                    open = Some(Run {
                        col,
                        cells: 0,
                        text: String::new(),
                        fg: style.fg,
                        bold: style.bold,
                        italic: style.italic,
                        underline: style.underline,
                        underline_color: style.underline_color,
                        strike: style.strike,
                        wide,
                    });
                }
                if let Some(run) = open.as_mut() {
                    // A run does not start with a blank: the blank is just the gap before it.
                    if run.text.is_empty() && blank {
                        run.col = col + width;
                        continue;
                    }
                    run.text.push(ch);
                    if !hidden {
                        for extra in cell.zerowidth().into_iter().flatten() {
                            run.text.push(*extra);
                        }
                    }
                    run.cells += width;
                }
            }
            finish(&mut open, &mut out);
            lines.push(out);
        }
        let cursor = self.cursor_snapshot();
        Snapshot {
            cols,
            rows: lines,
            cursor,
            offset,
            history: grid.history_size(),
            mode: *self.term.mode(),
            rtl,
        }
    }

    fn cursor_snapshot(&self) -> Option<Cursor> {
        let content = self.term.renderable_content();
        let shape = content.cursor.shape;
        if shape == CursorShape::Hidden {
            return None;
        }
        let point = content.cursor.point;
        let offset = self.term.grid().display_offset() as i32;
        let row = point.line.0 + offset;
        if row < 0 || row as usize >= self.size.rows || point.column.0 >= self.size.cols {
            return None;
        }
        let cell = &self.term.grid()[point.line][point.column];
        let ch = if cell.c == '\0' { ' ' } else { cell.c };
        let mut text = String::new();
        text.push(ch);
        for extra in cell.zerowidth().into_iter().flatten() {
            text.push(*extra);
        }
        Some(Cursor {
            col: point.column.0,
            row: row as usize,
            shape,
            wide: cell.flags.contains(Flags::WIDE_CHAR),
            text,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            blinking: self.term.cursor_style().blinking,
        })
    }

    // ---- Links

    /// The cells of a logical line around `row` (a line the program wrapped is several rows): each character
    /// with the viewport cell it sits in, and the rows it spans.
    fn logical_line(&self, row: usize) -> Vec<(char, (usize, usize))> {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let wraps = |r: usize| {
            let line = Line(r as i32 - offset);
            self.size.cols > 0 && grid[line][Column(self.size.cols - 1)].flags.contains(Flags::WRAPLINE)
        };
        const MAX_ROWS: usize = 12;
        let mut first = row;
        while first > 0 && row - first < MAX_ROWS && wraps(first - 1) {
            first -= 1;
        }
        let mut chars = Vec::new();
        let mut at = first;
        loop {
            let line = &grid[Line(at as i32 - offset)];
            for col in 0..self.size.cols {
                let cell = &line[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                chars.push((if cell.c == '\0' { ' ' } else { cell.c }, (at, col)));
            }
            if at + 1 < self.size.rows && at - first < MAX_ROWS * 2 && wraps(at) {
                at += 1;
            } else {
                break;
            }
        }
        chars
    }

    /// The link under a cell: an OSC 8 hyperlink the program wrote, else a web address or Windows path found
    /// in the text around it.
    pub fn link_at(&self, row: usize, col: usize) -> Option<LinkSpan> {
        if row >= self.size.rows || col >= self.size.cols {
            return None;
        }
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let line = &grid[Line(row as i32 - offset)];
        // A hyperlink the program wrote: the run of cells that share it.
        if let Some(link) = line[Column(col)].hyperlink() {
            if let Some(target) = super::links::target_of_uri(link.uri()) {
                let (mut a, mut b) = (col, col);
                while a > 0 && line[Column(a - 1)].hyperlink().is_some_and(|h| h.id() == link.id()) {
                    a -= 1;
                }
                while b + 1 < self.size.cols && line[Column(b + 1)].hyperlink().is_some_and(|h| h.id() == link.id()) {
                    b += 1;
                }
                return Some(LinkSpan {
                    start: (row, a),
                    end: (row, b),
                    target,
                });
            }
            return None;
        }
        let chars = self.logical_line(row);
        let at = chars.iter().position(|(_, cell)| *cell == (row, col))?;
        let text: Vec<char> = chars.iter().map(|(c, _)| *c).collect();
        let found = super::links::find_links(&text)
            .into_iter()
            .find(|f| f.start <= at && at < f.end)?;
        Some(LinkSpan {
            start: chars.get(found.start)?.1,
            end: chars.get(found.end - 1)?.1,
            target: found.target,
        })
    }
}

/// A cell's style once colors are resolved.
struct Style {
    fg: u32,
    bg: u32,
    bold: bool,
    italic: bool,
    underline: Underline,
    underline_color: Option<u32>,
    strike: bool,
}

fn style_of(cell: &Cell, colors: &Colors, palette: &Palette) -> Style {
    let flags = cell.flags;
    let bold = flags.contains(Flags::BOLD);
    let mut fg = resolve(cell.fg, colors, palette, bold);
    let mut bg = resolve(cell.bg, colors, palette, false);
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::DIM) {
        fg = mix(fg, bg, 2. / 3.);
    }
    let underline = if flags.contains(Flags::UNDERCURL) {
        Underline::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        Underline::Single
    } else {
        Underline::None
    };
    Style {
        fg,
        bg,
        bold,
        italic: flags.contains(Flags::ITALIC),
        underline,
        underline_color: cell.underline_color().map(|c| resolve(c, colors, palette, false)),
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

/// Puts the run being built into the row, unless it holds nothing to draw.
fn finish(open: &mut Option<Run>, row: &mut Row) {
    let Some(mut run) = open.take() else { return };
    if run.underline == Underline::None && !run.strike {
        // Spaces at the end draw nothing.
        let kept = run.text.trim_end_matches(' ').len();
        let dropped = run.text.len() - kept;
        run.text.truncate(kept);
        run.cells = run.cells.saturating_sub(dropped);
    }
    if !run.text.is_empty() {
        row.runs.push(run);
    }
}

/// The selected columns on a line.
fn selected_span(range: &SelectionRange, line: Line, cols: usize) -> Option<(usize, usize)> {
    if cols == 0 || line < range.start.line || line > range.end.line {
        return None;
    }
    let last = cols - 1;
    let (a, b) = if range.is_block {
        (
            range.start.column.0.min(range.end.column.0),
            range.start.column.0.max(range.end.column.0),
        )
    } else {
        (
            if line == range.start.line {
                range.start.column.0
            } else {
                0
            },
            if line == range.end.line {
                range.end.column.0
            } else {
                last
            },
        )
    };
    let (a, b) = (a.min(last), b.min(last));
    (a <= b).then_some((a, b))
}

// ---- Shared handle ------------------------------------------------------------------------------------------

struct Inner {
    emu: Mutex<Emu>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// A screen shared between the thread that feeds it and the UI that draws it.
#[derive(Clone)]
pub struct Screen(Arc<Inner>);

impl Screen {
    /// A screen of `cols` x `rows` (at least 10 x 2) that keeps [`SCROLLBACK`] lines.
    pub fn new(cols: usize, rows: usize, hooks: Hooks) -> Self {
        Self::with_history(cols, rows, SCROLLBACK, hooks)
    }

    /// A screen that keeps no scrollback, as a recording is played (`scrollback: 0`).
    pub fn fixed(cols: usize, rows: usize, hooks: Hooks) -> Self {
        Self::with_history(cols, rows, 0, hooks)
    }

    fn with_history(cols: usize, rows: usize, history: usize, hooks: Hooks) -> Self {
        let wake = hooks.wake.clone();
        Screen(Arc::new(Inner {
            emu: Mutex::new(Emu::new(cols, rows, history, hooks)),
            wake,
        }))
    }

    /// Runs `f` with the screen locked. Keep it short: the shell's output waits for it.
    pub fn with<R>(&self, f: impl FnOnce(&mut Emu) -> R) -> R {
        f(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Emu> {
        // A panic while parsing leaves a usable grid; keep going with it.
        self.0.emu.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Parses output from the shell. Callable from any thread; wakes the UI when done.
    pub fn feed(&self, bytes: &[u8]) {
        self.lock().feed(bytes);
        (self.0.wake)();
    }

    /// Replaces what is on the screen with a fresh terminal of the same size that has been fed `bytes`.
    /// The parse runs here, on the calling thread, and the swap under the lock is instant: a cast player
    /// calls this from a background task to seek.
    pub fn replay(&self, bytes: &[&[u8]]) {
        let (cols, rows, history, hooks) = {
            let emu = self.lock();
            (emu.size.cols, emu.size.rows, emu.history, emu.listener.hooks.clone())
        };
        let mut fresh = Emu::new(cols, rows, history, hooks);
        for chunk in bytes {
            fresh.feed(chunk);
        }
        let mut emu = self.lock();
        fresh.palette = emu.palette.clone();
        fresh.palette_id = emu.palette_id;
        fresh.generation = emu.generation + 1;
        fresh
            .listener
            .cell
            .store(emu.listener.cell.load(Ordering::Relaxed), Ordering::Relaxed);
        *emu = fresh;
        drop(emu);
        (self.0.wake)();
    }

    /// Calls the wake hook (for things changed from the UI side that should redraw).
    pub fn wake(&self) {
        (self.0.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(cols: usize, rows: usize) -> Screen {
        Screen::new(cols, rows, Hooks::none())
    }

    fn snapshot(screen: &Screen) -> Arc<Snapshot> {
        screen.with(|emu| emu.snapshot())
    }

    #[test]
    fn plain_text_lands_in_cells() {
        let s = screen(20, 4);
        s.feed(b"hello\r\nworld");
        let snap = snapshot(&s);
        assert_eq!(snap.line_text(0), "hello");
        assert_eq!(snap.line_text(1), "world");
        assert_eq!(snap.line_text(2), "");
        let cursor = snap.cursor.clone().expect("a cursor");
        assert_eq!((cursor.col, cursor.row), (5, 1));
    }

    #[test]
    fn colors_bold_and_inverse_are_resolved_against_the_palette() {
        let s = screen(20, 3);
        let palette = Palette::new(true, 0x242424, 0xf2f2f2, 0xec4899);
        s.with(|e| e.set_palette(&palette, 1));
        // Red on default, then bold red (bright red), then inverse, then 24-bit, then 256-color.
        s.feed(b"\x1b[31mred\x1b[0m \x1b[1;31mbold\x1b[0m \x1b[7minv\x1b[0m\r\n\x1b[38;2;1;2;3mrgb\x1b[0m \x1b[38;5;196mx\x1b[0m");
        let snap = snapshot(&s);
        let run = |row: usize, text: &str| {
            snap.rows[row]
                .runs
                .iter()
                .find(|r| r.text == text)
                .cloned()
                .expect(text)
        };
        assert_eq!(run(0, "red").fg, DARK[1]);
        let bold = run(0, "bold");
        assert!(bold.bold);
        assert_eq!(bold.fg, DARK[9], "bold draws the dark colors bright");
        // Inverse swaps: the text is drawn in the background color on a foreground-colored fill.
        let inv = run(0, "inv");
        assert_eq!(inv.fg, palette.background);
        assert!(snap.rows[0]
            .fills
            .iter()
            .any(|f| f.color == palette.foreground && f.cells == 3));
        assert_eq!(run(1, "rgb").fg, 0x010203);
        assert_eq!(run(1, "x").fg, 0xff0000);
    }

    #[test]
    fn the_256_color_table_is_xterms() {
        let p = Palette::new(true, 0, 0xffffff, 0);
        assert_eq!(p.indexed(16), 0x000000);
        assert_eq!(p.indexed(21), 0x0000ff);
        assert_eq!(p.indexed(196), 0xff0000);
        assert_eq!(p.indexed(231), 0xffffff);
        assert_eq!(p.indexed(232), 0x080808);
        assert_eq!(p.indexed(255), 0xeeeeee);
        assert_eq!(p.indexed(4), DARK[4]);
    }

    #[test]
    fn backgrounds_merge_into_one_fill_and_blanks_draw_nothing() {
        let s = screen(20, 2);
        s.feed(b"\x1b[44m  ab  \x1b[0m");
        let snap = snapshot(&s);
        let blue = DARK[4];
        assert_eq!(
            snap.rows[0].fills,
            vec![Fill {
                col: 0,
                cells: 6,
                color: blue
            }]
        );
        // The leading blanks are a gap; the text starts at column 2.
        assert_eq!(snap.rows[0].runs.len(), 1);
        assert_eq!(
            (snap.rows[0].runs[0].col, snap.rows[0].runs[0].text.as_str()),
            (2, "ab")
        );
    }

    #[test]
    fn wide_characters_take_two_columns_and_their_own_run() {
        let s = screen(20, 2);
        s.feed("a漢字b".as_bytes());
        let snap = snapshot(&s);
        let runs = &snap.rows[0].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!((runs[0].col, runs[0].cells, runs[0].wide), (0, 1, false));
        assert_eq!(
            (runs[1].col, runs[1].cells, runs[1].wide, runs[1].text.as_str()),
            (1, 4, true, "漢字")
        );
        assert_eq!((runs[2].col, runs[2].text.as_str()), (5, "b"));
    }

    #[test]
    fn arabic_is_noticed_so_the_pane_can_say_terminals_cannot_order_it() {
        let s = screen(30, 2);
        s.feed("مرحبا".as_bytes());
        assert!(snapshot(&s).rtl);
        let s = screen(30, 2);
        s.feed(b"hello");
        assert!(!snapshot(&s).rtl);
    }

    #[test]
    fn scrollback_holds_what_scrolled_off_and_the_view_moves_through_it() {
        let s = screen(20, 3);
        for n in 0..10 {
            s.feed(format!("line {n}\r\n").as_bytes());
        }
        s.with(|e| {
            let (offset, history) = e.scroll_position();
            assert_eq!(offset, 0);
            assert!(history >= 8);
            assert!(e.scroll_by(3));
            assert_eq!(e.scroll_position().0, 3);
            assert!(e.scroll_to_bottom());
            assert!(!e.scroll_to_bottom(), "already there");
            assert!(e.scroll_to_top());
            assert_eq!(e.visible_text()[0], "line 0");
        });
        let snap = snapshot(&s);
        assert!(snap.offset > 0);
        assert!(snap.cursor.is_none() || snap.cursor.as_ref().is_some_and(|c| c.row < 3));
    }

    #[test]
    fn selection_copies_the_text_it_covers() {
        let s = screen(20, 3);
        s.feed(b"hello world\r\nsecond line");
        s.with(|e| {
            e.start_selection(Pick::Cells, 0, 0, false);
            e.update_selection(4, 0, true);
            assert_eq!(e.selection_text().as_deref(), Some("hello"));
            // A double click selects a word.
            e.start_selection(Pick::Word, 8, 0, false);
            assert_eq!(e.selection_text().as_deref(), Some("world"));
            e.start_selection(Pick::Lines, 3, 1, false);
            assert_eq!(e.selection_text().as_deref(), Some("second line\n"));
            assert!(e.clear_selection());
            assert!(e.selection_text().is_none());
        });
    }

    #[test]
    fn the_selection_is_in_the_snapshot_as_columns_per_row() {
        let s = screen(20, 3);
        s.feed(b"abcdef\r\nghijkl\r\nmnopqr");
        s.with(|e| {
            e.start_selection(Pick::Cells, 3, 0, false);
            e.update_selection(1, 2, true);
        });
        let snap = snapshot(&s);
        assert_eq!(snap.rows[0].selected, Some((3, 19)));
        assert_eq!(snap.rows[1].selected, Some((0, 19)));
        assert_eq!(snap.rows[2].selected, Some((0, 1)));
        s.with(|e| {
            e.clear_selection();
            e.start_selection(Pick::Block, 2, 0, false);
            e.update_selection(4, 1, true);
        });
        let snap = snapshot(&s);
        assert_eq!(snap.rows[0].selected, Some((2, 4)));
        assert_eq!(snap.rows[1].selected, Some((2, 4)));
        assert_eq!(snap.rows[2].selected, None);
    }

    #[test]
    fn a_resize_reflows_and_keeps_the_text() {
        let s = screen(20, 4);
        s.feed(b"abcdefghijklmnopqrst0123456789");
        s.with(|e| {
            assert!(e.resize(40, 4));
            assert!(!e.resize(40, 4), "same size");
            assert_eq!(e.size(), (40, 4));
            assert_eq!(e.visible_text()[0], "abcdefghijklmnopqrst0123456789");
            // Never smaller than the minimum.
            e.resize(1, 1);
            assert_eq!(e.size(), (MIN_COLS, MIN_ROWS));
        });
    }

    #[test]
    fn a_program_that_asks_gets_answers_through_the_write_hook() {
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = sent.clone();
        let hooks = Hooks {
            write: Arc::new(move |text| log.lock().unwrap().push(text)),
            ..Hooks::none()
        };
        let s = Screen::new(20, 4, hooks);
        s.with(|e| e.set_palette(&Palette::new(true, 0x112233, 0xeeeeee, 0xec4899), 1));
        // Cursor position report after printing "ab", device attributes, and "what's your background?".
        s.feed(b"ab\x1b[6n\x1b[c\x1b]11;?\x07");
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.first().map(String::as_str), Some("\x1b[1;3R"));
        assert!(
            sent.iter().any(|t| t.starts_with("\x1b[?")),
            "device attributes: {sent:?}"
        );
        assert!(
            sent.iter().any(|t| t.contains("rgb:1111/2222/3333")),
            "background color: {sent:?}"
        );
    }

    #[test]
    fn the_title_and_the_clipboard_are_signalled() {
        let seen = Arc::new(Mutex::new(Vec::<Signal>::new()));
        let log = seen.clone();
        let hooks = Hooks {
            signal: Arc::new(move |signal| log.lock().unwrap().push(signal)),
            ..Hooks::none()
        };
        let s = Screen::new(20, 4, hooks);
        s.feed(b"\x1b]0;my shell\x07\x1b]52;c;aGk=\x07\x07");
        let seen = seen.lock().unwrap().clone();
        assert!(seen.contains(&Signal::Title(Some("my shell".into()))), "{seen:?}");
        assert!(seen.contains(&Signal::Clipboard("hi".into())), "{seen:?}");
        assert!(seen.contains(&Signal::Bell), "{seen:?}");
    }

    #[test]
    fn modes_a_program_sets_are_visible() {
        let s = screen(20, 4);
        assert!(!s.with(|e| e.mode()).contains(TermMode::BRACKETED_PASTE));
        s.feed(b"\x1b[?2004h\x1b[?1h\x1b[?1000h\x1b[?1006h");
        let mode = s.with(|e| e.mode());
        assert!(mode.contains(TermMode::BRACKETED_PASTE));
        assert!(mode.contains(TermMode::APP_CURSOR));
        assert!(mode.contains(TermMode::MOUSE_REPORT_CLICK));
        assert!(mode.contains(TermMode::SGR_MOUSE));
        s.feed(b"\x1b[?1049h");
        assert!(s.with(|e| e.mode()).contains(TermMode::ALT_SCREEN));
    }

    #[test]
    fn an_unfinished_synchronized_update_shows_after_its_timeout() {
        let s = screen(20, 3);
        // Begin synchronized update, print, never end it.
        s.feed(b"\x1b[?2026hpending");
        s.with(|e| {
            assert_eq!(e.visible_text()[0], "", "held back");
            let wait = e.sync_wait().expect("a pending update");
            assert!(wait <= Duration::from_millis(200));
        });
        std::thread::sleep(Duration::from_millis(220));
        s.with(|e| {
            assert!(e.flush_sync());
            assert_eq!(e.visible_text()[0], "pending");
            assert!(e.sync_wait().is_none());
        });
    }

    #[test]
    fn hyperlinks_and_addresses_on_screen_are_found() {
        let s = screen(60, 4);
        s.feed(b"see https://example.com/a?b=1, or \x1b]8;;https://glint.rafd.tech\x1b\\the site\x1b]8;;\x1b\\ now\r\nC:\\Users\\me\\a.txt:12 end");
        s.with(|e| {
            let url = e.link_at(0, 10).expect("a url");
            assert_eq!(url.target, LinkTarget::Url("https://example.com/a?b=1".into()));
            assert_eq!((url.start, url.end), ((0, 4), (0, 28)));
            let osc = e.link_at(0, 37).expect("an OSC 8 link");
            assert_eq!(osc.target, LinkTarget::Url("https://glint.rafd.tech".into()));
            assert!(e.link_at(0, 1).is_none(), "plain words are not links");
            let path = e.link_at(1, 3).expect("a path");
            assert_eq!(path.target, LinkTarget::Path("C:\\Users\\me\\a.txt".into()));
        });
    }

    #[test]
    fn a_url_the_program_wrapped_is_one_link() {
        let s = screen(20, 4);
        s.feed(b"go https://example.com/a/long/path/here now");
        s.with(|e| {
            let link = e.link_at(1, 2).expect("the wrapped part is the link too");
            assert_eq!(
                link.target,
                LinkTarget::Url("https://example.com/a/long/path/here".into())
            );
            // Starts at column 3 of the first row, ends at column 18 of the second.
            assert_eq!((link.start, link.end), ((0, 3), (1, 18)));
            assert_eq!(link.on_row(0, 20), Some((3, 19)));
            assert_eq!(link.on_row(1, 20), Some((0, 18)));
            assert_eq!(link.on_row(3, 20), None);
        });
    }

    #[test]
    fn replaying_swaps_in_a_screen_fed_from_the_start() {
        let s = screen(20, 3);
        s.feed(b"old stuff");
        s.replay(&[b"new ", b"stuff"]);
        s.with(|e| assert_eq!(e.visible_text()[0], "new stuff"));
    }

    #[test]
    fn the_snapshot_is_reused_until_the_screen_changes() {
        let s = screen(20, 3);
        s.feed(b"one");
        let a = snapshot(&s);
        let b = snapshot(&s);
        assert!(Arc::ptr_eq(&a, &b));
        s.feed(b"two");
        assert!(!Arc::ptr_eq(&a, &snapshot(&s)));
    }

    #[test]
    fn feeding_megabytes_stays_fast_and_scrollback_is_bounded() {
        let s = screen(120, 30);
        let line = "x".repeat(110) + "\r\n";
        let chunk = line.repeat(1000);
        let started = Instant::now();
        for _ in 0..40 {
            s.feed(chunk.as_bytes());
        }
        let took = started.elapsed();
        s.with(|e| assert!(e.scroll_position().1 <= SCROLLBACK));
        // 4.5 MB: even a debug build parses it well inside a few seconds.
        assert!(took < Duration::from_secs(20), "{took:?}");
    }
}
