//! Settings' pages: what the list on the left offers, in groups, and which sections each page shows; and the
//! search that finds a setting by its words.
//!
//! Settings opens on the Overview (how Glint is set up, with suggestions); a section asked for
//! (`open_at("settings-about")`) opens the page that has it. Each page is short enough to read at a glance, so
//! one page is drawn at a time rather than all ~150 rows in one long column.
//!
//! **Search.** [`find`] matches the words typed against each setting's name and the other words people use
//! for it ("tray", "dark mode", "startup"), every word typed having to match the start of one of them. A result
//! opens its page with the row marked for a moment. The entries name rows by the label they are drawn with
//! (`widgets::setting_row`'s id is `settings-row-<label>`), and a test checks every entry against the page
//! it names, so the index can't drift from what is drawn.

use crate::kit::prelude::*;
use crate::t;

/// A page of Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Overview,
    Appearance,
    Files,
    History,
    Terminal,
    Instant,
    Find,
    Quick,
    Inside,
    Clouds,
    Agents,
    Tools,
    About,
}

/// The groups of the list on the left, each under a small heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Start,
    Look,
    Browse,
    Search,
    Connect,
    Glint,
}

impl Group {
    /// The heading over the group; none over the first.
    pub fn heading(self) -> Option<SharedString> {
        match self {
            Group::Start => None,
            Group::Look => Some(t!("Look")),
            Group::Browse => Some(t!("Browse")),
            Group::Search => Some(t!("Search")),
            Group::Connect => Some(t!("Connect")),
            Group::Glint => Some(t!("Glint")),
        }
    }
}

impl Page {
    /// Every page, in the list's order.
    pub const ALL: [Page; 13] = [
        Page::Overview,
        Page::Appearance,
        Page::Files,
        Page::History,
        Page::Terminal,
        Page::Instant,
        Page::Find,
        Page::Quick,
        Page::Inside,
        Page::Clouds,
        Page::Agents,
        Page::Tools,
        Page::About,
    ];

    /// The page's id (after `settings-nav-`), the same as its first section's.
    pub fn slot(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Appearance => "appearance",
            Page::Files => "files",
            Page::History => "history",
            Page::Terminal => "terminal",
            Page::Instant => "instant-search",
            Page::Find => "find-files-by-name",
            Page::Quick => "quick-search",
            Page::Inside => "search-text-ripgrep",
            Page::Clouds => "clouds",
            Page::Agents => "ai-agents",
            Page::Tools => "tools",
            Page::About => "about",
        }
    }

    /// Its name in the list.
    pub fn name(self) -> SharedString {
        match self {
            Page::Overview => t!("Overview"),
            Page::Appearance => t!("Appearance"),
            Page::Files => t!("Files"),
            Page::History => t!("History"),
            Page::Terminal => t!("Terminal"),
            Page::Instant => t!("Instant search"),
            Page::Find => t!("Find files"),
            Page::Quick => t!("Quick search"),
            Page::Inside => t!("Inside files"),
            Page::Clouds => t!("Clouds"),
            Page::Agents => t!("AI agents"),
            Page::Tools => t!("Tools"),
            Page::About => t!("About"),
        }
    }

    /// Its sign in the list and at the top of the page.
    pub fn icon(self) -> IconName {
        match self {
            Page::Overview => IconName::LayoutDashboard,
            Page::Appearance => IconName::Palette,
            Page::Files => IconName::Files,
            Page::History => IconName::ClockArrowLeft,
            Page::Terminal => IconName::SquareTerminal,
            Page::Instant => IconName::Zap,
            Page::Find => IconName::FileSearch,
            Page::Quick => IconName::Search,
            Page::Inside => IconName::TextSearch,
            Page::Clouds => IconName::Cloud,
            Page::Agents => IconName::Sparkles,
            Page::Tools => IconName::Wrench,
            Page::About => IconName::Info,
        }
    }

    /// The line under the page's title.
    pub fn about(self) -> SharedString {
        match self {
            Page::Overview => t!("How Glint is set up, and what could make it better"),
            Page::Appearance => t!("Theme, backdrop, accent color and text size"),
            Page::Files => t!("What folders show, how they open, and copying"),
            Page::History => t!("Where you've been, and what Glint remembers of it"),
            Page::Terminal => t!("The terminal under the files"),
            Page::Instant => t!("Every drive's files found as you type, kept up to date as they change"),
            Page::Find => t!("Finding files by name where instant search doesn't reach"),
            Page::Quick => t!("A search box from anywhere, on a shortcut of yours"),
            Page::Inside => t!("Searching what files say, and what to leave out"),
            Page::Clouds => t!("Google Drive, OneDrive, Dropbox and more, as folders"),
            Page::Agents => t!("What AI agents may do through Glint's MCP server"),
            Page::Tools => t!("The programs Glint drives, installed and kept up to date here"),
            Page::About => t!("Version, updates, your settings file and help"),
        }
    }

    pub fn group(self) -> Group {
        match self {
            Page::Overview => Group::Start,
            Page::Appearance => Group::Look,
            Page::Files | Page::History | Page::Terminal => Group::Browse,
            Page::Instant | Page::Find | Page::Quick | Page::Inside => Group::Search,
            Page::Clouds | Page::Agents => Group::Connect,
            Page::Tools | Page::About => Group::Glint,
        }
    }

    /// The sections the page shows, in order (their ids after `settings-`).
    pub fn sections(self) -> &'static [&'static str] {
        match self {
            Page::Overview => &["overview"],
            Page::Appearance => &["appearance"],
            Page::Files => &["files", "labels", "marks"],
            Page::History => &["history"],
            Page::Terminal => &["terminal"],
            Page::Instant => &["instant-search"],
            Page::Find => &["find-files-by-name"],
            Page::Quick => &["quick-search"],
            Page::Inside => &["search-text-ripgrep", "text-search-skips"],
            Page::Clouds => &["clouds"],
            Page::Agents => &["ai-agents"],
            Page::Tools => &["tools"],
            Page::About => &["about"],
        }
    }

    /// The page that shows `section` ("settings-about" or "about"), or whose id it is.
    pub fn of_section(section: &str) -> Option<Page> {
        let slot = section.strip_prefix("settings-").unwrap_or(section);
        Page::ALL
            .into_iter()
            .find(|page| page.slot() == slot || page.sections().contains(&slot))
    }
}

