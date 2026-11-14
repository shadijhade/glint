//! Ported from src/lib/usage.ts.
//!
//! The disk usage map: a folder's biggest files and folders a few levels down, as the index service, pdu or
//! an archive's own list of entries drew it. This module makes the map of the items `disk_usage` measured, lays
//! it out as a squarified treemap, finds what is under a point, works out what grew, and prunes or narrows a map.
//!
//! Pixels are `f32` in the layout while the geometry math itself runs in `f64`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

use crate::entries::{attr, with_details, FileEntry, RawEntry};
use crate::js::{js_max, js_min};
use crate::paths::join_path;

/// The folder mapped has no parent.
pub const NO_PARENT: u32 = 0xffff_ffff;

/// Who drew a map: `"index"`, `"pdu"`, or (a folder in an archive) the archive's own list of entries, by its
/// format ("zip", "7z"…).
pub type UsageEngine = &'static str;

/// One item of a map as it was measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageItem {
    /// Where the folder it's in is in the map; [`NO_PARENT`] for the folder mapped, always first.
    pub parent: u32,
    pub name: String,
    /// Files: their size. Folders: everything below them.
    pub size: u64,
    /// Last write time, Unix ms (0 when unknown).
    pub modified: i64,
    /// FILE_ATTRIBUTE_* bits; folders have the directory bit.
    pub attrs: u32,
}

/// A disk-usage map of a folder: its biggest files and folders a few levels down. Node 0 is the folder
/// itself; every folder's items are stored together, biggest first, after the folder.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageMap {
    pub path: String,
    /// The index, pdu, or (a folder in an archive) the archive's own list of entries.
    pub engine: UsageEngine,
    /// Milliseconds Rust spent: the index's answer, or pdu's whole run.
    pub elapsed: f64,
    /// Smaller items were left out; they still count in their folders' sizes.
    pub limited: bool,
    /// pdu drew it because the instant search service runs, but is a version from before maps.
    pub service_too_old: bool,
    /// What grew was asked for, but the instant search service running is a version from before it.
    pub growth_too_old: bool,
    /// What each item held at the index's daily mark closest to the time asked (index maps only).
    pub growth: Option<UsageGrowth>,
    pub count: usize,
    pub size: Vec<f64>,
    pub modified: Vec<f64>,
    /// FILE_ATTRIBUTE_* bits; folders have the directory bit.
    pub attrs: Vec<u32>,
    pub parent: Vec<u32>,
    pub names: Vec<String>,
    pub paths: Vec<String>,
    /// A folder's items are `first[i]` up to (not including) `end[i]`.
    pub first: Vec<u32>,
    pub end: Vec<u32>,
}

/// What the items of a map held at a daily mark of the index's.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageGrowth {
    /// When the mark was taken (ms).
    pub since: f64,
    /// The oldest mark there is: how far back growth goes.
    pub oldest: f64,
    /// The whole drive's total then and now.
    pub drive_then: f64,
    pub drive_now: f64,
    /// Each item's size at the mark, or [`WAS_NOT_THERE`], or [`WRITTEN_SINCE`] (a file; what it held isn't kept).
    pub previous: Vec<f64>,
}

/// An item's size at the mark when it was not there.
pub const WAS_NOT_THERE: f64 = -1.0;
/// An item's size at the mark when it is a file written since (what it held isn't kept).
pub const WRITTEN_SINCE: f64 = -2.0;

/// Why the items measured make no map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageError {
    /// A node's parent does not come before it.
    BadParent,
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageError::BadParent => f.write_str("a node's parent does not come before it in the usage map"),
        }
    }
}

