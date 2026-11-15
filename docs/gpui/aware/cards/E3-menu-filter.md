# Card E3 — Finding rows by typing

Wave 2 · crate `glint-ui` · pure logic over the menu's data

**You own:** `crates/glint-ui/src/components/menus/filter.rs` (its tests at the bottom of the file).
**Read first:** PLAN.md §3 (F3), §5.4; `crates/glint-ui/src/components/menus/spec.rs` (all of it: `Node`, `Entry`, `Run`); `crates/glint-model/src/rank.rs:170` (`fuzzy_score(query, text, tight) -> Option<f64>`: higher is better, `None` is no match; read its doc and tests for what it rewards).
**Check with:** `cargo test -p glint-ui components::menus::filter` and clippy on the crate.

## What to build

```rust
/// A row found by typing.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The row, as it is in the menu (its label is its own: "Zip file").
    pub entry: Entry,
    /// The submenus it is under, outermost first, joined: "Compress to", or "A › B". None at the top.
    pub path: Option<SharedString>,
    pub score: f64,
}
/// The most rows a filter shows.
pub const MOST: usize = 12;
/// The rows of `nodes` that match `query`, best first, at most [`MOST`]. An empty or all-space query finds nothing.
pub fn matches(nodes: &[Node], query: &str) -> Vec<Found>;
```

Rules:

1. **What can be found:** every `Node::Item` and every button of a `Node::Strip`, at any depth of `Node::Menu` (a disabled submenu's rows are not looked at), that is not `disabled` and whose `run` is not `Run::Nothing`. `Head`, `Heading` and `Separator` are never found.
2. **Once each:** a row whose `run` equals (`==`) the `run` of a row already taken is skipped: the Suggested section holds copies. The first one met, top to bottom, is the one kept, **except** that a row with `entry.smart` set is passed over when the same `run` comes again later (so the real row, with its place in a submenu, is the one shown). Do it in two passes if that is simpler: collect all, then drop a `smart` one whenever another with the same `run` exists.
3. **The score** of a row is the better of:
   - `fuzzy_score(query, &entry.label, false)`, plus 2.0;
   - `fuzzy_score(query, &format!("{path} {label}"), false)` when it has a path (typing "pdf pass" finds "PDF tools › Add a password…").
   A row neither matches is left out. Trim the query first.
4. **Order:** score, highest first; equal scores keep the menu's order (stable sort). Cut at `MOST`.
5. The found `entry` is a clone, unchanged except that `reason` is cleared and `smart` is false (a found row is drawn as a plain row).

## Tests

Build menus with `Entry::task(label.into(), IconName::File, Task::RefreshDrives)` and friends, as the tests at the bottom of `spec.rs` do; give rows different `run`s with `Task::Navigate("a".into())`, `Task::Navigate("b".into())`, ….

1. An empty query, and a query of spaces, find nothing.
2. "ren" finds "Rename" in the strip.
3. "zip" finds "Zip file" inside the "Compress to" submenu, with `path == Some("Compress to")`.
4. A row two submenus deep has the path "A › B".
5. "pdf pass" finds "Add a password…" under "PDF tools" (matched through its path), and a row called "Password" at the top scores higher for "pass" than one matched only through its path.
6. Disabled rows, notes (`Entry::note`), headings and separators are never found; nor are the rows of a disabled submenu.
7. A suggested copy (`smart: true`, same `run` as a row further down) is found once, as the row further down (with its path), not twice.
8. Fifteen matching rows give twelve, best first; two with the same score stay in the menu's order.
9. A found row has no `reason` and isn't `smart`.
10. Letters with different case match ("ZIP" finds "Zip file"), and so does a name in Arabic typed as it is written (a row labelled "ضغط" found by "ضغط").

## Done when

The tests pass, clippy is clean, and the file has no GPUI drawing in it.
