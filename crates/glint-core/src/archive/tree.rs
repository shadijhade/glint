//! An archive's entries as a tree of folders, built once when the archive is read: what a listing,
//! a search or folder sizes need, without going back to the archive. Names are made safe for
//! Windows here, once, so the path shown is exactly where an entry would be extracted to.

use crate::fsx::{natural_key, Listed};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

/// The archive's top folder.
pub const ROOT: u32 = 0;
/// A folder no entry of the archive stands for: it's only on the way to the entries in it.
pub const NO_ITEM: u32 = u32::MAX;

/// Attribute bits on entries inside an archive. Windows' own FILE_ATTRIBUTE_ENCRYPTED marks a
/// password-protected entry; the two high bits aren't Windows attributes (it doesn't use them).
pub const ENCRYPTED: u32 = 0x4000;
/// Every entry of an archive: it's read-only, and opening it means extracting it.
pub const IN_ARCHIVE: u32 = 0x4000_0000;
/// Compressed with a method Glint can't read.
pub const UNREADABLE: u32 = 0x1000_0000;
/// Listed, but holding no data: a file in an ncdu export.
pub const LIST_ONLY: u32 = 0x2000_0000;
/// The attributes an archive may set on its entries (read-only, hidden, system); the rest of what
/// it stores is left out, so nothing in an archive looks like an online-only file or a link.
pub const KEPT_ATTRIBUTES: u32 = 0x1 | 0x2 | 0x4;

/// One entry as the archive lists it.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// Its path in the archive as stored (decoded), with `/` or `\` between folders.
    pub path: String,
    pub dir: bool,
    /// Uncompressed size.
    pub size: u64,
    /// Milliseconds since the Unix epoch; 0 when unknown.
    pub modified: i64,
    /// FILE_ATTRIBUTE_* bits (only `KEPT_ATTRIBUTES` count), plus `ENCRYPTED` or `UNREADABLE`.
    pub attrs: u32,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: Box<str>,
    pub parent: u32,
    /// The archive's entry for it (its index in the archive), or `NO_ITEM` for a folder that's
    /// only implied by the paths of what's in it.
    pub item: u32,
    pub dir: bool,
    /// A file's uncompressed size; a folder's is everything in it.
    pub size: u64,
    /// A folder without an entry of its own has the date of the newest thing in it.
    pub modified: i64,
    pub attrs: u32,
    /// A folder's contents, folders first, in natural order.
    pub children: Vec<u32>,
}

impl Listed for &Node {
    fn name(&self) -> &str {
        &self.name
    }
    fn dir(&self) -> bool {
        self.dir
    }
    fn size(&self) -> u64 {
        self.size
    }
    fn modified(&self) -> i64 {
        self.modified
    }
    fn attrs(&self) -> u32 {
        self.attrs
    }
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// (folder, lowercase name) → node.
    index: HashMap<(u32, Box<str>), u32>,
}

/// Names Windows keeps for devices: a file can't be called that, with or without an extension.
const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

/// A name from an archive as Windows can have it, or None for one that names no file at all (".",
/// "..", nothing). Characters Windows doesn't allow become "_" (7-Zip does the same), trailing dots
/// and spaces go (Windows drops them), device names get a "_" in front, and names longer than
/// Windows allows are cut.
pub fn clean_name(raw: &str) -> Option<String> {
    let bad = |c: char| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*');
    let mut name: String = if raw.contains(bad) { raw.chars().map(|c| if bad(c) { '_' } else { c }).collect() } else { raw.to_string() };
    let kept = name.trim_end_matches(['.', ' ']).len();
    name.truncate(kept);
    if name.is_empty() {
        return None;
    }
    // Device names are 3 or 4 letters before any extension (and spaces).
    let stem = name.split('.').next().unwrap_or("").trim_end();
    if (3..=5).contains(&stem.len()) {
        let stem = stem.to_ascii_uppercase();
        let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
        if DEVICES.contains(&stem.as_str()) || numbered("COM") || numbered("LPT") {
            name.insert(0, '_');
        }
    }
    // NTFS allows 255 UTF-16 units in a name.
    if name.encode_utf16().count() > 255 {
        let mut units = 0;
        let cut = name.char_indices().find(|(_, c)| {
            units += c.len_utf16();
            units > 255
        });
        if let Some((at, _)) = cut {
            name.truncate(at);
        }
    }
    Some(name)
}

