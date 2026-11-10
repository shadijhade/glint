use crate::{FontId, GlyphId, Pixels, PlatformTextSystem, Point, SharedString, Size, point, px};
use collections::{FxHashMap, FxHashSet};
use itertools::Itertools;
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;
use std::{
    borrow::Borrow,
    cell::RefCell,
    hash::{Hash, Hasher},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::LineWrapper;

/// A laid out and styled line of text
#[derive(Default, Debug)]
pub struct LineLayout {
    /// The font size for this line
    pub font_size: Pixels,
    /// The width of the line
    pub width: Pixels,
    /// The ascent of the line
    pub ascent: Pixels,
    /// The descent of the line
    pub descent: Pixels,
    /// The shaped runs that make up this line
    pub runs: Vec<ShapedRun>,
    /// The length of the line in utf-8 bytes
    pub len: usize,
}

/// A run of text that has been shaped .
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// The font id for this run
    pub font_id: FontId,
    /// The glyphs that make up this run
    pub glyphs: Vec<ShapedGlyph>,
}

/// A single glyph, ready to paint.
#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    /// The ID for this glyph, as determined by the text system.
    pub id: GlyphId,

    /// The position of this glyph in its containing line.
    pub position: Point<Pixels>,

    /// The index of this glyph in the original text.
    pub index: usize,

    /// Whether this glyph is an emoji
    pub is_emoji: bool,
}

/// Glyphs that share a text index, as `LineLayout`'s right-to-left aware caret code sees them.
#[derive(Clone, Copy, Debug)]
struct Cluster {
    /// The text it covers, in utf-8 bytes.
    start: usize,
    end: usize,
    /// Where it is on the line.
    left: Pixels,
    right: Pixels,
    rtl: bool,
}

impl Cluster {
    /// Where a caret at `index` (within the cluster's text) goes; a ligature's characters share its
    /// width.
    fn x_at(&self, index: usize) -> Pixels {
        let share = (index.saturating_sub(self.start)) as f32 / (self.end - self.start).max(1) as f32;
        let offset = (self.right - self.left) * share.clamp(0., 1.);
        if self.rtl {
            self.right - offset
        } else {
            self.left + offset
        }
    }
}

impl LineLayout {
    /// The index for the character at the given x coordinate
    pub fn index_for_x(&self, x: Pixels) -> Option<usize> {
        if x >= self.width {
            None
        } else if self.has_rtl() {
            let clusters = self.clusters();
            Some(clusters.iter().find(|c| c.left <= x && x < c.right).map_or(0, |c| c.start))
        } else {
            for run in self.runs.iter().rev() {
                for glyph in run.glyphs.iter().rev() {
                    if glyph.position.x <= x {
                        return Some(glyph.index);
                    }
                }
            }
            Some(0)
        }
    }

    /// closest_index_for_x returns the character boundary closest to the given x coordinate
    /// (e.g. to handle aligning up/down arrow keys)
    pub fn closest_index_for_x(&self, x: Pixels) -> usize {
        if self.has_rtl() {
            return self.bidi_closest_index_for_x(x);
        }
        let mut prev_index = 0;
        let mut prev_x = px(0.);

        for run in self.runs.iter() {
            for glyph in run.glyphs.iter() {
                if glyph.position.x >= x {
                    if glyph.position.x - x < x - prev_x {
                        return glyph.index;
                    } else {
                        return prev_index;
                    }
                }
                prev_index = glyph.index;
                prev_x = glyph.position.x;
            }
        }

        if self.len == 1 {
            if x > self.width / 2. {
                return 1;
            } else {
                return 0;
            }
        }

        self.len
    }

