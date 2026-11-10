//! Right-to-left paragraphs that wrap, through GPUI's text system with the real DirectWrite behind it.
//!
//! The headless test platform shapes with a stand-in that knows nothing of bidirectional text, so these
//! tests start the real platform once (no window is opened: the text system is all they need), shape
//! every paragraph inside that one run, and keep plain numbers to check. Every `#[test]` reads from
//! the same run.
//!
//! What they pin down (vendor/gpui-pre-0.3.12, text_system/line_layout.rs, `layout_rtl_rows`): the
//! break points of a right-to-left paragraph come in reading order, never inside a word; every row is
//! laid out as a line of its own, so it is as wide as its words and no wider than the wrap width; the
//! row's first word is on its right; and the hit testing (`index_for_position`,
//! `closest_index_for_position`, `position_for_index`) agrees with what is drawn.

use gpui_kit::{point, px, Font, FontWeight, Hsla, SharedString, TextRun, WindowTextSystem};
use std::sync::OnceLock;

const LINE_HEIGHT: f32 = 20.;
const FONT_SIZE: f32 = 16.;

/// A long Arabic paragraph, ending in a full stop.
const ARABIC: &str = "هذا نص عربي طويل يحتوي على عدة كلمات لكي نختبر التفاف الأسطر عندما يكون العرض محدودا في معاينة المستندات وما شابه ذلك من العروض.";

/// An Arabic paragraph with English words and a file name inside it.
const MIXED: &str = "يمكنك فتح ملف Word أو ملف PDF مباشرة من Glint ثم قراءة الفقرة العربية الطويلة مع بعض الكلمات English words داخل النص لكي نختبر الاتجاه.";

/// An English paragraph with an Arabic phrase inside it.
const ENGLISH_WITH_ARABIC: &str = "Open the file report-final.docx and then read the فقرة عربية طويلة inside an English sentence that wraps over several lines.";

/// A plain English paragraph.
const ENGLISH: &str = "This is an English paragraph that is long enough to wrap onto several lines, and it has to wrap exactly the way it always did.";

/// A short Arabic sentence that fits in one row.
const SHORT: &str = "جملة قصيرة.";

/// One glyph of a row: the text index it shows, and where it is on the (concatenated) line.
#[derive(Clone, Debug)]
struct Glyph {
    index: usize,
    x: f32,
}

/// One row of a shaped paragraph.
#[derive(Clone, Debug)]
struct Row {
    /// The row's glyphs in the order they are painted, which is left to right on screen.
    glyphs: Vec<Glyph>,
    /// Where the row starts and ends on the line.
    left: f32,
    right: f32,
    /// The text it shows: from the lowest index on it to the next row's.
    start: usize,
    end: usize,
}

/// Where a click lands: `index_for_position`'s or `closest_index_for_position`'s answer.
type Hit = Result<usize, usize>;

/// What a test wants to know about a shaped paragraph; plain numbers.
#[derive(Debug)]
struct Shaped {
    text: &'static str,
    wrap_width: f32,
    rows: Vec<Row>,
    /// `WrappedLine::width()` and `size(line_height)`.
    width: f32,
    height: f32,
    /// Every glyph of the paragraph shaped as one unwrapped line (`wrap_width` of `None`).
    unwrapped: Vec<Glyph>,
    /// `position_for_index` for every character boundary and the end.
    positions: Vec<(usize, Option<(f32, f32)>)>,
    /// A click on the middle of each glyph of each row: (row, glyph's index, `index_for_position`,
    /// `closest_index_for_position`).
    clicks: Vec<(usize, usize, Hit, Hit)>,
    /// A click 5 px past each end of each row: (row, left, right), each `closest_index_for_position`.
    edge_clicks: Vec<(usize, Hit, Hit)>,
}

impl Shaped {
    fn row_text(&self, row: &Row) -> &'static str {
        &self.text[row.start..row.end]
    }

    /// The glyphs of a row that show something: the spaces that hang at the end of a row have no width.
    fn content<'a>(&self, row: &'a Row) -> Vec<&'a Glyph> {
        row.glyphs
            .iter()
            .filter(|glyph| !self.text[glyph.index..].starts_with(' '))
            .collect()
    }
}

