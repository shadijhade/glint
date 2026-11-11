//! Timings for measuring, printed to stderr among the frame log's lines (`GLINT_FRAME_LOG`, see
//! docs/gpui/VENDOR.md): how long a folder took from the click to the screen, and what each step of it cost.
//! Nothing is formatted or printed unless the variable is set; asking costs one atomic load.

use std::sync::OnceLock;
use std::time::Instant;

/// Whether the log is on (read once).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GLINT_FRAME_LOG").is_some())
}

/// Prints the line `line` makes, when the log is on.
pub(crate) fn say(line: impl FnOnce() -> String) {
    if on() {
        eprintln!("{}", line());
    }
}

/// Milliseconds since `from`.
pub(crate) fn ms(from: Instant) -> f64 {
    from.elapsed().as_secs_f64() * 1000.0
}

/// Times a step from here to the end of the scope it is kept in, and says so if it took a quarter of a
/// millisecond or more (`span name=… ms=…`). Nothing, not even a clock read, when the log is off.
pub(crate) fn span(name: &'static str) -> Span {
    Span(on().then(|| (name, Instant::now())))
}

/// See [`span`].
pub(crate) struct Span(Option<(&'static str, Instant)>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, from)) = self.0 {
            let took = ms(from);
            if took >= 0.25 {
                eprintln!("span name={name} ms={took:.2}");
            }
        }
    }
}
