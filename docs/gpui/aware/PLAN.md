# The Aware wave: a menu and a dock that know what they are looking at

*Plan for 0.13. Written by the coordinator for agents who implement one card each. Read [STANDARDS.md](../STANDARDS.md) first, then sections 1 to 6 of this file, then your card in [cards/](cards/). Open [mock.html](mock.html) in a browser to see what is being built.*

## 1. Why

The person should think "what a smart app". That thought comes from four things, and every card serves one of them:

1. **It knows.** Glint says true, specific things about what the pointer is on, unasked: "In use by Word", "From github.com", "Not signed", "2 identical copies", "3.2 GB · 1,204 files inside", "1,230 lines".
2. **It is ahead.** The step the person was about to take is offered first, in the menu and in the dock: Extract here for an archive, Compare for two files, "Move to Invoices" because that is where they put such files.
3. **It never waits.** What it knows was worked out before the click, on cores that were idle, so a menu opens complete in its first frame.
4. **It can be trusted.** The rows the hand knows stay where they are. Nothing is said that isn't known. What is learned stays on the PC and can be forgotten.

Today the menu is a long, correct list that treats a zip, a photo and a program the same way until read, and the dock counts items. Both already have the data model to do better (`menus/spec.rs`, `command_bar/facts.rs`, `status_bar/summary.rs`); the engine already knows most of the facts (locks, signatures, Mark of the Web, media details, the index). What is missing is one place that gathers them ahead of time, remembers what the person does, and ranks.

## 2. Rules of the wave (they outrank a card's wording)

- **R1. A fact is shown only when it is known.** Unknown is drawn as nothing: no spinner, no "…", no placeholder, no guess. `Option::None` means "not known yet".
- **R2. A menu's rows never move after it is on screen.** What is suggested is decided when the menu opens, from what is known then. Facts that arrive later go only into the head, whose height is fixed.
- **R3. Suggesting never moves or removes a row.** A suggested row is a copy, above the list. The list keeps the order it has today (its tests still pass unchanged, see section 8).
- **R4. Nothing in this wave runs on the UI thread except reading what is already in memory.** Probing is `glint_core::aware::probe` on its own threads. Answers reach the views at most once a frame.
- **R5. Looking ahead must be free.** It runs on the quiet lane (efficiency-mode threads), never reads a cloud file that is not on the disk, never touches a network or removable drive, and stops when what it was for is gone.
- **R6. No new crates.** Everything needed is in the build already (`rayon`, `blake3`, `imagesize`, `memchr`, `windows`). The coordinator adds `memchr` to glint-core's manifest (it is a dependency of shadui-index today).
- **R7. Wording is plain and exact.** The strings in the cards are the strings. English keys through `t!`.
- **R8. The switches are obeyed everywhere.** `settings.suggest == false`: no Suggested section, no slot in the island, no "usual place". `settings.learn == false`: nothing is counted and nothing counted is used. The head and the dock's facts are not suggestions and stay.
- **R9. Animations are finite.** Nothing in this wave repeats forever: a window at rest draws nothing (the vsync thread rests, VENDOR.md). Windows' "Animation effects" off means everything lands at once.

## 3. What the person sees

The numbers (F1…) are how cards and tests refer to features.

### The right-click menu

- **F1. A head.** Every item menu and folder menu starts with a card: the item's picture, its name, a line of facts, and a third line of badges.
  - One file: `{type label} · {size}`, then what fits its kind: `1920 × 1080`, `2 min 31 s`, `1,230 lines`, `48 pages`, `38 files inside` (an archive read before).
  - One folder: `Folder`, then `{n} files · {size}` once its inside is known.
  - Several: the name is `{n} items`; the line is `{f} files, {d} folders · {size}` (a part that is zero is left out).
  - The folder's own menu (empty space): the folder's name, `{n} items`, and its Git branch as a badge when it has one.
  - Third line: badges, then `Changed {when}` in the room left. Badges, in this order, at most three and then `+{n}`: `In use by {app}` (warning), `From {host}` or `From the internet` (warning), `Not signed` (warning) or `Signed by {name}` (good), `{n} identical copies`, `Online only`, `Read-only`, `Hidden`, `Points to {name}` (a shortcut), Git's `Changed` or `New`.
  - A badge with something behind it is pressed with the mouse: the locks dialog, File X-Ray, the first copy's folder, the shortcut's target.
