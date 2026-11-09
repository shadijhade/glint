//! The explorer window's views, ported from src/components/explorer. Each is a GPUI entity the workspace places;
//! each reads the explorer model (`crate::explorer`) and keeps no copy of what it holds. Their features exist
//! while their wave runs; the coordinator drops them when it integrates the wave.

/// Breadcrumbs, editing and completion (AddressBar.tsx).
#[cfg(feature = "address-bar")]
pub mod address_bar;
/// The grid of tiles with thumbnails and tile sizes (FileGrid.tsx, Thumbnail.tsx, thumbs.ts).
#[cfg(feature = "file-grid")]
pub mod file_grid;
/// The details view: columns, sorting, groups, selection, keyboard, type-ahead (FileList.tsx, VirtualList.tsx, parts of Explorer.tsx).
#[cfg(feature = "file-list")]
pub mod file_list;
/// Places and the folder tree (Sidebar.tsx, FolderTree.tsx).
#[cfg(feature = "sidebar")]
pub mod sidebar;
/// Counts, selection, the index and the view switches (StatusBar.tsx).
#[cfg(feature = "status-bar")]
pub mod status_bar;
/// The tabs in the title bar (TabStrip.tsx).
#[cfg(feature = "tab-strip")]
pub mod tab_strip;
/// This PC: drives and their space (ThisPC.tsx).
#[cfg(feature = "this-pc")]
pub mod this_pc;
