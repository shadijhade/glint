//! Glint's pure logic (see docs/gpui/STANDARDS.md).
//!
//! No GPUI and no window: everything here is plain Rust that the interface and the engine call, each module
//! with its own tests.

/// Text compared in the user's language (Windows' CompareStringEx).
pub mod collate;
/// JavaScript semantics shared by the ports (white space, rounding, number text).
pub mod js;

pub mod archives;
/// What Glint knows about what a menu or the dock is for, and what it would do next (docs/gpui/aware).
pub mod aware;
pub mod cd;
pub mod color;
pub mod columns;
pub mod editor;
pub mod entries;
pub mod file_lego;
pub mod file_types;
pub mod flashfill;
pub mod format;
pub mod grid;
pub mod groups;
pub mod hex;
pub mod i18n;
pub mod json_tree;
pub mod markdown;
pub mod natural;
pub mod oklab;
pub mod paths;
pub mod plain;
pub mod rank;
pub mod small_kinds;
pub mod store;
pub mod text;
pub mod time_machine;
pub mod usage;
pub mod world_map;