/// A setting search can find: the page it's on, the label its row is drawn with, and other words for it.
pub struct Entry {
    pub page: Page,
    pub label: &'static str,
    pub words: &'static str,
}

const fn entry(page: Page, label: &'static str, words: &'static str) -> Entry {
    Entry { page, label, words }
}

/// Every setting search finds. The labels are the rows' own (English is the key of the words drawn), so a
/// row renamed in its page must be renamed here too: `the_search_index_names_rows_that_are_drawn` says so.
pub const SEARCH: &[Entry] = &[
    // Appearance
    entry(Page::Appearance, "Theme", "dark mode light mode colors night"),
    entry(Page::Appearance, "Backdrop", "mica transparency glass solid window"),
    entry(Page::Appearance, "Accent color", "pink highlight colour windows color"),
    entry(Page::Appearance, "Text size", "font size bigger smaller zoom scale"),
    entry(Page::Appearance, "Line numbers in preview", "code lines numbers"),
    entry(
        Page::Appearance,
        "Touchpad haptics",
        "haptics feedback touchpad vibration tactile",
    ),
    // Files
    entry(Page::Files, "Reopen my tabs", "restore session tabs startup"),
    entry(Page::Files, "Show hidden items", "hidden files system files dotfiles"),
    entry(Page::Files, "Folders first", "sort order folders on top"),
    entry(
        Page::Files,
        "Each folder keeps its own view",
        "per folder view sort columns",
    ),
    entry(Page::Files, "Preview pane", "preview panel quick look side"),
    entry(Page::Files, "Folders in the sidebar", "tree navigation pane sidebar"),
    entry(
        Page::Files,
        "Open the sidebar to the folder shown",
        "tree follows expand",
    ),
    entry(
        Page::Files,
        "Copy and move with Glint's queue",
        "copy queue transfers paste",
    ),
    entry(Page::Files, "Check each copy the queue makes", "verify copies checksum"),
    entry(Page::Files, "Thumbnails in the list", "pictures previews icons"),
    entry(Page::Files, "A folder's README above the list", "readme markdown"),
    entry(Page::Files, "Tile size in the grid", "icons size grid zoom"),
    entry(
        Page::Files,
        "Ask before moving to the Recycle Bin",
        "delete confirm recycle bin trash",
    ),
    entry(Page::Files, "Gallery folders", "photos pictures gallery"),
    entry(Page::Files, "Labels", "label colors names rename tags marks"),
    entry(Page::Files, "Add a label", "new label color"),
    entry(Page::Files, "Keep following XYplorer", "xyplorer tag.dat marks follow import"),
    entry(Page::Files, "Take back the last import", "undo import xyplorer marks"),
    // History
    entry(Page::History, "Save search history", "searches history privacy"),
    entry(Page::History, "Keep up to", "history length limit"),
    entry(
        Page::History,
        "Share folder history with zoxide",
        "zoxide terminal cd jump",
    ),
    entry(
        Page::History,
        "Suggest the next step",
        "smart suggestions suggested menu dock",
    ),
    entry(Page::History, "Learn from what I do", "habits learning usual"),
    entry(Page::History, "Forget what Glint learned", "habits clear reset privacy"),
    // Terminal
    entry(Page::Terminal, "Shell", "powershell cmd bash wsl terminal"),
    entry(Page::Terminal, "Follow the folder shown", "terminal cd follow"),
    // Find files
    entry(Page::Find, "Result limit", "find files results maximum"),
    entry(Page::Find, "Skip what .gitignore lists", "gitignore ignore"),
    // Quick search
    entry(Page::Quick, "Shortcut", "hotkey keyboard shortcut ctrl alt"),
    entry(
        Page::Quick,
        "Keep running in the tray",
        "tray background close minimize notification area",
    ),
    entry(
        Page::Quick,
        "Start with Windows",
        "startup sign in boot autostart login",
    ),
    // Inside files
    entry(Page::Inside, "Open files with", "editor code notepad open with"),
    // Pages themselves, by the words people use for them.
    entry(
        Page::Instant,
        "Instant search",
        "index indexing everything drives ntfs mft service fast search",
    ),
    entry(
        Page::Clouds,
        "Clouds",
        "google drive onedrive dropbox rclone cloud sync remote",
    ),
    entry(Page::Agents, "AI agents", "mcp claude copilot agent ai"),
    entry(
        Page::Tools,
        "Tools",
        "ffmpeg pandoc restic rclone 7-zip pdfcpu exiftool install update programs",
    ),
    entry(
        Page::About,
        "About",
        "version updates help diagnostics export import settings file shortcuts",
    ),
    entry(
        Page::About,
        "Install updates automatically",
        "update upgrade new version background restart scoop",
    ),
];