- **F2. Suggested.** Under the strip of icon buttons, a section "Suggested" with at most three rows: copies of rows the menu has further down (in a submenu too: "Compress to › Zip file"), chosen by the rules in card A3 and by what the person has done before with this kind of thing. A row has a second, small line only when the reason is not obvious: "In use by Word", "It came from github.com", "It isn't signed", "1.8 GB of build output inside", "You often do this with pictures", "Where you usually put zip files".
- **F3. Type to find.** With a menu open, typing filters every row, submenus included. Enter runs the first; arrows move; Backspace edits; Escape takes the typing back, and closes the menu when there is none.
- **F4. Greyed rows say why.** A row that can't be used has a tooltip and a description for screen readers: "Nothing changes inside an archive", "Read-only: on a remote connection", "Windows keeps this folder closed".
- **F5. Move or copy to.** A submenu with the folders this kind of file usually goes to, the folders used lately, and "Choose a folder…". The usual one is also offered under Suggested.
- **F6. Two new rows.** "Unblock" (when the file came from the internet) and "Copy the text in it" (pictures: Windows' own text recognition, already in the engine).
- **F7. Submenus slide.** Going into a submenu the rows arrive from the end edge; Back, from the start edge.

### The dock

- **F8. The next step is in the island.** While something is selected, the first suggestion (the same ranking as F2) is a button in the island, after Delete, with its word. A ring of the glint's light turns once around it when it changes, then rests.
- **F9. Paste says what it would paste.** With files on the clipboard and a folder that can take them, the Paste button has words: "Paste 3 files", or "Move 3 files here" after a cut. Its tooltip names them and where they are from.
- **F10. The words know the selection.** "1 selected · 3.2 GB · 1,204 files inside" for a folder (from the index at once, else counted on every core, the numbers rolling as they grow); "2 selected · 48 KB · 1,230 lines"; "· 12 min 31 s"; "· 1920 × 1080"; "· 48 pages".
- **F11. The folder is seen changing.** While files are being written or removed in the folder shown, the dock's end shows a small live line and "Changing now"; pressing it opens Recent changes. A drive nearly full says so there too ("C: is nearly full", the words Settings' Overview uses), and pressing it opens Disk usage.

### Elsewhere

- **F12. The palette learns.** Ctrl+K lists the commands the person runs most first.
- **F13. Settings.** Under History: "Suggest the next step" and "Learn from what I do" (both on), and "Forget what Glint learned".

## 4. How it is built

```
glint-model/src/aware/      pure: what a selection is (Shape), what is known (Facts, Probed),
                            what was done before (Habits), what to offer (rank::suggest)
glint-core/src/aware/       I/O: probe() answers questions about paths on two lanes of threads
glint-ui/src/aware/         one entity (Aware): the cache of answers, looking ahead, the clipboard,
                            the folder's pulse; the only door to habits
glint-ui/src/components/
  viz.rs                    small things drawn on the GPU: a rolling number, a spark line, the glint ring
  menus/                    head.rs (F1), smart.rs (F2, F5), filter.rs (F3), view.rs draws them
  command_bar/smart.rs      the island's slot (F8, F9)
  status_bar/sentence.rs    the selection's words (F10);  status_bar/chips.rs (F11)
```

**The flow.** The pointer rests on a row for 120 ms, or the selection changes: `Aware::look_ahead(entries)` asks the quiet lane for what isn't cached. A right-click opens the menu from what is in memory (`Aware::probed`), and asks the now lane for the rest (`Aware::look_now`); answers arriving while the menu is open update its head only (R2). Choosing a row tells `Aware::note_action`, which counts it in `AppState.habits` (saved by `persistence`, as everything else).

**Two lanes** (`glint_core::aware::pool`). *Quiet*: half the cores, each thread in Windows' efficiency mode (`SetThreadInformation(ThreadPowerThrottling)`: EcoQoS, the efficiency cores on a hybrid CPU) and background I/O priority, so looking ahead costs no heat and never slows a listing. *Now*: all cores at normal priority, for what something on screen waits for.

**What a probe may read.** Mark of the Web (one small stream), Restart Manager (who has it open), WinVerifyTrust (the signature), headers through the I/O ring (`ioring::read_many`: dimensions, lengths), a text file's line ends counted with `memchr` (SIMD), a PDF's page count from Windows' reader, a folder's inside from the instant search index (one pipe call) or else a walk on every core, and identical copies found by size in the index and proved with BLAKE3.

## 5. Contracts

