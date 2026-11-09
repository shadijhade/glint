//! The folder tree's data: which folders are open and what is in each.
//! Ported from the state and pure functions of src/components/explorer/FolderTree.tsx (`useTree`, `keyOf`,
//! `revealInTree`, `treeSaw`, `treeHiddenChanged`).
//!
//! Nothing here touches GPUI: the sidebar owns one [`Tree`], asks it what to read, and does the reading on
//! core's runtime. A folder is keyed by its path in lower case ([`key_of`]), so `C:\Users` and `c:\users\`
//! are one folder, as Windows sees them.

use glint_model::entries::{is_hidden, FileEntry};
use glint_model::paths::parent_path;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A folder in the tree: its name, and whether it has folders of its own (`None`: not known yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subfolder {
    pub name: String,
    pub children: Option<bool>,
}

/// What the tree knows of one folder's contents (`Kids`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kids {
    /// Being read for the first time.
    Loading,
    /// Reading failed, with the reason (shown as "Can't look inside", the reason in its tooltip).
    Failed(String),
    /// The folders in it, in the list's order.
    Listed(Vec<Subfolder>),
}

/// Where the subfolders come from: glint-core in the app, a fake in tests. Takes the folder and whether hidden
/// folders are wanted.
pub type SubfolderSource =
    Arc<dyn Fn(String, bool) -> Pin<Box<dyn Future<Output = Result<Vec<Subfolder>, String>> + Send>> + Send + Sync>;

/// glint-core's `list_subfolders`, as a [`SubfolderSource`].
pub fn core_subfolders() -> SubfolderSource {
    Arc::new(|path, hidden| {
        Box::pin(async move {
            let found = glint_core::fsx::list_subfolders(path, hidden).await?;
            Ok(found
                .into_iter()
                .map(|f| Subfolder {
                    name: f.name,
                    children: f.children,
                })
                .collect())
        })
    })
}

/// A path as the tree keys it: lower case, a drive as `c:\` and no other trailing separator (`keyOf`).
pub fn key_of(path: &str) -> String {
    let lower = path.to_lowercase();
    let bytes = lower.as_bytes();
    let drive_only = matches!(bytes, [d, b':'] | [d, b':', b'\\'] if d.is_ascii_lowercase());
    if drive_only {
        // The first two bytes are ASCII (checked above), so this is a char boundary.
        return format!("{}\\", lower.get(..2).unwrap_or(""));
    }
    lower.trim_end_matches(['\\', '/']).to_string()
}

/// The folders to open so `path` shows in the tree below the closest of `roots` it is inside, outermost first
/// (the chain `revealInTree` opens). Empty when no root holds it (`File Explorer's "Expand to open folder"`).
pub fn reveal_chain(path: &str, roots: &[String]) -> Vec<String> {
    let lower = key_of(path);
    let root = roots
        .iter()
        .map(|r| key_of(r))
        .filter(|r| {
            let prefix = if r.ends_with('\\') { r.clone() } else { format!("{r}\\") };
            lower != *r && lower.starts_with(&prefix)
        })
        .max_by_key(|r| r.len());
    let Some(root) = root else {
        return Vec::new();
    };
    let mut chain: Vec<String> = Vec::new();
    let mut p = parent_path(path);
    while !p.is_empty() && key_of(&p).len() >= root.len() && !chain.contains(&p) {
        chain.insert(0, p.clone());
        if key_of(&p) == root {
            break;
        }
        p = parent_path(&p);
    }
    chain
}

/// Which folders are open in the tree, and what is in each (`useTree`).
#[derive(Default)]
pub struct Tree {
    /// Open folders: key to the path as it was asked for (what is read again when the hidden setting changes).
    open: HashMap<String, String>,
    kids: HashMap<String, Kids>,
    /// The newest read of each folder: an answer that comes back for an older one is dropped.
    reads: HashMap<String, u64>,
    next_read: u64,
}

impl Tree {
    /// Whether the folder is open (`isTreeOpen`).
    pub fn is_open(&self, path: &str) -> bool {
        self.open.contains_key(&key_of(path))
    }

