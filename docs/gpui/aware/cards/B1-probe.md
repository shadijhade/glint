# Card B1 — The two lanes, and the probes of one file

Wave 1 · crate `glint-core` · I/O on worker threads

**You own:** `crates/glint-core/src/aware/pool.rs`, `aware/probe.rs`, `aware/file.rs` (tests at the bottom of each).
**Read first:** PLAN.md §2 (R1, R4, R5), §4, §5.1 (`Probed`, `Part`, `Want`), §5.2. Then the engine functions you wrap (the coordinator made the first four reachable from your module):
- `crate::locks::who_locks_now(paths: Vec<String>) -> Result<LockReport, String>` (`locks.rs`; `LockingProcess.name` is the app's name);
- `crate::xray::zone_info(path: &str) -> Option<ZoneIdentifier>` (`xray.rs`; `zone_id`, `host_url`, `referrer_url`);
- `crate::media::details_now(path: &str) -> Option<MediaDetails>` (`media.rs`; read the struct at line 28 for `width`, `height`, `duration`);
- `crate::recent::link_target(shortcut: &Path) -> Option<String>`;
- `crate::sign::verify(path: &str) -> Signature` (`sign.rs:55`; `state` is `"valid"`, `"unsigned"`, …);
- `crate::ioring::read_many(asks: &[ioring::Ask]) -> Option<Vec<Option<ioring::Got>>>` (`ioring.rs`: read its header);
- `crate::ocr::pdf_pages` (`ocr.rs:56`) for how Windows' PDF reader is called on a thread of its own, and `shell.rs:1311` (`mounts_by_default`) for how `AssocQueryStringW` is called.
**Check with:** `cargo test -p glint-core aware::` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

## pool.rs — two lanes of threads

```rust
pub fn spawn(lane: Lane, job: impl FnOnce() + Send + 'static);
/// Runs `work` inside the lane's pool, so rayon's `scope` and `par_iter` in it use the lane's threads.
pub fn install<R: Send>(lane: Lane, work: impl FnOnce() -> R + Send) -> R;
pub fn threads(lane: Lane) -> usize;
```

- Each lane is a `rayon::ThreadPool` in a `std::sync::OnceLock<Option<rayon::ThreadPool>>`, built the first time it is used. If building fails the lane is `None` for good, and `spawn` and `install` fall back to rayon's global pool (`rayon::spawn`, calling `work()` directly). No `unwrap` or `expect`.
- **Quiet:** `(cores / 2).clamp(2, 8)` threads named `glint-aware-quiet-{i}`. **Now:** `cores.clamp(2, 12)` threads named `glint-aware-now-{i}`. `cores` is `std::thread::available_parallelism()` (4 when it can't say).
- Both lanes' `start_handler` initialises COM for the thread (`CoInitializeEx(None, COINIT_MULTITHREADED)`, result ignored), because shortcuts and the PDF reader need it.
- The quiet lane's `start_handler` also puts the thread in efficiency mode and at background priority. All three calls are in `windows::Win32::System::Threading`; ignore their results (an older Windows may refuse):
  ```rust
  // SAFETY: the handle is this thread's own pseudo-handle, and the struct outlives the call.
  unsafe {
      let thread = GetCurrentThread();
      let _ = SetThreadPriority(thread, THREAD_MODE_BACKGROUND_BEGIN);
      let state = THREAD_POWER_THROTTLING_STATE {
          Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
          ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
          StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
      };
      let _ = SetThreadInformation(thread, ThreadPowerThrottling, (&raw const state).cast(), size_of_val(&state) as u32);
  }
  ```
  If a name above doesn't exist in the `windows` crate version in the tree, find its real name in `~/.cargo/registry` and say so in your report; don't add a feature to `Cargo.toml`.

## probe.rs — `probe(asks, lane, stop, out)`

Returns at once. For every ask and every part its `want` names, one job on the lane (`pool::spawn`): if `stop` is set when it starts, it does nothing; otherwise it works the part out and, if there is an answer, sends `Answer { path, part }` (ignore a send that fails: the window has gone). A part with no answer sends nothing (R1).

- **Pictures' dimensions are read together.** Take the asks that want `media` and whose extension is a picture's (`jpg jpeg jpe jfif png gif bmp webp tif tiff ico avif heic heif psd qoi`) into one job: `ioring::read_many` with one slice `(0, 65_536)` each, then `imagesize::blob_size(&bytes)` on what came back. Where the ring isn't there (`None`: Windows 10), a file couldn't be read, or the size can't be told from the first 64 KB, fall back to `imagesize::size(path)` for that file. The other media asks (videos, songs) are one job each (`file::media`).
- `inside` and `twins` are card B2's functions: `super::inside::inside(&path, lane, &stop, &mut |inside| send(Part::Inside(inside)))` and `super::twins::twins(&path, size, &stop)`. They are there already, doing nothing until B2 lands: call them.

## file.rs — one function per part

Each takes what it needs and returns `Option<…>`; none panics, whatever the path.

| Function | Does |
| :--- | :--- |
| `locked_by(path) -> Option<Vec<String>>` | `who_locks_now(vec![path])`: the processes' names, each once, in order. `Err` is `None`. An empty list is `Some(vec![])`. |
| `origin(path) -> Option<Origin>` | `zone_info(path)`; `None` when the file has no mark. `blocked` is `zone_id >= 3`. `host` is the host of `host_url`, else of `referrer_url`, without a leading `www.` (`url::Url::parse(..)` and `host_str()`), `""` when neither has one. |
| `signer(path) -> Option<Signer>` | `sign::verify(path)`: `"valid"` with a signer's name is `Signed(name)`; `"unsigned"` is `Unsigned`; anything else is `None` (not sure is not said). |
| `media(path) -> Option<Media>` | `details_now(path)`: width, height and seconds, zeros for what it doesn't say; `None` when it says none of the three. |
| `text(path, stop) -> Option<Text>` | Reads the file in 1 MB pieces. A zero byte in the first 8 KB: `Text { lines: 0, binary: true }` and stop reading. Else `lines` is the count of `\n` (`memchr::memchr_iter(b'\n', piece).count()`), plus one when the file isn't empty and doesn't end with `\n`. Checks `stop` between pieces (`None` when stopped). An empty file has 0 lines. |
| `pages(path) -> Option<u32>` | Windows' PDF reader: `StorageFile::GetFileFromPathAsync`, `PdfDocument::LoadFromFileAsync`, `PageCount()`, as `ocr.rs` does. A PDF with a password, or one Windows can't read, is `None`. |
| `target(path) -> Option<String>` | `recent::link_target`. |
| `opens_with(path) -> Option<String>` | `AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, ".ext", …)` into a 260-character buffer. `None` for no extension, an error or an empty name. Kept per extension in a `Mutex<HashMap<String, Option<String>>>` (a `static` with `LazyLock`), so each extension is asked once. |

## Tests

Use a folder of your own under `std::env::temp_dir()` (make it unique with the process id and a counter; remove it at the end).

1. `text`: `"a\nb\n"` has 2 lines; `"a\nb"` has 2; `""` has 0; a 3 MB file of 100-byte lines is counted right across the pieces; a file with a zero byte in its first bytes is `binary`; a stop flag set before the call gives `None`.
2. `origin`: write `"[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://www.example.com/a/file.zip\r\n"` to `<file>:Zone.Identifier` (as `xray.rs`'s own test does, line 437; skip the test with a plain `return` if the write fails, which means the temp drive isn't NTFS): `host == "example.com"`, `blocked`. With `ZoneId=1`: not blocked. A file with no stream: `None`.
3. `locked_by` on a file nobody has open is `Some(vec![])`; on a path that doesn't exist it doesn't panic.
4. `signer` of `C:\Windows\System32\notepad.exe` is `Signed(..)` or `None` (catalog-signed files differ by Windows version: only assert it isn't `Unsigned` and doesn't panic); a text file written by the test is `Unsigned`.
5. `opens_with` of a name with no extension is `None`, and asking twice for `.txt` gives the same answer.
6. `probe`: three small text files asked for `text` on `Lane::Now` send three `Part::Text` answers (collect them with a `Channel::new` that pushes into an `Arc<Mutex<Vec<_>>>`; wait up to 5 s for them); the same with `stop` set beforehand sends none.
7. `probe` with a picture: write a real 1×1 PNG (the 67 bytes of a minimal PNG; build it with the `png` crate, which is a dependency) and ask `media`: the answer says 1 × 1.
8. `pool`: `threads(Lane::Quiet) >= 2`, `threads(Lane::Now) >= threads(Lane::Quiet)`; a job spawned on each lane runs (an `AtomicBool` set, waited for up to 5 s); `install` returns its closure's value.

## Done when

The tests pass, clippy is clean, every `unsafe` block has its `// SAFETY:` line, and nothing in these files waits on the caller's thread.