The coordinator lands these as code before the first card starts (step S0 in section 7): every type, every signature, every module declaration, with bodies that compile and do nothing (`Vec::new()`, `None`, `Default::default()`; never `todo!()`). **Your card fills in bodies and adds tests. If the code in the tree and this section disagree, the code wins; say so in your report.** Don't change a signature another card uses; ask in your report.

### 5.1 glint-model: `crates/glint-model/src/aware/`

```rust
// shape.rs — card A1
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    #[default]
    Nothing,                       // no targets: the folder's own menu
    Folder, Folders,
    Archive, Archives, DiskImage,
    Program, Installer, Script, Shortcut,
    Picture, Pictures, Video, Videos, Song, Songs, Pdf, Pdfs, Document, Documents,
    Data, Code, Font,
    File,                          // one file of no kind above
    Files,                         // several files of different kinds
    Mixed,                         // files and folders together
}
impl Shape {
    pub fn of(targets: &[FileEntry]) -> Shape;
    /// A stable word for it ("picture", "pictures"): what habits are kept under. Never changes once shipped.
    pub fn key(self) -> &'static str;
    /// The words for "this kind of thing", plural, for a reason line ("pictures", "zip files" is not here: see words).
    pub fn is_many(self) -> bool;
}
/// The one extension every target has, lower case; None when they differ or a folder is among them.
pub fn shared_ext(targets: &[FileEntry]) -> Option<String>;
/// What is worth asking about `entry` (nothing for what isn't a plain file or folder on this PC's disk).
pub fn want_for(entry: &FileEntry) -> Want;

// facts.rs — coordinator (data only)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub shape: Shape,
    pub count: usize, pub files: usize, pub folders: usize,
    pub bytes: u64,                // the files' bytes; folders weigh nothing here
    pub ext: Option<String>,
    pub first_name: String,        // the first target's name, lower case ("" for none)
    pub fixed: bool,               // in an archive, on a remote, closed by Windows, in the Recycle Bin
    pub clip: Clip,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Clip {
    #[default]
    Empty,
    Files { count: usize, cut: bool, from: Option<String>, names: Vec<String> },   // names: the first five
    Picture,
    Text,
}

// probed.rs — coordinator (data only, plus Probed::take and Want::minus)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probed {
    pub locked_by: Option<Vec<String>>,   // Some(empty): nobody has it open
    pub origin: Option<Origin>,
    pub signer: Option<Signer>,
    pub media: Option<Media>,
    pub text: Option<Text>,
    pub pages: Option<u32>,
    pub inside: Option<Inside>,
    pub twins: Option<Vec<String>>,       // Some(empty): no identical copy found
    pub target: Option<String>,
    pub opens_with: Option<String>,       // the default app's name ("Photos")
}
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    LockedBy(Vec<String>), Origin(Origin), Signer(Signer), Media(Media), Text(Text), Pages(u32),
    Inside(Inside), Twins(Vec<String>), Target(String), OpensWith(String),
}
#[derive(Clone, Debug, PartialEq)] pub struct Origin { pub host: String, pub blocked: bool }  // host "" if unknown
#[derive(Clone, Debug, PartialEq)] pub enum Signer { Signed(String), Unsigned }
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Media { pub width: u32, pub height: u32, pub seconds: f64 }
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Text { pub lines: u64, pub binary: bool }
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inside { pub files: u64, pub folders: u64, pub bytes: u64, pub junk: u64, pub done: bool, pub from_index: bool }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub locked_by: bool, pub origin: bool, pub signer: bool, pub media: bool, pub text: bool,
    pub pages: bool, pub inside: bool, pub twins: bool, pub target: bool, pub opens_with: bool,
}
impl Probed { pub fn take(&mut self, part: Part); }       // an Inside replaces the one before it
impl Want { pub fn minus(self, have: &Probed) -> Want; pub fn is_empty(self) -> bool; }

// habits.rs — card A2
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Use { pub n: u32, pub day: u32 }                // times, and the day last done (days since 1970-01-01)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Habits {
    pub actions: BTreeMap<String, BTreeMap<String, Use>>,   // Shape::key() → action key → use
    pub places: BTreeMap<String, BTreeMap<String, Use>>,    // extension ("" for folders) → folder → use
}
#[derive(Clone, Debug, PartialEq)]
pub struct Place { pub folder: String, pub uses: u32, pub weight: f64, pub share: f64 }
impl Habits {
    pub fn is_empty(&self) -> bool;
    pub fn note_action(&mut self, shape: Shape, action: &str, today: u32);
    pub fn note_place(&mut self, ext: &str, folder: &str, today: u32);
    pub fn action_weight(&self, shape: Shape, action: &str, today: u32) -> f64;
    pub fn action_uses(&self, shape: Shape, action: &str) -> u32;
    pub fn overall_weight(&self, action: &str, today: u32) -> f64;     // over every shape: the palette's
    pub fn places_for(&self, ext: &str, today: u32) -> Vec<Place>;      // heaviest first
    pub fn forget(&mut self);
}
pub fn weight(used: Use, today: u32) -> f64;               // n × 0.5^((today − day) / 30)
pub fn day_of(unix_ms: i64) -> u32;

// rank.rs — card A3
#[derive(Clone, Debug, PartialEq)]
pub enum Why { Plain, InUse(String), FromInternet(String), Unsigned, BuildOutput(u64), Usual, UsualPlace }
#[derive(Clone, Debug, PartialEq)]
pub struct Pick { pub key: String, pub why: Why, pub score: f64 }
/// The rows to put forward, best first, at most `most`. `available` are the keys of the menu's rows that can be
/// chosen, in the menu's order; `probed` is the one target's answers (None for several targets).
pub fn suggest(facts: &Facts, probed: Option<&Probed>, available: &[&str], habits: &Habits, today: u32, most: usize) -> Vec<Pick>;
pub fn is_basic(key: &str) -> bool;
pub fn usual_place(places: &[Place]) -> Option<&Place>;    // uses ≥ 3 and share ≥ 0.6
```