    /// The x position of the character at the given index
    ///
    /// That is where a caret before it goes: a left-to-right character's left edge, a right-to-left
    /// character's right edge. At the end of the line, the last character's trailing edge.
    pub fn x_for_index(&self, index: usize) -> Pixels {
        if self.has_rtl() {
            return self.bidi_x_for_index(index);
        }
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return glyph.position.x;
                }
            }
        }
        self.width
    }

    /// The stretches of the line, left to right, that show the text in `range` (a selection, say).
    ///
    /// Left-to-right text gives one stretch, from caret to caret. Where right-to-left text is
    /// involved a range can show in several places, so each run of adjacent glyphs is a stretch.
    pub fn x_ranges_for_index_range(&self, range: Range<usize>) -> SmallVec<[Range<Pixels>; 2]> {
        let mut stretches = SmallVec::new();
        if range.start >= range.end {
            return stretches;
        }
        if !self.has_rtl() {
            stretches.push(self.x_for_index(range.start)..self.x_for_index(range.end));
            return stretches;
        }
        for cluster in self.clusters() {
            let start = range.start.max(cluster.start);
            let end = range.end.min(cluster.end);
            if start >= end {
                continue;
            }
            let (left, right) = (cluster.x_at(start), cluster.x_at(end));
            let (left, right) = (left.min(right), left.max(right));
            match stretches.last_mut() {
                Some(last) if (last.end - left).abs() < px(0.5) => last.end = right,
                _ => stretches.push(left..right),
            }
        }
        stretches
    }

    /// Whether some run lays its text out right to left: its glyphs, left to right, go down in index.
    fn has_rtl(&self) -> bool {
        self.runs
            .iter()
            .any(|run| run.glyphs.windows(2).any(|pair| pair[1].index < pair[0].index))
    }

    /// The line's clusters (glyphs that share a text index: a character, a ligature, a base with its
    /// marks) in visual order, each with the text it covers, where it is, and its run's direction.
    fn clusters(&self) -> Vec<Cluster> {
        let mut clusters: Vec<Cluster> = Vec::new();
        for run in &self.runs {
            let rtl = match (run.glyphs.first(), run.glyphs.last()) {
                (Some(first), Some(last)) => last.index < first.index,
                _ => false,
            };
            let run_start = clusters.len();
            for glyph in &run.glyphs {
                match clusters[run_start..].last_mut() {
                    Some(cluster) if cluster.start == glyph.index => {
                        cluster.left = cluster.left.min(glyph.position.x)
                    }
                    _ => clusters.push(Cluster {
                        start: glyph.index,
                        end: glyph.index,
                        left: glyph.position.x,
                        right: glyph.position.x,
                        rtl,
                    }),
                }
            }
        }
        // A cluster's text runs to the next cluster's, in text order.
        let mut starts: Vec<usize> = clusters.iter().map(|c| c.start).collect();
        starts.sort_unstable();
        starts.dedup();
        for cluster in &mut clusters {
            let next = starts.partition_point(|&start| start <= cluster.start);
            cluster.end = starts.get(next).copied().unwrap_or(self.len).max(cluster.start);
        }
        // On screen, a cluster reaches the next one to its right.
        clusters.sort_by_key(|c| (c.left, c.start));
        for ix in 0..clusters.len() {
            let next_left = clusters.get(ix + 1).map_or(self.width, |c| c.left);
            clusters[ix].right = next_left.max(clusters[ix].left);
        }
        clusters
    }

    fn bidi_x_for_index(&self, index: usize) -> Pixels {
        let clusters = self.clusters();
        let by_start: FxHashMap<usize, &Cluster> = clusters.iter().map(|c| (c.start, c)).collect();
        let by_end: FxHashMap<usize, &Cluster> =
            clusters.iter().filter(|c| c.end > c.start).map(|c| (c.end, c)).collect();
        self.bidi_caret_x(&clusters, &by_start, &by_end, index)
    }

    /// Where a caret at `index` goes. After a right-to-left character, on its trailing (left) edge,
    /// where typing goes on; otherwise on the leading edge of the character at `index`. Every edge
    /// between glyphs then belongs to exactly one index.
    fn bidi_caret_x(
        &self,
        clusters: &[Cluster],
        by_start: &FxHashMap<usize, &Cluster>,
        by_end: &FxHashMap<usize, &Cluster>,
        index: usize,
    ) -> Pixels {
        let index = index.min(self.len);
        if let Some(inside) = clusters.iter().find(|c| c.start < index && index < c.end) {
            return inside.x_at(index);
        }
        match (by_end.get(&index), by_start.get(&index)) {
            (Some(prev), _) if prev.rtl => prev.x_at(prev.end),
            (_, Some(next)) => next.x_at(next.start),
            (Some(prev), None) => prev.x_at(prev.end),
            (None, None) => clusters
                .iter()
                .filter(|c| c.start >= index)
                .min_by_key(|c| c.start)
                .map_or(self.width, |c| c.x_at(c.start)),
        }
    }

    fn bidi_closest_index_for_x(&self, x: Pixels) -> usize {
        self.closest_among(&self.clusters(), Some(self.len), x)
    }

    /// The caret position, among `clusters`, closest to `x`: the start of one of them, or `end` (the
    /// end of the text, if the clusters reach it).
    fn closest_among(&self, clusters: &[Cluster], end: Option<usize>, x: Pixels) -> usize {
        let by_start: FxHashMap<usize, &Cluster> = clusters.iter().map(|c| (c.start, c)).collect();
        let by_end: FxHashMap<usize, &Cluster> =
            clusters.iter().filter(|c| c.end > c.start).map(|c| (c.end, c)).collect();
        // The boundary whose caret is nearest the point: a click lands where the caret then shows.
        // Where two carets share a spot (the edge between "1" and ".pdf" after Arabic text, say), the
        // one belonging to the glyph under the point wins.
        let under = clusters.iter().find(|c| c.left <= x && x < c.right);
        clusters
            .iter()
            .map(|c| c.start)
            .chain(end)
            .min_by_key(|&ix| {
                let distance = (self.bidi_caret_x(clusters, &by_start, &by_end, ix) - x).abs();
                let own = under.is_some_and(|c| ix == c.start || ix == c.end);
                (distance.round(), !own)
            })
            .unwrap_or(0)
    }

    /// Where a caret at `index` goes, looking only at `clusters` (one row of a wrapped paragraph).
    fn caret_x_among(&self, clusters: &[Cluster], index: usize) -> Pixels {
        let by_start: FxHashMap<usize, &Cluster> = clusters.iter().map(|c| (c.start, c)).collect();
        let by_end: FxHashMap<usize, &Cluster> =
            clusters.iter().filter(|c| c.end > c.start).map(|c| (c.end, c)).collect();
        self.bidi_caret_x(clusters, &by_start, &by_end, index)
    }

    /// The corresponding Font at the given index
    pub fn font_id_for_index(&self, index: usize) -> Option<FontId> {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return Some(run.font_id);
                }
            }
        }
        None
    }

    /// Split this layout at a byte index, returning `(prefix, suffix)`.
    ///
    /// - `prefix` contains glyphs for bytes `[0, byte_index)` with original positions.
    ///   Its width equals the x-advance up to the split point.
    /// - `suffix` contains glyphs for bytes `[byte_index, len)` with positions
    ///   shifted left so the first glyph starts at x=0, and byte indices rebased to 0.
    /// - `font_size`, `ascent`, and `descent` are copied to both halves.
    pub fn split_at(&self, byte_index: usize) -> (LineLayout, LineLayout) {
        let x_offset = self.x_for_index(byte_index);

        // Partition glyph runs. A single run may contribute glyphs to both halves.
        let mut left_runs = Vec::new();
        let mut right_runs = Vec::new();

        for run in &self.runs {
            let split_pos = run.glyphs.partition_point(|g| g.index < byte_index);

            if split_pos > 0 {
                left_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs: run.glyphs[..split_pos].to_vec(),
                });
            }

            if split_pos < run.glyphs.len() {
                let right_glyphs = run.glyphs[split_pos..]
                    .iter()
                    .map(|g| ShapedGlyph {
                        id: g.id,
                        position: point(g.position.x - x_offset, g.position.y),
                        index: g.index - byte_index,
                        is_emoji: g.is_emoji,
                    })
                    .collect();
                right_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs: right_glyphs,
                });
            }
        }

        let left = LineLayout {
            font_size: self.font_size,
            width: x_offset,
            ascent: self.ascent,
            descent: self.descent,
            runs: left_runs,
            len: byte_index,
        };

        let right = LineLayout {
            font_size: self.font_size,
            width: self.width - x_offset,
            ascent: self.ascent,
            descent: self.descent,
            runs: right_runs,
            len: self.len - byte_index,
        };

        (left, right)
    }

    fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
        scratch: &mut WrapScratch,
    ) -> SmallVec<[WrapBoundary; 1]> {
        let mut boundaries = SmallVec::new();
        let mut current_segment: Option<Range<usize>> = None;
        let mut last_candidate_ix = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_ix: 0,
            glyph_ix: 0,
        };
        let mut last_boundary_x = px(0.);
        // Shaping can combine characters across a line break opportunity, so we ignore those
        // break opportunities that would separate them.
        let WrapScratch {
            glyph_indices,
            break_indices,
        } = scratch;
        glyph_indices.clear();
        glyph_indices.extend(
            self.runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index)),
        );
        break_indices.clear();
        break_indices
            .extend(LineWrapper::break_indices(text).filter(|index| glyph_indices.contains(index)));
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(move |(run_ix, run)| {
                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                    (
                        WrapBoundary { run_ix, glyph_ix },
                        glyph.index,
                        glyph.position.x,
                    )
                })
            })
            .dedup_by(|(_, previous_index, ..), (_, index, ..)| previous_index == index)
            .peekable();

        while let Some((boundary, index, x)) = glyphs.next() {
            let byte = text.as_bytes()[index];
            if byte == b'\n' {
                continue;
            }

            if byte != b' '
                && current_segment
                    .as_ref()
                    .is_none_or(|segment| !segment.contains(&index))
            {
                // Detect transitions between line-break segments rather than matching glyph
                // indices directly, to accommodate RTL text. Workaround for the lack of complete
                // UAX #9 support.
                let segment = break_indices.partition_point(|break_index| *break_index <= index);
                if current_segment.is_some() {
                    last_candidate_ix = Some(boundary);
                    last_candidate_x = x;
                }
                let start = segment
                    .checked_sub(1)
                    .and_then(|previous| break_indices.get(previous))
                    .copied()
                    .unwrap_or(0);
                let end = break_indices.get(segment).copied().unwrap_or(text.len());
                current_segment = Some(start..end);
            }

            let next_x = glyphs.peek().map_or(self.width, |(_, _, x)| *x);
            let width = next_x - last_boundary_x;

            if width > wrap_width && boundary > last_boundary {
                // When used line_clamp, we should limit the number of lines.
                if let Some(max_lines) = max_lines
                    && boundaries.len() >= max_lines.saturating_sub(1)
                {
                    break;
                }

                if let Some(last_candidate_ix) = last_candidate_ix.take() {
                    last_boundary = last_candidate_ix;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
        }

        boundaries
    }
}

