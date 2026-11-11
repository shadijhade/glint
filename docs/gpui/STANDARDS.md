# Glint on GPUI: standards for everyone porting code

*Read this before writing a line. The coordinator reviews every change against it; work that doesn't follow it gets sent back. The plan, and why each decision was made, is in [migration-plan.md](migration-plan.md).*

## 1. What we're building

Glint is a fast Windows file explorer. Its UI moves from WebView2 (React/TypeScript) to **GPUI**, drawn in-process by Rust. When the migration is done:

- There is **no WebView** of any kind: no WebView2, no wry, no HTML or JS engine, not even for previews.
- There is **no TypeScript**.
- Every preview that used a web library gets a native replacement: EPUB, Word, 3D, Mermaid, math, email HTML, the terminal, video.

**Speed and memory are the product.** A change that makes a hot path slower (listing, scrolling, typing, search, startup) is wrong, even if it works.

**Nothing leaves this PC.** The work lives on the local branch `gpui-migration`, in the worktree `.claude/worktrees/gpui-migration`. Never `git push`, never open a PR, never publish anything. The branch's push remote is deliberately broken.

## 2. Where code goes, and who may touch what

```
crates/glint-model/   pure logic ported from src/lib/*.ts. No GPUI, no Tauri, no I/O beyond what a
                      module did before. Fast to build and test.
crates/glint-core/    (coming) Glint's Rust from src-tauri/src, with Tauri replaced by a small host shim.
crates/glint-ui/      (coming) GPUI views on gpui-kit, behind Glint's own kit layer.
crates/glint/         (coming) Glint.exe: platform services and main().
vendor/               (coming) patched gpui-pre, gpui-pre-windows and gpui-base (the right-to-left fix).
src/, src-tauri/      the old app: read it, port from it, don't change it.
```

**The files you may edit are named in your task.** All other files are read-only to you. The coordinator owns these:

- every `Cargo.toml`
- every `lib.rs` and `mod.rs`
- `rustfmt.toml`
- everything in `docs/gpui/`

**If you need something from a file you don't own, say so in your report.** That covers a dependency, a module export, or a change to a shared type. Don't work around it by editing the file. For a small helper another module will export later, a private copy in your own file is fine; mark it `// TODO(coordinator): from <module>`.

**Don't run `git commit`, `git stash`, `git checkout` or anything that changes the repository's state.** The coordinator reviews and commits each wave.

## 3. Rust rules

**Formatting and lints:**
- Edition 2021, `rustfmt.toml` at the root (max width 120). Run `cargo fmt` on your files only: `cargo fmt -p <crate> -- <file>`, or rustfmt your file directly.
- **Clippy must be clean** with `-D warnings`. Don't add `#[allow]` to silence it. If a lint is really wrong for the case, explain why in a comment next to the allow.

**Never panic on input:**
- No `unwrap()`, `expect()`, indexing that can go out of bounds, or `as` casts that can truncate, on anything that comes from outside: user text, file names, file contents, settings files, the index service.
- Return `Option` or `Result`, or clamp, as the TypeScript did.
- `unwrap()` is fine only where the invariant is local and obvious, with a short comment saying why.

**Other rules:**
- **No `unsafe`** outside Win32 calls. Every `unsafe` block gets a `// SAFETY:` line.
- **No new dependencies.** The coordinator has already added what a crate may use (see its `Cargo.toml`). Need another? Ask in your report.
- **Names:**
  - TS `camelCase` functions become `snake_case`.
  - Types stay `PascalCase`.
  - TS string unions become `enum`s.
  - TS `interface`s become `struct`s with `pub` fields.
  - Keep the TS name recognisable (`fuzzyScore` becomes `fuzzy_score`) so the next person can find both.