`AppState` (glint-model/src/store.rs) gains `pub habits: Habits` (not written while empty); `Settings` gains `pub suggest: bool` and `pub learn: bool`, both true by default. The coordinator adds them, with their `lenient_deserialize!` lines and the default-JSON test.

### 5.2 glint-core: `crates/glint-core/src/aware/`

```rust
// mod.rs — coordinator
pub struct Ask { pub path: String, pub dir: bool, pub size: u64, pub want: Want }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Lane { Quiet, Now }
#[derive(Clone, Debug, PartialEq)] pub struct Answer { pub path: String, pub part: Part }

// probe.rs — card B1
/// Asks about every path at once and returns at once: each answer is sent as it is known, in no order. Setting
/// `stop` ends what hasn't started and makes walks and hashes give up. A part that can't be known sends nothing.
pub fn probe(asks: Vec<Ask>, lane: Lane, stop: Arc<AtomicBool>, out: host::ipc::Channel<Answer>);

// pool.rs — card B1
pub fn spawn(lane: Lane, job: impl FnOnce() + Send + 'static);
pub fn install<R: Send>(lane: Lane, work: impl FnOnce() -> R + Send) -> R;   // rayon inside `work` uses the lane
pub fn threads(lane: Lane) -> usize;

// inside.rs — card B2
/// What is in a folder, all the way down. From the index when it covers the folder (one answer, `done`); else a
/// walk on the lane's threads that reports about every 150 ms (`done: false`) and once at the end.
pub fn inside(path: &str, lane: Lane, stop: &AtomicBool, on: &mut dyn FnMut(Inside));

// twins.rs — card B2
/// Other files on this PC with the same bytes: found by size in the index, proved by BLAKE3. None without an index.
pub fn twins(path: &str, size: u64, stop: &AtomicBool) -> Option<Vec<String>>;
```

### 5.3 glint-ui: `crates/glint-ui/src/aware/`

```rust
// mod.rs — card D1 (the coordinator lands the signatures)
pub struct Aware { /* private */ }
pub fn init(cx: &mut App);                       // startup.rs calls it after explorer::init and persistence
pub fn init_with(cx: &mut App, sources: Sources);
pub fn shared(cx: &App) -> Entity<Aware>;
pub fn try_shared(cx: &App) -> Option<Entity<Aware>>;
impl Aware {
    pub fn look_ahead(&mut self, entries: &[FileEntry], cx: &mut Context<Self>);   // quiet lane
    pub fn look_now(&mut self, entries: &[FileEntry], cx: &mut Context<Self>);     // now lane
    pub fn probed(&self, entry: &FileEntry) -> Option<&Probed>;
    pub fn forget(&mut self, path: &str);                                          // after Unblock, a rename…
    pub fn rest(&mut self);                                                        // a menu closed: stop asking
    pub fn clip(&self) -> &Clip;
    /// The folder's changes over the last minute: thirty counts, two seconds each, the newest last.
    pub fn pulse(&self, folder: &str) -> Option<[u8; 30]>;
    pub fn changing(&self, folder: &str) -> bool;                                  // three or more in ten seconds
}
/// What is known at once about `targets` (no I/O): what they are, and what is on the clipboard.
pub fn facts(cx: &App, targets: &[FileEntry]) -> Facts;
/// `rank::suggest` with the saved habits and today's date; empty when `settings.suggest` is off.
pub fn suggest(cx: &App, facts: &Facts, probed: Option<&Probed>, available: &[&str], most: usize) -> Vec<Pick>;
pub fn note_action(cx: &mut App, shape: Shape, key: &str);          // nothing when `settings.learn` is off
pub fn note_place(cx: &mut App, exts: &[String], folder: &str);
pub fn places_for(cx: &App, ext: &str) -> Vec<Place>;               // empty when `settings.learn` is off
pub fn forget_habits(cx: &mut App);
```

