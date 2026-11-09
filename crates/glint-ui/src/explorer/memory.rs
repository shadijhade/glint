//! What the explorer remembers between runs: the last folder, the session (tabs and panes), the visits that
//! rank folders in Ctrl+K, and each folder's own view. In the web app these lived in the zustand store
//! (`useStore`: `lastFolder`, `session`, `visits`, `folderViews`, with `setLastFolder`, `setSession`,
//! `recordVisit` and `setFolderView`).
//!
//! `crate::state` holds only the live `Settings`, so until the persistence module owns the whole saved
//! state, the explorer keeps its four parts itself, in the shape the old app saved. [`Memory`] wraps a
//! `glint_model::store::AppState` so that `record_visit` and `set_folder_view` are the model's own tested
//! functions, not copies. **Only `last_folder`, `session`, `visits` and `folder_views` are used**; the
//! persistence module owns the rest of the `AppState` (settings, pinned folders, remotes…).
//!
//! Wiring, for the persistence module: after loading the saved state call
//! `Explorer::set_memory(AppState)` once, before `Explorer::init_explorer`, and save
//! `Explorer::memory().app()` whenever [`ExplorerEvent::MemoryChanged`](super::ExplorerEvent::MemoryChanged)
//! is raised (the changes are small, frequent and should be debounced there).

use glint_model::rank::Visit;
use glint_model::store::{AppState, FolderView, FolderViewPatch, OrderedMap, Session};

/// The saved parts the explorer reads and writes.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    app: AppState,
}

impl Memory {
    /// Remembering nothing.
    pub fn new() -> Self {
        Memory::default()
    }

    /// Remembering what a loaded `AppState` holds.
    pub fn from_app_state(app: AppState) -> Self {
        Memory { app }
    }

    /// The whole `AppState`, for saving. Only the four parts above are the explorer's.
    pub fn app(&self) -> &AppState {
        &self.app
    }

    /// The folder the app was last in (`lastFolder`): where it opens again with "Last folder" as the start folder.
    pub fn last_folder(&self) -> Option<&str> {
        self.app.last_folder.as_deref()
    }

    /// The tabs and panes the app closed with (`session`).
    pub fn session(&self) -> Option<&Session> {
        self.app.session.as_ref()
    }

    /// Folders opened, for ranking in the command palette (`visits`).
    pub fn visits(&self) -> &OrderedMap<Visit> {
        &self.app.visits
    }

    /// A folder's own view, as last set there (`folderViews[path.toLowerCase()]`).
    pub fn folder_view(&self, path: &str) -> Option<&FolderView> {
        self.app.folder_views.get(&path.to_lowercase())
    }

    /// Every folder's own view.
    pub fn folder_views(&self) -> &OrderedMap<FolderView> {
        &self.app.folder_views
    }

    pub(super) fn set_last_folder(&mut self, path: &str) -> bool {
        if self.app.last_folder.as_deref() == Some(path) {
            return false;
        }
        self.app.last_folder = Some(path.to_string());
        true
    }

    pub(super) fn set_session(&mut self, session: Session) -> bool {
        if self.app.session.as_ref() == Some(&session) {
            return false;
        }
        self.app.session = Some(session);
        true
    }

    pub(super) fn record_visit(&mut self, path: &str, now: i64) {
        self.app.record_visit(path, now);
    }

    pub(super) fn set_folder_view(&mut self, path: &str, view: Option<FolderViewPatch>, now: i64) {
        self.app.set_folder_view(path, view, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{Sort, SortKey};

    #[test]
    fn visits_and_folder_views_use_the_models_own_rules() {
        let mut memory = Memory::new();
        memory.record_visit("C:\\a", 1000);
        memory.record_visit("C:\\a", 2000);
        assert_eq!(memory.visits().get("C:\\a"), Some(&Visit { count: 2, last: 2000 }));
        let sort = Sort {
            key: SortKey::Size,
            desc: true,
        };
        memory.set_folder_view(
            "C:\\Photos",
            Some(FolderViewPatch {
                sort: Some(sort),
                ..Default::default()
            }),
            5,
        );
        // Keyed by the lower-cased path, found by any case.
        assert_eq!(memory.folder_view("c:\\PHOTOS").and_then(|v| v.sort), Some(sort));
        memory.set_folder_view("C:\\Photos", None, 6);
        assert!(memory.folder_view("C:\\Photos").is_none());
    }

    #[test]
    fn last_folder_and_session_report_whether_they_changed() {
        let mut memory = Memory::new();
        assert!(memory.set_last_folder("C:\\a"));
        assert!(!memory.set_last_folder("C:\\a"));
        assert_eq!(memory.last_folder(), Some("C:\\a"));
        let session = Session::default();
        assert!(memory.set_session(session.clone()));
        assert!(!memory.set_session(session));
    }
}