impl UsageMap {
    /// The map of `path` made of its items as measured: the folder first, each item after the folder it's in.
    /// Who drew it and how (`engine`, `elapsed`, the flags, `growth`) are the caller's to set: it's drawn by pdu,
    /// at once, until then. An item whose parent doesn't come before it makes no map.
    pub fn of_items(path: String, items: Vec<UsageItem>) -> Result<UsageMap, UsageError> {
        let count = items.len();
        let (mut size, mut modified) = (Vec::with_capacity(count), Vec::with_capacity(count));
        let (mut attrs, mut parent) = (Vec::with_capacity(count), Vec::with_capacity(count));
        let (mut names, mut paths) = (Vec::with_capacity(count), Vec::<String>::with_capacity(count));
        let (mut first, mut end) = (vec![0u32; count], vec![0u32; count]);
        for (i, item) in items.into_iter().enumerate() {
            size.push(item.size as f64);
            modified.push(item.modified as f64);
            attrs.push(item.attrs);
            parent.push(item.parent);
            if i == 0 {
                paths.push(path.clone());
                names.push(item.name);
                continue;
            }
            let p = item.parent as usize;
            if p >= i {
                return Err(UsageError::BadParent);
            }
            // `p < i`, so `paths`, `first` and `end` all have it.
            paths.push(join_path(&paths[p], &item.name));
            names.push(item.name);
            if end[p] == 0 {
                first[p] = i as u32; // i < count, which came from a u32
            }
            end[p] = i as u32 + 1;
        }
        Ok(UsageMap {
            path,
            engine: "pdu",
            elapsed: 0.0,
            limited: false,
            service_too_old: false,
            growth_too_old: false,
            growth: None,
            count,
            size,
            modified,
            attrs,
            parent,
            names,
            paths,
            first,
            end,
        })
    }
}

impl std::error::Error for UsageError {}

/// Whether node `i` is a folder (it has the directory bit).
pub fn is_folder(map: &UsageMap, i: usize) -> bool {
    map.attrs.get(i).is_some_and(|a| a & attr::DIRECTORY != 0)
}

fn size_at(map: &UsageMap, i: usize) -> f64 {
    map.size.get(i).copied().unwrap_or(0.0)
}

fn parent_at(map: &UsageMap, i: usize) -> u32 {
    map.parent.get(i).copied().unwrap_or(NO_PARENT)
}

/// The nodes in folder `i`, as the indices `first[i]..end[i]`. They always come after the folder; a
/// range that does not (a broken map) or runs past the nodes is cut to what exists.
fn children(map: &UsageMap, i: usize) -> Range<usize> {
    let first = map.first.get(i).map_or(0, |&v| v as usize);
    let end = map.end.get(i).map_or(0, |&v| v as usize);
    first.max(i + 1)..end.min(map.count)
}

/// The folders above node `i`, nearest first, up to the folder mapped. Stops at a broken parent link
/// (and after at most `count` steps, so a cycle cannot loop).
fn ancestors(map: &UsageMap, i: usize) -> impl Iterator<Item = usize> + '_ {
    let mut p = parent_at(map, i);
    let mut steps = 0;
    std::iter::from_fn(move || {
        if p == NO_PARENT || steps > map.count {
            return None;
        }
        let at = p as usize;
        if at >= map.count {
            return None;
        }
        steps += 1;
        p = parent_at(map, at);
        Some(at)
    })
}

/// How much item `i` grew since the mark (all of it, if it wasn't there); 0 for a file written since.
pub fn grew_by(map: &UsageMap, i: usize) -> f64 {
    let now = size_at(map, i);
    let then = map
        .growth
        .as_ref()
        .and_then(|g| g.previous.get(i).copied())
        .unwrap_or(now);
    if then == WAS_NOT_THERE {
        now
    } else if then == WRITTEN_SINCE {
        0.0
    } else {
        now - then
    }
}

