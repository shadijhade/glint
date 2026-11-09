//! What panicked, kept in `panics.log` with Glint's data: when, in which thread, the message and where
//! in the code, so a crash can be told about (Settings → About → Copy diagnostics adds the last one).
//! Nothing is sent anywhere. Glint carries on after some (a damaged archive's decoder, caught), and
//! those are kept too.
//! ponytail: no backtrace, as release builds carry no symbols; add `debug = "line-tables-only"` to the
//! release profile and `std::backtrace::Backtrace::force_capture()` here if reports need more.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Past this, the log moves to panics.old.log and starts again.
const MAX_LOG: u64 = 256 << 10;

static LOG: OnceLock<PathBuf> = OnceLock::new();

/// From now on, each panic is also written to `panics.log` in `data` (Glint's data folder), after
/// what Rust itself prints.
pub fn install(data: &Path) {
    if LOG.set(data.join("panics.log")).is_err() {
        return;
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let _ = write(info);
    }));
}

fn write(info: &std::panic::PanicHookInfo) -> std::io::Result<()> {
    let path = LOG.get().ok_or(std::io::ErrorKind::NotFound)?;
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(path, path.with_file_name("panics.old.log"));
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let thread = std::thread::current();
    writeln!(file, "--- {when} · Glint {} · thread {}\n{info}", env!("CARGO_PKG_VERSION"), thread.name().unwrap_or("unnamed"))
}

/// The last panic written, for diagnostics; None when there's none.
pub fn last() -> Option<String> {
    let text = std::fs::read_to_string(LOG.get()?).ok()?;
    let at = text.rfind("--- ")?;
    Some(text[at..].chars().take(4000).collect::<String>().trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_panicked() {
        let dir = std::env::temp_dir().join(format!("glint-panics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        install(&dir);
        let _ = std::thread::Builder::new().name("crasher".into()).spawn(|| panic!("on purpose")).unwrap().join();
        // Other tests' panics may land in it too: the whole log is looked at.
        let log = std::fs::read_to_string(dir.join("panics.log")).unwrap();
        assert!(log.contains("thread crasher") && log.contains("on purpose") && log.contains("crash.rs"), "{log}");
        assert!(last().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
