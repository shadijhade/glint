//! Finding things out about files and folders ahead of the click (docs/gpui/aware/PLAN.md): who has a file open,
//! where it came from, who signed it, a picture's size, a text file's lines, what is in a folder, whether the
//! same bytes are elsewhere on the PC.
//!
//! [`probe`] is the one door: it is given paths and what to ask about each ([`Ask`]), answers a part at a time
//! on a channel ([`Answer`]), and never holds its caller. The work runs on one of two lanes of threads
//! ([`Lane`], [`pool`]): *quiet* for looking ahead (Windows' efficiency mode: what nobody is waiting for costs
//! nothing that is felt), *now* for what something on screen waits for.
//!
//! The answers are glint-model's types ([`Part`], [`Probed`]): the window ranks with them, with no engine in
//! between.

pub mod file;
pub mod inside;
pub mod pool;
pub mod probe;
pub mod twins;

pub use glint_model::aware::{Inside, Media, Origin, Part, Probed, Signer, Text, Want};
pub use probe::probe;

/// One file or folder, and what to find out about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub path: String,
    /// It is a folder.
    pub dir: bool,
    /// A file's size, as its listing has it.
    pub size: u64,
    pub want: Want,
}

/// Which threads do the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Looking ahead: half the cores, in Windows' efficiency mode, at background priority.
    Quiet,
    /// Something on screen waits for it: every core, at normal priority.
    Now,
}

/// One thing found out about one path.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub path: String,
    pub part: Part,
}