**Comments** follow Glint's style: plain sentences about what something is for and why, written for a newcomer. Don't restate the code.
- **Module header:** `//! Ported from src/lib/<file>.ts.` followed by what the module is for.
- **Public items:** a `///` line each.
- **Notes:** anything surprising carried over from the TS ("Explorer sorts this way, so we do too") keeps its note.

**Tests** live in `#[cfg(test)] mod tests` at the bottom of the file, in plain `#[test]` functions. Use descriptive names, not `test1`.

## 4. Porting TypeScript to Rust

**The TypeScript is the spec.** Port behavior, not just signatures.
- Same results, same edge cases, same order of results, same rounding.
- If the TS looks like a bug, port it as it is, mark it `// NOTE(port):` with what you think is wrong, and mention it in your report. The coordinator decides.

**The tests come with it.** `src/lib/explorer.check.ts` and `src/lib/text.check.ts` hold Glint's assertions; port every assertion that exercises your module.
- Each `assert.equal(f(x), y)` becomes an `assert_eq!` in a `#[test]`. Group them as the check file does (one test per section or function).
- Count them: your report says how many you found and how many you ported. Skipping one needs a reason.

**Strings are the classic trap:**
- **JS strings are UTF-16.** `.length`, `.slice`, `.indexOf`, `charCodeAt` and regex indices count UTF-16 code units. Rust `str` is UTF-8 bytes.
- **Never index a `str` by a JS-derived number.** Work in `char`s or `char_indices`.
- **Indices you return** that the UI will use (highlight ranges, cursor offsets) are **UTF-8 byte offsets on char boundaries**, because GPUI's text runs use byte offsets. Document that in the item's comment.
- **`toLowerCase`** becomes `to_lowercase()`, not ASCII-only lowercasing. File names are Unicode: Arabic, Hebrew, CJK, emoji.
- **`localeCompare` and `Intl.Collator`** need a decision. The coordinator will provide Windows' own comparison (`CompareStringEx` with `SORT_DIGITSASNUMBERS`, the one Explorer uses). Until then, write the comparison as a single function your module calls, and note it.

**Numbers:**
- **Pick the Rust type from the meaning:** `u64` for byte sizes, `i64` for Unix milliseconds, `usize` for counts and indices, `f64` for real-valued math, `f32` only for pixels.
- **Match JS math exactly where results are visible:**
  - `Math.round` rounds .5 up, toward +∞. Rust's `f64::round` rounds half away from zero, so for negative values use `(x + 0.5).floor()`.
  - `%` on negative numbers.
  - `parseInt` stops at the first bad character.

**Locale-aware formatting:**
- **No hand-rolled formats.** Where TS used `Intl` (numbers, dates, times, durations), use Windows' own formatting through the `windows` crate (`GetNumberFormatEx`, `GetDateFormatEx`, `GetTimeFormatEx`) with the user's locale, as WebView2 did.
- **Tests stay deterministic:** functions take an explicit locale name (`"en-US"`) underneath, so tests don't depend on this PC's settings.

**Browser and app state:**
- **Browser APIs don't exist here** (`document`, `window`, `navigator`, `localStorage`, timers). Pure logic takes what it needs as parameters: the language, today's date, the settings.
- **zustand stores aren't ported as stores.** Port the state's types, defaults and pure update functions; the UI layer owns the live state.

**Serialization:**
- **Anything that reaches disk keeps its JSON shape** (settings, the settings export file, session): use `#[serde(rename_all = "camelCase")]` and the same field names and defaults.
- **Existing files still load:** add `#[serde(default)]` where the TS tolerated missing fields.

## 5. The UI (GPUI) rules, for the component waves

- **Use the kit, not gpui-component directly.** Build from `glint_ui::kit` (Button, Input, Menu, Dialog, Table…), which wraps gpui-component so one module absorbs its API churn. If the kit lacks something, ask; don't import `gpui_component` in a view.
- **Theme tokens only.** No hard-coded colors or sizes: they come from `glint_ui::theme` (Glint's index.css tokens, light and dark, Windows' accent color).
- **Accessibility is required:**
  - Every interactive element has an accessibility role and a name: `.role(...)`, plus labels on inputs and icon-only buttons.
  - Lists and tables expose rows with names, as gpui-component's DataTable does.
  - Keyboard-only use must work: focus order, Enter, Escape, the arrows.
