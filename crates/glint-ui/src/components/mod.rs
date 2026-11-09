//! The explorer window's views, ported from src/components/explorer. Each is a GPUI entity the workspace places;
//! each reads the explorer model (`crate::explorer`) and keeps no copy of what it holds. Wave 4 built
//! them; each folder's mod.rs says what it shows and how the workspace wires it.

/// Breadcrumbs, editing and completion (AddressBar.tsx).
pub mod address_bar;
/// The grid of tiles with thumbnails and tile sizes (FileGrid.tsx, Thumbnail.tsx, thumbs.ts).
pub mod file_grid;
/// The details view: columns, sorting, groups, selection, keyboard, type-ahead (FileList.tsx, VirtualList.tsx, parts of Explorer.tsx).
pub mod file_list;
/// Places and the folder tree (Sidebar.tsx, FolderTree.tsx).
pub mod sidebar;
/// Counts, selection, the index and the view switches (StatusBar.tsx).
pub mod status_bar;
/// The tabs in the title bar (TabStrip.tsx).
pub mod tab_strip;
/// This PC: drives and their space (ThisPC.tsx).
pub mod this_pc;