fn run(font_weight: FontWeight, len: usize) -> TextRun {
    TextRun {
        len,
        font: Font {
            family: "Segoe UI".into(),
            weight: font_weight,
            ..Default::default()
        },
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Shapes `text` wrapped at `wrap_width` (in one run, or with the middle of it in bold) and keeps what
/// the tests need.
fn shape(
    system: &WindowTextSystem,
    text: &'static str,
    wrap_width: f32,
    line_clamp: Option<usize>,
    bold_middle: bool,
) -> Shaped {
    let runs = if bold_middle {
        let mut a = text.len() / 3;
        while !text.is_char_boundary(a) {
            a += 1;
        }
        let mut b = 2 * text.len() / 3;
        while !text.is_char_boundary(b) {
            b += 1;
        }
        vec![
            run(FontWeight::NORMAL, a),
            run(FontWeight::BOLD, b - a),
            run(FontWeight::NORMAL, text.len() - b),
        ]
    } else {
        vec![run(FontWeight::NORMAL, text.len())]
    };
    let lines = system
        .shape_text(
            SharedString::from(text),
            px(FONT_SIZE),
            &runs,
            Some(px(wrap_width)),
            line_clamp,
        )
        .expect("the paragraph shapes");
    assert_eq!(lines.len(), 1, "one paragraph is one line of text");
    let line = &lines[0];
    let layout = &line.unwrapped_layout;
    let line_height = px(LINE_HEIGHT);

    // The rows: the glyphs between wrap boundaries.
    let mut rows: Vec<Row> = Vec::new();
    let mut boundaries = line.wrap_boundaries.iter().peekable();
    let mut glyphs: Vec<Glyph> = Vec::new();
    for (run_ix, run) in layout.runs.iter().enumerate() {
        for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
            if boundaries
                .peek()
                .is_some_and(|b| b.run_ix == run_ix && b.glyph_ix == glyph_ix)
            {
                boundaries.next();
                rows.push(Row {
                    glyphs: std::mem::take(&mut glyphs),
                    left: 0.,
                    right: 0.,
                    start: 0,
                    end: 0,
                });
            }
            glyphs.push(Glyph {
                index: glyph.index,
                x: f32::from(glyph.position.x),
            });
        }
    }
    rows.push(Row {
        glyphs,
        left: 0.,
        right: 0.,
        start: 0,
        end: 0,
    });
    let total = f32::from(layout.width);
    let lefts: Vec<f32> = rows
        .iter()
        .enumerate()
        .map(|(ix, row)| if ix == 0 { 0. } else { row.glyphs[0].x })
        .collect();
    let starts: Vec<usize> = rows
        .iter()
        .enumerate()
        .map(|(ix, row)| {
            if ix == 0 {
                0
            } else {
                row.glyphs.iter().map(|glyph| glyph.index).min().unwrap()
            }
        })
        .collect();
    let row_count = rows.len();
    for (ix, row) in rows.iter_mut().enumerate() {
        row.left = lefts[ix];
        row.right = lefts.get(ix + 1).copied().unwrap_or(total);
        row.start = starts[ix];
        row.end = starts.get(ix + 1).copied().unwrap_or(text.len());
    }

    let unwrapped = {
        let lines = system
            .shape_text(SharedString::from(text), px(FONT_SIZE), &runs, None, None)
            .expect("the paragraph shapes unwrapped");
        lines[0]
            .unwrapped_layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter())
            .map(|glyph| Glyph {
                index: glyph.index,
                x: f32::from(glyph.position.x),
            })
            .collect()
    };

    let mut positions = Vec::new();
    for (ix, _) in text.char_indices() {
        positions.push((
            ix,
            line.position_for_index(ix, line_height)
                .map(|p| (f32::from(p.x), f32::from(p.y))),
        ));
    }
    positions.push((
        text.len(),
        line.position_for_index(text.len(), line_height)
            .map(|p| (f32::from(p.x), f32::from(p.y))),
    ));

    let mut clicks = Vec::new();
    let mut edge_clicks = Vec::new();
    for (row_ix, row) in rows.iter().enumerate() {
        let y = px(row_ix as f32 * LINE_HEIGHT + LINE_HEIGHT / 2.);
        // Spaces inside the row are glyphs to click on; the ones that hang at its end have no width
        // and are skipped below.
        let mut sorted: Vec<&Glyph> = row.glyphs.iter().collect();
        sorted.sort_by(|a, b| a.x.total_cmp(&b.x));
        for (ix, glyph) in sorted.iter().enumerate() {
            let next = sorted.get(ix + 1).map_or(row.right, |next| next.x);
            if next - glyph.x < 1. {
                continue;
            }
            let x = px((glyph.x + next) / 2. - row.left);
            clicks.push((
                row_ix,
                glyph.index,
                line.index_for_position(point(x, y), line_height),
                line.closest_index_for_position(point(x, y), line_height),
            ));
        }
        let width = row.right - row.left;
        edge_clicks.push((
            row_ix,
            line.closest_index_for_position(point(px(-5.), y), line_height),
            line.closest_index_for_position(point(px(width + 5.), y), line_height),
        ));
    }
    assert_eq!(rows.len(), row_count);

    Shaped {
        text,
        wrap_width,
        rows,
        width: f32::from(line.width()),
        height: f32::from(line.size(line_height).height),
        unwrapped,
        positions,
        clicks,
        edge_clicks,
    }
}

