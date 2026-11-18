//! What Glint knows about the thing a menu or the dock is about, and what it would do next with it
//! (docs/gpui/aware/PLAN.md). Pure: no window, no disk, no clock.
//!
//! - [`shape`]: what a selection is, in one word ([`Shape`]), and what is worth finding out about an item
//!   ([`want_for`]).
//! - [`facts`]: what is known at once ([`Facts`], [`Clip`]).
//! - [`probed`]: what the engine finds out, a part at a time ([`Probed`], [`Part`], [`Want`]).
//! - [`habits`]: what the person did before ([`Habits`]).
//! - [`hash`]: a checksum copied from a download page, and whether a file matches it ([`copied_hash`]).
//! - [`rank`]: what to put forward ([`suggest`]).

pub mod facts;
pub mod habits;
pub mod hash;
pub mod probed;
pub mod rank;
pub mod shape;

pub use facts::{Clip, Facts};
pub use habits::{Habits, Place, Use};
pub use hash::{copied_hash, verdict, Algo, CopiedHash, Sums, SumsWant, Verdict, HASH_CAP};
pub use probed::{Inside, Media, Origin, Part, Probed, Signer, Text, Want};
pub use rank::{is_basic, suggest, usual_place, Pick, Why};
pub use shape::{shared_ext, want_for, Shape};
