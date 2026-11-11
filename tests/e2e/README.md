# End-to-end checks of the real app

These checks start the **debug `Glint.exe`** and drive it the way a person does: real mouse clicks and drags, real key
presses, a real File Explorer window. They find controls by their **accessible names** (UI Automation reads Glint's
AccessKit tree), so they double as an accessibility check: a control that isn't named can't be found.

What headless tests can't see, these do: the file list under the real keyboard, the address box taking typed text,
menus opening where the pointer is, the rename box on a real row, an OLE drag from one app to another, files
appearing on the real disk.

## Running it

```powershell
# 1. build the debug app (the same command as everywhere in this worktree)
$env:CARGO_TARGET_DIR = "C:/Repos/VisualStudioRepos/shadui-ripgrep/.claude/worktrees/gpui-migration/target/ui"
cargo build -p glint

# 2. run every check (about 100 s). From any shell; the runner re-starts itself under Windows PowerShell 5.1, STA.
powershell -STA -NoProfile -ExecutionPolicy Bypass -File tests\e2e\run.ps1
#    a different build, or only some checks:
powershell -STA -File tests\e2e\run.ps1 -Exe C:\path\to\Glint.exe -Only drag-copy,drag-move-shift
powershell -STA -File tests\e2e\run.ps1 -List        # what each check proves
```

While it runs, **leave the mouse and keyboard alone and keep the screen unlocked**: the checks send real input to
whichever window is in front, and the run takes the foreground for itself. Don't run it while another `Glint.exe` is
open (the run refuses: a second launch would hand its folder to the open one).

