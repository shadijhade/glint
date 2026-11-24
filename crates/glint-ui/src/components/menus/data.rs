//! What the menus read that isn't in the explorer: the editors and terminals found on this PC, the scripts,
//! the kinds File Explorer's New menu has, the tags on the items, the next Undo, the paired PCs.
//!
//! The first three come from glint-core, read in the background ([`Sources`]) when the host is made and again
//! each time a menu opens (the TypeScript refreshed them at start and when the window came back in front), so
//! a menu never waits: it shows what was last read. Tests give [`Sources`] fakes.

use crate::components::row_facts;
use glint_core::newmenu::NewKind;
use glint_core::openers::Opener;
use glint_core::scripts::Script;
use glint_model::marks::{Label, Marks};
use gpui_kit::App;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Everything the menus need that no service of the app holds yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuData {
    /// Editors and terminals on this PC (`useOpeners`), for "Open in".
    pub openers: Vec<Opener>,
    /// The scripts of Glint's Scripts folder (`useScripts`).
    pub scripts: Vec<Script>,
    /// The kinds of file Windows' New menu has (`newKinds()`).
    pub new_kinds: Vec<NewKind>,
    /// The tags each item has, by its path in lower case (`useTags.byPath`): what the rows read, for the items the
    /// menu is about. An item not read yet is missing.
    pub tags: HashMap<String, Vec<String>>,
    /// What each item is marked with, by its path in lower case (`RowFacts::marks_among`). An item not read yet is
    /// missing, and counts as having no marks: nothing is read from the disk to open a menu.
    pub marks: HashMap<String, Marks>,
    /// The labels of Settings (`state::settings(cx).labels`), in their order: the row of dots.
    pub labels: Vec<Label>,
    /// What Undo would take back, in a few words ("copy"), or None when there is nothing to undo
    /// (`nextUndo()?.label`). The host fills it from the explorer each time a folder menu opens.
    pub undo: Option<String>,
    /// Folders of a remote connection shown in File Explorer on demand (`useOnDemand.mounts`): connection ids.
    pub on_demand: Vec<String>,
}

impl MenuData {
    /// Takes the marks (and so the tags) read so far for `paths` as [`MenuData::marks`] and [`MenuData::tags`], and the
    /// labels of Settings. The file list reads the marks of the rows it shows and of what is selected
    /// (`components::row_facts`), so by the time a menu opens they are there; `MenuHost` calls this on the data of
    /// the item menu it is making, with the paths the menu is about. An item not read yet is left out and counts as
    /// having no marks. Nothing is read from the disk.
    pub fn read_tags<'a>(&mut self, cx: &App, paths: impl IntoIterator<Item = &'a str>) {
        self.marks = row_facts::try_shared(cx)
            .map(|facts| facts.read(cx).marks_among(paths))
            .unwrap_or_default();
        self.tags = self.marks.iter().map(|(path, marks)| (path.clone(), marks.tags.clone())).collect();
        self.labels = crate::state::try_settings(cx).map(|s| s.labels.clone()).unwrap_or_default();
    }
}

/// A boxed async read.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Where the three lists come from: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Sources {
    /// `openers::openers`.
    pub openers: Arc<dyn Fn() -> Fut<Vec<Opener>> + Send + Sync>,
    /// `scripts::scripts_list`'s scripts.
    pub scripts: Arc<dyn Fn() -> Fut<Vec<Script>> + Send + Sync>,
    /// `newmenu::new_kinds`.
    pub new_kinds: Arc<dyn Fn() -> Fut<Vec<NewKind>> + Send + Sync>,
}

impl Sources {
    /// glint-core.
    pub fn core() -> Sources {
        Sources {
            openers: Arc::new(|| Box::pin(glint_core::openers::openers())),
            scripts: Arc::new(|| {
                Box::pin(async { glint_core::scripts::scripts_list().await.map(|found| found.scripts) })
            }),
            new_kinds: Arc::new(|| Box::pin(glint_core::newmenu::new_kinds())),
        }
    }

    /// Nothing found anywhere (a PC with no editors, no scripts, no New kinds).
    pub fn empty() -> Sources {
        Sources {
            openers: Arc::new(|| Box::pin(async { Ok(Vec::new()) })),
            scripts: Arc::new(|| Box::pin(async { Ok(Vec::new()) })),
            new_kinds: Arc::new(|| Box::pin(async { Ok(Vec::new()) })),
        }
    }
}