/// Where most of the growth went: the deepest folders that grew by at least 5% of what the folder
/// mapped grew and that no folder in them holds 70% of. Biggest first, at most `max` (the TypeScript's
/// default was 3).
pub fn growth_leaders(map: &UsageMap, max: usize) -> Vec<usize> {
    let total = grew_by(map, 0);
    if map.growth.is_none() || total <= 0.0 {
        return Vec::new();
    }
    // A set that keeps the order things went in, as a JS Set does.
    let mut leaders: Vec<usize> = Vec::new();
    for i in 1..map.count {
        let grew = grew_by(map, i);
        if !is_folder(map, i) || grew < total * 0.05 {
            continue;
        }
        let concentrated = children(map, i).any(|c| is_folder(map, c) && grew_by(map, c) >= grew * 0.7);
        if !concentrated {
            leaders.push(i);
        }
    }
    // A folder with one of them inside is told by that one.
    let snapshot = leaders.clone();
    for &i in &snapshot {
        for p in ancestors(map, i) {
            leaders.retain(|&l| l != p);
        }
    }
    // A stable sort, biggest growth first.
    leaders.sort_by(|&a, &b| {
        grew_by(map, b)
            .partial_cmp(&grew_by(map, a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    leaders.truncate(max);
    leaders
}

/// Every node as a file-list entry (the folder mapped first), for selection, menus and Quick Look.
pub fn usage_entries(map: &UsageMap) -> Vec<FileEntry> {
    (0..map.count)
        .map(|i| {
            with_details(
                RawEntry {
                    name: map.names.get(i).cloned().unwrap_or_default(),
                    path: map.paths.get(i).cloned(),
                    dir: is_folder(map, i),
                    // Float to integer casts saturate (and read NaN as 0) in Rust.
                    size: size_at(map, i) as u64,
                    modified: map.modified.get(i).copied().unwrap_or(0.0) as i64,
                    attrs: map.attrs.get(i).copied().unwrap_or(0),
                    origin: None,
                },
                "",
            )
        })
        .collect()
}

/// How a node is drawn: not at all.
pub const HIDDEN: u8 = 0;
/// A file, or a folder drawn whole (nothing to show inside, or too small to).
pub const BLOCK: u8 = 1;
/// A folder drawn as a frame (with a header when there's room) around its items.
pub const FRAME: u8 = 2;

/// A frame's header height, in CSS pixels.
pub const HEADER: f64 = 17.0;
const PAD: f64 = 2.0;
const MIN_FRAME: f64 = 22.0;
const MIN_HEADER_W: f64 = 60.0;
const MIN_HEADER_H: f64 = 42.0;

/// Where every node of a map is drawn. All the arrays have one entry per node, except `order`.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageLayout {
    pub width: f64,
    pub height: f64,
    /// [`HIDDEN`], [`BLOCK`] or [`FRAME`].
    pub kind: Vec<u8>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub w: Vec<f32>,
    pub h: Vec<f32>,
    /// A frame's header height, 0 when it has none.
    pub header: Vec<f32>,
    /// Each frame's room for everything too small to list ("smaller items"); width 0 when none.
    pub rest_x: Vec<f32>,
    pub rest_y: Vec<f32>,
    pub rest_w: Vec<f32>,
    pub rest_h: Vec<f32>,
    /// Frame depth: 0 for the folder mapped (wraps at 256, as the TypeScript's `Uint8Array` did).
    pub depth: Vec<u8>,
    /// Nodes in drawing order, every frame before what's inside it.
    pub order: Vec<u32>,
}

/// Lays `values` (largest first) out in the box as a squarified treemap (Bruls, Huizing and van Wijk):
/// rows along the shorter side, each grown while that keeps its rectangles closer to square.
/// `place(k, x, y, w, h)` is told where value `k` goes.
pub fn squarify(
    values: &[f64],
    mut x: f64,
    mut y: f64,
    mut w: f64,
    mut h: f64,
    mut place: impl FnMut(usize, f64, f64, f64, f64),
) {
    let total: f64 = values.iter().sum();
    if total <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let scale = (w * h) / total;
    let mut i = 0;
    while i < values.len() {
        let wide = w >= h;
        let side = if wide { h } else { w };
        let mut sum = 0.0;
        let mut min = f64::INFINITY;
        let mut max = 0.0;
        let mut worst = f64::INFINITY;
        let mut j = i;
        while j < values.len() {
            let area = values[j] * scale;
            let s = sum + area;
            let lo = js_min(min, area);
            let hi = js_max(max, area);
            let ratio = js_max((side * side * hi) / (s * s), (s * s) / (side * side * lo));
            if j > i && ratio > worst {
                break;
            }
            (sum, min, max, worst) = (s, lo, hi, ratio);
            j += 1;
        }
        // The last row takes whatever is left, so rounding never leaves a gap.
        let room = if wide { w } else { h };
        let thick = if j == values.len() {
            room
        } else {
            js_min(sum / side, room)
        };
        let mut at = 0.0;
        for (k, &value) in values.iter().enumerate().take(j).skip(i) {
            let len = if k == j - 1 { side - at } else { (value * scale) / thick };
            if wide {
                place(k, x, y + at, thick, len);
            } else {
                place(k, x + at, y, len, thick);
            }
            at += len;
        }
        if wide {
            (x, w) = (x + thick, w - thick);
        } else {
            (y, h) = (y + thick, h - thick);
        }
        i = j;
    }
}

/// Fills a [`UsageLayout`] node by node.
struct Placer<'a> {
    map: &'a UsageMap,
    layout: UsageLayout,
}

impl Placer<'_> {
    /// `i` is always below `map.count`: the root is checked, children come from [`children`].
    fn place(&mut self, i: usize, x: f64, y: f64, w: f64, h: f64, depth: usize) {
        if w < 0.5 || h < 0.5 {
            return;
        }
        let map = self.map;
        let l = &mut self.layout;
        (l.x[i], l.y[i], l.w[i], l.h[i], l.depth[i]) = (x as f32, y as f32, w as f32, h as f32, depth as u8);
        l.order.push(i as u32); // i < count, which came from a u32
        l.kind[i] = BLOCK;
        let items = children(map, i);
        if items.is_empty() || (i != 0 && (w < MIN_FRAME || h < MIN_FRAME)) {
            return;
        }
        // The folder mapped fills the box; others get a frame, and a header when there's room.
        let header = if i != 0 && w >= MIN_HEADER_W && h >= MIN_HEADER_H {
            HEADER
        } else {
            0.0
        };
        let pad = if i == 0 { 0.0 } else { PAD };
        let top = if header != 0.0 { header } else { pad };
        let (ix, iy, iw, ih) = (x + pad, y + top, w - 2.0 * pad, h - top - pad);
        if iw < 2.0 || ih < 2.0 {
            return;
        }
        l.kind[i] = FRAME;
        l.header[i] = header as f32;
        // What the listed items don't add up to is drawn as one block of smaller items, in size order.
        let mut listed = 0.0;
        for c in items.clone() {
            listed += size_at(map, c);
        }
        let own = size_at(map, i);
        let rest = if own - listed > listed * 1e-4 {
            own - listed
        } else {
            0.0
        };
        let mut values: Vec<f64> = Vec::with_capacity(items.len() + 1);
        let mut rest_at: Option<usize> = None;
        for c in items.clone() {
            if rest_at.is_none() && rest > size_at(map, c) {
                values.push(rest);
                rest_at = Some(values.len() - 1);
            }
            values.push(js_max(size_at(map, c), 0.0));
        }
        if rest_at.is_none() && rest > 0.0 {
            values.push(rest);
            rest_at = Some(values.len() - 1);
        }
        let first = items.start;
        squarify(&values, ix, iy, iw, ih, |k, cx, cy, cw, ch| {
            if Some(k) == rest_at {
                let l = &mut self.layout;
                (l.rest_x[i], l.rest_y[i], l.rest_w[i], l.rest_h[i]) = (cx as f32, cy as f32, cw as f32, ch as f32);
            } else {
                let after_rest = rest_at.is_some_and(|r| k > r);
                self.place(first + k - usize::from(after_rest), cx, cy, cw, ch, depth + 1);
            }
        });
    }
}

/// Lays the map out in a `width` × `height` box (CSS pixels).
pub fn layout_usage(map: &UsageMap, width: f64, height: f64) -> UsageLayout {
    let n = map.count;
    let layout = UsageLayout {
        width,
        height,
        kind: vec![HIDDEN; n],
        x: vec![0.0; n],
        y: vec![0.0; n],
        w: vec![0.0; n],
        h: vec![0.0; n],
        header: vec![0.0; n],
        rest_x: vec![0.0; n],
        rest_y: vec![0.0; n],
        rest_w: vec![0.0; n],
        rest_h: vec![0.0; n],
        depth: vec![0; n],
        order: Vec::with_capacity(n),
    };
    let mut placer = Placer { map, layout };
    if n > 0 && width > 0.0 && height > 0.0 {
        placer.place(0, 0.0, 0.0, width, height, 0);
    }
    placer.layout
}

fn inside(l: &UsageLayout, i: usize, x: f64, y: f64) -> bool {
    let (lx, ly, lw, lh) = (
        f64::from(l.x[i]),
        f64::from(l.y[i]),
        f64::from(l.w[i]),
        f64::from(l.h[i]),
    );
    lw > 0.0 && x >= lx && x < lx + lw && y >= ly && y < ly + lh
}

/// What is under a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The innermost item.
    pub node: usize,
    /// The point is on its frame's smaller items.
    pub rest: bool,
}

