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
/// A character-level difference between two file names (what a rename deletes and inserts).
pub mod chardiff;
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
/// What a search means when it was typed on a keyboard that isn't an English one (a Chinese one's `：` and `；`).
pub mod keyboard;
pub mod markdown;
/// A label, tags and a comment that stay with the file (docs/gpui/marks).
pub mod marks;
pub mod natural;
pub mod oklab;
pub mod paths;
pub mod places;
pub mod plain;
/// The interface's catalogs as gettext files (`locales/*.po`): what build.rs compiles in, and a translator's file.
pub mod po;
pub mod rank;
pub mod small_kinds;
pub mod store;
pub mod text;
pub mod time_machine;
pub mod usage;
pub mod world_map;
