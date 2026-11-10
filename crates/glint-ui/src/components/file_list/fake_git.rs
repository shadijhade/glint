//! A fake Git for the tests of the views that show what Git says: the file list's pills and faded items, the status
//! bar's words and the address bar's chip. It answers with the views it was given, in the folders it was given them
//! for, and nothing touches the real core.

use crate::components::git::model::{Fut, Source, View};
use crate::components::git::{self, Counts};
use glint_core::git::FileDiff;
use glint_core::repo::RepoCard;
use glint_model::entries::GitMark;
use glint_model::groups::GitMarks;
use gpui_kit::App;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// The fake source: Git views by folder.
#[derive(Clone, Default)]
pub(crate) struct FakeGit(Arc<Mutex<HashMap<String, View>>>);

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl FakeGit {
    /// Says `folder` is in a repository with this view.
    pub(crate) fn put(&self, folder: &str, view: View) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key(folder), view);
    }

    /// Makes the shared Git model read through this source. After `explorer::init_with`, before the views.
    pub(crate) fn install(&self, cx: &mut App) {
        git::init_with(cx, Rc::new(self.clone()));
    }
}

/// A repository on branch `main` whose folder has these marks (by name, as the folder spells it): status took 12.4 ms.
pub(crate) fn repo_view(marks: &[(&str, GitMark)]) -> View {
    let marks: GitMarks = marks.iter().map(|(name, mark)| (name.to_lowercase(), *mark)).collect();
    View {
        root: "C:\\x".into(),
        branch: Some("main".into()),
        head: Some("abc1234".into()),
        counts: Counts {
            modified: 1,
            ..Default::default()
        },
        marks: Arc::new(marks),
        elapsed: 12.4,
        ..Default::default()
    }
}

impl Source for FakeGit {
    fn view(&self, folder: String, _lane: String, _trusted: Vec<String>, _force: bool) -> Fut<Option<View>> {
        let view = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key(&folder))
            .cloned();
        Box::pin(async move { Ok(view) })
    }

    fn cancel(&self, _lane: &str) {}

    fn watch(&self, _roots: Vec<String>) {}

    fn diff(&self, _path: String) -> Fut<Option<FileDiff>> {
        Box::pin(async { Ok(None) })
    }

    fn head_copy(&self, _path: String) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }

    fn head_discard(&self, _path: String) {}

    fn card(&self, _path: String) -> Fut<Option<RepoCard>> {
        Box::pin(async { Ok(None) })
    }
}