struct Paragraphs {
    arabic: Shaped,
    arabic_bold: Shaped,
    arabic_clamped: Shaped,
    arabic_wide: Shaped,
    mixed: Shaped,
    english_with_arabic: Shaped,
    english: Shaped,
    short: Shaped,
}

/// Starts the real platform once, shapes every paragraph, and shuts it down again.
fn paragraphs() -> &'static Paragraphs {
    static PARAGRAPHS: OnceLock<Paragraphs> = OnceLock::new();
    PARAGRAPHS.get_or_init(|| {
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = result.clone();
        gpui_kit::application().run(move |cx| {
            let system = WindowTextSystem::new(cx.text_system().clone());
            *slot.lock().unwrap() = Some(Paragraphs {
                arabic: shape(&system, ARABIC, 200., None, false),
                arabic_bold: shape(&system, ARABIC, 200., None, true),
                // The layout cache's key has no line clamp, so a clamped layout needs a width of its own.
                arabic_clamped: shape(&system, ARABIC, 201., Some(2), false),
                arabic_wide: shape(&system, ARABIC, 2000., None, false),
                mixed: shape(&system, MIXED, 220., None, false),
                english_with_arabic: shape(&system, ENGLISH_WITH_ARABIC, 220., None, false),
                english: shape(&system, ENGLISH, 200., None, false),
                short: shape(&system, SHORT, 200., None, false),
            });
            cx.quit();
        });
        let paragraphs = result.lock().unwrap().take();
        paragraphs.expect("the platform ran and shaped the paragraphs")
    })
}

/// A row never starts inside a word: the text before it ends in a space (or a hyphen).
fn assert_breaks_between_words(shaped: &Shaped) {
    for row in &shaped.rows[1..] {
        let before = shaped.text[..row.start].chars().next_back().unwrap();
        assert!(
            before == ' ' || before == '-',
            "row {:?} starts inside a word (after {before:?}) in {:?}",
            shaped.row_text(row),
            shaped.text
        );
    }
}

/// The rows together are the paragraph, in order, nothing lost or twice.
fn assert_rows_cover_text(shaped: &Shaped) {
    assert_eq!(shaped.rows[0].start, 0);
    assert_eq!(shaped.rows.last().unwrap().end, shaped.text.len());
    for pair in shaped.rows.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
        assert!(pair[0].start < pair[0].end);
    }
    let joined: String = shaped.rows.iter().map(|row| shaped.row_text(row)).collect();
    assert_eq!(joined, shaped.text);
    // Every glyph belongs to the row that shows its text.
    for row in &shaped.rows {
        for glyph in &row.glyphs {
            assert!(
                (row.start..row.end).contains(&glyph.index),
                "glyph {} is not in row {}..{}",
                glyph.index,
                row.start,
                row.end
            );
        }
    }
}

/// Every row is as wide as its words and no wider than the wrap width.
fn assert_rows_fit(shaped: &Shaped) {
    for row in &shaped.rows {
        let width = row.right - row.left;
        assert!(
            width <= shaped.wrap_width + 0.1,
            "row {:?} is {width} px wide, the wrap width is {}",
            shaped.row_text(row),
            shaped.wrap_width
        );
        assert!(width > 1., "row {:?} has no width", shaped.row_text(row));
    }
}