/// A line of text that has been wrapped to fit a given width
#[derive(Default, Debug)]
pub struct WrappedLineLayout {
    /// The line layout, pre-wrapping.
    pub unwrapped_layout: Arc<LineLayout>,

    /// The boundaries at which the line was wrapped
    pub wrap_boundaries: SmallVec<[WrapBoundary; 1]>,

    /// The width of the line, if it was wrapped
    pub wrap_width: Option<Pixels>,
}

/// A boundary at which a line was wrapped
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WrapBoundary {
    /// The index in the run just before the line was wrapped
    pub run_ix: usize,
    /// The index of the glyph just before the line was wrapped
    pub glyph_ix: usize,
}

impl WrappedLineLayout {
    /// The length of the underlying text, in utf8 bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.unwrapped_layout.len
    }

    /// The width of this line, in pixels, whether or not it was wrapped.
    pub fn width(&self) -> Pixels {
        self.wrap_width
            .unwrap_or(Pixels::MAX)
            .min(self.unwrapped_layout.width)
    }

    /// The size of the whole wrapped text, for the given line_height.
    /// can span multiple lines if there are multiple wrap boundaries.
    pub fn size(&self, line_height: Pixels) -> Size<Pixels> {
        Size {
            width: self.width(),
            height: line_height * (self.wrap_boundaries.len() + 1),
        }
    }

    /// The ascent of a line in this layout
    pub fn ascent(&self) -> Pixels {
        self.unwrapped_layout.ascent
    }

    /// The descent of a line in this layout
    pub fn descent(&self) -> Pixels {
        self.unwrapped_layout.descent
    }

    /// The wrap boundaries in this layout
    pub fn wrap_boundaries(&self) -> &[WrapBoundary] {
        &self.wrap_boundaries
    }

    /// The font size of this layout
    pub fn font_size(&self) -> Pixels {
        self.unwrapped_layout.font_size
    }

    /// The runs in this layout, sans wrapping
    pub fn runs(&self) -> &[ShapedRun] {
        &self.unwrapped_layout.runs
    }

    /// The index corresponding to a given position in this layout for the given line height.
    ///
    /// See also [`Self::closest_index_for_position`].
    pub fn index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, false)
    }

    /// The closest index to a given position in this layout for the given line height.
    ///
    /// Closest means the character boundary closest to the given position.
    ///
    /// See also [`LineLayout::closest_index_for_x`].
    pub fn closest_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, true)
    }

    fn _index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
        closest: bool,
    ) -> Result<usize, usize> {
        let wrapped_line_ix = (position.y / line_height) as usize;

        if let Some(rows) = self.rtl_rows() {
            return self.rtl_index_for_position(&rows, wrapped_line_ix, position.x, closest);
        }

        let wrapped_line_start_index;
        let wrapped_line_start_x;
        if wrapped_line_ix > 0 {
            let Some(line_start_boundary) = self.wrap_boundaries.get(wrapped_line_ix - 1) else {
                return Err(0);
            };
            let run = &self.unwrapped_layout.runs[line_start_boundary.run_ix];
            let glyph = &run.glyphs[line_start_boundary.glyph_ix];
            wrapped_line_start_index = glyph.index;
            wrapped_line_start_x = glyph.position.x;
        } else {
            wrapped_line_start_index = 0;
            wrapped_line_start_x = Pixels::ZERO;
        };

        let wrapped_line_end_index;
        let wrapped_line_end_x;
        if wrapped_line_ix < self.wrap_boundaries.len() {
            let next_wrap_boundary_ix = wrapped_line_ix;
            let next_wrap_boundary = self.wrap_boundaries[next_wrap_boundary_ix];
            let run = &self.unwrapped_layout.runs[next_wrap_boundary.run_ix];
            let glyph = &run.glyphs[next_wrap_boundary.glyph_ix];
            wrapped_line_end_index = glyph.index;
            wrapped_line_end_x = glyph.position.x;
        } else {
            wrapped_line_end_index = self.unwrapped_layout.len;
            wrapped_line_end_x = self.unwrapped_layout.width;
        };

        let mut position_in_unwrapped_line = position;
        position_in_unwrapped_line.x += wrapped_line_start_x;
        if position_in_unwrapped_line.x < wrapped_line_start_x {
            Err(wrapped_line_start_index)
        } else if position_in_unwrapped_line.x >= wrapped_line_end_x {
            Err(wrapped_line_end_index)
        } else {
            if closest {
                Ok(self
                    .unwrapped_layout
                    .closest_index_for_x(position_in_unwrapped_line.x))
            } else {
                // The shaper can place a trailing zero-width wrap boundary glyph slightly past
                // the line's width, so the row can extend past where `index_for_x` has glyphs.
                self.unwrapped_layout
                    .index_for_x(position_in_unwrapped_line.x)
                    .ok_or(wrapped_line_end_index)
            }
        }
    }

    /// Returns the pixel position for the given byte index.
    pub fn position_for_index(&self, index: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        if let Some(rows) = self.rtl_rows() {
            if index > self.len() {
                return None;
            }
            // A row starts at its first character in reading order, so an index that ends one row
            // starts the next.
            let row_ix = rows.iter().rposition(|row| row.start_ix <= index).unwrap_or(0);
            let row = &rows[row_ix];
            let layout = &self.unwrapped_layout;
            let clusters = self.rtl_row_clusters(row);
            let x = layout.caret_x_among(&clusters, index) - row.left;
            return Some(point(x, row_ix as f32 * line_height));
        }
        let mut line_start_ix = 0;
        let line_end_indices = self
            .wrap_boundaries
            .iter()
            .map(|wrap_boundary| {
                let run = &self.unwrapped_layout.runs[wrap_boundary.run_ix];
                let glyph = &run.glyphs[wrap_boundary.glyph_ix];
                glyph.index
            })
            .chain([self.len()])
            .enumerate();
        for (ix, line_end_ix) in line_end_indices {
            let line_y = ix as f32 * line_height;
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_start_ix = line_end_ix;
                continue;
            } else {
                let line_start_x = self.unwrapped_layout.x_for_index(line_start_ix);
                let x = self.unwrapped_layout.x_for_index(index) - line_start_x;
                return Some(point(x, line_y));
            }
        }

        None
    }
}

/// One row of a wrapped paragraph that has right-to-left text, as the hit testing sees it.
struct RtlRow {
    /// The text it shows, in utf-8 bytes, in reading order.
    start_ix: usize,
    end_ix: usize,
    /// Where it is on the concatenated line.
    left: Pixels,
    right: Pixels,
}

