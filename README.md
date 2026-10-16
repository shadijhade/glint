# ShadUI Ripgrep

**ShadUI Ripgrep** is a blazing fast file explorer and search app for Windows, built with [Tauri](https://tauri.app/). It's a modern UI on top of the fastest Rust command-line tools: [fd](https://github.com/sharkdp/fd) finds files by name, [dust](https://github.com/bootandy/dust) measures folder sizes and [ripgrep](https://github.com/BurntSushi/ripgrep) searches inside files, while folders themselves are read straight from Win32 in Rust. Turn on [instant search](#-instant-search-ntfs-index) and an index of your NTFS drives finds any file name on any drive in milliseconds and sizes every folder as it opens. The goal: a file explorer that's orders of magnitude faster than the one Windows ships with.

![App Screenshot](screenshots/screenshot-search-result-dark.png)

## 🚀 Features

### Files: a fast explorer
-   **Instant folders**: Each folder is read with one Win32 enumeration pass (names, sizes, dates and attributes all at once) and sorted in Rust, then drawn as a virtual list. The status bar shows how long it took, e.g. *read in 0.3 ms · shown in 19 ms*.
-   **Find files by name**: Typing in the search box filters the folder as you type; **Enter** searches every subfolder too (from This PC: every drive). With [instant search](#-instant-search-ntfs-index) on, the index answers in milliseconds; otherwise fd streams results as it finds them. All words must match, `*` and `?` work like in Explorer, and `"quotes"` keep spaces. The status bar says who answered: *index · 18 found in 6.5 ms* or *fd · 304 found in 8.09 s*.
-   **Folder sizes**: Explorer never shows them. With instant search on, every folder shows its subfolders' sizes as it opens (*index sized 219 GB in 0.4 ms*), and search results show folder totals too; otherwise one click measures them with dust. Sort by size to find what's filling your disk.
-   **Preview pane**: Syntax-highlighted code and text, images, video, audio, PDFs, and folder summaries. Online-only OneDrive files are left alone, so nothing downloads by accident.
-   **The usual explorer moves**: Back/forward/up, a breadcrumb address bar you can type into (`%APPDATA%` and `~` work), Quick access folders, pinned folders, drives with free space, sorting, multi-select, type-ahead, rename, new folder, and hidden items on demand.
-   **Windows-native file operations**: Copy, cut, paste, delete and rename go through Windows' own file engine, so progress, "Replace or Skip", the Recycle Bin and permission prompts behave exactly like Explorer. The clipboard is shared with Explorer: copy here, paste there, and the other way round.
-   **Drag and drop**: Drag files out to File Explorer, the desktop, a mail or a chat, or drop them in from anywhere onto a folder, a sidebar place or the open folder. As in Explorer, dropping on the same drive moves and on another drive copies.
-   **Live**: The open folder refreshes when files change elsewhere.
-   **Explorer integration**: Properties, Open with…, Show in File Explorer, and Open in Terminal.

### Text: search inside files (ripgrep)
-   **Blazing Fast Search**: Leverages the power of `ripgrep` to search through large codebases in milliseconds.
-   **Search & Replace**: Find text and replace it across multiple files with confidence.
-   **Real-time Preview**: Instantly view the file content and context of any search result with syntax highlighting.
-   **Regex, case, whole word, exclusions**: Full regular expressions, case sensitivity, whole words, and patterns to skip (e.g. `node_modules`, `.git`).
-   **Connected to Files**: **Search text** (Ctrl+Shift+F) searches the folder you're in; **Show in folder** on a result opens it in Files with the file selected.

### Modern User Interface
-   **Beautiful Design**: Built with Shadcn UI and Tailwind CSS for a sleek, modern look.
-   **Dark/Light Mode**: Fully supports system theme syncing, or manually toggle between Dark and Light modes.
-   **Search History**: Quickly access and re-run your previous text searches.
-   **Keyboard first**: Everything has a shortcut (see below).

---

## ⚡ Instant search (NTFS index)

Off until you turn it on in **Settings → Instant search**. Windows then asks for administrator approval once, and a small service (`ShadUI Ripgrep instant search`) starts indexing your fixed NTFS drives. From then on:

-   **Name searches** on indexed drives answer from memory: the same query rules as fd (every word must match, `*` and `?` match the whole name, case-insensitive, same exclusions and hidden-file setting), results streamed into the same list with size, date and attributes.
-   **Folder sizes** for every folder come from the index, kept current as files change.
-   **Fallbacks are automatic.** fd answers whatever the index can't: drives that aren't indexed (network shares, FAT/exFAT USB sticks, a drive still being read), folders behind junctions, and searches with *Skip what .gitignore lists* on (only fd reads .gitignore). From This PC, indexed drives and the rest are searched together (*index + fd*). dust sizes folders the index doesn't have.

### Measured

On this machine (Intel Core Ultra 7 265K, NVMe C: with 2.89 million MFT records: 2.33 M files, 509 k folders, 3.0 M names; plus a SATA SSD with D: and E:), release build:

| | |
| :--- | :--- |
| Indexing C: from scratch (one read of the whole MFT, 2.9 GB) | **2.3–2.5 s** to ready (E:, 343 k records: 0.1–0.5 s) |
| Restart (load the 214 MB snapshot, replay the journal) | **0.25 s** to ready |
| Memory for C: | 296 MB |
| Search inside the service, C: | 2.5 ms (`kernel32.dll`), 4.4 ms (`report`), 2.7 ms (no match), 14 ms (`*.dll`: 108 k matches, 50 k returned) |
| Search end to end over the pipe, all three drives | median 4.8 ms (`kernel32.dll`), 8.1 ms (`report`), 4.3 ms (no match), 31–33 ms (50 k results) |
| Search in the app, Enter to every result listed | 6.5–7.5 ms (`kernel32.dll`), 14–15 ms (`report`), 155–186 ms (50 k results; most of it is handing 50 k rows to the webview) |
| Folder sizes | 0.02–0.05 ms in the service, 0.4 ms in the app |
| A file created, grown, renamed, moved, hard-linked or deleted | in the index within ~0.3 s (the journal is checked every 250 ms) |
| The same name search with fd, home folder only (219 GB) | 8.09 s |

Measure your own with `shadui-index bench C:` (administrator) and `shadui-index search kernel32.dll --repeat 21`.

### How it works

-   **One read of the MFT.** The service reads each volume's Master File Table straight off the disk in 8 MB sequential chunks (parsed in parallel while the next chunk is read), the way Everything and WizTree do. One pass gives every name (hard links included), its folder, size, last-write time and attributes; `FSCTL_ENUM_USN_DATA` alone would give names but no sizes or dates.
-   **The USN journal** keeps it current: every 250 ms the service reads new journal records and re-reads each file record they touched through NTFS itself (`FSCTL_GET_NTFS_FILE_RECORD`), so creates, renames, moves, size changes, hard links and deletes converge on what's really on disk however late or often a change is seen. Folder totals are updated along each changed file's folder chain. If the journal wraps while the service isn't running, it reads the MFT again (seconds).
-   **Snapshots** of each volume's index are saved after indexing, every 30 minutes if anything changed, and when the service stops, so a restart is a load plus a short journal replay.
-   **The index** keeps, per volume, a record per MFT entry (size, date, attributes) and a link per name to its parent folder, with names in one UTF-8 arena (a lowercase copy only for names that aren't ASCII). A search is a parallel scan over every name, then a walk up each match's folders for the scope, hidden and exclusion rules.

### Security model

-   **Nothing runs elevated unless you turn it on.** Turning it on runs the bundled `shadui-index.exe install` behind the UAC prompt; turning it off runs `shadui-index.exe uninstall` the same way. Uninstalling the app removes the service too (one more prompt, only if it was on).
-   **Where it runs.** Reading a raw volume requires administrator rights, so the service runs as LocalSystem, but with every privilege removed except `SeChangeNotify`, `SeBackup` and `SeManageVolume`. It's copied to `C:\Program Files\ShadUI Ripgrep\`, which only administrators can change (a LocalSystem service must never run a file its users could swap), and registered with a quoted path, delayed start, and restart on failure. It only reads volumes; the one change it can make is turning on a drive's change journal if it was off (Windows' default size, 32 MB), as Everything does.
-   **Who can ask it.** It answers on the named pipe `\\.\pipe\shadui-ripgrep-index-v1`, whose ACL lets in only SYSTEM, administrators and the accounts that turned it on, and refuses remote clients. Even those accounts can't create an instance of the pipe, so no other program can pose as the service, and the app also refuses a pipe not owned by SYSTEM or Administrators (a program could otherwise take the name while the service is stopped and feed the app made-up results).
-   **What it exposes.** Like Everything, the index knows the name, size, date and attributes of every file on the indexed drives, **including files your account can't open** (other users' profiles, `System Volume Information`…), and anyone allowed on the pipe can search them. It never reads or returns file contents. On a shared PC, only turn it on for accounts that may see all file names. Its snapshots hold the same names, so they live in LocalSystem's own profile (`%SystemRoot%\System32\config\systemprofile\AppData\Local\ShadUI Ripgrep\Index`), readable only by SYSTEM and administrators, and are deleted when it's turned off.
-   **Installing per user.** The app installs per user by default, so the copy of `shadui-index.exe` the UAC prompt runs sits in your own folder, where a program running as you could swap it before you click. Install the app per machine if that matters to you.

### Why our own service

Before writing it, these were evaluated (October 2026):

| Option | What it offers | Why not |
| :--- | :--- | :--- |
| [goz](https://github.com/mustafaahci/goz) (MIT): daemon `gozd` + `goz --json` client over a named pipe | Fast, multi-volume, live journal tailing, sizes and dates via `FSCTL_QUERY_FILE_LAYOUT`, a careful pipe ACL | No folder-size totals, pipe open to every authenticated user, no snapshot (re-enumerates on each start), ~20 ms per query to spawn its CLI or a dependency on its internal wire format, and a three-month-old project to run as LocalSystem |
| [everyfind](https://github.com/kyo5uke/everyfind) (MIT/Apache-2.0): `efd` + `ef` | Snapshots, disk usage from the index | One volume per daemon, no dates, allocated rather than real sizes, terminal-first |
| [UFFS](https://github.com/skyllc-ai/UltraFastFileSearch) (MPL-2.0) | Raw MFT reading, daemon, 40+ filters | Very large, a separate access-broker service, its own clients in mind |
| [usn-journal-rs](https://crates.io/crates/usn-journal-rs) / [ntfs-reader](https://crates.io/crates/ntfs-reader) crates | Journal reading; a full raw-MFT reader | `FSCTL_ENUM_USN_DATA` has no sizes or dates; ntfs-reader keeps the whole MFT in memory (~1 KiB per record, ~3 GB here) or reads it twice |

What the app needed (folder totals, its exact query rules, results streamed with metadata, a pipe only the opting-in user can open, fast restarts, few dependencies in a SYSTEM service) came out simpler as a purpose-built service of about 4,000 lines of Rust (plus 850 of tests) in [`src-tauri/index`](src-tauri/index), with ideas taken from those projects: goz's pipe ACL and server-owner check, everyfind's snapshot and polling tail, Everything's single MFT pass.

### For admins and developers

```text
shadui-index status                      what the running service has indexed
shadui-index search QUERY [--in PATH]... [--max N] [--hidden] [--repeat N] [--print | --long]
shadui-index sizes PATH
shadui-index install [--allow-sid SID]   (administrator) install or update, and allow a user
shadui-index uninstall                   (administrator) remove it with its snapshots
shadui-index run [--volume C:]...        (administrator) run in a console instead, logging to it
shadui-index bench [C:]...               (administrator) index drives in-process and time everything
```

The NSIS installer accepts `/INSTANTSEARCH` to turn it on during a (silent) install. The service logs to `service.log` next to its snapshots.

Tested on a real install: turning it on and off from Settings, indexing, snapshot restarts, live changes, the fallbacks. Not yet exercised: updating a running service in place (Settings offers *Update* when the app is newer than the service; it runs the same `install`), and the installer hooks at install/uninstall time (the installer builds with them). Not indexed: ReFS, FAT and exFAT drives, removable drives, and volumes without a drive letter; fd covers them.

---

## 📥 Installation

### For Users (Noob Friendly)
1.  Go to the [Releases](https://github.com/shadijhade/shadui-ripgrep/releases) page.
2.  Download the latest `.exe` installer for Windows.
3.  Run the installer and you're good to go! ripgrep, fd and dust are bundled with the app, so there's nothing else to install.

### For Developers (Pro)
If you want to build the app from source or contribute, follow these steps:

#### Prerequisites
-   [Node.js](https://nodejs.org/) (v18+)
-   [Rust](https://www.rust-lang.org/) (latest stable)
-   The Windows x64 binaries of ripgrep, fd and dust ship in `src-tauri/binaries/`. On other platforms, add binaries named `rg-<target-triple>`, `fd-<target-triple>` and `dust-<target-triple>` there.

#### Build Instructions
1.  **Clone the repository**:
    ```bash
    git clone https://github.com/shadijhade/shadui-ripgrep.git
    cd shadui-ripgrep
    ```

2.  **Install dependencies**:
    ```bash
    npm install
    ```

3.  **Run in development mode**:
    ```bash
    npm run tauri dev
    ```

4.  **Build for production**:
    ```bash
    npm run tauri build
    ```
    The executable will be located in `src-tauri/target/release/bundle/nsis/`.

#### Checks
```bash
cd src-tauri && cargo test --workspace
node src/lib/explorer.check.ts
node src/lib/text.check.ts
```
The index's tests (MFT record parsing, queries, folder totals under random changes, snapshots, the wire format, and a real named-pipe round trip with its ACL) run without administrator rights.

---

## 🛠️ Tech Stack

-   **Frontend**: [React](https://react.dev/), [Vite](https://vitejs.dev/), [Tailwind CSS](https://tailwindcss.com/)
-   **UI Library**: [Shadcn UI](https://ui.shadcn.com/), [Lucide React](https://lucide.dev/)
-   **Backend**: [Tauri](https://tauri.app/) (Rust), with the [windows](https://github.com/microsoft/windows-rs) crate for Win32 and the shell, and [notify](https://github.com/notify-rs/notify) for folder watching
-   **Instant search**: a Windows service in Rust (`src-tauri/index`, built as `shadui-index.exe` and bundled with the app) reading the NTFS MFT and USN journal, with [rayon](https://github.com/rayon-rs/rayon) and [memchr](https://github.com/BurntSushi/memchr)
-   **State Management**: [Zustand](https://github.com/pmndrs/zustand)
-   **Bundled tools**:

    | Tool | Version | Used for | License |
    | :--- | :--- | :--- | :--- |
    | [ripgrep](https://github.com/BurntSushi/ripgrep) | 15.2.0 | Searching inside files | MIT or Unlicense |
    | [fd](https://github.com/sharkdp/fd) | 10.5.0 | Finding files by name | MIT or Apache-2.0 |
    | [dust](https://github.com/bootandy/dust) | 1.2.6 | Folder sizes | Apache-2.0 |

---

## ⌨️ Keyboard Shortcuts

### Files
| Key | Action |
| :--- | :--- |
| `Enter` | Open the selection |
| `Alt+Left` / `Backspace` · `Alt+Right` · `Alt+Up` | Back · Forward · Up |
| `Ctrl+L` / `Alt+D` / `F4` | Type a path in the address bar |
| `Ctrl+F` / `Ctrl+E` / `F3` | Search: typing filters, `Enter` searches subfolders, `Esc` clears |
| `Arrows`, `Home`, `End`, `PageUp/Down` | Move (`Shift` extends the selection, `Ctrl` moves without selecting) |
| Letters | Jump to the next name starting with them |
| `Ctrl+A` | Select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste (works with File Explorer too) |
| `Ctrl+Shift+C` | Copy path |
| `F2` | Rename |
| `Delete` / `Shift+Delete` | Move to the Recycle Bin / delete permanently |
| `Ctrl+Shift+N` | New folder |
| `Alt+Enter` | Properties |
| `F5` / `Ctrl+R` | Refresh |
| `Ctrl+H` | Show or hide hidden items |
| `Alt+P` | Show or hide the preview pane |
| `Ctrl+Shift+F` | Search text in this folder |

### Text search
| Key | Action |
| :--- | :--- |
| `Enter` | Search (in the search box) / open the selected result |
| `Shift+Enter` | New line in the search box (multi-line search) |
| `Arrow Up/Down` | Move through results |
| `Ctrl+F` / `Ctrl+K` | Focus the search box |
| `Ctrl+H` | Show or hide Replace |
| `Alt+C` / `Alt+W` / `Alt+R` | Toggle match case / whole word / regex |
| `Ctrl+Shift+E` | Back to Files |
| `Esc` | Stop a running search, close menus and dialogs |

`Ctrl+,` opens Settings from anywhere.

---

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details. The bundled tools keep their own licenses (see the table above).
