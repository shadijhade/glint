# The GPUI app's skeleton: what it is and how views plug in

*The coordinator's design for `crates/glint-ui` and `crates/glint`. Every component port copies these patterns, so the skeleton keeps them few and explicit. [STANDARDS.md](STANDARDS.md) still binds.*

## Crates and wiring

- **`crates/glint` (bin, `Glint.exe`):** `main()` handles worker mode first (`glint_core::worker()` returns an exit code when Glint was started as a worker), then calls `glint_ui::run()`. Single instance, tray, hotkey and the updater come later, in their own modules.
- **`crates/glint-ui` (lib, `glint_ui`):** everything GPUI.
  - Depends on `gpui-kit = "=0.9.1"`, `glint-model` and `glint-core`, plus `windows` 0.62 (Win32 features as needed), `raw-window-handle` 0.6 and `async-channel`. Nothing else without asking.
  - **Builds on the right-to-left fix.** The workspace's `[patch.crates-io]` points `gpui-pre`, `gpui-pre-windows` and `gpui-base` at `vendor/` (patched with docs/gpui/rtl-fix.patch). Never depend on another GPUI.
- **Starting the app:** `gpui_kit::application().with_assets(gpui_kit::assets::Assets)`. Without the assets, icons, including the title bar's buttons, are invisible. Inside `run`, in this order:
  1. `gpui_kit::init(cx)`
  2. `theme::init(cx)`
  3. `i18n::init(cx)`
  4. `actions::init(cx)`
  5. `core_bridge::init(cx)`
  6. open the main window

## core_bridge: glint-core inside the UI

- **One `glint_core::host::AppHandle`, created at start and kept in a GPUI `Global` (`Core`).** Views reach it with `cx.global::<Core>()`.
- **Events:**
  - `AppHandle`'s event hook sends each `host::Event` into an `async_channel`. A foreground task drains it into an entity, `CoreEvents`, which emits each one as a GPUI event.
  - A view that cares calls `core_bridge::subscribe(cx, |this, event, cx| ...)` and matches the `host::Event` variant it wants (`Event::CopyQueue(jobs)`, `Event::DirChanged(path)`…): values, never JSON.
  - Updates are coalesced: a view that gets many events per frame only marks itself dirty.
- **Main-thread jobs** (what core passes to `run_on_main_thread`) **never run inside GPUI's executor**, because some open modal Win32 loops.
  - glint-ui creates its own hidden message-only window (`HWND_MESSAGE`) on the UI thread at start.
  - The hook posts each job to it (`PostMessageW`, the job boxed in `LPARAM`), and that window's own WndProc runs it.
- **Core states:** register what src-tauri's `setup` registered with `manage` (`AppState`, `Thumbs`, `Installs`, the watchers…). Read `src-tauri/src/lib.rs`'s `run()` for the list. Do the same for the preview release hook (`host::previews::set_release_hook`) once native previews exist.
- **Windows:** each GPUI window registers itself as a `host::Window` with its HWND, so `window("main")` and shell dialogs' owner keep working.
- **Calling core from a view:** the work goes to `cx.background_spawn(...)`. A core `async fn` is driven on core's runtime (`glint_core::host::async_runtime::spawn`) and awaited from the GPUI task. Then `this.update(cx, ...)` with the result.

## theme

- **`Tokens`:** a struct with the colors and sizes of src/index.css, light and dark. Fields are named as the CSS variables are: background, foreground, chrome, card, popover, primary, muted, muted_foreground, accent, destructive, border, input, ring, field…
- **Conversion:** the HSL triplets from index.css become `gpui::Hsla`. The accent (primary and ring) follows Windows' accent color, as appearance.rs did. Port its accent reading here.
- **Applied to gpui-component's `Theme`:** mapped onto its fields at init and whenever Windows' appearance or the setting changes. Views read Glint's tokens with `theme::tokens(cx)`, never literals.
- **Sizes:**
  - The type scale and spacing follow Tailwind's 4 px unit at Glint's base size: `kit::space(n)` = n × 4 px.
  - Text sizes: `xs` 12, `sm` 14, `base` 16 (check index.css for the base actually used).
  - Fonts: "Segoe UI Variable Text" for sans, "Cascadia Mono" for mono.

## kit

- **Views import only `glint_ui::kit`.** It re-exports the gpui-component pieces Glint uses (Button, Input, InputState, Dialog, PopupMenu and ContextMenu, Tooltip, DataTable and TableState, VirtualList, Notification, Tab and TabBar, Sidebar, Resizable, Switch, Checkbox, Select, Kbd, Icon and IconName, TitleBar) under Glint's names. It also adds Glint's helpers:
  - `row()` and `col()` (h_flex and v_flex).
  - `row_start()`: a row that reverses when the UI is right to left.
  - `align_start()` and `align_end()`.
  - `icon(IconName)`.
  - `space(n)`.
- **When gpui-component's API changes, only kit changes.**

## i18n

- **The language:** a `Global` holding the current `glint_model::i18n::Language`, from the setting, or else Windows' UI languages (`GetUserPreferredUILanguages`) through `language_of`.
- **`t!`:** `t!("Text")` and `t!("Delete {name}?", name = value)` expand to `glint_model::i18n` lookups for the current language.
- **Direction:** `is_rtl(cx)` tells views to mirror.

## actions

- **One GPUI action per command**, with the key bindings src/components/ShortcutsDialog.tsx lists, in context `"Workspace"` and narrower contexts later.
- **The skeleton defines the actions and binds them.** The views that handle them come with the component waves.

## The workspace

- **The main window:**
  - **Title bar:** gpui-component's `TitleBar` (snap layouts work, as phase 0 showed), with a tab strip placeholder.
  - **Toolbar row:** back, forward and up buttons, an address bar (`Input`) and a search box.
  - **Body:** a resizable sidebar placeholder and a content placeholder.
  - **Status bar:** a placeholder.
  - **Window background:** Mica.
- **Placeholders** say what component will replace them. They must use `t!`, theme tokens and kit, like everything else.

## The exemplar: Drive health

**The task:** port src/components/explorer/DriveHealth.tsx completely, as the pattern every dialog follows:

1. **Data:** a call into glint-core (`drivehealth.rs`) on the background, with loading, error and data states, and no UI-thread waits.
2. **Layout:** kit only, theme tokens, `t!` for every string, right-to-left safe.
3. **Accessibility:** a dialog role and title, labelled rows, Escape closes, focus goes to the dialog.
4. **A headless GPUI test** (gpui-kit's test support) that opens it with fake data and checks the rows and the accessibility labels.
5. **Opening it:** the skeleton opens it from a temporary toolbar button and from an action, to show the wiring.

## Not in the skeleton

These come in later waves:

- The file list and grid, the real tab strip, the address bar's behavior, search, previews, and every other dialog.
- The platform services: single instance, tray and hotkey, drag and drop, the shell menu, the preview host, the jump list, the updater.

## Reference

- **Phase 0's spike**, in docs/gpui/reference:
  - `spike-kit.rs`: gpui-kit app start, TitleBar, DataTable, Input, driving input in tests.
  - `spike-phase0.rs`: window subclass, drop target, shell menu, WebView2 test (WebView is now forbidden), preview host pop-up.
  - `spike-raw.rs`: plain GPUI, timing.
- **gpui-kit 0.9.1's sources** in `~/.cargo/registry/src/index.crates.io-*/gpui-component-0.9.1`, `gpui-base-0.9.1` and `gpui-kit-0.9.1`. The vendored GPUI is in `vendor/`.