impl WrappedLineLayout {
    /// The rows of a layout that has right-to-left glyphs (wrapped or not), or `None` for any other:
    /// a row's text is not a run of the glyphs' indices there (the first glyph on screen can be the
    /// last one read), so the left-to-right shortcuts don't hold.
    fn rtl_rows(&self) -> Option<Vec<RtlRow>> {
        if !self.unwrapped_layout.has_rtl() {
            return None;
        }
        let layout = &self.unwrapped_layout;
        // Each row: its lowest glyph index (where it starts reading) and where its first glyph is.
        let mut starts: Vec<(usize, Pixels)> = Vec::with_capacity(self.wrap_boundaries.len() + 1);
        let mut boundaries = self.wrap_boundaries.iter().peekable();
        let mut lowest = usize::MAX;
        let mut left = Pixels::ZERO;
        for (run_ix, run) in layout.runs.iter().enumerate() {
            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                if boundaries.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    boundaries.next();
                    starts.push((lowest, left));
                    lowest = usize::MAX;
                    left = glyph.position.x;
                }
                lowest = lowest.min(glyph.index);
            }
        }
        starts.push((lowest, left));
        let rows = (0..starts.len())
            .map(|ix| RtlRow {
                start_ix: if ix == 0 { 0 } else { starts[ix].0 },
                end_ix: starts.get(ix + 1).map_or(layout.len, |next| next.0),
                left: starts[ix].1,
                right: starts.get(ix + 1).map_or(layout.width, |next| next.1),
            })
            .collect();
        Some(rows)
    }

    /// The clusters of the row, from the whole line's.
    fn rtl_row_clusters(&self, row: &RtlRow) -> Vec<Cluster> {
        self.unwrapped_layout
            .clusters()
            .into_iter()
            .filter(|cluster| cluster.start >= row.start_ix && cluster.start < row.end_ix)
            .collect()
    }

    /// `_index_for_position` for a layout wrapped around right-to-left text. `x` is from the row's
    /// start (its left edge); beyond either edge the answer is the caret at that edge, `Err`.
    fn rtl_index_for_position(
        &self,
        rows: &[RtlRow],
        row_ix: usize,
        x: Pixels,
        closest: bool,
    ) -> Result<usize, usize> {
        let Some(row) = rows.get(row_ix) else {
            return Err(0);
        };
        let layout = &self.unwrapped_layout;
        let clusters = self.rtl_row_clusters(row);
        let end = (row_ix + 1 == rows.len()).then_some(layout.len);
        let x_in_line = x + row.left;
        if x_in_line < row.left {
            Err(layout.closest_among(&clusters, end, row.left))
        } else if x_in_line >= row.right {
            Err(layout.closest_among(&clusters, end, row.right))
        } else if closest {
            Ok(layout.closest_among(&clusters, end, x_in_line))
        } else {
            clusters
                .iter()
                .find(|cluster| cluster.left <= x_in_line && x_in_line < cluster.right)
                .map(|cluster| cluster.start)
                .ok_or(row.end_ix)
        }
    }
}

pub(crate) struct LineLayoutCache {
    state: RefCell<LineLayoutCacheState>,
    wrap_scratch: RefCell<WrapScratch>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    /// Advances when [`TextSystem::add_fonts`] successfully changes the font database.
    font_generation: Arc<AtomicUsize>,
    /// Records the generation represented by both frame caches.
    cached_font_generation: AtomicUsize,
}

#[derive(Default)]
struct WrapScratch {
    glyph_indices: FxHashSet<usize>,
    break_indices: Vec<usize>,
}

#[derive(Default)]
struct LineLayoutCacheState {
    previous_frame: FrameCache,
    current_frame: FrameCache,
}

#[derive(Default)]
struct FrameCache {
    lines: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, Arc<WrappedLineLayout>>,
    used_lines: Vec<Arc<CacheKey>>,
    used_wrapped_lines: Vec<Arc<CacheKey>>,
}

#[derive(Clone, Default)]
pub(crate) struct LineLayoutIndex {
    font_generation: usize,
    lines_index: usize,
    wrapped_lines_index: usize,
}

impl LineLayoutCache {
    pub fn new(
        platform_text_system: Arc<dyn PlatformTextSystem>,
        font_generation: Arc<AtomicUsize>,
    ) -> Self {
        let cached_font_generation = font_generation.load(Ordering::Acquire);
        Self {
            state: RefCell::default(),
            wrap_scratch: RefCell::default(),
            platform_text_system,
            font_generation,
            cached_font_generation: AtomicUsize::new(cached_font_generation),
        }
    }

    pub fn layout_index(&self) -> LineLayoutIndex {
        let font_generation = self.clear_if_font_generation_changed();
        let state = self.state.borrow();
        let frame = &state.current_frame;
        LineLayoutIndex {
            font_generation,
            lines_index: frame.used_lines.len(),
            wrapped_lines_index: frame.used_wrapped_lines.len(),
        }
    }

    pub fn reuse_layouts(&self, range: Range<LineLayoutIndex>) {
        let font_generation = self.clear_if_font_generation_changed();
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return;
        }
        let mut state = self.state.borrow_mut();
        let LineLayoutCacheState {
            previous_frame,
            current_frame,
        } = &mut *state;

        for key in &previous_frame.used_lines[range.start.lines_index..range.end.lines_index] {
            if let Some((key, line)) = previous_frame.lines.remove_entry(key) {
                current_frame.lines.insert(key, line);
            }
            current_frame.used_lines.push(key.clone());
        }