/// What's under a point: the innermost item, and whether the point is on its frame's smaller items.
/// `layout` must be the layout of `map`.
pub fn hit_test(map: &UsageMap, layout: &UsageLayout, x: f64, y: f64) -> Option<Hit> {
    if map.count == 0 || layout.kind.len() < map.count || !inside(layout, 0, x, y) {
        return None;
    }
    let mut node = 0;
    while layout.kind[node] == FRAME {
        match children(map, node).find(|&c| inside(layout, c, x, y)) {
            Some(c) => node = c,
            None => break,
        }
    }
    let (rx, ry, rw, rh) = (
        f64::from(layout.rest_x[node]),
        f64::from(layout.rest_y[node]),
        f64::from(layout.rest_w[node]),
        f64::from(layout.rest_h[node]),
    );
    let rest = rw > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh;
    Some(Hit { node, rest })
}

/// The nodes of `map` that `place` gives a new place (-1 for the others), in their order, which keeps
/// a map's: a kept node's folder must be kept, and come first. `size` replaces the sizes.
fn pick(map: &UsageMap, place: &[i32], kept: usize, size: &[f64]) -> UsageMap {
    let index: Vec<usize> = (0..map.count)
        .filter(|&i| place.get(i).is_some_and(|&p| p >= 0))
        .collect();
    let take_f64 = |from: &[f64]| -> Vec<f64> { index.iter().map(|&i| from.get(i).copied().unwrap_or(0.0)).collect() };
    let take_u32 = |from: &[u32]| -> Vec<u32> { index.iter().map(|&i| from.get(i).copied().unwrap_or(0)).collect() };
    let parent: Vec<u32> = index
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            if k == 0 {
                NO_PARENT
            } else {
                // A parent that was not kept reads as -1, which a Uint32Array turned into NO_PARENT too.
                let p = map
                    .parent
                    .get(i)
                    .and_then(|&p| place.get(p as usize))
                    .copied()
                    .unwrap_or(-1);
                u32::try_from(p).unwrap_or(NO_PARENT)
            }
        })
        .collect();
    let mut first = vec![0u32; kept];
    let mut end = vec![0u32; kept];
    for k in 1..kept {
        let Some(&p) = parent.get(k) else { break };
        if let (Some(f), Some(e)) = (first.get_mut(p as usize), end.get_mut(p as usize)) {
            if *e == 0 {
                *f = k as u32; // k < kept <= count, which came from a u32
            }
            *e = k as u32 + 1;
        }
    }
    UsageMap {
        count: kept,
        size: take_f64(size),
        modified: take_f64(&map.modified),
        growth: map.growth.as_ref().map(|g| UsageGrowth {
            previous: take_f64(&g.previous),
            ..g.clone()
        }),
        attrs: take_u32(&map.attrs),
        parent,
        names: index
            .iter()
            .map(|&i| map.names.get(i).cloned().unwrap_or_default())
            .collect(),
        paths: index
            .iter()
            .map(|&i| map.paths.get(i).cloned().unwrap_or_default())
            .collect(),
        first,
        end,
        ..map.clone()
    }
}