/// Rows are laid out left to right: the first glyph painted is at the row's start, and glyphs go up.
fn assert_rows_are_left_to_right(shaped: &Shaped) {
    for row in &shaped.rows {
        assert!((row.glyphs[0].x - row.left).abs() < 0.5, "{row:?}");
        for pair in row.glyphs.windows(2) {
            assert!(pair[0].x <= pair[1].x + 0.01, "{pair:?} in {row:?}");
        }
        for glyph in &row.glyphs {
            assert!(
                glyph.x >= row.left - 0.5 && glyph.x <= row.right + 0.5,
                "{glyph:?} is outside the row {}..{}",
                row.left,
                row.right
            );
        }
    }
}

#[test]
fn a_long_arabic_paragraph_wraps_into_rows_between_words() {
    let shaped = &paragraphs().arabic;
    assert!(
        shaped.rows.len() >= 4,
        "{} rows for {} px of text",
        shaped.rows.len(),
        shaped.unwrapped.last().map_or(0., |glyph| glyph.x)
    );
    assert_eq!(shaped.height, shaped.rows.len() as f32 * LINE_HEIGHT);
    assert_breaks_between_words(shaped);
    assert_rows_cover_text(shaped);
    assert_rows_fit(shaped);
    assert_rows_are_left_to_right(shaped);
    // A wrapped paragraph is as wide as the room it was given.
    assert_eq!(shaped.width, shaped.wrap_width);
}

#[test]
fn the_first_row_holds_the_first_words_and_they_are_on_its_right() {
    let shaped = &paragraphs().arabic;
    let first = &shaped.rows[0];
    assert!(
        shaped.row_text(first).starts_with("هذا نص عربي"),
        "the first row is {:?}",
        shaped.row_text(first)
    );
    // In a right-to-left row the first character is at the right: it is the right-most glyph with a
    // width, and the row's last words are on the left.
    let content = shaped.content(first);
    let rightmost = content.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(rightmost.index, 0, "the first letter is not at the right end");
    let leftmost = content.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    let last_word_start = shaped.row_text(first).trim_end().rfind(' ').map(|ix| ix + 1).unwrap();
    assert!(
        leftmost.index >= last_word_start,
        "the left end of the row is {} but its last word starts at {last_word_start}",
        leftmost.index
    );
    // The same goes for every row: its first character is at its right end.
    for row in &shaped.rows {
        let content = shaped.content(row);
        let rightmost = content.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        assert_eq!(rightmost.index, row.start, "{:?}", shaped.row_text(row));
    }
}

#[test]
fn the_full_stop_ends_the_last_row_on_its_left() {
    let shaped = &paragraphs().arabic;
    let last = shaped.rows.last().unwrap();
    let full_stop = shaped.text.len() - 1;
    let content = shaped.content(last);
    let leftmost = content.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(leftmost.index, full_stop, "the full stop is not at the left end");
}

#[test]
fn a_short_arabic_sentence_stays_one_row_and_reads_from_the_right() {
    let shaped = &paragraphs().short;
    assert_eq!(shaped.rows.len(), 1);
    let content = shaped.content(&shaped.rows[0]);
    let rightmost = content.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(rightmost.index, 0);
    let leftmost = content.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(leftmost.index, shaped.text.len() - 1, "the full stop goes left");
    // Wrapping didn't change a thing about its width.
    assert!(shaped.width <= shaped.wrap_width);
    assert!((shaped.rows[0].right - shaped.width).abs() < 0.5);
}

#[test]
fn a_paragraph_that_fits_is_not_split() {
    let shaped = &paragraphs().arabic_wide;
    assert_eq!(shaped.rows.len(), 1);
    assert_eq!(shaped.height, LINE_HEIGHT);
    assert_rows_cover_text(shaped);
}

#[test]
fn styled_runs_do_not_change_the_breaks_of_a_paragraph() {
    let shaped = &paragraphs().arabic_bold;
    assert!(shaped.rows.len() >= 4);
    assert_breaks_between_words(shaped);
    assert_rows_cover_text(shaped);
    assert_rows_fit(shaped);
    assert_rows_are_left_to_right(shaped);
    // Bold is wider, so there can only be as many rows or more.
    assert!(shaped.rows.len() >= paragraphs().arabic.rows.len());
}

#[test]
fn line_clamp_stops_the_breaks_where_it_does_for_left_to_right_text() {
    let shaped = &paragraphs().arabic_clamped;
    assert_eq!(shaped.rows.len(), 2, "a clamp of 2 lines is 2 rows");
    assert_rows_cover_text(shaped);
    assert_breaks_between_words(shaped);
    // The first row is the same as it is without the clamp.
    assert_eq!(shaped.rows[0].end, paragraphs().arabic.rows[0].end);
}