        for key in &previous_frame.used_wrapped_lines
            [range.start.wrapped_lines_index..range.end.wrapped_lines_index]
        {
            if let Some((key, line)) = previous_frame.wrapped_lines.remove_entry(key) {
                current_frame.wrapped_lines.insert(key, line);
            }
            current_frame.used_wrapped_lines.push(key.clone());
        }
    }

    pub fn truncate_layouts(&self, index: LineLayoutIndex) {
        let font_generation = self.clear_if_font_generation_changed();
        if index.font_generation != font_generation {
            return;
        }
        let mut state = self.state.borrow_mut();
        let current_frame = &mut state.current_frame;
        current_frame.used_lines.truncate(index.lines_index);
        current_frame
            .used_wrapped_lines
            .truncate(index.wrapped_lines_index);
    }

    pub fn finish_frame(&self) {
        let _font_generation = self.clear_if_font_generation_changed();
        let LineLayoutCacheState {
            previous_frame,
            current_frame,
        } = &mut *self.state.borrow_mut();
        std::mem::swap(previous_frame, current_frame);
        current_frame.lines.clear();
        current_frame.wrapped_lines.clear();
        current_frame.used_lines.clear();
        current_frame.used_wrapped_lines.clear();
    }

    pub fn layout_wrapped_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
    ) -> Arc<WrappedLineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let _font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
            force_width: None,
        } as &dyn AsCacheKeyRef;

        let state = self.state.borrow();
        if let Some(layout) = state.current_frame.wrapped_lines.get(key) {
            return layout.clone();
        }
        drop(state);

        let mut state = self.state.borrow_mut();
        let previous_frame_entry = state.previous_frame.wrapped_lines.remove_entry(key);
        if let Some((key, layout)) = previous_frame_entry {
            state
                .current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            state.current_frame.used_wrapped_lines.push(key);
            layout
        } else {
            drop(state);
            let text = SharedString::from(text);
            // Right-to-left text that wraps: the break points come from the text's reading order and
            // every row is laid out (and reordered) as a line of its own. An English paragraph, with or
            // without a few Arabic words, that fits or never gets here, wraps the way it always did.
            let rtl_base = wrap_width.is_some() && paragraph_reads_right_to_left(&text);
            let mut rows = None;
            let mut unwrapped_layout = None;
            if let Some(wrap_width) = wrap_width {
                if rtl_base {
                    rows = self.layout_rtl_rows(&text, None, font_size, runs, wrap_width, max_lines);
                } else {
                    let whole = self.layout_line::<&SharedString>(&text, font_size, runs, None);
                    if whole.width > wrap_width && has_rtl_text(&text) {
                        rows = self.layout_rtl_rows(
                            &text,
                            Some(&*whole),
                            font_size,
                            runs,
                            wrap_width,
                            max_lines,
                        );
                    }
                    unwrapped_layout = Some(whole);
                }
            }
            let (unwrapped_layout, wrap_boundaries) = match rows {
                Some((layout, boundaries)) => (Arc::new(layout), boundaries),
                None => {
                    let layout = unwrapped_layout.unwrap_or_else(|| {
                        self.layout_line::<&SharedString>(&text, font_size, runs, None)
                    });
                    let boundaries = match wrap_width {
                        Some(wrap_width) => layout.compute_wrap_boundaries(
                            text.as_ref(),
                            wrap_width,
                            max_lines,
                            &mut self.wrap_scratch.borrow_mut(),
                        ),
                        None => SmallVec::new(),
                    };
                    (layout, boundaries)
                }
            };
            let layout = Arc::new(WrappedLineLayout {
                unwrapped_layout,
                wrap_boundaries,
                wrap_width,
            });
            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width,
                force_width: None,
            });

            let mut state = self.state.borrow_mut();
            state
                .current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            state.current_frame.used_wrapped_lines.push(key);

            layout
        }
    }

    pub fn layout_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let _font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
            force_width,
        } as &dyn AsCacheKeyRef;

        let state = self.state.borrow();
        if let Some(layout) = state.current_frame.lines.get(key) {
            return layout.clone();
        }
        drop(state);

        let mut state = self.state.borrow_mut();
        let LineLayoutCacheState {
            previous_frame,
            current_frame,
        } = &mut *state;
        if let Some((key, layout)) = previous_frame.lines.remove_entry(key) {
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            layout
        } else {
            let text = SharedString::from(text);
            let mut layout = self
                .platform_text_system
                .layout_line(&text, font_size, runs);

            if let Some(force_width) = force_width {
                apply_force_width_to_layout(&mut layout, force_width);
            }

            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width: None,
                force_width,
            });
            let layout = Arc::new(layout);
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            layout
        }
    }

    /// `text` laid out as a line in a right-to-left embedding, its glyphs indexed as if the embedding's
    /// control characters (which have no glyphs of their own) were not there.
    fn layout_embedded(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> Arc<LineLayout> {
        const RLE: char = '\u{202B}';
        const PDF: char = '\u{202C}';
        let mut embedded = String::with_capacity(text.len() + 6);
        embedded.push(RLE);
        embedded.push_str(text);
        embedded.push(PDF);
        let mut embedded_runs: SmallVec<[FontRun; 4]> = SmallVec::from(runs);
        if let Some(first) = embedded_runs.first_mut() {
            first.len += RLE.len_utf8();
        }
        if let Some(last) = embedded_runs.last_mut() {
            last.len += PDF.len_utf8();
        }
        let layout = self.layout_line::<&str>(&embedded, font_size, &embedded_runs, None);

        let offset = RLE.len_utf8();
        let mut stripped = LineLayout {
            font_size: layout.font_size,
            width: layout.width,
            ascent: layout.ascent,
            descent: layout.descent,
            runs: Vec::with_capacity(layout.runs.len()),
            len: text.len(),
        };
        for run in &layout.runs {
            let glyphs: Vec<ShapedGlyph> = run
                .glyphs
                .iter()
                .filter(|glyph| glyph.index >= offset && glyph.index < offset + text.len())
                .map(|glyph| ShapedGlyph {
                    index: glyph.index - offset,
                    ..glyph.clone()
                })
                .collect();
            if !glyphs.is_empty() {
                stripped.runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs,
                });
            }
        }
        // The platform hands the runs over in reading order, each at its place on the line; painting
        // and hit testing want them left to right, with the row's first glyph at its start.
        stripped
            .runs
            .sort_by_key(|run| run.glyphs.first().map(|glyph| glyph.position.x));
        Arc::new(stripped)
    }

    /// Wraps a paragraph that has right-to-left text the way a browser does: the breaks are found in
    /// reading order (the text's own break opportunities, at grapheme and shaping-cluster boundaries,
    /// with the width of each piece taken from the shaped glyph advances keyed by their logical index),
    /// then every row is laid out as a line of its own, so the platform reorders it by itself. The rows'
    /// glyphs are concatenated, each row starting at its own x, so the boundary-based painting and hit
    /// testing work on the result. A row's trailing spaces hang: they have glyphs but no width.
    ///
    /// `None` when the text doesn't split into rows after all; the caller then wraps the old way.
    fn layout_rtl_rows(
        &self,
        text: &str,
        whole: Option<&LineLayout>,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Pixels,
        max_lines: Option<usize>,
    ) -> Option<(LineLayout, SmallVec<[WrapBoundary; 1]>)> {
        // A paragraph that reads right to left sets each row in a right-to-left embedding (RLE ... PDF),
        // which is how the platform learns the paragraph's direction: the first words go on the right,
        // an English phrase keeps its place in the reading order, the full stop lands on the left.
        let embed = whole.is_none();
        let embedded_whole;
        let whole = match whole {
            Some(whole) => whole,
            None => {
                embedded_whole = self.layout_embedded(text, font_size, runs);
                &*embedded_whole
            }
        };
        let units = break_units(text, whole);
        let rows = rows_by_width(text.len(), &units, wrap_width, max_lines);
        if rows.len() < 2 {
            // It fits (or can't be split): one row, still in its embedding.
            return embed.then(|| {
                (
                    LineLayout {
                        font_size,
                        width: whole.width,
                        ascent: whole.ascent,
                        descent: whole.descent,
                        runs: whole.runs.clone(),
                        len: text.len(),
                    },
                    SmallVec::new(),
                )
            });
        }

        let mut out_runs: Vec<ShapedRun> = Vec::new();
        let mut boundaries = SmallVec::new();
        let mut x = px(0.);
        let mut ascent = px(0.);
        let mut descent = px(0.);
        for (row_ix, row_range) in rows.iter().enumerate() {
            let (start, end) = (row_range.start, row_range.end);
            let is_last = row_ix + 1 == rows.len();
            let visible_end = if is_last {
                end
            } else {
                start + text[start..end].trim_end_matches(' ').len()
            };
            let row_runs = slice_font_runs(runs, start..end);
            let row = if embed {
                self.layout_embedded(&text[start..end], font_size, &row_runs)
            } else {
                self.layout_line::<&str>(&text[start..end], font_size, &row_runs, None)
            };
            if row.runs.first().is_none_or(|run| run.glyphs.is_empty()) {
                return None;
            }

            // The trailing spaces hang: they keep their glyphs but take no room, and sit at the end
            // of the row, where the next one begins. The platform put them next to the text's end,
            // on the right in a left-to-right row and on the left in a right-to-left one.
            let hung_from = visible_end - start;
            let mut width = row.width;
            let mut shift = px(0.);
            if visible_end < end {
                let hung = row
                    .clusters()
                    .iter()
                    .filter(|cluster| cluster.start >= hung_from)
                    .fold(px(0.), |sum, cluster| sum + (cluster.right - cluster.left));
                width = (row.width - hung).max(px(0.));
                let lowest_x = |hanging: bool| {
                    row.runs
                        .iter()
                        .flat_map(|run| &run.glyphs)
                        .filter(|glyph| (glyph.index >= hung_from) == hanging)
                        .map(|glyph| glyph.position.x)
                        .fold(None, |lowest: Option<Pixels>, x| Some(lowest.map_or(x, |l| l.min(x))))
                };
                if let (Some(hanging), Some(content)) = (lowest_x(true), lowest_x(false))
                    && hanging < content
                {
                    shift = hung;
                }
            }

            if row_ix > 0 {
                boundaries.push(WrapBoundary {
                    run_ix: out_runs.len(),
                    glyph_ix: 0,
                });
            }
            // The row's glyphs in visual order, then the hanging spaces, which are not part of it.
            let mut hanging: Vec<(FontId, ShapedGlyph)> = Vec::new();
            for run in &row.runs {
                let mut glyphs = Vec::with_capacity(run.glyphs.len());
                for glyph in &run.glyphs {
                    let hangs = glyph.index >= hung_from;
                    let placed = ShapedGlyph {
                        id: glyph.id,
                        position: point(
                            if hangs {
                                x + width
                            } else {
                                x + glyph.position.x - shift
                            },
                            glyph.position.y,
                        ),
                        index: start + glyph.index,
                        is_emoji: glyph.is_emoji,
                    };
                    if hangs {
                        hanging.push((run.font_id, placed));
                    } else {
                        glyphs.push(placed);
                    }
                }
                if !glyphs.is_empty() {
                    out_runs.push(ShapedRun {
                        font_id: run.font_id,
                        glyphs,
                    });
                }
            }
            hanging.sort_by_key(|(_, glyph)| glyph.index);
            for (font_id, glyph) in hanging {
                match out_runs.last_mut() {
                    Some(last) if last.font_id == font_id && last.glyphs.last().is_some_and(|g| g.index >= start + hung_from) => {
                        last.glyphs.push(glyph)
                    }
                    _ => out_runs.push(ShapedRun {
                        font_id,
                        glyphs: vec![glyph],
                    }),
                }
            }
            x += width;
            ascent = ascent.max(row.ascent);
            descent = descent.max(row.descent);
        }

        Some((
            LineLayout {
                font_size,
                width: x,
                ascent,
                descent,
                runs: out_runs,
                len: text.len(),
            },
            boundaries,
        ))
    }

    fn clear_if_font_generation_changed(&self) -> usize {
        let font_generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) == font_generation {
            return font_generation;
        }

        let mut state = self.state.borrow_mut();
        if self.cached_font_generation.load(Ordering::Acquire) == font_generation {
            return font_generation;
        }

        *state = LineLayoutCacheState::default();
        self.cached_font_generation
            .store(font_generation, Ordering::Release);
        font_generation
    }
}