- **Right to left is required:**
  - No left/right assumptions: use the kit's start/end helpers and row direction, so the Arabic UI mirrors.
  - Never assume x grows with the text index. Use `LineLayout`'s caret functions, and `x_ranges_for_index_range` for highlights.
  - Test with an Arabic name in every text you lay out.
- **Strings are translatable:** every user-visible string goes through `t!("English text")`, with English as the key. Arabic comes from the same catalog the TS used.
- **The UI thread never waits:**
  - File system, index service, process work and hashing go to `cx.background_spawn`, with results coming back to the entity.
  - Streams arrive through `Sink`s and update the view at most once per frame.
- **Modal Win32 loops never run inside a GPUI callback:** the shell context menu, drag-out, and anything that pumps messages. Post a message and run it from the window subclass (see phase 0's `phase0.rs`).
- **Nor do Win32 calls that send a GPUI window messages at once:** ShowWindow, SetWindowPos, SetWindowPlacement, SetForegroundWindow, SetFocus, DestroyWindow on a GPUI window. GPUI's window procedure handles them by taking the app, which every GPUI callback and entity update already holds, so they panic. Post them with `core_bridge::post_to_ui`, or call them in a task between updates (outside `cx.update`). GetWindowPlacement and the other getters are safe.
- **Native windows:**
  - No WebView, ever.
  - GDI-drawn preview handlers (Office) live in an owned, never-activated pop-up over the pane.
  - DirectX-drawn native views must be checked visible over GPUI first.
- **Big lists are virtualized** (`uniform_list`, `list`, `DataTable`). Nothing allocates per row per frame that could be cached. Thumbnails and icons are `RenderImage`s in an LRU, never PNG round-trips.
- **Port behavior and feel, not markup:** same shortcuts, same menus, same wording, same states (empty, loading, error). Tailwind classes map to the theme's spacing and type scale, not to pixel copies.

### 5.1 How the skeleton does it (copy these exactly)

*Learned building the skeleton (crates/glint-ui). `dialogs/drive_health.rs` is the reference port.*

**Imports.** Use `use crate::kit::prelude::*; use crate::{t, theme};` and never `gpui_component`. The prelude re-exports the std `test` macro on purpose: GPUI's own `test` would otherwise shadow `#[test]` once test-support is on.

**Icons:**
- The app embeds `gpui_kit::assets::AllAssets`, the full Lucide set. A missing icon draws nothing and raises no error.
- Add every `IconName` your view uses to the test list in kit.rs (`every_icon_the_skeleton_draws_is_in_the_embedded_assets`).

**Accessibility:**
- `.role(..)`, `.aria_label(..)` and `.aria_description(..)` exist only after `.id(..)`.
- Icon-only buttons need `.accessibility_label(t!(..))`. Inputs take `.aria_label(..)`.
- gpui-component's Dialog host has no name: put `.role(Role::Group).aria_label(t!(title))` on your content root.

**Calling core:**
- Use `core_bridge::run(glint_core::x::y(args)).await` inside `cx.spawn(async move |this, cx| ...)`. It returns `Result<T, String>` for the join; flatten with `.and_then(|r| r)` when core's function returns `Result` itself.
- `WeakEntity::update` returns a `Result`.
- Streams: `core_bridge::stream::<T>()` gives a `host::ipc::Channel<T>` for core plus a receiver for a GPUI task.
- Core events: `core_bridge::subscribe(cx, ...)`.

**Modal Win32 loops** start through `core_bridge::post_to_ui(job)`. Get the owner HWND with `core_bridge::hwnd_of(window)` while you have the `Window`.

**Models:**
- A dialog's data lives in a model entity with `State { Idle, Loading, Failed(msg), Ready }` and a throttled `refresh(force)`. The view observes it.
- Give the model a `Source` (a boxed async fn), so tests inject fake data and never touch the real core.

**Dialogs:**
- `dialogs::show(window, cx, t!("Title"), px(width), view)`.
- Create entities **before** opening the dialog: its builder re-runs, and entities made inside it lose their state.
- Set `.text_start()` on the content root, because the dialog layer sits outside the workspace tree.

**Right to left:**
- `row_start()` for rows whose order matters.
- `align_start/end`, `text_start/end` and `ps/pe/ms/me` instead of left and right.
- `back_icon()` and `forward_icon()` mirror.
- `isolate(text)` (RLI…PDI) is for display only, never in paths, inputs or aria labels.
- `rtl_line(text)` (RLE…PDF) is for a line of counts, words and units ("6 folders · 15 files · 13 MB"): in Arabic it reads from the right as one line. `isolate` is not enough there: a line that begins with a number keeps that number at the left. Display only, like `isolate`.
- A column child that should fill needs `.min_w_0()`, or long text won't wrap.
- Debug builds take `GLINT_LANGUAGE=ar` and `GLINT_THEME=light|dark`, so you can look at both.

**Style:**
- `let t = theme::tokens(cx).clone();` at the top of render; colors come only from `t`.
- Sizes are rems, so the text-size setting scales them: `theme::size::{XXS, XS, SM, BASE}` and `space(n)` (takes f32: `space(3.)`).
- Tinted buttons use `Button::custom(ButtonCustomVariant::new(cx)...)`.

**The window: one bar, a rail, a sheet with a dock.** (`workspace.rs` places them; a new view goes into one of them, never beside them.)
- **The bar** is the title bar. It holds the address: Back, Forward, Up, and one field with where you are at its start and the search at its end. There is no toolbar row under it. Whatever sits in the bar `occlude`s, so pressing it never drags the window.
- **The rail** (start edge, on the chrome): the open tabs as a list, then the places. Tabs are rows, like places: every name whole.
- **The sheet**: the files, the preview pane at its end edge, the terminal under the files. Each pane of files starts with its **head** (`components/pane_head`): the folder's picture and name in the largest type in the window, what is in it in a line ("6 folders · 15 files · 13 MB"), a chip for every kind of thing in it (pressing one puts that kind's filter in the search box; again takes it out), and under them a thin band of the folder's make-up. In a short window the head is one line. This PC and a search's results have no head: their page and the search bar are theirs. Don't put another bar between the head and the files.
- **The dock** is the sheet's last 52 px: the status bar's words at its two ends, the commands' island over its middle. What acts on the selection is in the island while there is a selection and not otherwise; nothing greyed out stands in it for long. Its menus open upward.
- **One search.** The box looks in names and inside files, and what it finds is one list of files: a file found by its contents is a row like any other (under "Inside files"), with its matching lines under it. Don't add a second place to search from.