/// What search found: each entry whose name or words have every word typed at the start of one of theirs, the
/// ones named by the words first. Nothing for nothing typed.
pub fn find(query: &str) -> Vec<&'static Entry> {
    let wanted: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let starts = |haystack: &str, word: &str| {
        haystack
            .split(|c: char| !c.is_alphanumeric() && c != '.' && c != '+' && c != '-')
            .any(|w| w.to_lowercase().starts_with(word))
    };
    let mut found: Vec<(bool, &'static Entry)> = SEARCH
        .iter()
        .filter(|entry| {
            wanted
                .iter()
                .all(|word| starts(entry.label, word) || starts(entry.words, word))
        })
        .map(|entry| (wanted.iter().all(|word| starts(entry.label, word)), entry))
        .collect();
    // Named by it first; otherwise as the pages come.
    found.sort_by_key(|(named, entry)| (!*named, Page::ALL.iter().position(|p| *p == entry.page).unwrap_or(0)));
    found.into_iter().map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_is_on_one_page() {
        let mut seen = std::collections::HashSet::new();
        for page in Page::ALL {
            for section in page.sections() {
                assert!(seen.insert(*section), "{section} is on two pages");
                assert_eq!(Page::of_section(section), Some(page));
                assert_eq!(Page::of_section(&format!("settings-{section}")), Some(page));
            }
        }
        assert_eq!(Page::of_section("text-search-skips"), Some(Page::Inside));
        assert_eq!(Page::of_section("check"), None, "Check went with YARA-X");
    }

    #[test]
    fn search_matches_the_start_of_words_and_puts_names_first() {
        let labels = |q: &str| find(q).iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels("tray"), ["Keep running in the tray"]);
        assert_eq!(labels("dark"), ["Theme"]);
        assert_eq!(labels("startup"), ["Reopen my tabs", "Start with Windows"]);
        assert!(labels("hid").contains(&"Show hidden items"));
        assert!(labels("index drives").contains(&"Instant search"));
        assert!(labels("").is_empty() && labels("   ").is_empty());
        assert!(labels("zzzz").is_empty());
        // A word that names an entry outranks one that's only among its other words.
        let shortcut = labels("shortcut");
        assert_eq!(shortcut.first(), Some(&"Shortcut"));
    }
}