/// Whether `text` has characters that read right to left (Hebrew, Arabic, Syriac, Thaana, NKo and
/// the other right-to-left scripts, and the explicit right-to-left marks).
fn has_rtl_text(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c as u32,
            0x0590..=0x08FF
                | 0xFB1D..=0xFDFF
                | 0xFE70..=0xFEFF
                | 0x10800..=0x10FFF
                | 0x1E800..=0x1EFFF
                | 0x200F
                | 0x202B
                | 0x202E
                | 0x2067
        )
    })
}

/// Whether a paragraph's first strong character reads right to left (Unicode's rule for a paragraph
/// without a direction of its own), so it is set in a right-to-left embedding. Text that has direction
/// controls of its own is left alone.
fn paragraph_reads_right_to_left(text: &str) -> bool {
    if !has_rtl_text(text) {
        return false;
    }
    if text
        .chars()
        .any(|c| matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069))
    {
        return false;
    }
    for c in text.chars() {
        match c as u32 {
            0x200F => return true,
            0x200E => return false,
            _ if c.is_alphabetic() => {
                return matches!(
                    c as u32,
                    0x0590..=0x08FF
                        | 0xFB1D..=0xFDFF
                        | 0xFE70..=0xFEFF
                        | 0x10800..=0x10FFF
                        | 0x1E800..=0x1EFFF
                );
            }
            _ => {}
        }
    }
    false
}

/// A stretch of text that wrapping may start a row at: a shaping cluster (or several, where a grapheme
/// takes more than one) with its width, as shaped on the paragraph's single line.
#[derive(Debug)]
struct BreakUnit {
    /// Where it starts, in utf-8 bytes.
    start: usize,
    width: Pixels,
    /// Only spaces: they hang at the end of a row instead of counting against its width.
    space: bool,
    /// Whether the text allows a break right before it.
    can_break: bool,
}

/// The paragraph's break units in reading order. Their widths come from the glyph advances of the
/// paragraph shaped as one line, keyed by logical index (the x positions there are in visual order).
fn break_units(text: &str, whole: &LineLayout) -> Vec<BreakUnit> {
    let mut widths: Vec<(usize, Pixels)> = whole
        .clusters()
        .iter()
        .map(|cluster| (cluster.start, (cluster.right - cluster.left).max(px(0.))))
        .collect();
    widths.sort_by_key(|(start, _)| *start);
    let graphemes: FxHashSet<usize> = text.grapheme_indices(true).map(|(ix, _)| ix).collect();
    let breaks: FxHashSet<usize> = LineWrapper::break_indices(text).collect();

    // A unit starts where a cluster does, if that is a grapheme boundary too: shaping and graphemes
    // both stay whole.
    let mut units: Vec<BreakUnit> = Vec::new();
    for (start, width) in widths {
        match units.last_mut() {
            Some(last) if last.start == start || !graphemes.contains(&start) => last.width += width,
            _ => units.push(BreakUnit {
                start,
                width,
                space: false,
                can_break: false,
            }),
        }
    }
    if let Some(first) = units.first_mut() {
        first.start = 0;
    }
    for ix in 0..units.len() {
        let end = units.get(ix + 1).map_or(text.len(), |next| next.start);
        let start = units[ix].start;
        units[ix].space = text[start..end].chars().all(|c| c == ' ');
        units[ix].can_break = start > 0 && breaks.contains(&start);
    }
    units
}

