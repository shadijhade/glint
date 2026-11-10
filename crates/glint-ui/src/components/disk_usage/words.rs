//! What the map says in words: shares, signed sizes, dates of marks, the growth lines, and the lines of the hover
//! tooltip. Ported from the helpers in src/components/explorer/DiskUsage.tsx (`share`, `signed`, `markDate`,
//! `growthLine`, `Tooltip`). Pure functions of a map, so they are tested without a window.

use crate::i18n;
use crate::t;
use glint_model::entries::FileEntry;
use glint_model::file_types::type_label;
use glint_model::format::{format_count, format_date, format_size};
use glint_model::i18n::translate;
use glint_model::usage::{is_folder, UsageMap, WAS_NOT_THERE, WRITTEN_SINCE};
use gpui_kit::SharedString;

/// An item's share of the whole, as people read it: "42%", "3.1%", "<0.1%" (`share`).
pub fn share(bytes: f64, total: f64) -> String {
    if total <= 0. || total.is_nan() {
        return "0%".to_string();
    }
    let pct = bytes / total * 100.;
    if pct >= 10. {
        // Math.round: halves go up.
        format!("{}%", (pct + 0.5).floor())
    } else if pct >= 0.1 {
        format!("{pct:.1}%")
    } else {
        "<0.1%".to_string()
    }
}

/// A size change with its sign: "+1.2 GB", "−340 MB" (`signed`).
pub fn signed(bytes: f64) -> String {
    let sign = if bytes < 0. { '−' } else { '+' };
    format!("{sign}{}", format_size(bytes.abs() as u64))
}

/// When a mark was taken, as the app writes dates elsewhere: "Today 9:02 AM", "Yesterday 12:00 AM", "Oct 16, 2026
/// 9:02 AM". The second is whether it reads as a day word, which "on" doesn't go before (`markDate`).
pub fn mark_date(ms: f64) -> (String, bool) {
    let language = i18n::language();
    let text = format_date(ms as i64, None, language);
    let day_word = [translate(language, "Today"), translate(language, "Yesterday")]
        .iter()
        .any(|word| text.starts_with(word));
    (text, day_word)
}

/// What an item was at the mark, against what it is (`growthLine`). Only for a map that has growth.
pub fn growth_line(map: &UsageMap, i: usize) -> Option<String> {
    let growth = map.growth.as_ref()?;
    let then = growth.previous.get(i).copied()?;
    let (when, day_word) = mark_date(growth.since);
    let on = if day_word {
        when.clone()
    } else {
        t!("on {when}", when = when.clone()).to_string()
    };
    Some(if then == WAS_NOT_THERE {
        t!("New since {when}", when = when).to_string()
    } else if then == WRITTEN_SINCE {
        t!("Written to since {when}", when = when).to_string()
    } else if !is_folder(map, i) {
        t!("Not written to since {when}", when = when).to_string()
    } else {
        let grew = map.size.get(i).copied().unwrap_or(0.) - then;
        if grew == 0. {
            t!("As big as {on}", on = on).to_string()
        } else {
            t!(
                "Was {size} {on} · {change}",
                size = format_size(then.max(0.) as u64),
                on = on,
                change = signed(grew)
            )
            .to_string()
        }
    })
}

/// The growth strip's headline: "Downloads grew 1.2 GB since Today 9:02 AM".
pub fn growth_headline(title: &str, grew: f64, since: f64) -> String {
    let (when, _) = mark_date(since);
    let change = if grew > 0. {
        t!("grew {size}", size = format_size(grew as u64))
    } else if grew < 0. {
        t!("shrank {size}", size = format_size((-grew) as u64))
    } else {
        t!("hasn't changed size")
    };
    t!(
        "{title} {change} since {when}",
        title = title,
        change = change,
        when = when
    )
    .to_string()
}

/// A folder's path under the folder mapped, as the strip names it: the last two parts (`relative`).
pub fn relative_to(root: &str, path: &str) -> String {
    let root = root.trim_end_matches('\\');
    let rest = path.get(root.len() + 1..).unwrap_or("");
    let parts: Vec<&str> = rest.split('\\').collect();
    parts[parts.len().saturating_sub(2)..].join("\\")
}

/// One line of a tooltip, and how it is set.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: SharedString,
    pub style: Style,
}

/// How a tooltip line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The name: medium weight.
    Title,
    /// A path or a hint: muted.
    Muted,
    /// The size line.
    Plain,
}

fn line(style: Style, text: impl Into<SharedString>) -> Line {
    Line {
        text: text.into(),
        style,
    }
}

