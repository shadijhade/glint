//! Tags and media facts for the rows a view shows, read in batches through core and kept per folder.
//!
//! Ported from `src/lib/tags.ts` (`wantTags`, `forgetTags`, `useTags.byPath`) and `src/lib/media.ts`
//! (`readFacts`, `useFacts`, `takenKey`), with one difference that matters for speed: the web asked for the tags
//! of every row it drew and the facts of every file in the folder, while this reads only the rows a view says it
//! shows (and a little ahead), so a folder of 10,000 files scrolls as smoothly as one of ten.
//!
//! # How a view uses it
//!
//! 1. Get the shared model once: `let facts = row_facts::shared(cx);` and observe it to redraw when reads land.
//! 2. While drawing, say which entries are on screen: `facts.update(cx, |f, cx| f.ask(folder, entries, want, cx))`.
//!    `folder` is any string that names what the view shows (the explorer tab's `view_key()`); `want` says what
//!    to read ([`Want`]). Asking again for what is read, or being read, costs a lookup and nothing else, and the
//!    reads of one frame go out together (a batch of tags, a batch of media details).
//! 3. Draw from what is kept: [`RowFacts::tags_of`], [`RowFacts::media_of`] (`None` while unread: draw nothing,
//!    as the web did) and, for a media column, [`media_cells`].
//!
//! The list uses it first; the grid will use it for its tiles' dots, and needs nothing from the list.
//!
//! # What is kept, and for how long
//!
//! - **Per folder** (a few at a time: panes and tabs), least recently asked for let go first. A folder keeps at
//!   most [`MOST_PER_FOLDER`] reads.
//! - **Tags** come back when the folder is listed again (the listing's `ListingChanged`): what was read keeps
//!   showing meanwhile and the rows on screen read again, so a dot never blinks. They come back for the paths a
//!   tag change names too ([`TagsChanged`] from `dialogs::tags`).
//! - **Media** is read once per version of a file (its path and modified time, `takenKey`).
//! - A failed read is kept as "says nothing" (no dots, empty cells) rather than asked for again every frame, as
//!   the web did (`.catch(() => part.map(() => null))`); the next listing of the folder tries again.
//!
//! Core is called through `core_bridge::run`, never on the UI thread. Tests give the tags backend
//! (`dialogs::tags::init_with`) and the media source ([`init_with`]) fakes.

use crate::core_bridge;
use crate::dialogs::tags::{self, Backend, Fut, TagsChanged};
use crate::explorer::{self, ExplorerEvent};
use crate::i18n;
use crate::kit::prelude::*;
use crate::t;
use glint_core::media::MediaDetails;
use glint_model::columns::clock_time;
#[cfg(test)]
use glint_model::columns::{ColumnId, FACT_SORTS};
#[cfg(test)]
use glint_model::entries::SortKey;
use glint_model::entries::{is_archived, is_recycled, FileEntry, MediaFacts};
use glint_model::format::{format_date, format_full_date};
use glint_model::marks::Marks;
use glint_model::paths::is_remote_path;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How many paths one `tags_read` call is given.
const TAGS_BATCH: usize = 200;
/// How many files one `media_details_many` call is given (`readFacts` read 300 at a time).
const MEDIA_BATCH: usize = 300;
/// How many folders' reads are kept (two panes, a few tabs).
const KEEP_FOLDERS: usize = 8;
/// The most reads a folder keeps; past it the folder starts over (the rows on screen are read again).
pub const MOST_PER_FOLDER: usize = 20_000;

// ---- What a view asks for ---------------------------------------------------------------------------------

/// Which files to read what they say about themselves for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Media {
    /// None: no media column is shown.
    #[default]
    None,
    /// Photos and videos, the files that can say when they were taken (`canBeTaken`).
    Taken,
    /// Photos, videos and songs (`hasDetails`).
    All,
}

impl Media {
    /// Whether the details of `entry` are read under this choice.
    pub fn reads(self, entry: &FileEntry) -> bool {
        match self {
            Media::None => false,
            Media::Taken => can_be_taken(entry),
            Media::All => has_details(entry),
        }
    }
}

/// What to read for the entries asked about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    /// Each entry's tags (the dots).
    pub tags: bool,
    /// What the files say about themselves (the Taken, Dimensions, Duration, Artist and Album columns).
    pub media: Media,
}

/// What a list needs read for its columns and sort: every media file's facts while a column of them is shown or
/// the list is sorted by them (`factsWanted`), else nothing.
#[cfg(test)]
pub fn media_wanted(columns: &[ColumnId], sort: SortKey) -> Media {
    if FACT_SORTS.contains(&sort) || columns.iter().any(|id| id.def().facts) {
        Media::All
    } else {
        Media::None
    }
}

const PICTURES: &str = "jpg jpeg jpe jfif heic heif avif png tif tiff webp dng cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef iiq srw gif bmp ico jxl psd qoi";
const VIDEOS: &str = "mp4 m4v mov 3gp 3g2 mkv webm";

fn listed(list: &str, ext: &str) -> bool {
    !ext.is_empty() && list.split(' ').any(|known| known == ext)
}

/// Photos and videos: the files that can say when they were taken (`canBeTaken`).
pub fn can_be_taken(entry: &FileEntry) -> bool {
    !entry.dir && (listed(PICTURES, entry.ext()) || listed(VIDEOS, entry.ext()))
}

/// Photos, videos and songs: the files with details to read (`hasDetails`).
pub fn has_details(entry: &FileEntry) -> bool {
    !entry.dir && crate::components::preview::details::has_details(entry)
}

/// Whether the files behind `entry` can be asked for their tags: not an item inside an archive, not one in the
/// Recycle Bin (its path is not where the file is), not one on a remote connection.
fn on_this_disk(entry: &FileEntry) -> bool {
    !is_archived(entry) && !is_recycled(entry) && !is_remote_path(&entry.path)
}