/// The rows (byte ranges) a paragraph breaks into at `wrap_width`: as many units as fit, breaking at
/// the last break opportunity, or inside the word if it alone is wider than a row. A row's trailing
/// spaces don't count against it. With `max_lines`, the last row takes whatever is left.
fn rows_by_width(
    text_len: usize,
    units: &[BreakUnit],
    wrap_width: Pixels,
    max_lines: Option<usize>,
) -> Vec<Range<usize>> {
    let mut rows: Vec<Range<usize>> = Vec::new();
    let mut row_start = 0;
    let mut total = px(0.);
    let mut candidate: Option<(usize, Pixels)> = None;
    let mut has_content = false;
    'units: for (ix, unit) in units.iter().enumerate() {
        if unit.can_break && has_content && ix > row_start {
            candidate = Some((ix, total));
        }
        total += unit.width;
        if unit.space {
            continue;
        }
        has_content = true;
        while total > wrap_width && ix > row_start {
            if max_lines.is_some_and(|max| rows.len() >= max.saturating_sub(1)) {
                break 'units;
            }
            match candidate.take() {
                Some((at, width_before)) => {
                    rows.push(row_start..at);
                    row_start = at;
                    total -= width_before;
                }
                None => {
                    rows.push(row_start..ix);
                    row_start = ix;
                    total = unit.width;
                }
            }
        }
    }
    rows.push(row_start..units.len());
    rows.into_iter()
        .map(|unit_range| {
            let start = units.get(unit_range.start).map_or(text_len, |unit| unit.start);
            let end = units.get(unit_range.end).map_or(text_len, |unit| unit.start);
            start..end
        })
        .collect()
}

/// The font runs that cover `range` of the text, rebased to start at its first byte.
fn slice_font_runs(runs: &[FontRun], range: Range<usize>) -> SmallVec<[FontRun; 4]> {
    let mut sliced = SmallVec::new();
    let mut run_start = 0;
    for run in runs {
        let run_end = run_start + run.len;
        let start = run_start.max(range.start);
        let end = run_end.min(range.end);
        if start < end {
            sliced.push(FontRun {
                len: end - start,
                font_id: run.font_id,
            });
        }
        if run_end >= range.end {
            break;
        }
        run_start = run_end;
    }
    sliced
}

// Combining marks (e.g. Thai vowel signs, Arabic diacritics) are shaped by
// HarfBuzz at the same x position as their base character. The force-width
// loop must not advance the cell counter for these zero-advance glyphs,
// otherwise they get displaced into the next cell. We detect them by checking
// whether shaped x has advanced by at least half a cell beyond the last base.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos: usize = 0;
    // NEG_INFINITY ensures the first glyph is always classified as a base.
    let mut last_base_shaped_x = px(f32::NEG_INFINITY);
    let mut last_base_actual_x = px(0.);

    for run in layout.runs.iter_mut() {
        for glyph in run.glyphs.iter_mut() {
            let shaped_x = glyph.position.x;

            if shaped_x > last_base_shaped_x + force_width * 0.5 {
                let forced_x = glyph_pos * force_width;
                if (shaped_x - forced_x).abs() > px(1.) {
                    glyph.position.x = forced_x;
                }
                last_base_shaped_x = shaped_x;
                last_base_actual_x = glyph.position.x;
                glyph_pos += 1;
            } else {
                glyph.position.x = last_base_actual_x + (shaped_x - last_base_shaped_x);
            }
        }
    }
}

/// A run of text with a single font.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[expect(missing_docs)]
pub struct FontRun {
    pub len: usize,
    pub font_id: FontId,
}

trait AsCacheKeyRef {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_>;
}

#[derive(Clone, Debug, Eq)]
struct CacheKey {
    text: SharedString,
    font_size: Pixels,
    runs: SmallVec<[FontRun; 1]>,
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct CacheKeyRef<'a> {
    text: &'a str,
    font_size: Pixels,
    runs: &'a [FontRun],
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

impl PartialEq for dyn AsCacheKeyRef + '_ {
    fn eq(&self, other: &dyn AsCacheKeyRef) -> bool {
        self.as_cache_key_ref() == other.as_cache_key_ref()
    }
}

impl Eq for dyn AsCacheKeyRef + '_ {}

impl Hash for dyn AsCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state)
    }
}

impl AsCacheKeyRef for CacheKey {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        CacheKeyRef {
            text: &self.text,
            font_size: self.font_size,
            runs: self.runs.as_slice(),
            wrap_width: self.wrap_width,
            force_width: self.force_width,
        }
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_cache_key_ref().eq(&other.as_cache_key_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl<'a> Borrow<dyn AsCacheKeyRef + 'a> for Arc<CacheKey> {
    fn borrow(&self) -> &(dyn AsCacheKeyRef + 'a) {
        self.as_ref() as &dyn AsCacheKeyRef
    }
}

impl AsCacheKeyRef for CacheKeyRef<'_> {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GlyphId;

    fn glyph_at(x: f32, index: usize) -> ShapedGlyph {
        ShapedGlyph {
            id: GlyphId(0),
            position: point(px(x), px(0.)),
            index,
            is_emoji: false,
        }
    }

    fn make_layout(glyphs: Vec<ShapedGlyph>) -> LineLayout {
        LineLayout {
            font_size: px(16.),
            width: px(100.),
            ascent: px(12.),
            descent: px(4.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs,
            }],
            len: 0,
        }
    }

    /// "abc " then three right-to-left characters (2 bytes each, as Arabic is in utf-8) then " 1",
    /// shaped the way DirectWrite and CoreText lay it out: the Arabic run's glyphs in visual order with
    /// their indices going down. Each glyph is 10 px wide.
    fn mixed_layout() -> LineLayout {
        // text bytes: a0 b1 c2 ' '3 | ARA1 4..6, ARA2 6..8, ARA3 8..10 | ' '10 '1'11 -> len 12
        LineLayout {
            font_size: px(16.),
            width: px(100.),
            ascent: px(12.),
            descent: px(4.),
            runs: vec![
                ShapedRun { font_id: FontId(0), glyphs: vec![glyph_at(0., 0), glyph_at(10., 1), glyph_at(20., 2), glyph_at(30., 3)] },
                // Visual order: ARA3 at 40, ARA2 at 50, ARA1 at 60.
                ShapedRun { font_id: FontId(0), glyphs: vec![glyph_at(40., 8), glyph_at(50., 6), glyph_at(60., 4)] },
                ShapedRun { font_id: FontId(0), glyphs: vec![glyph_at(70., 10), glyph_at(80., 11)] },
            ],
            len: 12,
        }
        .with_width(90.)
    }

    impl LineLayout {
        fn with_width(mut self, width: f32) -> Self {
            self.width = px(width);
            self
        }
    }

    #[test]
    fn test_rtl_caret_positions() {
        let layout = mixed_layout();
        // Left-to-right characters: their left edges.
        assert_eq!(layout.x_for_index(0), px(0.));
        assert_eq!(layout.x_for_index(3), px(30.));
        // The first right-to-left character (index 4) sits at 60..70: a caret before it is on its right.
        assert_eq!(layout.x_for_index(4), px(70.));
        assert_eq!(layout.x_for_index(6), px(60.));
        assert_eq!(layout.x_for_index(8), px(50.));
        // Inside a 2-byte character, halfway.
        assert_eq!(layout.x_for_index(5), px(65.));
        // After the last right-to-left character, on its trailing (left) edge, where typing goes on.
        assert_eq!(layout.x_for_index(10), px(40.));
        assert_eq!(layout.x_for_index(11), px(80.));
        // The end of the line: after "1", on its right.
        assert_eq!(layout.x_for_index(12), px(90.));
    }