/// What the tooltip over a node says (`Tooltip`): the name, where it is, how big, how much of the whole, what
/// it was at the mark (with growth colors), when it changed, and what a click does.
pub fn tooltip_lines(map: &UsageMap, entry: Option<&FileEntry>, node: usize, rest: bool, growing: bool) -> Vec<Line> {
    let total = map.size.first().copied().unwrap_or(0.);
    let path = map.paths.get(node).cloned().unwrap_or_default();
    if rest {
        let first = map.first.get(node).copied().unwrap_or(0) as usize;
        let end = map.end.get(node).copied().unwrap_or(0) as usize;
        let listed: f64 = (first..end).map(|k| map.size.get(k).copied().unwrap_or(0.)).sum();
        let bytes = (map.size.get(node).copied().unwrap_or(0.) - listed).max(0.);
        let mut lines = vec![
            line(Style::Title, t!("Smaller items")),
            line(Style::Muted, t!("in {path}", path = path)),
            line(
                Style::Plain,
                format!("{} · {}", format_size(bytes as u64), share(bytes, total)),
            ),
        ];
        if node != 0 {
            lines.push(line(Style::Muted, t!("Click to map this folder")));
        }
        return lines;
    }
    let folder = is_folder(map, node);
    let deeper = !folder && map.parent.get(node).copied().unwrap_or(0) != 0;
    let size = map.size.get(node).copied().unwrap_or(0.);
    let name = entry
        .map(|e| e.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| map.path.clone());
    let kind = if folder {
        t!("Folder").to_string()
    } else {
        entry.map(type_label).unwrap_or_default()
    };
    let mut sizes = format!("{} · {} · {kind}", format_size(size as u64), share(size, total));
    if size >= 1024. {
        sizes.push_str(&t!(" · {count} bytes", count = format_count(size as usize)));
    }
    let mut lines = vec![
        line(Style::Title, name),
        line(Style::Muted, path),
        line(Style::Plain, sizes),
    ];
    if growing {
        if let Some(growth) = growth_line(map, node) {
            lines.push(line(Style::Plain, growth));
        }
    }
    if let Some(e) = entry.filter(|e| e.modified > 0) {
        let when = format_date(e.modified, None, i18n::language());
        lines.push(line(Style::Muted, t!("Modified {when}", when = when)));
    }
    if node != 0 {
        lines.push(line(
            Style::Muted,
            if folder {
                t!("Click to map this folder")
            } else if deeper {
                t!("Click to go to its folder")
            } else {
                t!("Double-click to open")
            },
        ));
    }
    lines
}

/// The map's accessible name: where it is and its first five items (`aria-label`).
pub fn map_label(title: &str, map: Option<&UsageMap>) -> SharedString {
    let Some(map) = map else {
        return t!("Disk usage of {title}", title = title);
    };
    let items = map
        .names
        .iter()
        .zip(&map.size)
        .skip(1)
        .take(5)
        .map(|(name, size)| format!("{name} {}", format_size(*size as u64)))
        .collect::<Vec<_>>()
        .join(", ");
    t!("Disk usage of {title}: {items}", title = title, items = items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_read_as_the_typescript_wrote_them() {
        assert_eq!(share(50., 100.), "50%");
        assert_eq!(share(10., 100.), "10%");
        assert_eq!(share(9.96, 100.), "10.0%");
        assert_eq!(share(3.16, 100.), "3.2%");
        assert_eq!(share(0.12, 100.), "0.1%");
        assert_eq!(share(0.05, 100.), "<0.1%");
        assert_eq!(share(5., 0.), "0%");
        // Math.round(12.5) is 13.
        assert_eq!(share(12.5, 100.), "13%");
    }

    #[test]
    fn sizes_carry_their_sign() {
        assert_eq!(signed(1536.), "+1.5 KB");
        assert_eq!(signed(-1536.), "−1.5 KB");
        assert_eq!(signed(0.), "+0 B");
    }

    #[test]
    fn a_folder_is_named_by_its_last_two_parts() {
        assert_eq!(
            relative_to("C:\\Users", "C:\\Users\\me\\AppData\\Local\\Temp"),
            "Local\\Temp"
        );
        assert_eq!(relative_to("C:\\Users\\", "C:\\Users\\me"), "me");
        assert_eq!(relative_to("C:\\Users", "C:\\Users"), "");
        assert_eq!(relative_to("C:\\Users", "C:\\Users\\تقرير\\ملفات"), "تقرير\\ملفات");
    }
}