### 5.4 Menus: additions to `components/menus/spec.rs` (coordinator)

```rust
pub enum Node { /* … as today … */ Head(Head) }
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub title: SharedString,
    pub picture: HeadPicture,
    pub line: SharedString,
    pub badges: Vec<Badge>,
    pub changed: Option<SharedString>,           // "Changed yesterday"
}
#[derive(Clone, Debug, PartialEq)]
pub enum HeadPicture { Kind(&'static glint_model::file_types::Kind), Folder, Several }   // a Kind is compared by address
#[derive(Clone, Debug, PartialEq)]
pub struct Badge { pub label: SharedString, pub icon: IconName, pub tone: Tone, pub tip: Option<SharedString>, pub run: Run }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Tone { Plain, Warn, Good }
pub struct Entry {
    /* … as today … */
    pub reason: Option<SharedString>,            // the small second line of a suggested row
    pub why_not: Option<SharedString>,           // why it is greyed out
    pub smart: bool,                             // drawn with the accent: a suggested row
}
impl Entry { pub fn disabled_because(self, yes: bool, why: &Option<SharedString>) -> Entry; }
impl Run { pub fn key(&self) -> Option<String>; }          // card E2 (in smart.rs): the table below

// item.rs: ItemCtx gains two fields, empty by default, so `item_menu` stays a function of its arguments
pub struct ItemCtx { /* … as today … */ pub probed: Option<Probed>, pub places: Vec<Place> }

// event.rs: four new things a menu can ask the window for (the coordinator handles them in workspace.rs)
MenuEvent::Unblock { paths: Vec<String> }                       // takes the Mark of the Web off
MenuEvent::CopyTextIn { path: String }                          // a picture's text, to the clipboard
MenuEvent::SendTo { paths: Vec<String>, folder: String, moving: bool }
MenuEvent::ChooseSendTo { paths: Vec<String>, moving: bool }    // asks for the folder first

// mod.rs
pub fn host(cx: &App) -> Option<Entity<MenuHost>>;            // the window's menu host, for the island
impl MenuHost { pub fn suggestions(&self, pane: usize, most: usize, cx: &App) -> Vec<Suggested>; }   // card E4

// smart.rs and head.rs (card E2): `keys`, `row_for`, `reason`, `zone`, `dress`, `head_of`, `folder_head`
// send_to.rs (card G1): `menu`, `usual_row`;   filter.rs (card E3): `matches`
```

`visible_items` keeps a `Head` (it is never dropped) and treats it like the strip for separators (no line right after it). The engine functions the probes wrap are made reachable from `glint_core::aware` in S0: `locks::who_locks_now`, `xray::zone_info`, `media::details_now`, `recent::link_target`. The menus' test harness (`menus/tests.rs`: `open`, `disk`, `Ui`, `labels`, `find`, `at`, `ALPHA`, `DOCS`) is made visible to the new test files beside it. The dock's end tells the window what was pressed with `StatusBarEvent::Insight` (card F2).

### 5.5 The key of every row (`Run::key`)

Habits are kept under these strings, so they are part of what is saved to disk: once shipped a key is never renamed.

