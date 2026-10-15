//! The instant search service (and its installer), bundled next to the app. See the shadui-index
//! crate in index/ for what it does; the app runs it elevated once, to install it, when you turn
//! instant search on.

fn main() {
    #[cfg(windows)]
    std::process::exit(shadui_index::cli::main());
    #[cfg(not(windows))]
    {
        eprintln!("shadui-index reads NTFS drives, so it runs on Windows only.");
        std::process::exit(1);
    }
}