/// Whether `path` is on another computer (a share, `\\server\share\…`, or a mapped network drive). A row there
/// doesn't ask for its tags: each read is a trip over the network, one per row on screen. The selection's still
/// are read (`ask_tags_of`), for the menus and the Tags dialog. A drive's kind is asked of Windows once a minute.
fn on_network(path: &str) -> bool {
    if path.starts_with(r"\\") {
        return !path.starts_with(r"\\?\") || path.starts_with(r"\\?\UNC\");
    }
    let Some(letter) = path
        .as_bytes()
        .first()
        .filter(|l| l.is_ascii_alphabetic() && path.as_bytes().get(1) == Some(&b':'))
    else {
        return false;
    };
    thread_local! {
        static DRIVES: std::cell::RefCell<[Option<(bool, std::time::Instant)>; 26]> =
            const { std::cell::RefCell::new([None; 26]) };
    }
    let index = usize::from(letter.to_ascii_uppercase() - b'A');
    DRIVES.with(|drives| {
        let mut drives = drives.borrow_mut();
        match drives[index] {
            Some((remote, asked)) if asked.elapsed() < std::time::Duration::from_secs(60) => remote,
            _ => {
                let remote = glint_core::fsx::on_other_computer(&format!("{}:\\", char::from(*letter)));
                drives[index] = Some((remote, std::time::Instant::now()));
                remote
            }
        }
    })
}

// ---- Where the reads go -----------------------------------------------------------------------------------

/// The one call into glint-core's media.rs that the rows need. The app uses [`CoreMedia`]; tests a fake.
pub trait MediaSource: Send + Sync {
    /// What each file says (`media_details_many`, without covers): None for a file that says nothing.
    fn read(&self, paths: Vec<String>) -> Fut<Vec<Option<MediaFacts>>>;
}

/// glint-core's media details.
pub struct CoreMedia;

impl MediaSource for CoreMedia {
    fn read(&self, paths: Vec<String>) -> Fut<Vec<Option<MediaFacts>>> {
        Box::pin(async move {
            let found = glint_core::media::media_details_many(paths).await?;
            Ok(found.iter().map(|details| details.as_ref().map(facts_of)).collect())
        })
    }
}

/// What the rows need of a file's details (`factsOf` in media.ts).
pub fn facts_of(details: &MediaDetails) -> MediaFacts {
    MediaFacts {
        // Milliseconds; a date that is not a number says nothing.
        taken: details.taken.filter(|ms| ms.is_finite()).map(|ms| ms as i64),
        width: details.width,
        height: details.height,
        duration: details.duration,
        camera: details.camera.clone(),
        artist: details.artist.clone(),
        album: details.album.clone(),
    }
}

// ---- What is kept -----------------------------------------------------------------------------------------

/// A path's marks as read (its tags among them), and when.
struct TagsRead {
    marks: Marks,
    /// The store's epoch when read. Fresh while it equals the store's; 0 never does (a mark change named this
    /// path), so the next ask reads it again while `marks` keeps showing.
    epoch: u64,
}

/// A file's media facts as read, for one version of the file.
struct MediaRead {
    /// The file's modified time when asked: another one means another version.
    modified: i64,
    facts: Option<MediaFacts>,
}

/// Everything read for one folder.
struct Store {
    /// The marks read, by the path the listing gave (they began as tags: the name stayed).
    tags: HashMap<String, TagsRead>,
    /// Paths being read, with the ticket of the read that counts (a tag change cancels it by dropping it).
    tags_inflight: HashMap<String, u64>,
    media: HashMap<String, MediaRead>,
    /// Paths being read, and the modified time they were asked for.
    media_inflight: HashMap<String, i64>,
    /// A whole read of the folder has landed (`ask_all`): a sort needs every row, so `trim` leaves it be.
    whole: bool,
    /// The ticket of the whole read on its way.
    whole_inflight: Option<u64>,
    /// How many entries the last whole read was asked with: a list that has more since (a search's results come in
    /// batches under one view) isn't whole until what came after is read too.
    whole_count: usize,
    /// Counts the times the folder was listed again; starts at 1 so that 0 can mean "stale".
    epoch: u64,
}

impl Store {
    fn new() -> Self {
        Store {
            tags: HashMap::new(),
            tags_inflight: HashMap::new(),
            media: HashMap::new(),
            media_inflight: HashMap::new(),
            whole: false,
            whole_inflight: None,
            whole_count: 0,
            epoch: 1,
        }
    }

    /// The folder holds too many reads: it starts over, and the rows on screen are asked for again.
    fn trim(&mut self) {
        if self.whole || self.whole_inflight.is_some() {
            return;
        }
        if self.tags.len() + self.media.len() > MOST_PER_FOLDER {
            self.tags.clear();
            self.media.clear();
            self.tags_inflight.clear();
            self.media_inflight.clear();
        }
    }
}

/// A read to make, for the folder `folder`.
struct TagsAsk {
    folder: String,
    path: String,
    ticket: u64,
    /// A whole read only: what the index says can be older than this item's change, so it is read from its file.
    file: bool,
}

/// A folder's marks to read in one go ([`RowFacts::ask_all`]).
struct WholeAsk {
    ticket: u64,
    asks: Vec<TagsAsk>,
    /// The folder the items are right inside, to ask the index for; None where they are not all in one.
    folder: Option<String>,
}

struct MediaAsk {
    folder: String,
    path: String,
    modified: i64,
}

/// The reads of one frame, taken to be made.
struct Batch {
    tags: Vec<TagsAsk>,
    /// A folder's marks read in one call, with the ticket that tells the read still counts.
    whole: Vec<WholeAsk>,
    media: Vec<MediaAsk>,
    backend: Arc<dyn Backend>,
    source: Arc<dyn MediaSource>,
}

/// The tags and media facts of the rows views show. One per app ([`shared`]); see the module notes.
pub struct RowFacts {
    source: Arc<dyn MediaSource>,
    stores: HashMap<String, Store>,
    /// The folders asked about, the most recent last.
    order: Vec<String>,
    pending_tags: Vec<TagsAsk>,
    pending_whole: Vec<WholeAsk>,
    pending_media: Vec<MediaAsk>,
    /// A task is reading (or about to): it takes whatever is pending when it looks.
    flushing: bool,
    next_ticket: u64,
    _subscriptions: Vec<Subscription>,
}

impl RowFacts {
    /// A model reading media through `source`, tags through the shared tags backend, listening for folders listed
    /// again and for tags changed.
    pub fn new(source: Arc<dyn MediaSource>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(explorer) = explorer::try_explorer(cx) {
            subscriptions.push(cx.subscribe(&explorer, |this, explorer, event: &ExplorerEvent, cx| {
                if let ExplorerEvent::ListingChanged { tab, .. } = event {
                    let key = explorer.read(cx).tab(*tab).map(|tab| tab.view_key());
                    if let Some(key) = key {
                        this.listed_again(&key, cx);
                    }
                }
            }));
        }
        let hub = tags::hub(cx);
        subscriptions.push(cx.subscribe(&hub, |this, _, event: &TagsChanged, cx| {
            this.tags_changed(&event.paths, cx)
        }));
        RowFacts {
            source,
            stores: HashMap::new(),
            order: Vec::new(),
            pending_tags: Vec::new(),
            pending_whole: Vec::new(),
            pending_media: Vec::new(),
            flushing: false,
            next_ticket: 0,
            _subscriptions: subscriptions,
        }
    }

    // ---- Asking ---------------------------------------------------------------------------------------

    /// These entries of `folder` are on screen: reads what `want` says of the ones not read yet (together with
    /// whatever else is asked for in the same moment). Nothing here waits for core.
    pub fn ask<'a>(
        &mut self,
        folder: &str,
        entries: impl IntoIterator<Item = &'a FileEntry>,
        want: Want,
        cx: &mut Context<Self>,
    ) {
        if !want.tags && want.media == Media::None {
            return;
        }
        let ticket = &mut self.next_ticket;
        let store = Self::store_for(&mut self.stores, &mut self.order, folder);
        let (mut tags, mut media) = (Vec::new(), Vec::new());
        for entry in entries {
            if want.tags && on_this_disk(entry) && !on_network(&entry.path) {
                ask_tags(store, folder, &entry.path, ticket, &mut tags);
            }
            if want.media.reads(entry) && on_this_disk(entry) {
                ask_media(store, folder, entry, &mut media);
            }
        }
        self.queue(tags, Vec::new(), media, cx);
    }

    /// [`RowFacts::ask`] for tags only, by path: the items a menu will be about (the selection), which may be
    /// off screen.
    pub fn ask_tags_of<'a>(&mut self, folder: &str, paths: impl IntoIterator<Item = &'a str>, cx: &mut Context<Self>) {
        let ticket = &mut self.next_ticket;
        let store = Self::store_for(&mut self.stores, &mut self.order, folder);
        let mut tags = Vec::new();
        for path in paths {
            ask_tags(store, folder, path, ticket, &mut tags);
        }
        self.queue(tags, Vec::new(), Vec::new(), cx);
    }

    /// The folder's store, made if it is new, and now the most recently used; the oldest beyond [`KEEP_FOLDERS`]
    /// are let go.
    fn store_for<'s>(stores: &'s mut HashMap<String, Store>, order: &mut Vec<String>, folder: &str) -> &'s mut Store {
        if order.last().map(String::as_str) != Some(folder) {
            order.retain(|known| known != folder);
            order.push(folder.to_string());
            while order.len() > KEEP_FOLDERS {
                let oldest = order.remove(0);
                stores.remove(&oldest);
            }
        }
        stores.entry(folder.to_string()).or_insert_with(Store::new)
    }

    /// Puts the reads to make in line, and starts the task that makes them if none is running.
    fn queue(
        &mut self,
        tags: Vec<TagsAsk>,
        whole: Vec<WholeAsk>,
        media: Vec<MediaAsk>,
        cx: &mut Context<Self>,
    ) {
        if tags.is_empty() && whole.is_empty() && media.is_empty() {
            return;
        }
        self.pending_tags.extend(tags);
        self.pending_whole.extend(whole);
        self.pending_media.extend(media);
        if self.flushing {
            return;
        }
        self.flushing = true;
        // The task starts after the frame that asked, so everything that frame asks for goes out together.
        cx.spawn(async move |this, cx| loop {
            let Ok(batch) = this.update(cx, |this, cx| this.take(cx)) else {
                return;
            };
            let Some(batch) = batch else { return };
            for part in batch.tags.chunks(TAGS_BATCH) {
                let paths = part.iter().map(|ask| ask.path.clone()).collect();
                let read = core_bridge::run(batch.backend.read_marks(paths)).await.and_then(|r| r);
                if this.update(cx, |this, cx| this.tags_arrived(part, read, cx)).is_err() {
                    return;
                }
            }
            // A folder read whole is one call, however long (core reads a long list on every core).
            for whole in &batch.whole {
                let read = read_whole(&batch.backend, whole).await;
                let landed = this.update(cx, |this, cx| this.whole_arrived(whole.ticket, &whole.asks, read, cx));
                if landed.is_err() {
                    return;
                }
            }
            for part in batch.media.chunks(MEDIA_BATCH) {
                let paths = part.iter().map(|ask| ask.path.clone()).collect();
                let read = core_bridge::run(batch.source.read(paths)).await.and_then(|r| r);
                if this.update(cx, |this, cx| this.media_arrived(part, read, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Takes the reads waiting. With none, the task is over (and the next ask starts another).
    fn take(&mut self, cx: &mut Context<Self>) -> Option<Batch> {
        if self.pending_tags.is_empty() && self.pending_whole.is_empty() && self.pending_media.is_empty() {
            self.flushing = false;
            return None;
        }
        Some(Batch {
            tags: std::mem::take(&mut self.pending_tags),
            whole: std::mem::take(&mut self.pending_whole),
            media: std::mem::take(&mut self.pending_media),
            backend: tags::hub(cx).read(cx).backend(),
            source: self.source.clone(),
        })
    }

    // ---- What came back -------------------------------------------------------------------------------

    fn tags_arrived(&mut self, asked: &[TagsAsk], read: Result<Vec<Marks>, String>, cx: &mut Context<Self>) {
        if self.keep_marks(asked, read) {
            cx.notify();
        }
    }

    /// A folder read whole landed: its marks are kept, and the folder counts as read whole unless it was listed
    /// again or a mark in it changed meanwhile.
    fn whole_arrived(&mut self, ticket: u64, asked: &[TagsAsk], read: Result<Vec<Marks>, String>, cx: &mut Context<Self>) {
        let mut changed = self.keep_marks(asked, read);
        if let Some(store) = asked.first().and_then(|ask| self.stores.get_mut(&ask.folder)) {
            if store.whole_inflight == Some(ticket) {
                store.whole_inflight = None;
                store.whole = true;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Keeps what a read answered for the paths asked; whether anything shown is different now.
    fn keep_marks(&mut self, asked: &[TagsAsk], read: Result<Vec<Marks>, String>) -> bool {
        let mut lists = read.unwrap_or_default().into_iter();
        let mut changed = false;
        for ask in asked {
            // A failed or short answer says nothing for the paths it lacks.
            let marks = lists.next().unwrap_or_default();
            let Some(store) = self.stores.get_mut(&ask.folder) else {
                continue;
            };
            // Cancelled by a tag change, or asked for again since: another read is on its way.
            if store.tags_inflight.get(&ask.path) != Some(&ask.ticket) {
                continue;
            }
            store.tags_inflight.remove(&ask.path);
            store.trim();
            let shown = store.tags.get(&ask.path).map(|read| &read.marks);
            changed |= shown.unwrap_or(&Marks::default()) != &marks;
            let epoch = store.epoch;
            store.tags.insert(ask.path.clone(), TagsRead { marks, epoch });
        }
        changed
    }

    fn media_arrived(
        &mut self,
        asked: &[MediaAsk],
        read: Result<Vec<Option<MediaFacts>>, String>,
        cx: &mut Context<Self>,
    ) {
        let mut found = read.unwrap_or_default().into_iter();
        let mut changed = false;
        for ask in asked {
            let facts = found.next().flatten();
            let Some(store) = self.stores.get_mut(&ask.folder) else {
                continue;
            };
            if store.media_inflight.get(&ask.path) != Some(&ask.modified) {
                continue;
            }
            store.media_inflight.remove(&ask.path);
            store.trim();
            store.media.insert(
                ask.path.clone(),
                MediaRead {
                    modified: ask.modified,
                    facts,
                },
            );
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    // ---- Changes ----------------------------------------------------------------------------------------

    /// The folder was listed again (`forgetTags` after a refresh): its tags are read again for the rows shown,
    /// keeping what was read on screen meanwhile.
    fn listed_again(&mut self, folder: &str, cx: &mut Context<Self>) {
        let Some(store) = self.stores.get_mut(folder) else {
            return;
        };
        store.epoch += 1;
        // Reads under way may predate the listing: they are asked again.
        store.tags_inflight.clear();
        store.whole = false;
        store.whole_inflight = None;
        cx.notify();
    }

    /// Tags were set on `paths`: they are read again, keeping the old dots until the new ones arrive.
    fn tags_changed(&mut self, paths: &[String], cx: &mut Context<Self>) {
        let named: HashSet<String> = paths.iter().map(|path| path.to_lowercase()).collect();
        for store in self.stores.values_mut() {
            let mut touched = false;
            for (path, read) in store.tags.iter_mut() {
                if named.contains(&path.to_lowercase()) {
                    read.epoch = 0;
                    touched = true;
                }
            }
            if touched {
                store.whole = false;
                store.whole_inflight = None;
            }
            store
                .tags_inflight
                .retain(|path, _| !named.contains(&path.to_lowercase()));
        }
        cx.notify();
    }

    // ---- Reading what is kept ---------------------------------------------------------------------------

    /// The tags read for `path` in `folder`, or None while they are unread. Empty for an item with none.
    pub fn tags_of(&self, folder: &str, path: &str) -> Option<&[String]> {
        self.stores.get(folder)?.tags.get(path).map(|read| read.marks.tags.as_slice())
    }

    /// The marks read for `path` in `folder` (docs/gpui/marks), or None while they are unread (draw nothing).
    pub fn marks_of(&self, folder: &str, path: &str) -> Option<&Marks> {
        self.stores.get(folder)?.tags.get(path).map(|read| &read.marks)
    }

    /// Whether every mark of `folder` was read in one go (`ask_all`) and is still good: false again once the
    /// folder is listed again or a mark in it changes, until the next `ask_all` lands.
    pub fn has_all(&self, folder: &str) -> bool {
        self.stores.get(folder).is_some_and(|store| store.whole)
    }

    /// Reads the marks of every entry of `folder` on core's now lane, for a list sorted, grouped or filtered by a
    /// mark (M5); the rows are built again once they are in.
    ///
    /// Everything `ask` would read marks for and that is neither fresh nor on its way is read in one call. Asking
    /// again while that read is on its way, or when everything is fresh, calls nothing.
    ///
    /// Where instant search covers the folder, its one answer gives each item's label and tags (an item it leaves
    /// out has no marks), and only the items it says have a comment are read from their files, for its text. An
    /// item a change was named for since (`TagsChanged`), and every item where the index can't say, is read from
    /// the file.
    pub fn ask_all(&mut self, folder: &str, entries: &[FileEntry], cx: &mut Context<Self>) {
        let ticket = &mut self.next_ticket;
        let store = Self::store_for(&mut self.stores, &mut self.order, folder);
        if store.whole_inflight.is_some() || (store.whole && store.whole_count == entries.len()) {
            return;
        }
        // Not whole until what is asked now has landed (the entries that came since the last whole read, if any).
        store.whole = false;
        store.whole_count = entries.len();
        let mut asks = Vec::new();
        for entry in entries {
            if !on_this_disk(entry) || on_network(&entry.path) || store.tags_inflight.contains_key(&entry.path) {
                continue;
            }
            let file = match store.tags.get(&entry.path) {
                Some(read) if read.epoch == store.epoch => continue,
                // Zero: a change was named for it since.
                Some(read) => read.epoch == 0,
                None => false,
            };
            *ticket += 1;
            store.tags_inflight.insert(entry.path.clone(), *ticket);
            asks.push(TagsAsk {
                folder: folder.to_string(),
                path: entry.path.clone(),
                ticket: *ticket,
                file,
            });
        }
        if asks.is_empty() {
            // Nothing left to read: if no batch is still on its way, every row is read.
            if store.tags_inflight.is_empty() {
                store.whole = true;
                cx.notify();
            }
            return;
        }
        *ticket += 1;
        store.whole_inflight = Some(*ticket);
        let in_folder = common_parent(asks.iter().filter(|ask| !ask.file).map(|ask| ask.path.as_str()));
        let whole = vec![WholeAsk {
            ticket: *ticket,
            asks,
            folder: in_folder,
        }];
        self.queue(Vec::new(), whole, Vec::new(), cx);
    }

    /// What `entry` says of itself, as read for its current version: None while unread (draw nothing), Some(None)
    /// for a file that says nothing.
    pub fn media_of(&self, folder: &str, entry: &FileEntry) -> Option<Option<&MediaFacts>> {
        let read = self.stores.get(folder)?.media.get(&entry.path)?;
        (read.modified == entry.modified).then_some(read.facts.as_ref())
    }

    /// The marks read for these paths in any folder, by path in lower case. Paths not read are left out.
    pub fn marks_among<'a>(&self, paths: impl IntoIterator<Item = &'a str>) -> HashMap<String, Marks> {
        let mut found = HashMap::new();
        for path in paths {
            let read = self.stores.values().find_map(|store| store.tags.get(path));
            if let Some(read) = read {
                found.insert(path.to_lowercase(), read.marks.clone());
            }
        }
        found
    }

    /// The tags read for these paths in any folder, by path in lower case (what the menus read as
    /// `MenuData::tags`). Paths not read are left out.
    pub fn tags_among<'a>(&self, paths: impl IntoIterator<Item = &'a str>) -> HashMap<String, Vec<String>> {
        let mut found = HashMap::new();
        for path in paths {
            let read = self.stores.values().find_map(|store| store.tags.get(path));
            if let Some(read) = read {
                found.insert(path.to_lowercase(), read.marks.tags.clone());
            }
        }
        found
    }

    /// How many folders are kept (tests).
    #[cfg(test)]
    pub(crate) fn folders_kept(&self) -> usize {
        self.stores.len()
    }
}

/// The folder every path is right inside (any case), or None for no path or paths in several.
fn common_parent<'a>(paths: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut found: Option<&str> = None;
    for path in paths {
        let at = path.rfind(['\\', '/'])?;
        let parent = &path[..at];
        match found {
            Some(known) if !known.eq_ignore_ascii_case(parent) => return None,
            _ => found = Some(parent),
        }
    }
    // A drive's root keeps its slash ("C:\").
    found.map(|parent| if parent.ends_with(':') { format!("{parent}\\") } else { parent.to_string() })
}

/// What a whole read of a folder's marks says, in the order of its asks: the index's answer where it can give one,
/// the files' for the rest and for every item with a comment (the index never sends a comment's text).
async fn read_whole(backend: &Arc<dyn Backend>, whole: &WholeAsk) -> Result<Vec<Marks>, String> {
    let mut marks = vec![Marks::default(); whole.asks.len()];
    let mut files: Vec<usize> = Vec::new();
    let mut indexed: Vec<usize> = Vec::new();
    for (i, ask) in whole.asks.iter().enumerate() {
        if ask.file { files.push(i) } else { indexed.push(i) }
    }
    let answer = match &whole.folder {
        Some(folder) if !indexed.is_empty() => {
            core_bridge::run(backend.marks_in(folder.clone())).await.and_then(|r| r).ok().flatten()
        }
        _ => None,
    };
    match answer {
        Some(items) => {
            let by_name: HashMap<String, _> = items.into_iter().map(|item| (item.name.to_lowercase(), item)).collect();
            for i in indexed {
                let path = &whole.asks[i].path;
                let name = path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase();
                let Some(item) = by_name.get(&name) else { continue };
                if item.comment {
                    files.push(i);
                } else {
                    marks[i] = Marks { label: item.label.clone(), tags: item.tags.clone(), comment: None };
                }
            }
        }
        None => files.extend(indexed),
    }
    if !files.is_empty() {
        let paths = files.iter().map(|&i| whole.asks[i].path.clone()).collect();
        // A failed read says nothing for these items, as it always has.
        if let Ok(read) = core_bridge::run(backend.read_marks(paths)).await.and_then(|r| r) {
            for (&i, read) in files.iter().zip(read) {
                marks[i] = read;
            }
        }
    }
    Ok(marks)
}

/// Queues `path` for a read of its tags if they are neither fresh nor on their way.
fn ask_tags(store: &mut Store, folder: &str, path: &str, ticket: &mut u64, queue: &mut Vec<TagsAsk>) {
    let fresh = store.tags.get(path).is_some_and(|read| read.epoch == store.epoch);
    if fresh || store.tags_inflight.contains_key(path) {
        return;
    }
    *ticket += 1;
    store.tags_inflight.insert(path.to_string(), *ticket);
    queue.push(TagsAsk {
        folder: folder.to_string(),
        path: path.to_string(),
        ticket: *ticket,
        file: false,
    });
}

/// Queues `entry` for a read of its details if this version of it is neither read nor on its way.
fn ask_media(store: &mut Store, folder: &str, entry: &FileEntry, queue: &mut Vec<MediaAsk>) {
    let read = store
        .media
        .get(&entry.path)
        .is_some_and(|read| read.modified == entry.modified);
    if read || store.media_inflight.get(&entry.path) == Some(&entry.modified) {
        return;
    }
    store.media_inflight.insert(entry.path.clone(), entry.modified);
    queue.push(MediaAsk {
        folder: folder.to_string(),
        path: entry.path.clone(),
        modified: entry.modified,
    });
}

// ---- The shared model -------------------------------------------------------------------------------------

struct SharedFacts(Entity<RowFacts>);

impl Global for SharedFacts {}

/// The media source chosen before the model was made (tests).
struct ChosenSource(Arc<dyn MediaSource>);

impl Global for ChosenSource {}

/// Makes the media reads go through `source` (the app uses core's; tests a fake), for the model made later and
/// the one made already.
pub fn init_with(cx: &mut App, source: Arc<dyn MediaSource>) {
    cx.set_global(ChosenSource(source.clone()));
    if let Some(shared) = cx.try_global::<SharedFacts>().map(|shared| shared.0.clone()) {
        shared.update(cx, |facts, _| facts.source = source);
    }
}

/// The shared model, made on first use. The tags are read through the tags hub's backend (`dialogs::tags`).
pub fn shared(cx: &mut App) -> Entity<RowFacts> {
    if let Some(shared) = cx.try_global::<SharedFacts>() {
        return shared.0.clone();
    }
    let source: Arc<dyn MediaSource> = match cx.try_global::<ChosenSource>() {
        Some(chosen) => chosen.0.clone(),
        None => Arc::new(CoreMedia),
    };
    let facts = cx.new(|cx| RowFacts::new(source, cx));
    cx.set_global(SharedFacts(facts.clone()));
    facts
}

/// The shared model, if a view has made it already (for code that only reads).
pub fn try_shared(cx: &App) -> Option<Entity<RowFacts>> {
    cx.try_global::<SharedFacts>().map(|shared| shared.0.clone())
}

// ---- What the media columns say ---------------------------------------------------------------------------

/// What a row's media cells say (the `case "taken"`… of `renderRow` in FileList.tsx), all empty while unread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaCells {
    /// When it was taken; a picture or video that doesn't say shows when it was modified, as File Explorer's Date
    /// does (but muted: see `taken_says`).
    pub taken: String,
    /// The file itself said when it was taken.
    pub taken_says: bool,
    /// The tooltip of the date: the full date, or why the date is the modified one.
    pub taken_title: Option<String>,
    /// "1920 × 1080".
    pub dimensions: String,
    /// "3:05" or "1:02:03".
    pub duration: String,
    pub artist: String,
    pub album: String,
}

/// "1920 × 1080", or nothing when either is missing or 0 (`facts?.width && facts.height`).
pub fn dimensions_text(facts: &MediaFacts) -> String {
    match (facts.width, facts.height) {
        (Some(width), Some(height)) if width != 0 && height != 0 => format!("{width} × {height}"),
        _ => String::new(),
    }
}

/// A video's or song's length as `clockTime` writes it, or nothing when it is 0 or missing.
pub fn duration_text(facts: &MediaFacts) -> String {
    match facts.duration {
        Some(seconds) if seconds != 0. && !seconds.is_nan() => clock_time(seconds),
        _ => String::new(),
    }
}

/// What the media cells of `entry` say, given what was read of it (`media_of`): empty while `read` is None and
/// for folders. The date is written in the interface's language.
pub fn media_cells(entry: &FileEntry, read: Option<Option<&MediaFacts>>) -> MediaCells {
    if entry.dir {
        return MediaCells::default();
    }
    let facts = read.flatten();
    let says = facts.and_then(|facts| facts.taken).filter(|&ms| ms != 0);
    // A picture that doesn't say shows when it was modified, once its file has been read.
    let shown =
        says.or_else(|| (read.is_some() && can_be_taken(entry) && entry.modified != 0).then_some(entry.modified));
    let taken_title = match (says, shown) {
        (Some(ms), _) => Some(t!("Taken {date}", date = format_full_date(ms)).to_string()),
        (None, Some(ms)) => Some(
            t!(
                "It doesn't say when it was taken: modified {date}",
                date = format_full_date(ms)
            )
            .to_string(),
        ),
        (None, None) => None,
    };
    MediaCells {
        taken: shown
            .map(|ms| format_date(ms, None, i18n::language()))
            .unwrap_or_default(),
        taken_says: says.is_some(),
        taken_title,
        dimensions: facts.map(dimensions_text).unwrap_or_default(),
        duration: facts.map(duration_text).unwrap_or_default(),
        artist: facts.and_then(|facts| facts.artist.clone()).unwrap_or_default(),
        album: facts.and_then(|facts| facts.album.clone()).unwrap_or_default(),
    }
}

/// Fakes for the tests of the rows, the file list and the menus: a tags backend and a media source that keep what
/// they were asked.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A tags backend over a map (by path in lower case), logging each `read`'s paths.
    #[derive(Default)]
    pub(crate) struct Tags {
        pub tags: Mutex<HashMap<String, Vec<String>>>,
        pub reads: Mutex<Vec<Vec<String>>>,
        /// Every read fails.
        pub fails: Mutex<bool>,
    }

    impl Tags {
        pub(crate) fn with(items: &[(&str, &[&str])]) -> Arc<Self> {
            let fake = Tags::default();
            for (path, tags) in items {
                fake.put(path, tags);
            }
            Arc::new(fake)
        }

        pub(crate) fn put(&self, path: &str, tags: &[&str]) {
            if let Ok(mut map) = self.tags.lock() {
                map.insert(path.to_lowercase(), tags.iter().map(|t| t.to_string()).collect());
            }
        }

        /// The paths of each `read` so far.
        pub(crate) fn calls(&self) -> Vec<Vec<String>> {
            self.reads.lock().map(|r| r.clone()).unwrap_or_default()
        }

        /// How many paths were read in all.
        pub(crate) fn paths_read(&self) -> usize {
            self.calls().iter().map(Vec::len).sum()
        }
    }

    impl Backend for Tags {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            let fails = self.fails.lock().map(|f| *f).unwrap_or(false);
            let known = self.tags.lock().map(|m| m.clone()).unwrap_or_default();
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(paths.clone());
            }
            Box::pin(async move {
                if fails {
                    return Err("access denied".to_string());
                }
                Ok(paths
                    .iter()
                    .map(|p| known.get(&p.to_lowercase()).cloned().unwrap_or_default())
                    .collect())
            })
        }

        fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Fut<Vec<(String, String)>> {
            if let Ok(mut map) = self.tags.lock() {
                for path in &paths {
                    let tags = map.entry(path.to_lowercase()).or_default();
                    tags.retain(|t| !remove.iter().any(|r| r.eq_ignore_ascii_case(t)));
                    for tag in &add {
                        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                            tags.push(tag.clone());
                        }
                    }
                }
            }
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// A backend that knows marks: it answers `read_marks` from a map (by path in lower case), logging each call's
    /// paths, and counts the tags-only `read`s it should never be asked.
    #[derive(Default)]
    pub(crate) struct MarksBackend {
        pub marks: Mutex<HashMap<String, Marks>>,
        pub reads: Mutex<Vec<Vec<String>>>,
        pub tags_reads: Mutex<usize>,
        /// What the index says of the folder (`marks_in`); None: it can't say.
        pub index: Mutex<Option<Vec<glint_core::marks::MarkedIn>>>,
        /// The folders `marks_in` was asked about.
        pub index_calls: Mutex<Vec<String>>,
    }

    impl MarksBackend {
        /// Makes the index answer for the folder with these items.
        pub(crate) fn indexed(&self, items: Vec<glint_core::marks::MarkedIn>) {
            if let Ok(mut index) = self.index.lock() {
                *index = Some(items);
            }
        }

        pub(crate) fn index_asked(&self) -> Vec<String> {
            self.index_calls.lock().map(|c| c.clone()).unwrap_or_default()
        }

        pub(crate) fn with(items: &[(&str, Marks)]) -> Arc<Self> {
            let fake = MarksBackend::default();
            for (path, marks) in items {
                fake.put(path, marks.clone());
            }
            Arc::new(fake)
        }

        pub(crate) fn put(&self, path: &str, marks: Marks) {
            if let Ok(mut map) = self.marks.lock() {
                map.insert(path.to_lowercase(), marks);
            }
        }

        /// The paths of each `read_marks` so far.
        pub(crate) fn calls(&self) -> Vec<Vec<String>> {
            self.reads.lock().map(|r| r.clone()).unwrap_or_default()
        }

        pub(crate) fn paths_read(&self) -> usize {
            self.calls().iter().map(Vec::len).sum()
        }
    }

    impl Backend for MarksBackend {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            if let Ok(mut n) = self.tags_reads.lock() {
                *n += 1;
            }
            let _ = paths;
            Box::pin(async { Ok(Vec::new()) })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            let known = self.marks.lock().map(|m| m.clone()).unwrap_or_default();
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(paths.clone());
            }
            Box::pin(async move {
                Ok(paths
                    .iter()
                    .map(|p| known.get(&p.to_lowercase()).cloned().unwrap_or_default())
                    .collect())
            })
        }

        fn marks_in(&self, folder: String) -> Fut<Option<Vec<glint_core::marks::MarkedIn>>> {
            if let Ok(mut calls) = self.index_calls.lock() {
                calls.push(folder);
            }
            let answer = self.index.lock().map(|i| i.clone()).unwrap_or_default();
            Box::pin(async move { Ok(answer) })
        }
    }

    /// A media source over a map (by path), logging each `read`'s paths.
    #[derive(Default)]
    pub(crate) struct Media {
        pub facts: Mutex<HashMap<String, MediaFacts>>,
        pub reads: Mutex<Vec<Vec<String>>>,
    }

    impl Media {
        pub(crate) fn with(items: Vec<(&str, MediaFacts)>) -> Arc<Self> {
            let fake = Media::default();
            if let Ok(mut map) = fake.facts.lock() {
                for (path, facts) in items {
                    map.insert(path.to_string(), facts);
                }
            }
            Arc::new(fake)
        }

        /// The paths of each `read` so far.
        pub(crate) fn calls(&self) -> Vec<Vec<String>> {
            self.reads.lock().map(|r| r.clone()).unwrap_or_default()
        }

        /// How many paths were read in all.
        pub(crate) fn paths_read(&self) -> usize {
            self.calls().iter().map(Vec::len).sum()
        }
    }

    impl MediaSource for Media {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Option<MediaFacts>>> {
            let known = self.facts.lock().map(|m| m.clone()).unwrap_or_default();
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(paths.clone());
            }
            Box::pin(async move { Ok(paths.iter().map(|p| known.get(p).cloned()).collect()) })
        }
    }

    /// Runs the app until `done` is true, giving core's threads time (they wake the tasks that wait on them).
    pub(crate) fn settle(
        cx: &mut gpui_kit::TestAppContext,
        mut done: impl FnMut(&mut gpui_kit::TestAppContext) -> bool,
    ) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the rows' facts never reached the state the test waits for");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};
    use glint_model::entries::attr;

    fn facts(width: u32, height: u32, duration: f64) -> MediaFacts {
        MediaFacts {
            width: Some(width),
            height: Some(height),
            duration: Some(duration),
            ..MediaFacts::default()
        }
    }

    #[test]
    fn photos_and_videos_can_be_taken_and_songs_only_have_details() {
        let picture = file("C:\\p", "a.jpg", 1, 0);
        let video = file("C:\\p", "a.mp4", 1, 0);
        let song = file("C:\\p", "a.mp3", 1, 0);
        let text = file("C:\\p", "a.txt", 1, 0);
        assert!(can_be_taken(&picture) && can_be_taken(&video) && !can_be_taken(&song) && !can_be_taken(&text));
        assert!(has_details(&picture) && has_details(&video) && has_details(&song) && !has_details(&text));
        assert!(
            !has_details(&folder("C:\\p", "a.jpg")),
            "a folder named like a picture is not one"
        );
        assert!(Media::Taken.reads(&video) && !Media::Taken.reads(&song) && Media::All.reads(&song));
        assert!(!Media::None.reads(&picture));
    }

    #[test]
    fn a_column_of_facts_or_a_sort_by_them_wants_every_media_file_read() {
        let plain = [ColumnId::Modified, ColumnId::Type, ColumnId::Size];
        let name = SortKey::Name;
        assert_eq!(media_wanted(&plain, name), Media::None);
        assert_eq!(
            media_wanted(&[ColumnId::Modified, ColumnId::Duration], name),
            Media::All
        );
        assert_eq!(media_wanted(&plain, SortKey::Taken), Media::All);
        assert_eq!(media_wanted(&plain, SortKey::Size), Media::None);
    }

    #[test]
    fn dimensions_and_durations_read_as_the_web_wrote_them() {
        assert_eq!(dimensions_text(&facts(1920, 1080, 0.)), "1920 × 1080");
        assert_eq!(
            dimensions_text(&facts(0, 1080, 0.)),
            "",
            "a zero is falsy in the web's test"
        );
        assert_eq!(dimensions_text(&MediaFacts::default()), "");
        assert_eq!(duration_text(&facts(1, 1, 185.)), "3:05");
        assert_eq!(duration_text(&facts(1, 1, 3723.)), "1:02:03");
        assert_eq!(duration_text(&facts(1, 1, 0.)), "");
        assert_eq!(duration_text(&MediaFacts::default()), "");
        assert_eq!(duration_text(&facts(1, 1, f64::NAN)), "");
    }

    #[test]
    fn details_become_facts_and_a_date_that_is_not_a_number_is_dropped() {
        let mut details = MediaDetails::default();
        details.taken = Some(1_700_000_000_000.);
        details.width = Some(4000);
        details.height = Some(3000);
        details.artist = Some("Nour".into());
        let got = facts_of(&details);
        assert_eq!(got.taken, Some(1_700_000_000_000));
        assert_eq!((got.width, got.height), (Some(4000), Some(3000)));
        assert_eq!(got.artist.as_deref(), Some("Nour"));
        details.taken = Some(f64::NAN);
        assert_eq!(facts_of(&details).taken, None);
    }

    #[test]
    fn cells_are_empty_until_read_and_a_picture_falls_back_to_its_modified_date() {
        let picture = file("C:\\p", "a.jpg", 1, 1_700_000_000_000);
        assert_eq!(media_cells(&picture, None), MediaCells::default(), "unread");
        // Read, and the file says nothing: the modified date, muted, with the reason as its title.
        let silent = media_cells(&picture, Some(None));
        assert_eq!(silent.taken, format_date(1_700_000_000_000, None, i18n::language()));
        assert!(!silent.taken_says);
        assert!(silent
            .taken_title
            .as_deref()
            .is_some_and(|t| t.starts_with("It doesn't say when")));
        assert_eq!(silent.dimensions, "");
        // Read, and the file says when: that date, with "Taken …" as its title.
        let said = MediaFacts {
            taken: Some(1_600_000_000_000),
            width: Some(10),
            height: Some(20),
            artist: Some("Nour".into()),
            album: Some("Dawn".into()),
            ..MediaFacts::default()
        };
        let cells = media_cells(&picture, Some(Some(&said)));
        assert_eq!(cells.taken, format_date(1_600_000_000_000, None, i18n::language()));
        assert!(cells.taken_says);
        assert!(cells.taken_title.as_deref().is_some_and(|t| t.starts_with("Taken ")));
        assert_eq!(
            (cells.dimensions.as_str(), cells.artist.as_str(), cells.album.as_str()),
            ("10 × 20", "Nour", "Dawn")
        );
        // A song has no date to fall back to; a folder says nothing at all.
        let song = file("C:\\p", "a.mp3", 1, 1_700_000_000_000);
        assert_eq!(media_cells(&song, Some(None)).taken, "");
        assert_eq!(
            media_cells(&folder("C:\\p", "d"), Some(Some(&said))),
            MediaCells::default()
        );
    }

    #[test]
    fn only_items_on_this_disk_are_asked_about() {
        let mut inside = file("C:\\z.zip", "a.txt", 1, 0);
        inside.attrs |= attr::IN_ARCHIVE;
        let mut gone = file("C:\\x", "a.txt", 1, 0);
        gone.attrs |= attr::RECYCLED;
        assert!(on_this_disk(&file("C:\\x", "a.txt", 1, 0)));
        assert!(on_this_disk(&folder("C:\\x", "docs")));
        assert!(!on_this_disk(&inside));
        assert!(!on_this_disk(&gone));
    }

    #[test]
    fn rows_on_other_computers_are_not_asked_for_tags() {
        assert!(on_network(r"\\server\share\a.txt"));
        assert!(on_network(r"\\?\UNC\server\share\a.txt"));
        assert!(!on_network(r"\\?\C:\x\a.txt"), "a long local path");
        assert!(!on_network(r"C:\x\a.txt"), "this PC's system drive");
        assert!(!on_network("remote:abc/a.txt"));
    }

    // ---- The model, on fakes ----------------------------------------------------------------------------

    use super::fake;
    use crate::testing;
    use gpui_kit::TestAppContext;

    const DIR: &str = "dir:C:\\x";

    /// The shared model on fakes, in an app with no window.
    fn app(cx: &mut TestAppContext, tags: &Arc<fake::Tags>, media: &Arc<fake::Media>) -> Entity<RowFacts> {
        cx.executor().allow_parking();
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = tags.clone();
        let source: Arc<dyn MediaSource> = media.clone();
        cx.update(|cx| {
            tags::init_with(cx, backend);
            init_with(cx, source);
            shared(cx)
        })
    }

    fn entries(count: usize) -> Vec<FileEntry> {
        (0..count)
            .map(|i| file("C:\\x", &format!("f{i:04}.jpg"), 1, 1000 + i as i64))
            .collect()
    }

    fn want_all() -> Want {
        Want {
            tags: true,
            media: Media::All,
        }
    }

    fn tags_only() -> Want {
        Want {
            tags: true,
            media: Media::None,
        }
    }

    fn tags_are(facts: &Entity<RowFacts>, cx: &mut TestAppContext, path: &str, wanted: &[&str]) -> bool {
        facts.read_with(cx, |f, _| {
            f.tags_of(DIR, path)
                .is_some_and(|tags| tags.iter().map(String::as_str).eq(wanted.iter().copied()))
        })
    }

    #[gpui_kit::test]
    fn what_a_frame_asks_for_goes_out_together_and_is_asked_only_once(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[("C:\\x\\f0001.jpg", &["Red", "Work"])]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(5);
        facts.update(cx, |f, cx| {
            // Two asks in one moment (two views, or one view's two ranges) make one read.
            f.ask(DIR, list.iter().take(3), want_all(), cx);
            f.ask(DIR, list.iter(), want_all(), cx);
        });
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0004.jpg", &[]));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.media_of(DIR, &list[4]).is_some()));
        assert_eq!(tags.calls().len(), 1, "one call for the five");
        assert_eq!(tags.calls()[0].len(), 5);
        assert_eq!(media.calls().len(), 1);
        assert!(tags_are(&facts, cx, "C:\\x\\f0001.jpg", &["Red", "Work"]));
        facts.read_with(cx, |f, _| {
            assert_eq!(f.tags_of(DIR, "C:\\x\\nope.jpg"), None, "never asked for");
            assert!(
                f.media_of(DIR, &list[0]).is_some_and(|facts| facts.is_none()),
                "read: says nothing"
            );
        });
        // Asking again for what is read reads nothing.
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), want_all(), cx));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(30));
        cx.run_until_parked();
        assert_eq!((tags.calls().len(), media.calls().len()), (1, 1));
    }

    #[gpui_kit::test]
    fn a_long_ask_is_read_in_batches(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(450);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 450);
        let sizes: Vec<usize> = tags.calls().iter().map(Vec::len).collect();
        assert_eq!(sizes, [200, 200, 50]);
        assert_eq!(media.calls().len(), 0, "no media column: no media read");
    }

    #[gpui_kit::test]
    fn tags_changed_are_read_again_and_the_old_dots_stay_until_then(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[("C:\\x\\f0000.jpg", &["Red"])]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(2);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 2);
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0000.jpg", &["Red"]));
        // The tags change; `set_tags` says so, naming the path in another case.
        tags.put("C:\\x\\f0000.jpg", &["Blue"]);
        let hub = cx.update(tags::hub);
        hub.update(cx, |_, cx| {
            cx.emit(TagsChanged {
                paths: vec!["c:\\X\\F0000.JPG".to_string()],
            })
        });
        assert!(tags_are(&facts, cx, "C:\\x\\f0000.jpg", &["Red"]), "no blink");
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 3);
        assert_eq!(tags.calls()[1], ["C:\\x\\f0000.jpg"], "only the path that changed");
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0000.jpg", &["Blue"]));
    }

    #[gpui_kit::test]
    fn a_folder_listed_again_reads_its_tags_again(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(3);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 3);
        tags.put("C:\\x\\f0002.jpg", &["Green"]);
        facts.update(cx, |f, cx| {
            f.listed_again(DIR, cx);
            f.listed_again("dir:C:\\elsewhere", cx);
            f.ask(DIR, list.iter(), tags_only(), cx);
        });
        fake::settle(cx, |_| tags.paths_read() == 6);
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0002.jpg", &["Green"]));
    }

    #[gpui_kit::test]
    fn a_failed_read_says_nothing_and_is_not_asked_for_again(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[]);
        if let Ok(mut fails) = tags.fails.lock() {
            *fails = true;
        }
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(2);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0001.jpg", &[]));
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        cx.run_until_parked();
        assert_eq!(tags.calls().len(), 1);
    }

    #[gpui_kit::test]
    fn a_file_that_changed_has_its_details_read_again(cx: &mut TestAppContext) {
        let said = MediaFacts {
            width: Some(10),
            height: Some(20),
            ..MediaFacts::default()
        };
        let media = fake::Media::with(vec![("C:\\x\\f0000.jpg", said)]);
        let tags = fake::Tags::with(&[]);
        let facts = app(cx, &tags, &media);
        let mut list = entries(1);
        let want = Want {
            tags: false,
            media: Media::All,
        };
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), want, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.media_of(DIR, &list[0]).is_some()));
        // Saved again: the read is for the old version, so it is unread until the new one is.
        list[0].modified += 5;
        facts.read_with(cx, |f, _| assert!(f.media_of(DIR, &list[0]).is_none()));
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), want, cx));
        fake::settle(cx, |_| media.paths_read() == 2);
        assert_eq!(tags.calls().len(), 0, "tags were not wanted");
    }

    #[gpui_kit::test]
    fn only_the_latest_folders_are_kept(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(1);
        for i in 0..12 {
            facts.update(cx, |f, cx| f.ask(&format!("dir:C:\\{i}"), list.iter(), tags_only(), cx));
        }
        fake::settle(cx, |cx| {
            facts.read_with(cx, |f, _| f.tags_of("dir:C:\\11", "C:\\x\\f0000.jpg").is_some())
        });
        facts.read_with(cx, |f, _| {
            assert_eq!(f.folders_kept(), KEEP_FOLDERS);
            assert!(
                f.tags_of("dir:C:\\0", "C:\\x\\f0000.jpg").is_none(),
                "the oldest is let go"
            );
            assert!(f.tags_of("dir:C:\\11", "C:\\x\\f0000.jpg").is_some());
        });
    }

    #[gpui_kit::test]
    fn items_in_archives_and_the_recycle_bin_are_never_asked_about(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let mut inside = file("C:\\z.zip", "a.jpg", 1, 0);
        inside.attrs |= attr::IN_ARCHIVE;
        let list = [inside];
        facts.update(cx, |f, cx| f.ask("dir:C:\\z.zip", list.iter(), want_all(), cx));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(30));
        cx.run_until_parked();
        assert_eq!((tags.calls().len(), media.calls().len()), (0, 0));
    }

    #[gpui_kit::test]
    fn the_menus_read_the_tags_the_rows_read(cx: &mut TestAppContext) {
        let tags = fake::Tags::with(&[("C:\\x\\f0000.jpg", &["Red"]), ("C:\\x\\f0001.jpg", &["Red", "Blue"])]);
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(3);
        facts.update(cx, |f, cx| {
            f.ask_tags_of(DIR, list.iter().take(2).map(|e| e.path.as_str()), cx)
        });
        fake::settle(cx, |_| tags.paths_read() == 2);
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0001.jpg", &["Red", "Blue"]));
        let mut data = crate::components::menus::MenuData::default();
        cx.update(|cx| data.read_tags(cx, list.iter().map(|e| e.path.as_str())));
        assert_eq!(
            data.tags.get("c:\\x\\f0001.jpg"),
            Some(&vec!["Red".to_string(), "Blue".to_string()])
        );
        assert_eq!(data.tags.get("c:\\x\\f0000.jpg"), Some(&vec!["Red".to_string()]));
        assert!(!data.tags.contains_key("c:\\x\\f0002.jpg"), "not read: left out");
    }

    // ---- Marks ------------------------------------------------------------------------------------------

    fn marked(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: label.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: comment.map(String::from),
        }
    }

    /// The shared model on a backend that knows marks.
    fn marks_app(cx: &mut TestAppContext, backend: &Arc<fake::MarksBackend>) -> Entity<RowFacts> {
        cx.executor().allow_parking();
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = backend.clone();
        let media: Arc<dyn MediaSource> = fake::Media::with(vec![]);
        cx.update(|cx| {
            tags::init_with(cx, backend);
            init_with(cx, media);
            shared(cx)
        })
    }

    fn marks_are(facts: &Entity<RowFacts>, cx: &mut TestAppContext, path: &str, wanted: &Marks) -> bool {
        facts.read_with(cx, |f, _| f.marks_of(DIR, path) == Some(wanted))
    }

    fn changed(cx: &mut TestAppContext, paths: &[&str]) {
        let hub = cx.update(tags::hub);
        let paths = paths.iter().map(|p| p.to_string()).collect();
        hub.update(cx, |_, cx| cx.emit(TagsChanged { paths }));
    }

    #[gpui_kit::test]
    fn marks_are_none_until_read_and_then_a_label_tags_and_a_comment_and_an_item_with_none_reads_empty(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let full = marked(Some("Red"), &["Work", "Blue"], Some("see the totals"));
        let backend = fake::MarksBackend::with(&[("C:\\x\\f0001.jpg", full.clone())]);
        let facts = marks_app(cx, &backend);
        let list = entries(3);
        facts.read_with(cx, |f, _| {
            assert_eq!(f.marks_of(DIR, "C:\\x\\f0001.jpg"), None, "unread: draw nothing");
            assert_eq!(f.tags_of(DIR, "C:\\x\\f0001.jpg"), None);
        });
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| backend.paths_read() == 3);
        fake::settle(cx, |cx| marks_are(&facts, cx, "C:\\x\\f0001.jpg", &full));
        facts.read_with(cx, |f, _| {
            assert_eq!(f.tags_of(DIR, "C:\\x\\f0001.jpg"), Some(&["Work".to_string(), "Blue".to_string()][..]));
            assert_eq!(f.marks_of(DIR, "C:\\x\\f0002.jpg"), Some(&Marks::default()), "none: empty, not unread");
            assert_eq!(f.tags_of(DIR, "C:\\x\\f0002.jpg"), Some(&[][..]));
        });
    }

    #[gpui_kit::test]
    fn a_frames_rows_are_read_with_one_read_marks_call_and_read_is_never_asked(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let backend = fake::MarksBackend::with(&[]);
        let facts = marks_app(cx, &backend);
        let list = entries(5);
        facts.update(cx, |f, cx| {
            f.ask(DIR, list.iter().take(2), tags_only(), cx);
            f.ask(DIR, list.iter(), tags_only(), cx);
        });
        fake::settle(cx, |_| backend.paths_read() == 5);
        assert_eq!(backend.calls().len(), 1, "one call for the frame");
        assert_eq!(*backend.tags_reads.lock().unwrap(), 0, "read is never asked");
    }

    #[gpui_kit::test]
    fn a_tags_only_backend_reads_the_same_rows_with_one_read_call_as_before(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let list = entries(5);
        let tags = fake::Tags::with(&[("C:\\x\\f0001.jpg", &["Red"])]);
        let media = fake::Media::with(vec![]);
        let plain = app(cx, &tags, &media);
        plain.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 5);
        assert_eq!(tags.calls().len(), 1);
        fake::settle(cx, |cx| tags_are(&plain, cx, "C:\\x\\f0001.jpg", &["Red"]));
    }

    #[gpui_kit::test]
    fn a_mark_changed_is_read_again_and_the_old_marks_show_until_the_new_ones_arrive(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (red, blue) = (marked(Some("Red"), &[], None), marked(Some("Blue"), &["x"], None));
        let backend = fake::MarksBackend::with(&[("C:\\x\\f0000.jpg", red.clone())]);
        let facts = marks_app(cx, &backend);
        let list = entries(2);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| backend.paths_read() == 2);
        fake::settle(cx, |cx| marks_are(&facts, cx, "C:\\x\\f0000.jpg", &red));
        backend.put("C:\\x\\f0000.jpg", blue.clone());
        changed(cx, &["c:\\X\\F0000.JPG"]);
        assert!(marks_are(&facts, cx, "C:\\x\\f0000.jpg", &red), "no blink");
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| backend.paths_read() == 3);
        assert_eq!(backend.calls()[1], ["C:\\x\\f0000.jpg"], "only the path that changed");
        fake::settle(cx, |cx| marks_are(&facts, cx, "C:\\x\\f0000.jpg", &blue));
    }

    #[gpui_kit::test]
    fn a_failed_or_short_answer_says_nothing_for_the_paths_it_lacks(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let tags = fake::Tags::with(&[]);
        if let Ok(mut fails) = tags.fails.lock() {
            *fails = true;
        }
        let media = fake::Media::with(vec![]);
        let facts = app(cx, &tags, &media);
        let list = entries(2);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| tags.paths_read() == 2);
        fake::settle(cx, |cx| tags_are(&facts, cx, "C:\\x\\f0000.jpg", &[]));
        facts.read_with(cx, |f, _| assert_eq!(f.marks_of(DIR, "C:\\x\\f0001.jpg"), Some(&Marks::default())));
    }

    /// Entries of a folder that are asked for (a long list), and four that never are.
    fn mixed(count: usize) -> (Vec<FileEntry>, Vec<String>) {
        let mut list = entries(count);
        let asked: Vec<String> = list.iter().map(|e| e.path.clone()).collect();
        let mut inside = file("C:\\z.zip", "a.txt", 1, 0);
        inside.attrs |= attr::IN_ARCHIVE;
        let mut gone = file("C:\\x", "gone.txt", 1, 0);
        gone.attrs |= attr::RECYCLED;
        list.extend([inside, gone, file(r"\\server\share", "net.txt", 1, 0)]);
        (list, asked)
    }

    #[gpui_kit::test]
    fn ask_all_reads_every_entry_in_one_call_says_when_it_landed_and_asks_nothing_twice(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let red = marked(Some("Red"), &[], None);
        let backend = fake::MarksBackend::with(&[("C:\\x\\f0999.jpg", red.clone())]);
        let facts = marks_app(cx, &backend);
        let (list, asked) = mixed(1000);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        assert!(!facts.read_with(cx, |f, _| f.has_all(DIR)), "not before it lands");
        // Asking again while it is on its way calls nothing.
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(backend.calls(), [asked], "one call, with all of them and only them");
        assert!(marks_are(&facts, cx, "C:\\x\\f0999.jpg", &red));
        // Everything is fresh: nothing is called.
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(30));
        cx.run_until_parked();
        assert_eq!(backend.calls().len(), 1);
        // The folder listed again: not whole any more, and asked for again.
        facts.update(cx, |f, cx| f.listed_again(DIR, cx));
        assert!(!facts.read_with(cx, |f, _| f.has_all(DIR)));
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(backend.calls().len(), 2);
        assert_eq!(backend.calls()[1].len(), 1000);
        // A mark in it changes: not whole until the next read lands, which reads only what changed.
        changed(cx, &["C:\\x\\f0003.jpg"]);
        assert!(!facts.read_with(cx, |f, _| f.has_all(DIR)));
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(backend.calls()[2], ["C:\\x\\f0003.jpg"]);
    }

    /// A search's results come in batches under one view: asked first with none, the view was read whole at once,
    /// and the file that came next was never read, so `label:red` in the box hid what the search had found.
    #[gpui_kit::test]
    fn entries_that_come_after_a_whole_read_are_read_before_it_is_whole_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let red = marked(Some("Red"), &[], None);
        let backend = fake::MarksBackend::with(&[("C:\\x\\found.txt", red.clone())]);
        let facts = marks_app(cx, &backend);
        let view = "find:7";
        facts.update(cx, |f, cx| f.ask_all(view, &[], cx));
        assert!(facts.read_with(cx, |f, _| f.has_all(view)), "nothing to read yet");
        let found = vec![file("C:\\x", "found.txt", 1, 0)];
        facts.update(cx, |f, cx| f.ask_all(view, &found, cx));
        assert!(!facts.read_with(cx, |f, _| f.has_all(view)), "not whole while the new entry is read");
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(view)));
        assert_eq!(backend.calls(), [["C:\\x\\found.txt"]]);
        assert_eq!(facts.read_with(cx, |f, _| f.marks_of(view, "C:\\x\\found.txt").cloned()), Some(red));
        // The same entries again: nothing more is called.
        facts.update(cx, |f, cx| f.ask_all(view, &found, cx));
        cx.run_until_parked();
        assert_eq!(backend.calls().len(), 1);
    }

    #[gpui_kit::test]
    fn a_folder_read_whole_keeps_more_than_most_per_folder_reads(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (first, last) = ("C:\\x\\f0000.jpg", format!("C:\\x\\f{:04}.jpg", MOST_PER_FOLDER + 99));
        let backend = fake::MarksBackend::with(&[
            (first, marked(Some("First"), &[], None)),
            (&last, marked(Some("Last"), &[], None)),
        ]);
        let facts = marks_app(cx, &backend);
        let list = entries(MOST_PER_FOLDER + 100);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        facts.read_with(cx, |f, _| {
            assert_eq!(f.marks_of(DIR, first).and_then(|m| m.label.as_deref()), Some("First"));
            assert_eq!(f.marks_of(DIR, &last).and_then(|m| m.label.as_deref()), Some("Last"));
        });
    }

    #[gpui_kit::test]
    fn marks_among_gives_the_marks_read_by_path_in_lower_case_and_leaves_out_the_unread(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let red = marked(Some("Red"), &["Work"], None);
        let backend = fake::MarksBackend::with(&[("C:\\x\\f0001.jpg", red.clone())]);
        let facts = marks_app(cx, &backend);
        let list = entries(2);
        facts.update(cx, |f, cx| f.ask(DIR, list.iter(), tags_only(), cx));
        fake::settle(cx, |_| backend.paths_read() == 2);
        fake::settle(cx, |cx| marks_are(&facts, cx, "C:\\x\\f0001.jpg", &red));
        let asked = ["C:\\x\\f0001.jpg", "C:\\x\\f0000.jpg", "C:\\x\\nope.jpg"];
        let got = facts.read_with(cx, |f, _| f.marks_among(asked));
        assert_eq!(got.len(), 2);
        assert_eq!(got.get("c:\\x\\f0001.jpg"), Some(&red));
        assert_eq!(got.get("c:\\x\\f0000.jpg"), Some(&Marks::default()));
        assert_eq!(got.get("c:\\x\\nope.jpg"), None);
    }

    // ---- A sort or a group by a mark, from the index ----------------------------------------------------------

    fn in_index(name: &str, label: Option<&str>, tags: &[&str], comment: bool) -> glint_core::marks::MarkedIn {
        glint_core::marks::MarkedIn {
            name: name.to_string(),
            label: label.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment,
        }
    }

    /// A backend whose index covers C:\x and says every seventh of `count` entries is marked (the 4th has a comment).
    fn indexed_backend(count: usize) -> Arc<fake::MarksBackend> {
        let backend = fake::MarksBackend::with(&[]);
        let items = (0..count)
            .step_by(7)
            .map(|i| in_index(&format!("F{i:04}.JPG"), Some("Red"), &["Work"], i % 28 == 0))
            .collect();
        backend.indexed(items);
        backend
    }

    #[gpui_kit::test]
    fn the_index_gives_a_whole_folders_labels_and_tags_in_one_call_and_only_files_with_a_comment_are_read(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let backend = indexed_backend(10_000);
        let facts = marks_app(cx, &backend);
        let list = entries(10_000);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(backend.index_asked(), ["C:\\x"], "one question, about the folder");
        assert_eq!(backend.paths_read(), (0..10_000).step_by(28).count(), "only the items with a comment");
        facts.read_with(cx, |f, _| {
            let red = f.marks_of(DIR, "C:\\x\\f0007.jpg").expect("read");
            assert_eq!((red.label.as_deref(), red.tags.as_slice()), (Some("Red"), &["Work".to_string()][..]));
            assert_eq!(f.marks_of(DIR, "C:\\x\\f0008.jpg"), Some(&Marks::default()), "left out: no marks");
        });
    }

    #[gpui_kit::test]
    fn the_items_the_index_says_have_a_comment_are_read_from_their_files_and_no_others(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let backend = indexed_backend(100);
        backend.put("C:\\x\\f0000.jpg", marked(Some("Red"), &["Work"], Some("first\nmore")));
        backend.put("C:\\x\\f0028.jpg", marked(Some("Red"), &["Work"], Some("second")));
        let facts = marks_app(cx, &backend);
        let list = entries(100);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        let mut asked = backend.calls();
        assert_eq!(asked.len(), 1, "one read");
        asked[0].sort();
        assert_eq!(asked[0], ["C:\\x\\f0000.jpg", "C:\\x\\f0028.jpg", "C:\\x\\f0056.jpg", "C:\\x\\f0084.jpg"]);
        let first = facts.read_with(cx, |f, _| f.marks_of(DIR, "C:\\x\\f0000.jpg").and_then(|m| m.comment.clone()));
        assert_eq!(first.as_deref(), Some("first\nmore"), "the text is in");
    }

    #[gpui_kit::test]
    fn where_the_index_cannot_say_every_entry_is_read_from_its_file_as_before(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let red = marked(Some("Red"), &[], None);
        let backend = fake::MarksBackend::with(&[("C:\\x\\f0009.jpg", red.clone())]);
        let facts = marks_app(cx, &backend);
        let (list, asked) = mixed(50);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(backend.calls(), [asked]);
        assert!(marks_are(&facts, cx, "C:\\x\\f0009.jpg", &red));
    }

    #[gpui_kit::test]
    fn a_path_named_by_tags_changed_after_the_index_answered_is_read_from_its_file_and_wins(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let backend = indexed_backend(30);
        let blue = marked(Some("Blue"), &[], None);
        backend.put("C:\\x\\f0007.jpg", blue.clone());
        let facts = marks_app(cx, &backend);
        let list = entries(30);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        assert_eq!(facts.read_with(cx, |f, _| f.marks_of(DIR, "C:\\x\\f0007.jpg").and_then(|m| m.label.clone())), Some("Red".into()));
        changed(cx, &["C:\\x\\f0007.jpg"]);
        facts.update(cx, |f, cx| f.ask_all(DIR, &list, cx));
        fake::settle(cx, |cx| facts.read_with(cx, |f, _| f.has_all(DIR)));
        // The first read was the two with a comment.
        assert_eq!(backend.calls()[1..], [["C:\\x\\f0007.jpg"]], "that one, from its file");
        assert_eq!(backend.index_asked().len(), 1, "the index is not asked again for it");
        assert!(marks_are(&facts, cx, "C:\\x\\f0007.jpg", &blue));
    }

    /// Measures (`cargo test -p glint-ui row_facts -- --ignored --nocapture`): 10,000 marked entries sorted by label,
    /// from `ask_all` to `has_all`, with the index's answer in hand and with the files' read answering at once.
    /// It cannot say what the real index or 10,000 real files cost: only what Glint's own side adds (the batch,
    /// the map, the keeping of the marks).
    #[gpui_kit::test]
    #[ignore = "measures, prints"]
    fn measured_ten_thousand_marked_entries_from_the_index_and_from_the_files(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let n = 10_000;
        let list = entries(n);
        let marks = marked(Some("Red"), &["Work"], None);
        for (name, from_index) in [("files", false), ("index", true)] {
            let backend = fake::MarksBackend::with(&[]);
            let items = (0..n).map(|i| in_index(&format!("f{i:04}.jpg"), Some("Red"), &["Work"], false)).collect();
            if from_index {
                backend.indexed(items);
            }
            for e in &list {
                backend.put(&e.path, marks.clone());
            }
            let dir = format!("dir:{name}");
            let facts = marks_app(cx, &backend);
            let start = std::time::Instant::now();
            facts.update(cx, |f, cx| f.ask_all(&dir, &list, cx));
            for _ in 0..4000 {
                cx.run_until_parked();
                if facts.read_with(cx, |f, _| f.has_all(&dir)) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let held = facts.read_with(cx, |f, _| f.has_all(&dir));
            let all: std::collections::HashMap<_, _> = facts.read_with(cx, |f, _| {
                list.iter().filter_map(|e| Some((e.path.clone(), f.marks_of(&dir, &e.path)?.clone()))).collect()
            });
            println!("{name}: has_all={held}, {} rows' marks in {:?}, files read: {}", all.len(), start.elapsed(), backend.paths_read());
        }
    }
}