**A folder's row says what is in it.** In the list, the folder under the pointer and the one the keyboard is on have been read ahead (`Explorer::peeks`: the listing Enter will open with). Their rows say what they hold, in the head's words and with its band small (`file_list::Peek`, `cells::peek_cell`), at the end of the name's column: "10 files · 8.7 MB". It is drawn in every row whose folder was read ahead and shown by the row's hover group (`ROW_GROUP`), so the pointer coming and going draws nothing again; the row the keyboard is on keeps it. It gives way before the name does in a narrow column.

**Light, and glass.** Under everything the window draws its light (`art::aurora`, placed by `Workspace::light`): soft color at the corners, the color of the place shown at the start corner over the rail, turning when the place changes and dimming while another window is in front. Every place has a color of its own (`theme::place_tone`): Home is the accent, Documents blue, Downloads green, Pictures rose, Music orange, Videos indigo; a folder inside one is lit as it is. The same color is the place's chip in the rail, its card on This PC and its folder in a list, so where you are can be told from the corner of the eye. The sheet is glass (`t.sheet`, see-through enough to take the light's tint), and so is what lies on the chrome and is current: the address field, the tab and the place shown (`t.glass`). A view on the sheet has no fill of its own, or it would be a patch on the glass; only what must hide what is under it does (`t.background`: a message over tiles, a box being typed in).

