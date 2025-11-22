# ShadUI Ripgrep

**ShadUI Ripgrep** is a blazing fast, modern text search application powered by [Ripgrep](https://github.com/BurntSushi/ripgrep) and built with [Tauri](https://tauri.app/). It combines the raw performance of a command-line tool with a beautiful, responsive user interface designed for both power users and casual developers.

![App Screenshot](https://via.placeholder.com/800x450?text=ShadUI+Ripgrep+Screenshot)

## 🚀 Features

### Core Functionality
-   **Blazing Fast Search**: Leverages the power of `ripgrep` to search through large codebases in milliseconds.
-   **Search & Replace**: Find text and replace it across multiple files with confidence.
-   **Real-time Preview**: Instantly view the file content and context of any search result with syntax highlighting.

### Advanced Search Options
-   **Regex Support**: Use full regular expressions for complex search patterns.
-   **Case Sensitivity**: Toggle case-sensitive or case-insensitive search.
-   **Whole Word**: Restrict matches to whole words only.
-   **Exclusions**: Easily exclude specific files or directories (e.g., `node_modules`, `.git`) from your search.

### Modern User Interface
-   **Beautiful Design**: Built with Shadcn UI and Tailwind CSS for a sleek, modern look.
-   **Dark/Light Mode**: Fully supports system theme syncing, or manually toggle between Dark and Light modes.
-   **Search History**: Quickly access and re-run your previous searches.
-   **Keyboard Navigation**: Navigate results using arrow keys and open files with `Enter`.

### Developer Friendly
-   **Open in Editor**: Open files directly in your default editor or a configured custom editor (e.g., VS Code, Notepad++).
-   **Customizable**: Configure your preferred editor path and default exclusions.

---

## 📥 Installation

### For Users (Noob Friendly)
1.  Go to the [Releases](https://github.com/shadijhade/shadui-ripgrep/releases) page.
2.  Download the latest `.exe` installer for Windows.
3.  Run the installer and you're good to go!

> **Note**: You will need to have `ripgrep` installed on your system.
> To install it, you can run `cargo install ripgrep` or download it from the [official repository](https://github.com/BurntSushi/ripgrep).

### For Developers (Pro)
If you want to build the app from source or contribute, follow these steps:

#### Prerequisites
-   [Node.js](https://nodejs.org/) (v18+)
-   [Rust](https://www.rust-lang.org/) (latest stable)
-   [Ripgrep](https://github.com/BurntSushi/ripgrep) installed and in your PATH.

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

---

## 🛠️ Tech Stack

-   **Frontend**: [React](https://react.dev/), [Vite](https://vitejs.dev/), [Tailwind CSS](https://tailwindcss.com/)
-   **UI Library**: [Shadcn UI](https://ui.shadcn.com/), [Lucide React](https://lucide.dev/)
-   **Backend**: [Tauri](https://tauri.app/) (Rust)
-   **State Management**: [Zustand](https://github.com/pmndrs/zustand)
-   **Search Engine**: [Ripgrep](https://github.com/BurntSushi/ripgrep)

---

## ⌨️ Keyboard Shortcuts

| Key | Action |
| :--- | :--- |
| `Enter` | Search (when in input) / Open File (when result selected) |
| `Arrow Up/Down` | Navigate through search results |
| `Esc` | Close modals (History, Settings) |

---

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