/// The folders on the way to an entry and its own name, each made safe; empty for a path that
/// names nothing ("/", "../.."). Leading separators, "." and ".." are left out, so no entry can
/// reach outside the archive's top folder however its path is written.
pub fn clean_path(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter_map(clean_name).collect()
}

fn fold(name: &str) -> Box<str> {
    if name.is_ascii() { name.to_ascii_lowercase() } else { name.to_lowercase() }.into_boxed_str()
}

impl Tree {
    /// Builds the tree from the archive's entries, in the archive's order. Folders are made for every
    /// path that implies them. When names clash (ignoring case, as Windows does), a later file
    /// replaces an earlier one, as extracting them in order would, and a folder wins over a file.
    pub fn build(items: &[Item]) -> Tree {
        let mut tree = Tree {
            nodes: vec![Node { name: "".into(), parent: ROOT, item: NO_ITEM, dir: true, size: 0, modified: 0, attrs: IN_ARCHIVE, children: Vec::new() }],
            index: HashMap::with_capacity(items.len() + items.len() / 8),
        };
        // The folder each folder path (as the archive writes it) leads to, and the last one: archives
        // usually list a folder's entries together, so most entries find theirs without a lookup.
        let mut seen: HashMap<&str, u32> = HashMap::new();
        let mut last_folder: (&str, u32) = ("", ROOT);
        for (i, item) in items.iter().enumerate() {
            let path = item.path.trim_end_matches(['/', '\\']);
            let (folders, own) = path.rsplit_once(['/', '\\']).unwrap_or(("", path));
            let (parent, last) = match clean_name(own) {
                Some(last) => {
                    let parent = if folders == last_folder.0 {
                        last_folder.1
                    } else if let Some(&parent) = seen.get(folders) {
                        parent
                    } else {
                        let mut parent = ROOT;
                        for folder in clean_path(folders) {
                            parent = tree.folder(parent, &folder);
                        }
                        seen.insert(folders, parent);
                        parent
                    };
                    last_folder = (folders, parent);
                    (parent, last)
                }
                // A name that names nothing ("a/.."): the path without it, as `clean_path` reads it.
                _ => {
                    let parts = clean_path(&item.path);
                    let Some((last, folders)) = parts.split_last() else { continue };
                    let mut parent = ROOT;
                    for folder in folders {
                        parent = tree.folder(parent, folder);
                    }
                    (parent, last.clone())
                }
            };
            let last = &last;
            let attrs = IN_ARCHIVE | (item.attrs & (KEPT_ATTRIBUTES | ENCRYPTED | UNREADABLE | LIST_ONLY));
            if item.dir {
                let node = tree.folder(parent, last);
                let n = &mut tree.nodes[node as usize];
                n.item = i as u32;
                n.modified = item.modified;
                n.attrs = IN_ARCHIVE | (attrs & KEPT_ATTRIBUTES);
                // What's in it is usually next, and finds it by its path at once (a deep tree would
                // otherwise walk its whole path for each).
                seen.insert(path, node);
                continue;
            }
            let file = Node { name: last.clone().into_boxed_str(), parent, item: i as u32, dir: false, size: item.size, modified: item.modified, attrs, children: Vec::new() };
            let Tree { nodes, index } = &mut tree;
            match index.entry((parent, fold(last))) {
                Entry::Occupied(existing) if nodes[*existing.get() as usize].dir => {}
                Entry::Occupied(existing) => nodes[*existing.get() as usize] = file,
                Entry::Vacant(place) => {
                    let node = nodes.len() as u32;
                    nodes.push(file);
                    nodes[parent as usize].children.push(node);
                    place.insert(node);
                }
            }
        }
        tree.total_up();
        tree.sort();
        tree
    }