**Pictures: `art`, not icons, for what a row or a tile is.** A folder is `art::folder` (a known folder in its place's color with its sign on it: `art::known_folder`), a file is `art::kind` (a page in its kind's color, the kind's sign in white), a place is `art::chip`. They are drawn once at the pixels they cover and kept: a frame costs one sprite. The stroke icons (`icon(IconName::…)`) are for buttons, menus and marks beside a name, in the text's color. Don't draw a file or a folder with a stroke icon, and don't tint stroke icons to tell kinds apart.

**Depth: three levels, and nothing else.**
- **Chrome** (the bar, the rail): no fill of its own. The window is filled once (`theme::window_fill`), Mica shows through, and anything on it that needs a surface uses a see-through one (`t.accent`).
- **The sheet** (`t.sheet`): the files, the preview pane and the dock, on one card of glass with a hairline (`t.stroke`), rounded all round (`theme::size::RADIUS_SHEET`) and clear of the window's end and bottom edges. A part of the sheet has no fill of its own either; cards on it are `t.glass` with a `t.stroke` hairline.
- **Floating** (menus, the palette, dialogs, notices, the dock's island): `t.popover`, a `t.stroke` hairline, and a shadow from `theme::elevation::{raised, floating, modal}`. Never `shadow_md()` and friends: the elevations know the theme.

**States: see-through fills, the same everywhere.**
- Hover is `t.accent`, held down is `t.pressed`, selected is `t.selected` (`t.selected_hover` under the pointer, `t.selected_inactive` in the pane the keyboard isn't in); a selected row where the keyboard is has the accent's sweep (`theme::selected_fill`). They are translucent, so they read on all three levels: don't mix your own with `.opacity(..)`.
- A highlight is a rounded fill *inside* its line (`theme::size::RADIUS_ROW`, a few pixels in from the edges, a pixel of air above and below), so rows selected together read as rows. The line itself is what is clicked and named. `file_list/cells.rs` is the reference.
- The item the keyboard is on carries the accent bar at its start edge; the 1 px accent outline is only for when the selection doesn't already say where the keyboard is.
- `theme::glint_fill(cx)` (the logo's pink to fuchsia, or Windows' accent to its shade) is for the few things that should catch the eye: what is loading (`loading_sweep`), what is current (the bar at the showing tab's start edge), a field being typed in. Not for decoration.

**Motion: `theme::motion`, and it is never in the way.**
- Hover fills fade by themselves (the vendored GPUI does it for every element with an id: see VENDOR.md). Give an element that has a `.hover(..)` an id and it fades; don't animate hover by hand.
- Something that arrives uses `Animation::new(theme::motion::BASE).with_easing(theme::motion::enter)` (`FAST` for menus and small things, `SLOW` for big surfaces), and is readable from its first frame: start at an opacity of a third or more and move ten pixels at most. Leaving is quicker than arriving.
- Key an arrival by what arrived (`("file-list-enter", generation)`), so it plays once for it and not again on every redraw.
- An animation never delays the thing itself: the new rows, the menu, the dialog are there and usable in the first frame.
- Windows' "Animation effects" switch turns all of it off (`theme::motion::sync_with_windows`); so do tests. Anything you animate must be right when every animation lands at once: check with the switch off.
- The wheel glides (the vendored GPUI again). Don't add your own smoothing to a scroll.

**Keys:**
- Add a row to `actions::BINDINGS` (keys, context, action, English description from ShortcutsDialog.tsx) and the action to the `actions!` list.
- Contexts: Workspace, FileList, AddressFind (the search box), QuickLook, Compare.
- Never bind Ctrl+A/C/X/V/Z, Backspace or Delete outside FileList (a test enforces it).
- Use `KeyBinding::load` for strings built at runtime; `KeyBinding::new` panics on bad text.

**Tests:**
- `testing::init(cx)`, then `testing::open_window(cx, |window, cx| ...)` gives a window with the dialog layer.
- Hold `let _guard = testing::lock();` in every test that renders, because language and direction are process-wide.
- Call `window.render_frame(cx)` before querying.
- Tests that await core poll: `cx.run_until_parked()`, check, sleep 5 ms, repeat. `testing::open_window` allows parking for core's threads.

**Windows:**
- `TitleBar::window_options()` is the base for a window with the kit title bar.
- Keep the `Subscription` that `observe_window_appearance` returns.

**The app's services** (wave 3, always built; `startup.rs` is the only place they are wired to each other):
- **Folders, tabs, selection:** `explorer::explorer(cx)` is the `Entity<Explorer>`. Read with `.read(cx)` (`active_tab()`, `panes()`, `drives()`…), act with `.update(cx, |e, cx| e.navigate(..))`; Task-returning calls are `#[must_use]` (`.detach()` or await). Observe it to redraw; subscribe to `ExplorerEvent` for specifics. Views never keep their own copy of what the explorer holds.
- **Telling the person something** from model code: emit `ExplorerEvent::Notice` (startup.rs words it, as the web app did). From a view: `window.push_notification(..)`.
- **Settings:** read `state::settings(cx)`, change with `state::update` (saved soon by itself). React to a setting by observing `state::UiState`.
- **The rest of the saved state** (pinned, history, saved searches, remotes, rules): `persistence::app(cx)` to read, `persistence::update_app(cx, |app| ..)` to change (saved soon). Observing `persistence::Persistence` hears every change.
- **Drops** onto the window arrive at the workspace (`Workspace::on_drop_event`); the views under the pointer answer with `DropAnswers::set` and act on the drop. Drag-out: `platform::dragdrop::drag_out`.
- **Native previews:** `platform::preview_host::{show, set_rect, hide, reveal, unload}`; overlays (menus, dialogs, the palette, Quick Look) call `hide`/`reveal` while they cover the pane.
- **Files dropped onto a view** arrive as GPUI's own file drops (payload `ExternalPaths`), handed over by `crate::drops`: highlight with `.drag_over::<ExternalPaths>(..)`, answer what a drop at the pointer would do from `.on_drag_move::<ExternalPaths>(..)` with `drops::answer(cx, Answer::Accept(Under { .. }))` when `event.bounds.contains(&event.event.position)` (each move starts refused), take the drop with `.on_drop::<ExternalPaths>(..)`, reading `drops::effect(cx)` (copy, move, link) and `drops::paths(drop)`. Do the operation through the explorer (`Explorer::drop_files(paths, dest, move_, cx)`).
- **Dragging files out** of a view: `platform::dragdrop::drag_out(cx, workspace::MAIN, paths, copy_only)` once the pointer has moved past the drag threshold with the button down (spawn the future it returns).
- **File operations** go through the explorer (`copy_to_clipboard`, `paste`, `delete_paths`, `begin_rename`/`commit_rename`, `new_item`, `undo`, `drop_files`...), never straight to glint-core from a view: the explorer offers Undo and words the result.
- **Menus**: a view emits a `ContextMenu` event with the target and a window position; from the keyboard (the menu key, Shift+F10) the position is the focused row's bottom-start corner. The workspace opens the shared menus (`components::menus::MenuHost`).
- **Taskbar progress:** `platform::window_state::taskbar_progress`.

**Measuring during a draw:** a `canvas` (or `on_children_prepainted`) may record bounds, but never call `cx.notify` there: GPUI drops a redraw asked for during prepaint or paint, so the view would keep its first, unmeasured layout (the picture preview drew nothing until this was found). Ask after the frame instead: `window.on_next_frame(move |_, cx| cx.notify(id))`.

**A view that is not on screen does nothing.** Every pane has its list, grid, columns view, months strip and head from the start, whichever of them it shows, and they all follow the explorer. One that reads, lists or keeps something whenever the explorer changes does that for every folder opened while it is hidden: the columns view listed every focused folder again in each pane and freed its 100,000 entries on the UI thread (12 ms a pane, in the middle of showing the next folder); the Gallery's months were worked out for every folder. In an observer or a subscriber, check first that the pane shows your view, and let go of what you kept when it stops (`ColumnsView::rest`).

**A big listing is let go of with `explorer::let_go`.** An entry owns four strings and whoever holds a listing last frees them all. A model or a view that replaces a listing it holds hands the old one to `let_go`, which frees it on a background thread a moment later.

**Don't draw a frame that only says "wait".** A window at rest draws the moment something changes (docs/gpui/VENDOR.md, early frames), so a change announced just before its result arrives costs the result a frame. A folder says it is loading only if it takes 40 ms (`LOADING_SAID_AFTER` in `explorer/nav.rs`); a selection that is the one already there tells nobody (`apply_selection`). Do the same in new code: tell the views once, when there is something to show.

**Speed is measured in the real app.** `GLINT_FRAME_LOG=1` prints to stderr, among every frame's cost: `input kind=… at=…` for every press, `nav read`, `nav applied`, `nav told` and `nav shown` for a folder on its way from the press to the screen, `rows built`, and `span name=… ms=…` for any timed step that took a quarter of a millisecond or more. Time a render or a handler you suspect with `let _span = crate::perf::span("name");` at its top (nothing is read or printed when the log is off). `GLINT_BENCH_START=<file>` writes the time from the moment Windows made the process to the first folder painted, and every mark on the way (`glint_core::startup::mark`). Measure with the optimized build that still behaves as a development copy: `cargo build --release -p glint --config "profile.release.package.glint-ui.debug-assertions=true"` (glint-ui only: with it on the `glint` crate too the program is a console program, and the console it opens adds 170 ms to every start).

**Editing files with scripts:** open and write with `encoding='utf-8'`. The default on this PC (cp1252) mangles the sources' non-ASCII characters.

## 6. Core rules (src-tauri to glint-core)

- **Module code moves nearly unchanged:**
  - `tauri::` paths become `crate::host::` (`AppHandle`, `State`, `ipc::Channel`, `ipc::Response`, `Emitter`, `Manager`, `async_runtime`).
  - `#[tauri::command]` attributes go; the functions stay.
- **Window and WebView code** (`WebviewWindow`, `get_webview_window`, appearance, print, quick, more_windows, placement, snap, native_preview, dragdrop, jumplist and the setup in lib.rs) **doesn't move to core.** It's rewritten for GPUI in glint-ui or glint.
- **Behavior doesn't change** in the move. Bugs found on the way are reported, not fixed in the same change.

## 7. Done means

1. It builds, and the tests and clippy pass for your crate and features.
   - glint-model: `cargo test -p glint-model --no-default-features --features <yours>`
   - glint-model: `cargo clippy -p glint-model --no-default-features --features <yours> -- -D warnings`
2. Every exported TS function, type and constant of your module has a Rust counterpart, or a line in your report saying why not (for example, a browser-only helper the UI layer replaces).
3. Every relevant assertion is ported and passes.
4. No `todo!()`, `unimplemented!()`, placeholder logic, or silently dropped behavior.
5. Your report (the structured output you're asked for) is honest. Say what's partial, what you weren't sure of, and what you need from the coordinator. A wrong "done" costs more than an honest "partial".
