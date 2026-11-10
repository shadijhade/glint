//! The explorer window's views, ported from src/components/explorer. Each is a GPUI entity the workspace places;
//! each reads the explorer model (`crate::explorer`) and keeps no copy of what it holds. Wave 4 built
//! them; each folder's mod.rs says what it shows and how the workspace wires it.

/// Breadcrumbs, editing and completion (AddressBar.tsx).
pub mod address_bar;
/// The copy queue: jobs, progress, pause, cancel, questions (CopyQueue.tsx).
pub mod copy_queue;
/// The grid of tiles with thumbnails and tile sizes (FileGrid.tsx, Thumbnail.tsx, thumbs.ts).
pub mod file_grid;
/// The details view: columns, sorting, groups, selection, keyboard, type-ahead (FileList.tsx, VirtualList.tsx, parts of Explorer.tsx).
pub mod file_list;
/// The item, background and folder menus, and Windows' own menu (Explorer.tsx's menus, Menu.tsx, ActionParts.tsx).
pub mod menus;
/// Ctrl+K: commands, folders by frecency, the calculator (CommandPalette.tsx, History.tsx).
pub mod palette;
/// The preview pane and Quick Look.
pub mod preview;
/// The Quick Search window: the shortcut's and the tray's search (QuickSearch.tsx).
pub mod quick_search;
/// Settings (SettingsView.tsx).
pub mod settings;
/// Places and the folder tree (Sidebar.tsx, FolderTree.tsx).
pub mod sidebar;
/// Counts, selection, the index and the view switches (StatusBar.tsx).
pub mod status_bar;
/// The tabs in the title bar (TabStrip.tsx).
pub mod tab_strip;
/// This PC: drives and their space (ThisPC.tsx).
pub mod this_pc;
/// Searching inside files: the query, its options, the results and the history (Search.tsx, Results.tsx, History.tsx). Wave 8 slot.
#[cfg(feature = "text-search")]
pub mod text_search;
/// The disk usage map of a folder or drive (DiskUsage.tsx, diskUsage.ts). Wave 8 slot.
#[cfg(feature = "disk-usage")]
pub mod disk_usage;
