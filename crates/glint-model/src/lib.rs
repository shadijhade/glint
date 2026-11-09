//! Glint's pure logic, ported from the TypeScript in `src/lib` (see docs/gpui/STANDARDS.md).
//!
//! No GPUI, no Tauri, no window: everything here is plain Rust that the UI and the core call, and
//! each module carries the assertions its TypeScript had in `explorer.check.ts`.

/// JavaScript semantics shared by the ports (white space, rounding, number text).
pub mod js;
/// Text compared in the user's language (Windows' CompareStringEx).
pub mod collate;

#[cfg(feature = "archives")]
pub mod archives;
#[cfg(feature = "paths")]
pub mod paths;
#[cfg(feature = "cast")]
pub mod cast;
#[cfg(feature = "cd")]
pub mod cd;
#[cfg(feature = "color")]
pub mod color;
#[cfg(feature = "format")]
pub mod format;
#[cfg(feature = "grid")]
pub mod grid;
#[cfg(feature = "hex")]
pub mod hex;
#[cfg(feature = "i18n")]
pub mod i18n;
#[cfg(feature = "i18n")]
pub mod i18n_ar;
#[cfg(feature = "json_tree")]
pub mod json_tree;
#[cfg(feature = "markdown")]
pub mod markdown;
#[cfg(feature = "plain")]
pub mod plain;
#[cfg(feature = "rank")]
pub mod rank;
#[cfg(feature = "small_kinds")]
pub mod small_kinds;
#[cfg(any(feature = "store", feature = "columns"))]
pub mod store;
#[cfg(feature = "entries")]
pub mod entries;
#[cfg(feature = "file_types")]
pub mod file_types;
#[cfg(feature = "columns")]
pub mod columns;
#[cfg(feature = "groups")]
pub mod groups;
#[cfg(feature = "usage")]
pub mod usage;
#[cfg(feature = "text")]
pub mod text;