| Run | Key |
| :--- | :--- |
| `Run::Nothing`, `MenuEvent::Shown`, `MenuEvent::Closed` | none |
| `Run::Command(c)` | `cmd:` and the variant in kebab case: `cmd:open`, `cmd:open-in-new-tab`, `cmd:quick-look`, `cmd:rename`, `cmd:batch-rename`, `cmd:cut`, `cmd:copy`, `cmd:paste`, `cmd:copy-path`, `cmd:delete`, `cmd:delete-permanently`, `cmd:undo`, `cmd:select-all`, `cmd:refresh`, `cmd:new-folder`, `cmd:properties`, `cmd:compare-files` |
| `Run::Do(task)` | `task:` and the variant in kebab case (`task:open-terminal`, `task:open-with`, `task:run-as-admin`, `task:show-properties`, `task:open-in-other-pane`, `task:open-in-new-tabs`, `task:navigate`, `task:copy-text`, `task:toggle-pin`, `task:refresh-drives`, `task:get-access`, `task:windows-menu`, `task:open-in-file-explorer`); `Task::OpenIn` is `task:open-in:` and the opener's id |
| `Run::Ask(event)` | `do:` and the variant in kebab case (`do:extract-here`, `do:mount`, `do:run-in-sandbox`, `do:x-ray`, `do:locks`, `do:defender`, `do:cleanup`, `do:duplicates`, `do:make-smaller`, `do:convert-pictures`, `do:edit-songs`, `do:convert-document`, `do:query-together`, `do:convert-data`, `do:compare`, `do:set-wallpaper`, `do:search-text`, `do:timeline`, …), except the rows below |
| `Compress { format }` | `do:compress:zip`, `do:compress:7z`, `do:compress:tar-zst` |
| `Media { tool }` | `do:media:convert`, `:audio`, `:gif`, `:subtitles`, `:volume` |
| `PdfTools { tool }` | `do:pdf:merge`, `:split`, `:keep`, `:rotate`, `:optimize`, `:images`, `:encrypt`, `:decrypt`, `:stamp` |
| `Vault { decrypt }` | `do:vault:encrypt`, `do:vault:decrypt` |
| `Rotate { clockwise }` | `do:rotate:right`, `do:rotate:left` |
| `CloudPin { state }` | `do:cloud:keep`, `do:cloud:free`, `do:cloud:auto` |
| `CopyToOtherPane { moving }` | `do:copy-other`, `do:move-other` |
| `SetTags` | `do:tags` |
| `NewOfKind { ext }` | `do:new:` and the extension, lower case |
| `RunScript { script }` | `do:script:` and the script's file name, lower case |
| new: `SendTo { moving, .. }` | `do:copy-to`, `do:move-to` (the folder is not part of the key) |
| new: `ChooseSendTo { moving, .. }` | `do:copy-to-choose`, `do:move-to-choose` |
| new: `Unblock`, `CopyTextIn` | `do:unblock`, `do:copy-text-in` |

**Basic keys are never suggested** (`rank::is_basic`): every `cmd:` key, and `task:toggle-pin`, `task:windows-menu`, `task:copy-text`, `task:show-properties`, `task:navigate`, `task:refresh-drives`, `task:get-access`, `do:tags`, `do:edit-tags`, `do:paste-into`, `do:paste-queued`, `do:open-scripts-folder`, `do:reveal-in-file-explorer`, `do:reveal-archive`, `do:forget-recent`, `do:restore`, `do:empty-recycle-bin`, `do:install-tool`, `do:batch-rename-folder`, `do:copy-to-choose`, `do:move-to-choose`.

## 6. Rules for every agent

Everything in STANDARDS.md holds. On top of it:

1. **You own the files your card names and nothing else.** That includes `mod.rs`, `lib.rs` and `Cargo.toml`: the module lines you need are already there. Need something outside your files? Write it in your report; don't edit it.
2. **Other agents are working in the same tree at the same time, in other files.** If the build fails in a file that isn't yours, it is someone's half-written work: wait two minutes and build again, up to five times, then report it. Never "fix" a file that isn't yours.
3. **Never run `cargo fmt`.** Format by hand to the file's style (120 columns). Never run `git` commands that change anything (`commit`, `stash`, `checkout`, `restore`, `add`).
4. **Build and test only your crate, with a filter**, from the repository root:
   - model: `cargo test -p glint-model aware::`
   - core: `cargo test -p glint-core aware::`
   - ui: `cargo test -p glint-ui <your module's path, e.g. components::menus::tests_filter>`
   - then `cargo clippy -p <crate> --all-targets -- -D warnings` and fix what is in your files.
   The build folder is shared and Cargo locks it: a build may wait for another agent's. Don't set `CARGO_TARGET_DIR`.
