//! Glint's pure logic (see docs/gpui/STANDARDS.md).
//!
//! No GPUI and no window: everything here is plain Rust that the interface and the engine call, each module
//! with its own tests.

/// Text compared in the user's language (Windows' CompareStringEx).
pub mod collate;
/// JavaScript semantics shared by the ports (white space, rounding, number text).
pub mod js;

pub mod archives;
pub mod cast;
pub mod cd;
pub mod color;
pub mod columns;
pub mod entries;
pub mod file_types;
pub mod format;
pub mod grid;
pub mod groups;
pub mod hex;
pub mod i18n;
pub mod i18n_ar;
pub mod json_tree;
pub mod markdown;
pub mod paths;
pub mod plain;
pub mod rank;
pub mod small_kinds;
pub mod store;
pub mod text;
pub mod usage;