The exit code is 0 when no check failed (known issues don't count), 1 when a check failed, 3 when the run could not
start or was aborted, 4 when something was not put back (see below). The last lines say which.

### Result words

| Word | Meaning |
| :--- | :--- |
| `PASS` | the check did what it says |
| `FAIL` | it didn't: the log names the assertion, and `results\` of the kept scratch folder holds a capture and the UI tree |
| `KNOWN` | a check marked `-Known "<bug>"` failed, as it does today: an app bug the run found and keeps visible. It doesn't fail the run |
| `FIXED` | a `-Known` check passed: the bug is gone, so remove the `-Known` flag from the check |
| `SKIP` | the check could not run on this PC (the reason is given) |

## What the run does to your PC, and puts back

- **Fresh profile.** It copies `state.json` (`%APPDATA%\com.shado.shadui-ripgrep`), `window.json` and `startup.json`
  (`%LOCALAPPDATA%\com.shado.shadui-ripgrep`) to `%TEMP%\glint-e2e-backups\<stamp>`, deletes them so the app starts on
  defaults (English names, light theme: `GLINT_LANGUAGE=en`, `GLINT_THEME=light`, set for the child process only), and
  writes a `window.json` that places the window at a known size. At the end it puts the originals back and compares
  SHA-256 hashes before and after: the summary says whether they match.
- **Clipboard.** Its text, file list and picture are saved first and put back at the end (the summary says whether it
  matches). Other formats (private ones) can't be copied through .NET and would be lost; the log lists what the
  clipboard held.
- **Files.** Everything the checks create, copy, move, rename or drop lives in a scratch folder
  `%TEMP%\glint-e2e-<stamp>\arena`, made fresh before every check (`Reset-Arena` refuses any path outside it). Nothing
  is deleted, recycled or emptied anywhere else, and the checks never use Delete (that would fill your Recycle Bin).
  The scratch folder is removed when no check failed; after a failure it is kept, with `results\run.log`, a PNG of
  every window of the app (PrintWindow, or the screen's pixels when a GPU-drawn window comes back black) and a dump
  of the UI tree.
- **Windows.** The one File Explorer window it opens is found by its handle and closed again (only that window:
  if Windows put the folder in a tab of a window that was already open, the check skips instead). Glint is asked to
  close, and killed after 8 s if it hangs. If it stops answering the run ends and says so.
- **If the run is killed** (Ctrl+C in a hard way, power loss) your state files stay replaced by the fresh profile.
  Run `powershell -STA -File tests\e2e\run.ps1 -Restore` to put the newest unrestored backup back; a new run refuses to
  start until you do, so it can never back up the fresh profile as if it were yours. The clipboard is only saved in
  memory.
- The debug app shares the data folders with the installed Glint (it has its own single-instance name and never
  touches the Run key), which is why the above matters.

## The checks

| Check | What it proves |
| :--- | :--- |
| `launch` | The app starts on the folder passed on the command line: a window called Glint, the tab named after the folder, the address bar, command bar and sidebar named, and the list showing exactly what is on disk, folders first (an Arabic file name included) |
| `keyboard-list` | Down/Up move the selection, Home/End jump, a typed letter jumps to a name (type-ahead), Enter opens a folder, Backspace goes back, Alt+Up goes to the parent |
| `address-bar` | Ctrl+L opens the address box, a typed path plus Enter opens that folder, Escape cancels and leaves the folder as it was, a breadcrumb and the Up button go up |
| `context-menu` | A right-click on the empty list opens the *Folder menu* with its items; Escape closes it; **New folder** from it creates a folder, in the rename box, named as typed, on disk |
| `rename-undo` | Right-click a row, **Rename** from the *Item menu* opens the real rename box, the new name reaches the disk; an Arabic name typed into the box (F2) arrives intact; two presses of Ctrl+Z put both names back |
| `rename-cancel-focus` | After Escape closes the rename box, Ctrl+L still opens the address box |
| `rename-enter` | Enter in the rename box commits the name and does nothing else |
| `drag-copy` | A real mouse drag of a file onto a folder row copies it into the folder; the original stays, the contents match |
| `drag-move-shift` | The same drag with Shift held moves it |
| `drop-from-explorer` | A file dragged out of a real File Explorer window (opened on the scratch folder) onto a folder row of Glint is copied in, and stays in the Explorer folder |
| `drop-after-key` | The same drop also works after a key was pressed in Glint |
| `quick-look` | Space opens Quick Look named after the file and showing its text, the right arrow moves to the next item, Space and Escape close it |
| `command-bar` | The buttons: New folder, Copy (onto the real clipboard) and Paste (Windows names the copy), Rename (and Escape), Grid and List (the layout really changes), the palette button and a command run from it, the View and Tools menus |
| `dual-pane-compare` | Ctrl+Shift+D shows two named panes, a second pane can be taken elsewhere, Ctrl+Shift+M marks "Only here" items in both and unmarks, Ctrl+Shift+D closes the second pane |
| `search-inside` | Ctrl+Shift+F, a typed word, Enter: the file that holds the word is a row of the results under "Inside files", with the matching line and its number under it; Escape returns to the folder |
| `settings` | Ctrl+, opens Settings (it is a dialog inside the main window), a section can be chosen, Escape closes it |
| `tabs-sidebar` | Ctrl+T adds a tab, a sidebar place opens in it, Ctrl+W closes it |
| `quit` | Closing the window ends the process with exit code 0 and writes `state.json` with the session |

Every check begins from `Reset-App`: overlays closed, Files mode, one pane, one tab, the arena open with its original
contents, nothing selected.

### Bugs of the app these checks found (fixed)

These three were kept as `-Known` checks until they were fixed; the checks now pass and guard them.

1. **`rename-enter`:** Enter in the rename box committed the name *and also* ran the list's Open on the old name (gpui's `Input` propagates a single-line Enter to the list's `OpenItem`). Fixed in `file_list/rename.rs`: the box's wrapper takes `OpenItem`, `OpenInNewTab`, `Rename` and `Properties` so they stop there.
2. **`rename-cancel-focus`:** when the rename box closed, nothing held the keyboard focus, so Ctrl+L, F5 and the buttons that dispatch an action did nothing until a click. Fixed in `file_list/rename.rs` (`end_rename`): the list takes the focus back when the box held it (or nothing did); a box closed by going to another control leaves the focus there.
3. **`drop-after-key`:** after any key press in Glint, a file dropped from another app was ignored until the mouse had moved over the window. The vendored GPUI set its input modality from the file-drop event (not pointer input), so hover, which every drop listener checks, stayed off after a key press. Fixed in `Window::dispatch_event` (see `docs/gpui/VENDOR.md`); `drops.rs` has the headless test.

## What it can't check yet

- **Screen readers.** The tree's names and roles are checked, not what Narrator or NVDA say.
- **The Arabic UI** (`GLINT_LANGUAGE=ar`): the checks run in English. Arabic is only exercised as a file name in the
  list and as text typed into the rename box. Caret movement and selection with Arabic input in the address bar, and
  the mirrored layout, still need a pass.
- **The shell menu** ("Show more options"), drag *out* of Glint onto a File Explorer window, the Jump List, the tray
  menu and the global hotkeys, the Quick Search window: they live outside the main window's tree or need OS surfaces
  the harness doesn't drive yet.
- **Delete and the Recycle Bin**, undo of a delete, and the copy queue: deliberately not touched (they would leave
  items in your Recycle Bin).
- **Previews that draw natively** (video, PDF pages, 3D): the checks see their frame, not their pixels.
- **A release build, elevation, other DPI scales and several monitors, touch and pen.** It ran at 125% on one
  display; the geometry is read from the tree, but nothing varies it.
- **Updater, installer and MSIX**: never run (the run installs and uninstalls nothing).
- **Speed.** The waits are generous (seconds); use `Glint.exe bench` for timings.

## Writing a check

1. Look at what the app shows: `powershell -STA -File tests\e2e\dump-ui.ps1 -Steps 'rclick:alpha.txt'` starts the app
   on a scratch folder, does the steps (keys like `ctrl+shift+f`, `text:abc`, `click:Name`, `rclick:Name`,
   `dclick:Name`, `drag:A>B`, `sdrag:A>B`, `clickat:x,y`, `wait:1`, `capture:file.png`), and prints every element with
   its role, name, rectangle and state. It restores your files exactly as `run.ps1` does.
2. Add it to `checks\Checks.ps1`: `Add-Check 'name' 'what it proves' { ... }`. Use `Assert-Eventually { cond } 'what'`
   for anything the app does asynchronously, and read results from the disk (`Test-Path`) as well as the tree.
   `Add-Check ... -Known "the bug"` keeps a check for a bug that exists today.
3. The helpers: `lib\Harness.ps1` (session, UI Automation, input, runner) and `lib\App.ps1` (Glint's names, menus,
   `Reset-App`, the Explorer window). `lib\Native.cs` is the input and window code (C# 5, compiled by `Add-Type`).

## Layout

```
tests/e2e/
  run.ps1            the runner (-Exe, -Only, -List, -Keep, -Restore)
  dump-ui.ps1        prints the UI tree after some steps: the tool for writing a check
  checks/Checks.ps1  the checks
  lib/Harness.ps1    session, backup and restore, UI Automation helpers, input, check runner
  lib/App.ps1        what knows Glint: names, menus, Reset-App, File Explorer
  lib/Native.cs      SendInput (mouse, keyboard, drag), window focus and placement, captures, clipboard backup
```

## Things learned while writing it

- UI Automation and `SendInput` agree only when the process is DPI aware (`Win.DpiAware()` runs first); at 125% a
  non-aware process clicks off target.
- Glint's AccessKit tree answers only after a client first asks, so the first query can come back empty: the checks
  wait with `Assert-Eventually` and `Reset-App` asks first.
- A row is dragged by its name: pressing in the empty middle of a wide row starts a marquee instead (in Glint and in
  File Explorer).
- Shift for a move is pressed once the drag is under way: pressed at the mouse-down it would change the selection.
- A notification ("Copied 1 item") floats over the top of the toolbar and takes the clicks meant for the buttons
  under it: `Click-Button` clears toasts first.
- `Send-Text` presses real keys for what the keyboard layout can type (so key handlers such as type-ahead see them)
  and sends Unicode key events for the rest (Arabic on an English layout).