5. **Tests.** Every behaviour your card lists is a test, named as a sentence (`a_zip_is_an_archive_and_two_are_archives`). Model and core tests are plain `#[test]`. UI tests follow STANDARDS §5.1 "Tests" (`testing::init`, `let _guard = testing::lock();`, `window.render_frame(cx)`, never a real disk, clipboard or index: use the fakes your card names). A test that needs the real file system uses `std::env::temp_dir()` and a folder of its own, removed at the end.
6. **Strings** are exactly as the card gives them, through `t!("…")` (`t!("{n} files", n = n)` for parts). Numbers go through `glint_model::format::{format_count, format_size, format_duration}`.
7. **Editing with scripts:** read and write as UTF-8. Don't use shell heredocs to patch Rust (backslashes get mangled); use the edit tool.
8. **No panics on input** (STANDARDS §3): paths, names, clipboard contents, index answers and settings come from outside.
9. **Your report**, as your last message, in this shape:
   ```
   CARD: <id>
   STATUS: done | partial
   FILES: <each file changed or created>
   TESTS: <the command you ran> → <n passed, n failed>
   CLIPPY: clean | <what is left, and in whose file>
   NOT DONE: <what the card asked for that isn't there, and why>
   NEEDS FROM COORDINATOR: <a signature, a module line, an icon in the kit's list, a wiring call>
   UNSURE: <anything you guessed>
   ```
   An honest "partial" is worth more than a wrong "done".

## 7. The waves

Cards in a wave run at the same time. A wave starts when the one before it is integrated (the coordinator builds everything, runs every test, and commits).

| Wave | Card | What | Crate | Owns |
| :--- | :--- | :--- | :--- | :--- |
| S0 | — | **Coordinator**: the contracts of section 5 as code (bodies that compile and do nothing), every module line, `memchr`, the two settings and the saved habits, the four new menu events and their handlers in `workspace.rs`, `menus::host`, the engine helpers, empty test files | all | — |
| 1 | [A1](cards/A1-shape.md) | What a selection is, and what to ask about it | model | `aware/shape.rs` |
| 1 | [A2](cards/A2-habits.md) | What the person did before | model | `aware/habits.rs` |
| 1 | [A3](cards/A3-rank.md) | What to put forward | model | `aware/rank.rs` |
| 1 | [B1](cards/B1-probe.md) | The two lanes and the probes of one file | core | `aware/pool.rs`, `aware/probe.rs`, `aware/file.rs` |
| 1 | [B2](cards/B2-inside-twins.md) | A folder's inside, and identical copies | core | `aware/inside.rs`, `aware/twins.rs` |
| 1 | [C1](cards/C1-viz.md) | The rolling number, the spark line, the glint ring | ui | `components/viz.rs` |
| 2 | [D1](cards/D1-aware.md) | The Aware entity | ui | `aware/*.rs` |
| 2 | [E1](cards/E1-menu-view.md) | The menu's view: head, reasons, typing, slide | ui | `menus/view.rs`, `menus/tests_view.rs` |
| 2 | [E3](cards/E3-menu-filter.md) | Finding rows by typing | ui | `menus/filter.rs` |
| 3 | [E2](cards/E2-menu-content.md) | The head's contents and Suggested, as data; every row's key | ui | `menus/head.rs`, `menus/smart.rs`, `menus/tests_smart.rs` |
| 3 | [E4](cards/E4-menu-host.md) | The menu host uses them; why a row is greyed; the two new rows | ui | `menus/mod.rs`, `menus/item.rs`, `menus/folder.rs`, `menus/tests_host.rs` |
| 3 | [F2](cards/F2-dock-words.md) | The dock's words and its end | ui | `status_bar/sentence.rs`, `status_bar/chips.rs`, `status_bar/mod.rs`, `status_bar/tests_aware.rs` |
| 4 | [F1](cards/F1-dock-island.md) | The island's slot and Paste | ui | `command_bar/smart.rs`, `command_bar/mod.rs`, `command_bar/tests_smart.rs` |
| 4 | [G1](cards/G1-send-to.md) | Move or copy to, and the usual place | ui | `menus/send_to.rs`, `menus/tests_send_to.rs` (and two marked places in `item.rs`, `mod.rs`) |
| 4 | [G2](cards/G2-palette-settings.md) | The palette learns; the Settings rows | ui | `palette/commands.rs`, `palette/mod.rs`, `settings/general.rs`, `settings/pages.rs` |
| 5 | — | **Coordinator**: the list and the grid look ahead at the row the pointer rests on (120 ms, as they read folders ahead today); every move and copy counts its place (paste, a drop, the other pane, Move or copy to); the dock's chip opens Recent changes and Disk usage; then the look in the real app in both themes, speed, e2e, docs (section 9) | all | — |