/// The map without the items at `gone` (and what's in them), its folders' sizes reduced to match.
/// Returns the map itself, borrowed, when nothing in it is gone.
pub fn prune_usage<'a>(map: &'a UsageMap, gone: &HashSet<String>) -> Cow<'a, UsageMap> {
    let mut place = vec![-1i32; map.count];
    let mut size = map.size.clone();
    let mut kept = 0usize;
    for i in 0..map.count {
        if i > 0 {
            let parent_kept = place.get(parent_at(map, i) as usize).is_some_and(|&p| p >= 0);
            let removed = map.paths.get(i).is_some_and(|p| gone.contains(p.as_str()));
            if !parent_kept || removed {
                // Only the topmost item removed takes its size off the folders above it.
                if parent_kept {
                    let off = size_at(map, i);
                    for p in ancestors(map, i) {
                        if let Some(s) = size.get_mut(p) {
                            *s -= off;
                        }
                    }
                }
                continue;
            }
        }
        place[i] = kept as i32; // kept <= count; maps are far below 2^31 nodes
        kept += 1;
    }
    if kept == map.count {
        Cow::Borrowed(map)
    } else {
        Cow::Owned(pick(map, &place, kept, &size))
    }
}

/// The part of `map` below its folder `node`, as a map of that folder (one level shallower).
/// `None` when `node` is not a node of the map.
pub fn sub_usage(map: &UsageMap, node: usize) -> Option<UsageMap> {
    if node >= map.count {
        return None;
    }
    let mut place = vec![-1i32; map.count];
    place[node] = 0;
    let mut kept = 1usize;
    // Everything below a node comes after it.
    for i in node + 1..map.count {
        if place.get(parent_at(map, i) as usize).is_some_and(|&p| p >= 0) {
            place[i] = kept as i32; // kept <= count; maps are far below 2^31 nodes
            kept += 1;
        }
    }
    Some(UsageMap {
        path: map.paths.get(node).cloned().unwrap_or_default(),
        ..pick(map, &place, kept, &map.size)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Node {
        name: &'static str,
        parent: u32,
        dir: bool,
        size: f64,
        modified: f64,
        attrs: u32,
    }

    fn node(name: &'static str, parent: u32, dir: bool, size: f64) -> Node {
        Node {
            name,
            parent,
            dir,
            size,
            modified: 0.0,
            attrs: 0,
        }
    }

    fn items(nodes: &[Node]) -> Vec<UsageItem> {
        nodes
            .iter()
            .map(|n| UsageItem {
                parent: n.parent,
                name: n.name.to_string(),
                size: n.size as u64,
                modified: n.modified as i64,
                attrs: n.attrs | if n.dir { attr::DIRECTORY } else { 0 },
            })
            .collect()
    }

    // C:\ 1000: Windows 600 (System32 400 (big.dll 300), notes.txt 50), Users 300 (é😀.mkv 250), pagefile.sys 60;
    // 40 bytes of smaller items directly in C:\, 150 in Windows, 100 in System32, 50 in Users.
    fn sample() -> Vec<Node> {
        vec![
            node("", NO_PARENT, true, 1000.0),
            node("Windows", 0, true, 600.0),
            node("Users", 0, true, 300.0),
            Node {
                attrs: 0x26,
                ..node("pagefile.sys", 0, false, 60.0)
            },
            node("System32", 1, true, 400.0),
            Node {
                modified: 1.7e12,
                ..node("notes.txt", 1, false, 50.0)
            },
            node("é😀.mkv", 2, false, 250.0),
            node("big.dll", 4, false, 300.0),
        ]
    }

    fn usage() -> UsageMap {
        UsageMap {
            engine: "index",
            elapsed: 3.5,
            limited: true,
            ..UsageMap::of_items("C:\\".into(), items(&sample())).unwrap()
        }
    }

    fn grown() -> UsageMap {
        let previous = vec![700.0, 200.0, 350.0, 60.0, WAS_NOT_THERE, WRITTEN_SINCE, 250.0, 300.0];
        UsageMap {
            engine: "index",
            elapsed: 1.0,
            growth: Some(UsageGrowth {
                since: 1e12,
                oldest: 9e11,
                drive_then: 5000.0,
                drive_now: 5300.0,
                previous,
            }),
            ..UsageMap::of_items("C:\\".into(), items(&sample())).unwrap()
        }
    }

    #[test]
    fn a_map_is_made_of_its_items() {
        let usage = usage();
        assert_eq!(
            (
                usage.path.as_str(),
                usage.engine,
                usage.elapsed,
                usage.limited,
                usage.service_too_old,
                usage.count
            ),
            ("C:\\", "index", 3.5, true, false, 8)
        );
        assert_eq!(
            usage.paths,
            [
                "C:\\",
                "C:\\Windows",
                "C:\\Users",
                "C:\\pagefile.sys",
                "C:\\Windows\\System32",
                "C:\\Windows\\notes.txt",
                "C:\\Users\\é😀.mkv",
                "C:\\Windows\\System32\\big.dll"
            ]
        );
        assert_eq!(usage.first, [1, 4, 6, 0, 7, 0, 0, 0]);
        assert_eq!(usage.end, [4, 6, 7, 0, 8, 0, 0, 0]);
        assert!(usage.growth.is_none());
    }

    #[test]
    fn turns_nodes_into_entries() {
        let mapped = usage_entries(&usage());
        assert_eq!(
            (
                mapped[3].name.as_str(),
                mapped[3].ext.as_str(),
                mapped[3].attrs,
                mapped[4].dir,
                mapped[5].modified,
                mapped[6].ext.as_str()
            ),
            ("pagefile.sys", "sys", 0x26, true, 1_700_000_000_000, "mkv")
        );
    }

    #[test]
    fn an_item_whose_folder_comes_after_it_makes_no_map() {
        let mut nodes = sample();
        nodes[1].parent = 5;
        assert_eq!(
            UsageMap::of_items("C:\\".into(), items(&nodes)),
            Err(UsageError::BadParent)
        );
        assert_eq!(UsageMap::of_items("C:\\".into(), Vec::new()).map(|m| m.count), Ok(0));
    }

    #[test]
    fn works_out_what_grew() {
        // What grew since a mark comes with the dates: each item's size then (or that it wasn't there, or a
        // file written since). Windows grew 400 from 200, all of it System32 (new); Users shrank by 50.
        let usage = usage();
        let grown = grown();
        let g = grown.growth.as_ref().unwrap();
        assert_eq!(
            (g.since, g.oldest, g.drive_then, g.drive_now),
            (1e12, 9e11, 5000.0, 5300.0)
        );
        assert_eq!((&grown.paths, &grown.size), (&usage.paths, &usage.size));
        let by: Vec<f64> = [0, 1, 2, 4, 5].iter().map(|&i| grew_by(&grown, i)).collect();
        assert_eq!(by, [300.0, 400.0, -50.0, 400.0, 0.0]);
        assert_eq!(
            growth_leaders(&grown, 3),
            [4],
            "System32 tells where Windows' growth went"
        );
        assert!(growth_leaders(&usage, 3).is_empty());
        assert_eq!(grew_by(&usage, 1), 0.0);
        assert_eq!(
            sub_usage(&grown, 1).unwrap().growth.unwrap().previous,
            [200.0, WAS_NOT_THERE, WRITTEN_SINCE, 300.0]
        );
        let gone = HashSet::from(["C:\\Users".to_string()]);
        assert_eq!(
            prune_usage(&grown, &gone).growth.as_ref().unwrap().previous,
            [700.0, 200.0, 60.0, WAS_NOT_THERE, WRITTEN_SINCE, 300.0]
        );
    }

    #[test]
    fn squarifies_a_box() {
        let values = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let mut boxes = vec![[0.0; 4]; values.len()];
        squarify(&values, 0.0, 0.0, 6.0, 4.0, |k, x, y, w, h| boxes[k] = [x, y, w, h]);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        for (k, b) in boxes.iter().enumerate() {
            assert!(near(b[2] * b[3], values[k]), "area of {k}");
        }
        for a in 0..boxes.len() {
            for b in a + 1..boxes.len() {
                let [ax, ay, aw, ah] = boxes[a];
                let [bx, by, bw, bh] = boxes[b];
                assert!(
                    ax + aw <= bx + 1e-6 || bx + bw <= ax + 1e-6 || ay + ah <= by + 1e-6 || by + bh <= ay + 1e-6,
                    "{a} and {b} overlap"
                );
            }
        }
        // The first two (the biggest) share the first row along the short side, as squares would.
        let rounded = |b: [f64; 4]| b.map(|v| (v * 1e6).round() / 1e6);
        assert_eq!(rounded(boxes[0]), [0.0, 0.0, 3.0, 2.0]);
        assert_eq!(rounded(boxes[1]), [0.0, 2.0, 3.0, 2.0]);
        // Nothing to lay out, or no room.
        let mut called = false;
        squarify(&[], 0.0, 0.0, 6.0, 4.0, |_, _, _, _, _| called = true);
        squarify(&values, 0.0, 0.0, 0.0, 4.0, |_, _, _, _, _| called = true);
        squarify(&[0.0, 0.0], 0.0, 0.0, 6.0, 4.0, |_, _, _, _, _| called = true);
        assert!(!called);
    }

    #[test]
    fn lays_out_the_map() {
        let usage = usage();
        let layout = layout_usage(&usage, 800.0, 500.0);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let f = |v: f32| f64::from(v);
        let within = |i: usize, p: usize| {
            f(layout.x[i]) >= f(layout.x[p]) - 1e-3
                && f(layout.y[i]) >= f(layout.y[p]) - 1e-3
                && f(layout.x[i]) + f(layout.w[i]) <= f(layout.x[p]) + f(layout.w[p]) + 1e-3
                && f(layout.y[i]) + f(layout.h[i]) <= f(layout.y[p]) + f(layout.h[p]) + 1e-3
        };
        assert_eq!(
            (layout.x[0], layout.y[0], layout.w[0], layout.h[0]),
            (0.0, 0.0, 800.0, 500.0)
        );
        for i in 1..usage.count {
            assert!(
                within(i, usage.parent[i] as usize),
                "{} inside its folder",
                usage.names[i]
            );
        }
        assert_eq!(layout.kind, [FRAME, FRAME, FRAME, BLOCK, FRAME, BLOCK, BLOCK, BLOCK]);
        assert_eq!(
            layout.order,
            [0, 1, 4, 7, 5, 2, 6, 3],
            "frames before what's inside them"
        );
        // The folder mapped is split by size exactly: Windows 60%, Users 30%, pagefile.sys 6%, smaller items 4%.
        let area = |i: usize| f(layout.w[i]) * f(layout.h[i]);
        assert!(near(area(1) / 400_000.0, 0.6) && near(area(2) / 400_000.0, 0.3) && near(area(3) / 400_000.0, 0.06));
        assert!(near(f(layout.rest_w[0]) * f(layout.rest_h[0]) / 400_000.0, 0.04));
        assert!(layout.header[1] > 0.0, "big frames have a header");
        // Smaller items fill what's left of a frame: System32's 100 of 400 is a quarter of its inside.
        let rest4 = f(layout.rest_w[4]) * f(layout.rest_h[4]);
        assert!(near(rest4 / (area(7) + rest4), 0.25));
    }

    #[test]
    fn finds_what_is_under_a_point() {
        // What's under the pointer: the innermost item; on a frame's header, the frame; on smaller items, its folder.
        let usage = usage();
        let layout = layout_usage(&usage, 800.0, 500.0);
        let f = |v: f32| f64::from(v);
        let centre = |i: usize| {
            (
                f(layout.x[i]) + f(layout.w[i]) / 2.0,
                f(layout.y[i]) + f(layout.h[i]) / 2.0,
            )
        };
        let hit = |(x, y): (f64, f64)| hit_test(&usage, &layout, x, y);
        assert_eq!(hit(centre(7)), Some(Hit { node: 7, rest: false }));
        assert_eq!(hit(centre(3)), Some(Hit { node: 3, rest: false }));
        assert_eq!(
            hit((f(layout.x[1]) + 5.0, f(layout.y[1]) + 5.0)),
            Some(Hit { node: 1, rest: false })
        );
        assert_eq!(
            hit((
                f(layout.rest_x[4]) + f(layout.rest_w[4]) / 2.0,
                f(layout.rest_y[4]) + f(layout.rest_h[4]) / 2.0
            )),
            Some(Hit { node: 4, rest: true })
        );
        assert_eq!(hit((900.0, 10.0)), None);
    }

    #[test]
    fn draws_small_boxes_whole_or_not_at_all() {
        // A box too small for anything inside draws its folder whole.
        let usage = usage();
        let tiny = layout_usage(&usage, 30.0, 20.0);
        assert!(tiny.kind[1] == BLOCK || tiny.kind[1] == HIDDEN);
        assert_eq!(tiny.kind[7], HIDDEN);
        assert_eq!(layout_usage(&usage, 0.0, 0.0).order.len(), 0);
    }

    #[test]
    fn prunes_deleted_items() {
        // Deleting from the map: the item and what's in it go, and every folder above it shrinks.
        let usage = usage();
        let gone = HashSet::from(["C:\\Windows\\System32".to_string()]);
        let pruned = prune_usage(&usage, &gone);
        assert_eq!(
            pruned.names,
            ["", "Windows", "Users", "pagefile.sys", "notes.txt", "é😀.mkv"]
        );
        assert_eq!(pruned.size, [600.0, 200.0, 300.0, 60.0, 50.0, 250.0]);
        assert_eq!(pruned.parent, [NO_PARENT, 0, 0, 0, 1, 2]);
        assert_eq!(pruned.first, [1, 4, 5, 0, 0, 0]);
        assert_eq!(pruned.end, [4, 5, 6, 0, 0, 0]);
        let nothing = HashSet::from(["C:\\nothing".to_string()]);
        assert!(matches!(prune_usage(&usage, &nothing), Cow::Borrowed(m) if std::ptr::eq(m, &usage)));
    }

    #[test]
    fn goes_into_a_folder_of_a_map() {
        // Going into a folder of a map: its part of the map, as a map of that folder.
        let usage = usage();
        let sub = sub_usage(&usage, 1).unwrap();
        assert_eq!((sub.path.as_str(), sub.count), ("C:\\Windows", 4));
        assert_eq!(sub.names, ["Windows", "System32", "notes.txt", "big.dll"]);
        assert_eq!(sub.parent, [NO_PARENT, 0, 0, 1]);
        assert_eq!(sub.size, [600.0, 400.0, 50.0, 300.0]);
        assert_eq!(
            (sub.first.as_slice(), sub.end.as_slice()),
            (&[1, 3, 0, 0][..], &[3, 4, 0, 0][..])
        );
        assert_eq!(sub_usage(&usage, 3).unwrap().names, ["pagefile.sys"]);
        assert!(sub_usage(&usage, 99).is_none());
    }
}
