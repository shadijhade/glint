# ShadUI Ripgrep

**ShadUI Ripgrep** is a blazing fast file explorer and search app for Windows, built with [Tauri](https://tauri.app/). It's a modern UI on top of the fastest Rust command-line tools: [fd](https://github.com/sharkdp/fd) finds files by name, [dust](https://github.com/bootandy/dust) measures folder sizes and [ripgrep](https://github.com/BurntSushi/ripgrep) searches inside files, while folders themselves are read straight from Win32 in Rust. The goal: a file explorer that's orders of magnitude faster than the one Windows ships with.

![App Screenshot](screenshots/screenshot-search-result-dark.png)

## 🚀 Features

### Files: a fast explorer
-   **Instant folders**: Each folder is read with one Win32 enumeration pass (names, sizes, dates and attributes all at once) and sorted in Rust, then drawn as a virtual list. The status bar shows how long it took, e.g. *read in 0.3 ms · shown in 19 ms*.
-   **Find files by name (fd)**: Typing in the search box filters the folder as you type; **Enter** searches every subfolder too, streaming results as fd finds them. All words must match, `*` and `?` work like in Explorer, and `"quotes"` keep spaces.
-   **Folder sizes (dust)**: One click sizes every subfolder, which Explorer never shows. Sort by size to find what's filling your disk.
-   **Preview pane**: Syntax-highlighted code and text, images, video, audio, PDFs, and folder summaries. Online-only OneDrive files are left alone, so nothing downloads by accident.
-   **The usual explorer moves**: Back/forward/up, a breadcrumb address bar you can type into (`%APPDATA%` and `~` work), Quick access folders, pinned folders, drives with free space, sorting, multi-select, type-ahead, rename, new folder, and hidden items on demand.
-   **Windows-native file operations**: Copy, cut, paste, delete and rename go through Windows' own file engine, so progress, "Replace or Skip", the Recycle Bin and permission prompts behave exactly like Explorer. The clipboard is shared with Explorer: copy here, paste there, and the other way round.
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
cd src-tauri && cargo test
node src/lib/explorer.check.ts
node src/lib/text.check.ts
```

---

## 🛠️ Tech Stack

-   **Frontend**: [React](https://react.dev/), [Vite](https://vitejs.dev/), [Tailwind CSS](https://tailwindcss.com/)
-   **UI Library**: [Shadcn UI](https://ui.shadcn.com/), [Lucide React](https://lucide.dev/)
-   **Backend**: [Tauri](https://tauri.app/) (Rust), with the [windows](https://github.com/microsoft/windows-rs) crate for Win32 and the shell, and [notify](https://github.com/notify-rs/notify) for folder watching
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