    #[test]
    fn test_rtl_hit_testing() {
        let layout = mixed_layout();
        // The character under a point.
        assert_eq!(layout.index_for_x(px(5.)), Some(0));
        assert_eq!(layout.index_for_x(px(45.)), Some(8));
        assert_eq!(layout.index_for_x(px(65.)), Some(4));
        assert_eq!(layout.index_for_x(px(95.)), None);
        // The closest caret: every glyph edge belongs to one index, and a click takes the nearest.
        assert_eq!(layout.closest_index_for_x(px(68.)), 4);
        assert_eq!(layout.closest_index_for_x(px(62.)), 6);
        assert_eq!(layout.closest_index_for_x(px(47.)), 8);
        assert_eq!(layout.closest_index_for_x(px(42.)), 10);
        assert_eq!(layout.closest_index_for_x(px(12.)), 1);
        assert_eq!(layout.closest_index_for_x(px(-5.)), 0);
        assert_eq!(layout.closest_index_for_x(px(200.)), 12);
        // Every boundary a click can give maps back near the click.
        for x in [3., 17., 44., 56., 66., 74., 86.] {
            let ix = layout.closest_index_for_x(px(x));
            assert!((layout.x_for_index(ix) - px(x)).abs() <= px(5.), "x {x} -> index {ix} -> {:?}", layout.x_for_index(ix));
        }
    }

    #[test]
    fn test_rtl_selection_stretches() {
        let layout = mixed_layout();
        // The first two Arabic characters (bytes 4..8) show at 50..70.
        assert_eq!(layout.x_ranges_for_index_range(4..8).to_vec(), vec![px(50.)..px(70.)]);
        // "c" and the first Arabic character are logically adjacent but apart on screen.
        assert_eq!(layout.x_ranges_for_index_range(2..6).to_vec(), vec![px(20.)..px(40.), px(60.)..px(70.)]);
        // A left-to-right line keeps the old caret-to-caret stretch.
        let ltr = make_layout(vec![glyph_at(0., 0), glyph_at(10., 1), glyph_at(20., 2)]).with_width(30.);
        assert_eq!(ltr.x_ranges_for_index_range(1..2).to_vec(), vec![px(10.)..px(20.)]);
    }

    fn glyph_x_positions(layout: &LineLayout) -> Vec<f32> {
        layout.runs[0]
            .glyphs
            .iter()
            .map(|g| f32::from(g.position.x))
            .collect()
    }

    #[test]
    fn test_wrap_preserves_shaped_clusters() {
        let mut layout = make_layout(vec![
            glyph_at(0., 0),
            glyph_at(5., 0),
            glyph_at(10., 3),
            glyph_at(15., 3),
        ]);
        layout.width = px(20.);
        layout.len = "你好".len();
        assert_eq!(
            layout
                .compute_wrap_boundaries("你好", px(15.), None, &mut WrapScratch::default())
                .as_slice(),
            &[WrapBoundary {
                run_ix: 0,
                glyph_ix: 2
            }],
        );

        let mut layout = make_layout(vec![glyph_at(0., 0), glyph_at(10., 0)]);
        layout.width = px(20.);
        layout.len = 1;
        assert!(
            layout
                .compute_wrap_boundaries("a", px(10.), None, &mut WrapScratch::default())
                .is_empty()
        );
    }

    #[test]
    fn test_wrap_right_to_left_text() {
        for (text, indices, expected_boundaries) in [
            ("אבג דהו", vec![11, 9, 7, 6, 4, 2, 0], vec![4]),
            ("a אבג b", vec![0, 1, 6, 4, 2, 8, 9], vec![2, 6]),
        ] {
            let mut layout = make_layout(
                indices
                    .iter()
                    .enumerate()
                    .map(|(position, index)| glyph_at(position as f32 * 10., *index))
                    .collect(),
            );
            layout.width = px(indices.len() as f32 * 10.);
            layout.len = text.len();
            assert_eq!(
                layout.compute_wrap_boundaries(text, px(40.), None, &mut WrapScratch::default()),
                expected_boundaries
                    .into_iter()
                    .map(|glyph_ix| WrapBoundary {
                        run_ix: 0,
                        glyph_ix
                    })
                    .collect::<SmallVec<[WrapBoundary; 1]>>(),
                "{text:?}",
            );
        }
    }

    #[test]
    fn test_wrap_ignores_breaks_inside_shaped_clusters() {
        let text = "x a-bcde";
        let mut layout = make_layout(
            [0, 1, 2, 5, 6, 7]
                .into_iter()
                .map(|index| glyph_at(index as f32 * 10., index))
                .collect(),
        );
        layout.width = px(80.);
        layout.len = text.len();
        assert_eq!(
            layout
                .compute_wrap_boundaries(text, px(60.), None, &mut WrapScratch::default())
                .as_slice(),
            &[WrapBoundary {
                run_ix: 0,
                glyph_ix: 2
            }],
        );
    }

    #[test]
    fn test_force_width_latin_unchanged() {
        let cell_width = px(8.);
        let mut layout = make_layout(vec![glyph_at(0., 0), glyph_at(8., 1), glyph_at(16., 2)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 8., 16.]);
    }

    #[test]
    fn test_force_width_combining_marks_not_advanced() {
        let cell_width = px(8.);
        // Simulates Thai "กี" — base consonant at x=0, combining vowel also at x=0
        let mut layout = make_layout(vec![
            glyph_at(0., 0), // ก (base)
            glyph_at(0., 3), // ี (combining mark, same x)
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0.]);
    }

    #[test]
    fn test_force_width_base_after_combining_mark() {
        let cell_width = px(8.);
        let mut layout = make_layout(vec![glyph_at(0., 0), glyph_at(0., 3), glyph_at(8., 6)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0., 8.]);
    }

    #[test]
    fn test_force_width_multiple_combining_marks() {
        let cell_width = px(8.);
        // Simulates "ก้" — base + vowel + tone mark (two combining marks stacked)
        let mut layout = make_layout(vec![
            glyph_at(0., 0), // ก (base)
            glyph_at(0., 3), // vowel (combining)
            glyph_at(0., 6), // tone mark (combining)
            glyph_at(8., 9), // next base
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0., 0., 8.]);
    }

    #[test]
    fn test_force_width_corrects_drifted_base_positions() {
        let cell_width = px(8.);
        // Font metrics don't perfectly match cell grid — glyphs drift >1px from cell boundary
        let mut layout = make_layout(vec![
            glyph_at(0.5, 0),  // within 1px tolerance, kept as-is
            glyph_at(10.2, 1), // >1px off from 8.0, corrected
            glyph_at(19.8, 2), // >1px off from 16.0, corrected
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0.5, 8., 16.]);
    }

    #[test]
    fn test_force_width_combining_mark_after_within_tolerance_base() {
        let cell_width = px(8.);
        // Base glyph is within 1px of grid so it keeps its shaped position.
        // The combining mark must align to the base's actual position, not the grid slot.
        let mut layout = make_layout(vec![glyph_at(0.5, 0), glyph_at(0.5, 3)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0.5, 0.5]);
    }
}