#[test]
fn a_mixed_arabic_and_english_paragraph_wraps_between_words() {
    let shaped = &paragraphs().mixed;
    assert!(shaped.rows.len() >= 3, "{} rows", shaped.rows.len());
    assert_breaks_between_words(shaped);
    assert_rows_cover_text(shaped);
    assert_rows_fit(shaped);
    assert_rows_are_left_to_right(shaped);

    // The first row has the first words, at its right.
    let first = &shaped.rows[0];
    assert!(
        shaped.row_text(first).starts_with("يمكنك فتح ملف"),
        "{:?}",
        shaped.row_text(first)
    );
    let content = shaped.content(first);
    let rightmost = content.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(rightmost.index, 0);

    // An English word keeps its place in the reading order of a right-to-left paragraph: in the row
    // that has "English words", it is to the right of the Arabic word that comes after it.
    let english = shaped.text.find("English").unwrap();
    let after = shaped.text.find("داخل").unwrap();
    let row = shaped
        .rows
        .iter()
        .find(|row| (row.start..row.end).contains(&english))
        .unwrap();
    // "داخل" may be on the next row; then there is nothing to compare, but the row is still in order.
    if (row.start..row.end).contains(&after) {
        let x_of = |index: usize| row.glyphs.iter().find(|glyph| glyph.index == index).unwrap().x;
        assert!(x_of(english) > x_of(after), "{:?}", shaped.row_text(row));
    }
    // And the Latin letters of a word run left to right.
    let word = &shaped.text[english..english + "English".len()];
    let xs: Vec<f32> = (english..english + word.len())
        .filter_map(|index| {
            shaped
                .rows
                .iter()
                .flat_map(|row| &row.glyphs)
                .find(|glyph| glyph.index == index)
        })
        .map(|glyph| glyph.x)
        .collect();
    assert_eq!(xs.len(), "English".len());
    assert!(xs.windows(2).all(|pair| pair[0] < pair[1]), "{xs:?}");
}

#[test]
fn an_english_paragraph_with_arabic_in_it_wraps_between_words_and_reads_left_to_right() {
    let shaped = &paragraphs().english_with_arabic;
    assert!(shaped.rows.len() >= 3);
    assert_breaks_between_words(shaped);
    assert_rows_cover_text(shaped);
    assert_rows_fit(shaped);
    assert_rows_are_left_to_right(shaped);
    // The first row starts at the left with the first letter.
    let content = shaped.content(&shaped.rows[0]);
    let leftmost = content.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert_eq!(leftmost.index, 0);
}

#[test]
fn an_english_paragraph_wraps_exactly_as_before() {
    let shaped = &paragraphs().english;
    assert!(shaped.rows.len() >= 3);
    assert_breaks_between_words(shaped);
    assert_rows_cover_text(shaped);
    assert_rows_fit(shaped);
    // Left-to-right text keeps the one line's layout: the wrapped glyphs sit exactly where they do
    // in the unwrapped line, and the rows are cut out of it at the words.
    let wrapped: Vec<(usize, f32)> = shaped
        .rows
        .iter()
        .flat_map(|row| row.glyphs.iter().map(|glyph| (glyph.index, glyph.x)))
        .collect();
    let unwrapped: Vec<(usize, f32)> = shaped.unwrapped.iter().map(|glyph| (glyph.index, glyph.x)).collect();
    assert_eq!(wrapped.len(), unwrapped.len());
    for (a, b) in wrapped.iter().zip(&unwrapped) {
        assert_eq!(a.0, b.0);
        assert!((a.1 - b.1).abs() < 0.01, "{a:?} against {b:?}");
    }
    // Rows fill: each one would not have fitted the next word.
    for pair in shaped.rows.windows(2) {
        // Left-to-right wrapping counts the space after a word with it.
        let next_word_end = pair[1].start
            + shaped
                .row_text(&pair[1])
                .find(' ')
                .map_or(pair[1].end - pair[1].start, |space| space + 1);
        let first_x = |index: usize| shaped.unwrapped.iter().find(|glyph| glyph.index >= index).unwrap().x;
        let end_x = shaped
            .unwrapped
            .iter()
            .find(|glyph| glyph.index >= next_word_end)
            .map_or(shaped.rows.last().unwrap().right, |glyph| glyph.x);
        assert!(
            end_x - first_x(pair[0].start) > shaped.wrap_width,
            "row {:?} had room for {:?}",
            shaped.row_text(&pair[0]),
            &shaped.text[pair[1].start..next_word_end]
        );
    }
}