    /// What is known of the folder's contents.
    pub fn kids(&self, path: &str) -> Option<&Kids> {
        self.kids.get(&key_of(path))
    }

    /// Whether the folder is open and its first read is still under way.
    pub fn is_loading(&self, path: &str) -> bool {
        matches!(self.kids(path), Some(Kids::Loading))
    }

    /// Opens or closes a folder (`toggleFolder`). Returns true when it opened, so the caller reads it again.
    pub fn set_open(&mut self, path: &str, open: bool) -> bool {
        let key = key_of(path);
        if open {
            self.open.insert(key, path.to_string());
        } else {
            self.open.remove(&key);
        }
        open
    }

    /// Starts reading a folder (`load`): marks it loading unless something is already shown for it, and numbers
    /// the read. Pass the number to [`Tree::finish_read`].
    pub fn begin_read(&mut self, path: &str) -> u64 {
        let key = key_of(path);
        self.kids.entry(key.clone()).or_insert(Kids::Loading);
        self.next_read += 1;
        self.reads.insert(key, self.next_read);
        self.next_read
    }

    /// A read finished. False (and nothing changes) when a newer read of the same folder has begun since.
    pub fn finish_read(&mut self, path: &str, read: u64, found: Result<Vec<Subfolder>, String>) -> bool {
        let key = key_of(path);
        if self.reads.get(&key) != Some(&read) {
            return false;
        }
        self.kids.insert(
            key,
            match found {
                Ok(list) => Kids::Listed(list),
                Err(reason) => Kids::Failed(reason),
            },
        );
        true
    }

    /// What a folder's listing says is in it (`treeSaw`): the folders it already knew keep what it knew of
    /// them. Does nothing for a folder that isn't open. True when the tree's rows changed.
    pub fn saw(&mut self, path: &str, entries: &[FileEntry], hidden: bool) -> bool {
        let key = key_of(path);
        if !self.open.contains_key(&key) {
            return false;
        }
        let before: &[Subfolder] = match self.kids.get(&key) {
            Some(Kids::Listed(list)) => list,
            _ => &[],
        };
        let known: HashMap<String, Option<bool>> = before.iter().map(|k| (k.name.to_lowercase(), k.children)).collect();
        let now: Vec<Subfolder> = entries
            .iter()
            .filter(|e| e.dir && (hidden || !is_hidden(e)))
            .map(|e| Subfolder {
                name: e.name.clone(),
                children: known.get(&e.lower).copied().flatten(),
            })
            .collect();
        if now.len() == before.len() && now.iter().zip(before).all(|(a, b)| a.name == b.name) {
            return false;
        }
        self.kids.insert(key, Kids::Listed(now));
        true
    }

    /// Hidden folders were shown or hidden (`treeHiddenChanged`): forgets what was read and returns the open
    /// folders, to read again.
    pub fn hidden_changed(&mut self) -> Vec<String> {
        self.kids.clear();
        self.reads.clear();
        self.open.values().cloned().collect()
    }

    /// The open folders' keys, for tests.
    #[cfg(test)]
    pub fn open_keys(&self) -> std::collections::HashSet<String> {
        self.open.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder, hidden};

    fn sub(name: &str, children: Option<bool>) -> Subfolder {
        Subfolder {
            name: name.into(),
            children,
        }
    }

    #[test]
    fn keys_ignore_case_and_trailing_separators_and_keep_a_drive_root() {
        assert_eq!(key_of("C:\\Users\\"), "c:\\users");
        assert_eq!(key_of("c:\\users"), "c:\\users");
        assert_eq!(key_of("C:"), "c:\\");
        assert_eq!(key_of("C:\\"), "c:\\");
        assert_eq!(key_of("\\\\server\\share\\"), "\\\\server\\share");
        assert_eq!(key_of("D:\\Ünï\\"), "d:\\ünï");
    }

