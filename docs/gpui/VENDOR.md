# Glint's changes to the vendored GPUI

*`vendor/` holds gpui-pre 0.3.12, gpui-pre-windows 0.3.12 and gpui-base 0.9.1 (from gpui-kit 0.9.1), patched for Glint and used through the workspace's `[patch.crates-io]`. Updating GPUI means carrying these changes forward, so every change is listed here with why.*

| Where | Change | Why |
| :--- | :--- | :--- |
| gpui-pre, gpui-pre-windows, gpui-base | The right-to-left fix: [rtl-fix.patch](rtl-fix.patch) (DirectWrite glyph placement and bidi levels, visual-order runs, cluster-based caret and hit testing, `x_ranges_for_index_range`, per-stretch selection in Input) | Arabic text laid out and edited correctly (migration plan, appendix) |
| gpui-pre `resources/windows/gpui.manifest.xml` | Glint's application manifest: GPUI's own (asInvoker, Windows 10/11, PerMonitorV2, SegmentHeap, Common Controls 6) plus `longPathAware` | gpui-pre-platform always turns on gpui-pre's `windows-manifest` feature, which links this manifest into every GPUI binary; a second manifest in Glint.exe fails the link (CVT1100). Test binaries get the same manifest as the app. Glint.exe's own resources carry the icon and version info only |
| gpui-pre-windows `window.rs`, `set_window_placement` | Skips the placement asked for at open when the window is already visible | Glint opens its main window hidden, puts it in its saved place and shows it itself (no flash in the wrong place). Without this, the first `activate_window()` (gpui-component's notifications call it) would move the window back to the open-time bounds |
| gpui-pre-windows `window.rs`, `WindowsWindow::new` | A window opened with `show: false` gets its requested placement at once (hidden) | Glint shows its hidden windows with ShowWindow (posted, outside GPUI's callbacks); without this a hidden window kept CreateWindowEx's default size (Quick Search came up 1920 x 1010) |