#[test]
fn the_caret_is_where_the_text_is_drawn() {
    for shaped in [&paragraphs().arabic, &paragraphs().mixed] {
        for (index, position) in &shaped.positions {
            let (x, y) = position.unwrap_or_else(|| panic!("no position for {index}"));
            let row_ix = (y / LINE_HEIGHT) as usize;
            let row = &shaped.rows[row_ix];
            // The index is on the row it is shown on...
            assert!(
                (row.start..row.end).contains(index) || *index == shaped.text.len(),
                "index {index} on row {row_ix} ({}..{})",
                row.start,
                row.end
            );
            // ... and within it, from the row's left edge.
            assert!(
                x >= -0.5 && x <= row.right - row.left + 0.5,
                "index {index}: x {x} is outside the row (0..{})",
                row.right - row.left
            );
        }
        // The start of a right-to-left row is its right edge; the end of the text is the last row's
        // left edge.
        for row_ix in 0..shaped.rows.len() {
            let row = &shaped.rows[row_ix];
            let (x, y) = shaped
                .positions
                .iter()
                .find(|(index, _)| *index == row.start)
                .unwrap()
                .1
                .unwrap();
            assert_eq!(y, row_ix as f32 * LINE_HEIGHT);
            // A row that starts with an Arabic letter starts at its right edge (one that starts with
            // an English word starts at the left edge of that word).
            let first = shaped.text[row.start..].chars().next().unwrap();
            if ('\u{0600}'..='\u{06FF}').contains(&first) {
                assert!(
                    (x - (row.right - row.left)).abs() < 1.5,
                    "row {row_ix} starts at x {x}, its right edge is {}",
                    row.right - row.left
                );
            }
        }
        let (x_end, y_end) = shaped.positions.last().unwrap().1.unwrap();
        assert_eq!(y_end, (shaped.rows.len() - 1) as f32 * LINE_HEIGHT);
        assert!(x_end < 1.5, "the end of the text is at x {x_end}");
    }
}

#[test]
fn a_click_on_a_glyph_finds_its_character() {
    for shaped in [
        &paragraphs().arabic,
        &paragraphs().arabic_bold,
        &paragraphs().mixed,
        &paragraphs().english_with_arabic,
    ] {
        assert!(!shaped.clicks.is_empty());
        for (row, glyph_index, found, closest) in &shaped.clicks {
            assert_eq!(
                found,
                &Ok(*glyph_index),
                "row {row}: a click on the glyph of {glyph_index} in {:?}",
                shaped.text
            );
            // The closest caret to the middle of a glyph is one of its two edges.
            let glyph_end = shaped.text[*glyph_index..]
                .chars()
                .next()
                .map_or(*glyph_index, |c| glyph_index + c.len_utf8());
            let closest = closest.unwrap_or_else(|e| e);
            assert!(
                closest == *glyph_index || closest == glyph_end,
                "row {row}: a click on {glyph_index} chose the caret at {closest}"
            );
        }
    }
}

#[test]
fn a_click_past_the_end_of_a_row_chooses_the_caret_at_that_end() {
    let shaped = &paragraphs().arabic;
    for (row_ix, left, right) in &shaped.edge_clicks {
        let row = &shaped.rows[*row_ix];
        // Right-to-left: the row's start is on its right, its end is on its left.
        assert_eq!(right, &Err(row.start), "row {row_ix} at its right edge");
        let on_the_left = (*left).unwrap_or_else(|e| e);
        // The caret at the left edge follows the row's last letter: the trailing space on a row that
        // ends in one, the next row's start only for the last row (the end of the text).
        let end_of_visible = shaped.row_text(row).trim_end_matches(' ').len() + row.start;
        assert!(
            on_the_left == end_of_visible || (*row_ix + 1 == shaped.rows.len() && on_the_left == shaped.text.len()),
            "row {row_ix}: the left edge chose {on_the_left}, the row's text ends at {end_of_visible}"
        );
    }
}
