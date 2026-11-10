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
- A column child that should fill needs `.min_w_0()`, or long text won't wrap.
- Debug builds take `GLINT_LANGUAGE=ar` and `GLINT_THEME=light|dark`, so you can look at both.

**Style:**
- `let t = theme::tokens(cx).clone();` at the top of render; colors come only from `t`.
- Sizes are rems, so the text-size setting scales them: `theme::size::{XXS, XS, SM, BASE}` and `space(n)` (takes f32: `space(3.)`).
- Chrome (title bar, toolbar, sidebar, status bar) uses `theme::chrome_fill(cx)` so Mica shows through.
- Tinted buttons use `Button::custom(ButtonCustomVariant::new(cx)...)`.

**Keys:**
- Add a row to `actions::BINDINGS` (keys, context, action, English description from ShortcutsDialog.tsx) and the action to the `actions!` list.
- Contexts: Workspace, FileList, TextSearch, QuickLook, Compare.
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