    /// The folder `name` in `parent`, made if it isn't there yet (a file of that name becomes it).
    fn folder(&mut self, parent: u32, name: &str) -> u32 {
        if let Some(&node) = self.index.get(&(parent, fold(name))) {
            let n = &mut self.nodes[node as usize];
            if !n.dir {
                *n = Node { name: n.name.clone(), parent, item: NO_ITEM, dir: true, size: 0, modified: 0, attrs: IN_ARCHIVE, children: Vec::new() };
            }
            return node;
        }
        let node = self.nodes.len() as u32;
        self.nodes.push(Node { name: name.into(), parent, item: NO_ITEM, dir: true, size: 0, modified: 0, attrs: IN_ARCHIVE, children: Vec::new() });
        self.nodes[parent as usize].children.push(node);
        self.index.insert((parent, fold(name)), node);
        node
    }

    /// Folder totals, and dates for folders the archive has no entry for. A node always comes after
    /// its folder, so one pass from the end adds everything up.
    fn total_up(&mut self) {
        for i in (1..self.nodes.len()).rev() {
            let (size, modified, parent) = (self.nodes[i].size, self.nodes[i].modified, self.nodes[i].parent as usize);
            let folder = &mut self.nodes[parent];
            folder.size += size;
            if folder.item == NO_ITEM {
                folder.modified = folder.modified.max(modified);
            }
        }
    }

    /// Folders first, then names in natural order, as folders on disk are listed (`fsx::sort_entries`).
    fn sort(&mut self) {
        let keys: Vec<Vec<u8>> = self.nodes.iter().map(|n| if n.name.is_ascii() { natural_key(&n.name) } else { natural_key(&n.name.to_lowercase()) }).collect();
        for i in 0..self.nodes.len() {
            if self.nodes[i].children.len() < 2 {
                continue;
            }
            let mut children = std::mem::take(&mut self.nodes[i].children);
            let nodes = &self.nodes;
            children.sort_unstable_by(|&a, &b| {
                let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
                nb.dir.cmp(&na.dir).then_with(|| keys[a as usize].cmp(&keys[b as usize])).then_with(|| na.name.cmp(&nb.name))
            });
            self.nodes[i].children = children;
        }
    }

    pub fn node(&self, node: u32) -> &Node {
        &self.nodes[node as usize]
    }

    /// `name` in the folder `parent`, ignoring case.
    pub fn child(&self, parent: u32, name: &str) -> Option<u32> {
        self.index.get(&(parent, fold(name))).copied()
    }

    /// The path of `node` below the archive's top, with "\" between folders.
    pub fn path_of(&self, mut node: u32) -> String {
        let mut parts = Vec::new();
        while node != ROOT {
            let n = self.node(node);
            parts.push(&*n.name);
            node = n.parent;
        }
        parts.reverse();
        parts.join("\\")
    }