    #[test]
    fn the_chain_opens_from_the_closest_place_down_to_the_parent() {
        let roots = vec!["C:\\".to_string(), "C:\\Users\\me".to_string()];
        // Inside the more specific root: only the folders below it.
        assert_eq!(
            reveal_chain("C:\\Users\\me\\Documents\\Reports", &roots),
            ["C:\\Users\\me", "C:\\Users\\me\\Documents"]
        );
        // Only the drive holds it.
        assert_eq!(reveal_chain("C:\\Windows\\System32", &roots), ["C:\\", "C:\\Windows"]);
        // A place itself shows once, as a place: nothing to open below itself.
        assert!(reveal_chain("C:\\Users\\me", &roots[1..]).is_empty());
        // Though the drive holds it too, so under the drive it is a folder in the tree.
        assert_eq!(reveal_chain("C:\\Users\\me", &roots), ["C:\\", "C:\\Users"]);
        // No place holds it.
        assert!(reveal_chain("E:\\Stuff\\More", &roots).is_empty());
        assert!(reveal_chain("", &roots).is_empty());
    }

    #[test]
    fn a_read_that_is_not_the_newest_is_dropped() {
        let mut tree = Tree::default();
        tree.set_open("C:\\a", true);
        let first = tree.begin_read("C:\\a");
        assert!(tree.is_loading("C:\\a"));
        let second = tree.begin_read("c:\\A\\");
        assert!(!tree.finish_read("C:\\a", first, Ok(vec![sub("old", None)])));
        assert!(tree.is_loading("C:\\a"));
        assert!(tree.finish_read("C:\\a", second, Ok(vec![sub("new", Some(true))])));
        assert_eq!(tree.kids("C:\\a"), Some(&Kids::Listed(vec![sub("new", Some(true))])));
        // Reading again keeps what is shown until the answer comes.
        let third = tree.begin_read("C:\\a");
        assert!(!tree.is_loading("C:\\a"));
        assert!(tree.finish_read("C:\\a", third, Err("denied".into())));
        assert_eq!(tree.kids("C:\\a"), Some(&Kids::Failed("denied".into())));
    }

    #[test]
    fn a_listing_updates_an_open_folder_and_keeps_what_was_known_of_its_folders() {
        let mut tree = Tree::default();
        // A folder that is not open is left alone.
        assert!(!tree.saw("C:\\a", &[folder("C:\\a", "x")], false));
        assert!(tree.kids("C:\\a").is_none());

        tree.set_open("C:\\a", true);
        let read = tree.begin_read("C:\\a");
        tree.finish_read(
            "C:\\a",
            read,
            Ok(vec![sub("Alpha", Some(true)), sub("Beta", Some(false))]),
        );
        // The same folders: nothing changed.
        let same = [
            folder("C:\\a", "Alpha"),
            folder("C:\\a", "Beta"),
            file("C:\\a", "f.txt", 1, 0),
        ];
        assert!(!tree.saw("C:\\a", &same, false));
        // A new folder, a hidden one (ignored while hidden ones are off), and a file.
        let more = [
            folder("C:\\a", "Alpha"),
            folder("C:\\a", "Beta"),
            folder("C:\\a", "Gamma"),
            hidden("C:\\a", "secret"),
        ];
        assert!(tree.saw("c:\\a\\", &more, false));
        assert_eq!(
            tree.kids("C:\\a"),
            Some(&Kids::Listed(vec![
                sub("Alpha", Some(true)),
                sub("Beta", Some(false)),
                sub("Gamma", None)
            ]))
        );
    }

    #[test]
    fn hiding_or_showing_hidden_folders_reads_the_open_ones_again() {
        let mut tree = Tree::default();
        tree.set_open("C:\\a", true);
        tree.set_open("C:\\b", true);
        tree.set_open("C:\\b", false);
        let read = tree.begin_read("C:\\a");
        tree.finish_read("C:\\a", read, Ok(vec![]));
        let again = tree.hidden_changed();
        assert_eq!(again, ["C:\\a"]);
        assert!(tree.kids("C:\\a").is_none());
        assert_eq!(tree.open_keys().len(), 1);
    }
}