Why this order: wave 1 has no card that reads another's code (the model, the engine and the drawing are apart). D1 needs A and B. E1 and E3 need only section 5.4. E2 and E4 need D1 and E1's `MenuView::set_head`, and are written against each other's signatures. F1 needs E4's `MenuHost::suggestions`. G1 edits one marked place in each of two files E4 owned, after E4 is done.

**Who runs a card.** Any agent a tier below the coordinator (Sonnet). Give it three things and nothing else: STANDARDS.md, this file's sections 1 to 6, and its card. A card is sized to be finished in one run; one that comes back `partial` is finished by the coordinator or sent again with the report attached.

## 8. What must not break

- **The menus' existing tests pass unchanged** (`components/menus/tests.rs`, `spec.rs`, `kinds.rs`): `item_menu` and `folder_menu` return what they return today. The head and Suggested are added by `smart::dress` after them (card E2), so a test that reads `item_menu`'s labels sees no new line. Two exceptions are allowed, each named in its card: "Copy the text in it" in a picture's menu (card E4), and the "Move or copy to" submenu (card G1). The tests that list a whole menu are updated for exactly those lines.
- **The end-to-end checks** (`tests/e2e/checks/Checks.ps1`) find things by these names, which stay: the menus `Folder menu` and `Item menu`; menu items `New folder`, `Paste`, `Select all`, `Refresh`, `Rename`; the toolbar `Commands`; buttons `New folder`, `Copy (Ctrl+C)`, `Paste (Ctrl+V)`, `Rename (F2)…`, `Grid, with thumbnails…`, `List (Ctrl+Shift+6)`, `Command palette (Ctrl+K)`, `Tools`, `View: hidden items…`. A button may gain visible words; its accessible name may not change.
- **The dock's speed.** Selecting all of a 10,000-row folder must cost what it costs today: nothing in this wave walks a selection of more than 200 items (`MOST_TO_LOOK_AT`, card D1).
- **Opening a menu** stays under a millisecond of the UI thread for one item, and under four for 200 (`perf::span("menu build")`).

## 9. Checking it (the coordinator, after wave 4)

1. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
2. The optimized build that behaves as a development copy (STANDARDS, "Speed is measured in the real app"), on a scratch profile through `tests/e2e/lib/Harness.ps1`, with `GLINT_FRAME_LOG=1`: the pointer along a folder of 2,000 mixed files, right-clicks, selections. Wanted: no frame over 8 ms that names an aware span; `menu build` as in section 8; the quiet lane idle within a second of the pointer leaving.
3. The look, in both themes, at 100% and 125%, in a narrow (900 px) and a short (620 px) window, beside the mock: a downloaded unsigned program, a zip, a folder with `target\` in it, two source files, three PDFs, a cloud-only file, a file open in another program, an item inside an archive, a search's results. With Windows' "Animation effects" off.
4. `tests\e2e\run.ps1` (the person quits their own Glint first): 18 of 18 as before, plus the new check `smart-menu` (a zip in the arena: its menu has "Suggested" with "Extract here"; typing "ren" and Enter opens the rename box).
5. Docs: STANDARDS.md §5.1 gains "Aware" (rules R1 to R9 in the repository's words), VENDOR.md is untouched (nothing in `vendor/` changes), `docs/features.md`, `NEXT_RELEASE.md` and the README's "What's new".

## 10. Left out, on purpose

- **A custom shader in the vendored GPUI.** The ring is a stroked path with a two-stop gradient whose angle turns: no renderer change to carry forward.
- **Anything that needs a model or the network** (naming files by their content, "ask in words"). Everything here is rules, counts and the PC's own facts.
- **Reordering the menu's own rows by use.** It would break the hand's memory (R3).
- **A live feed from the index service.** The pulse comes from the folder watcher Glint already has; the service's protocol stays one request per connection.
- **Duplicates across a whole folder in the dock.** Too heavy to be free (R5); the Duplicates dialog stays the way to it.

## 11. Decided for you (say if you want otherwise)

- **Learning is on by default**, counted only on this PC in `state.json`, with a switch and "Forget what Glint learned" in Settings.
- **Three suggestions at most in a menu, one in the island.** More reads as noise.
- **Lower-tier agents do the cards; the coordinator does S0, the wiring and the checking.** Fifteen cards over four waves.