    /// Every file at or below `node` (itself, for a file), in tree order.
    pub fn files_under(&self, node: u32) -> Vec<u32> {
        let mut found = Vec::new();
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            let at = self.node(n);
            if at.dir {
                stack.extend(at.children.iter().rev());
            } else {
                found.push(n);
            }
        }
        found
    }

    /// Every folder at or below `node`, parents first.
    pub fn folders_under(&self, node: u32) -> Vec<u32> {
        let mut found = Vec::new();
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            let at = self.node(n);
            if at.dir {
                found.push(n);
                stack.extend(at.children.iter().rev());
            }
        }
        found
    }

    /// How many nodes (files and folders) the tree has, the top folder left out.
    pub fn len(&self) -> usize {
        self.nodes.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, size: u64, modified: i64) -> Item {
        Item { path: path.into(), dir: path.ends_with('/'), size, modified, attrs: 0 }
    }

    fn names(tree: &Tree, node: u32) -> Vec<String> {
        tree.node(node).children.iter().map(|&c| tree.node(c).name.to_string()).collect()
    }

    #[test]
    fn makes_names_windows_can_have() {
        assert_eq!(clean_name("photo.jpg").as_deref(), Some("photo.jpg"));
        assert_eq!(clean_name("a:b*c?.txt").as_deref(), Some("a_b_c_.txt"));
        assert_eq!(clean_name("trailing. . ").as_deref(), Some("trailing"));
        assert_eq!(clean_name("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(clean_name("LPT1").as_deref(), Some("_LPT1"));
        assert_eq!(clean_name("COM10").as_deref(), Some("COM10"));
        assert_eq!(clean_name("console").as_deref(), Some("console"));
        assert_eq!(clean_name("tab\there").as_deref(), Some("tab_here"));
        assert_eq!(clean_name(".hidden").as_deref(), Some(".hidden"));
        assert_eq!(clean_name("é😀"), Some("é😀".into()));
        assert_eq!(clean_name(".."), None);
        assert_eq!(clean_name("."), None);
        assert_eq!(clean_name(""), None);
        assert_eq!(clean_name("😀".repeat(200).as_str()).unwrap().encode_utf16().count(), 254);
    }

    #[test]
    fn keeps_every_path_inside_the_archive() {
        assert_eq!(clean_path("../../evil.txt"), ["evil.txt"]);
        assert_eq!(clean_path("/etc/passwd"), ["etc", "passwd"]);
        assert_eq!(clean_path("C:\\Windows\\win.ini"), ["C_", "Windows", "win.ini"]);
        assert_eq!(clean_path("a/./b/../c"), ["a", "b", "c"]);
        assert_eq!(clean_path("\\\\server\\share\\x"), ["server", "share", "x"]);
        assert!(clean_path("../..").is_empty());
    }

    #[test]
    fn builds_folders_with_totals() {
        let items = [
            item("docs/", 0, 5),
            item("docs/b.txt", 10, 7),
            item("docs/a10.txt", 20, 9),
            item("docs/a9.txt", 30, 8),
            item("photos/2024/summer/x.jpg", 100, 50),
            item("readme.md", 1, 1),
            item("Docs/sub/deep.bin", 1000, 60),
        ];
        let tree = Tree::build(&items);
        assert_eq!(names(&tree, ROOT), ["docs", "photos", "readme.md"]);
        let docs = tree.child(ROOT, "DOCS").unwrap();
        assert_eq!(names(&tree, docs), ["sub", "a9.txt", "a10.txt", "b.txt"]);
        // A folder with an entry keeps its own date; implied ones have their newest item's.
        assert_eq!((tree.node(docs).size, tree.node(docs).modified, tree.node(docs).item), (1060, 5, 0));
        let photos = tree.child(ROOT, "photos").unwrap();
        assert_eq!((tree.node(photos).size, tree.node(photos).modified, tree.node(photos).item), (100, 50, NO_ITEM));
        assert_eq!(tree.node(ROOT).size, 1161);
        let summer = tree.child(tree.child(photos, "2024").unwrap(), "summer").unwrap();
        let x = tree.child(summer, "x.jpg").unwrap();
        assert_eq!(tree.path_of(x), "photos\\2024\\summer\\x.jpg");
        assert_eq!(tree.files_under(photos), [x]);
        assert_eq!(tree.files_under(ROOT).len(), 6);
        assert_eq!(tree.folders_under(ROOT).len(), 6);
        assert!(tree.node(x).attrs & IN_ARCHIVE != 0);
    }

    #[test]
    fn later_files_replace_earlier_ones_and_folders_win() {
        let items = [item("a.txt", 1, 0), item("A.TXT", 2, 0), item("b", 3, 0), item("b/c.txt", 4, 0), item("d/", 0, 0), item("d", 5, 0)];
        let tree = Tree::build(&items);
        assert_eq!(names(&tree, ROOT), ["b", "d", "A.TXT"]);
        let a = tree.child(ROOT, "a.txt").unwrap();
        assert_eq!((tree.node(a).size, tree.node(a).item), (2, 1));
        let b = tree.child(ROOT, "b").unwrap();
        assert!(tree.node(b).dir);
        assert_eq!(tree.node(b).size, 4);
        assert!(tree.node(tree.child(ROOT, "d").unwrap()).dir);
        assert_eq!(tree.node(ROOT).size, 6);
    }

    #[test]
    fn keeps_only_harmless_attributes() {
        let items = [Item { path: "x".into(), dir: false, size: 1, modified: 0, attrs: 0x2 | 0x400 | 0x1000 | ENCRYPTED }];
        let tree = Tree::build(&items);
        assert_eq!(tree.node(1).attrs, IN_ARCHIVE | 0x2 | ENCRYPTED);
    }
}
