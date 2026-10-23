# Archives

Glint opens archives like folders: zip and 7z archives, tar archives (plain, or compressed as `.tar.gz` or `.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst` and `.tar.lz4`), and single compressed files (`.gz`, `.xz`, `.bz2`, `.zst`, `.lz4`). Press `Enter` on one (or double-click it, or middle-click to open it in a new tab), and its folders show in the list or the grid like any others, in either pane and in any tab. Go into its folders, go back up out of it, and use Back, Forward and the address bar as anywhere else: the path goes on into the archive, as in `C:\Downloads\photos.zip\2024\summer`, and the archive shows in the address bar with an archive icon. Type or paste such a path and it opens; tabs in archives come back when Glint starts. An archive inside an archive opens the same way (`backup.zip\old\photos.7z\2019`).

Archives are **read-only** inside. New folder, Cut, Paste, Rename, Delete, Move to other pane and Open in Terminal are greyed out, a *Read-only archive* badge takes Open in Terminal's place in the command bar, and their keys only say why. Nothing can be dropped into an archive. To change what's in one, copy it out first.

## Archives by other names

Many files are zips inside:
-   Word, Excel and PowerPoint documents (`.docx`, `.xlsx`, `.pptx`, and their macro and template kinds), and OpenDocument files;
-   Java and Android packages (`.jar`, `.war`, `.apk`, `.aab`);
-   NuGet packages, VSIX extensions, Python wheels, EPUB books, MSIX and AppX packages and Firefox add-ons;
-   KMZ and 3MF files.

`Enter` still opens them in their app. Right-click **Open as archive** to look inside, read-only, like any zip, or **Extract here** to unpack one into a folder next to it.

Glint goes by a file's first bytes, not its name. A renamed zip (`data.bin`), or a zip inside a zip under another name, opens all the same once a path goes into it: through Open as archive, a path you type or paste, or a tab that comes back. Glint reads the first 4 KB of the file to tell, and only after the path has failed as a folder, so plain folders never pay for it.

## Tar archives and compressed files

-   **A plain `.tar`** keeps no list of its entries: each file has a header just before its data. Glint reads them header by header and skips the data in between, so a tar opens nearly as fast as a zip.
-   **A compressed tar** (`.tar.gz` and the others) has to be decompressed from start to end to find those headers. Glint does that once, the first time it's opened, and then keeps its list of entries in memory like any archive's. When that takes more than a moment, a notice says how far along it is, with **Stop**. Selecting one over 64 MB doesn't read it: the preview pane gives its size and says that opening it reads it whole.
-   **Getting a file out** of a compressed tar means decompressing everything stored before it, as in a solid 7z, so a selection is extracted in one pass, in the order stored.
-   **A single compressed file** opens as a folder holding the one file it unpacks to: `app.log.gz` holds `app.log`. An `.xz` file says how big that is in its index, and `.zst` and `.lz4` files usually do in their headers. A `.gz` or `.bz2` file doesn't, so Glint decompresses it once to find out, with the same progress and Stop. **Extract here** writes the file next to it, as `gzip -d` would, and keeps the compressed one.
-   **What a tar holds.** Files and folders, with long names (GNU tar's, and POSIX's PAX headers), sizes past 8 GB and exact times. A symlink is extracted as a small file holding where it points, as zips' links are, and a hard link as a copy of its target. Sparse files and files split across tar volumes are listed but can't be extracted, and say so. Devices and pipes are left out.
-   **By their bytes here too.** A `.gz` that holds a tar opens as a tar, and a `.tar.gz` that isn't compressed opens as a plain tar.

## What works in an archive

-   **The list and the grid**, with names, uncompressed sizes, dates, and every folder's total (sort by size works). Hidden and system items stay hidden unless you show them, as on disk.
-   **Previews.** Text and code are read straight out of the archive (only the start of the file). Pictures, videos, sound and PDFs are shown from a temp copy of the one file, extracted when you select it. Quick Look (`Space`) the same. An archive previews as a summary: how many files and folders it holds, and how big they are once extracted (a big compressed tar that isn't open yet, only its size).
-   **Thumbnails** in the grid (and the list's small ones) are made from temp copies too. In a solid 7z, where getting at a file means decompressing everything before it in its block, the pictures on the way are kept as well, so one read serves the whole screenful.
-   **Opening a file** (`Enter`, Open with…, Open with your editor) extracts it to a temp copy and opens that. The copy is read-only, so an app that edits it says so instead of saving changes that would go nowhere.
-   **Copying out.** **Copy** (`Ctrl+C`) extracts the selection, folders with what's in them, and puts the copies on the clipboard, to paste here or in File Explorer. **Copy to other pane** extracts the selection and copies it into the other pane's folder. **Dragging** items out extracts them and drags the copies (as copies, never a move). Every copy goes through Windows' own file engine, so progress and *Replace or Skip* are Windows' own, and a big extraction shows its progress and can be stopped. Each copy keeps the date it has in the archive.
-   **Name search** (`Enter` in the search box) searches the archive's folders from its list of entries, with the same rules as everywhere else; the status bar says *zip · 13 found in 10 ms*.
-   **Folder sizes and the disk usage map** come from the archive's own list of entries, at once.
-   **Live.** An archive that changes on disk while it's open (a download finishing, a backup tool rewriting it) is read again.
-   **Extract here** (right-click an archive on disk) extracts all of it, with every check below, straight into a new folder next to it named after it (`photos` for `photos.zip` or `photos.tar.gz`, `photos (2)` if that's taken). It shows its progress and can be stopped; stopped or failed, the folder goes. Files that can't be extracted are left out and named.
-   **Compress to zip file** (right-click files and folders on disk) writes a zip next to them, named after the first one: Deflate, the files' dates, zip64 only for an entry that needs it, junctions and symlinked folders not followed. Progress and stopping work the same; a file that can't be read is left out and named. `Ctrl+Z` takes back either.

## How it's read

A zip keeps a list of its entries at its end, the central directory, and a 7z has one in its header. That list is all Glint reads to show an archive: names, sizes, dates and attributes, with nothing extracted. It's read once, into a tree of folders kept in memory while the archive is in use (the archives used most recently, up to three million entries in all), and every folder in it lists from there in a fraction of a millisecond. The archive is read again when its size or date changes.

-   **zip** is read with [zip](https://github.com/zip-rs/zip2) (zip-rs), the maintained successor of the original crate, which is fuzzed and spots overlapping entries. It reads the central directory a few bytes at a time with a seek between, which takes 150 ms for 50,000 entries straight from the file, so Glint reads the end of the file once and serves those reads from memory: 13 ms. Names in UTF-8 (flagged so or not) are read as such; older ones are in your PC's OEM code page, as File Explorer and 7-Zip read them. Dates come from the NTFS or Unix timestamps when the zip has them, else from its local date and time. Methods: stored, Deflate, Deflate64, BZip2, LZMA, XZ, PPMd, Zstandard (method 93, as WinZip and Python 3.14 write it, and its old number 20), and the old Shrink, Reduce and Implode.
-   **7z** is read with [sevenz-rust2](https://github.com/hasenbanck/sevenz-rust2), the maintained fork of sevenz-rust, hardened against crafted headers. Methods: LZMA, LZMA2, PPMd, BZip2, Deflate, copy, and the BCJ, BCJ2, ARM and Delta filters.
-   Both are pure Rust. zip-rs's Zstandard is C, so Glint decodes Zstandard entries itself with [ruzstd](https://github.com/KillingSpark/zstd-rs), a pure Rust decoder:
    -   frame by frame, skipping skippable frames, and checking each frame's checksum and the entry's CRC32;
    -   with windows of at most 256 MB. zstd's own levels use 8 MB or less, and only `--long` goes past 128 MB.
-   **tar** is read by Glint itself rather than with the [tar](https://github.com/alexcrichton/tar-rs) crate, which reads a GNU long name or a PAX header into memory at whatever size it says it is.
-   **Compressed streams** are decompressed in pure Rust too: gzip by [flate2](https://github.com/rust-lang/flate2-rs) on [zlib-rs](https://github.com/trifectatechfoundation/zlib-rs), xz by [lzma-rust2](https://github.com/hasenbanck/lzma-rust2), bzip2 by [bzip2](https://github.com/trifectatechfoundation/bzip2-rs) on libbz2-rs-sys, Zstandard by ruzstd and LZ4 by [lz4_flex](https://github.com/PSeitz/lz4_flex). These were in the build already for zip-rs, except lz4_flex.
    -   Several gzip members, bzip2 or xz streams, or Zstandard or LZ4 frames one after another (as pigz and `zstd -T` write them) read as one file.
    -   Their checksums are checked. A tar's checksum is the stream's, at its very end, and covers every file in it: the first read of a compressed tar goes on past the tar's end to check it before anything is listed, and a single compressed file is always read to its end.

Names are made safe for Windows once, as the tree is built: characters Windows doesn't allow become `_` (as 7-Zip does), trailing dots and spaces go, device names such as `CON` get a `_` in front, and `.`, `..`, empty parts, leading slashes and drive letters are dropped. What's listed is exactly where an entry would be extracted to. When names clash (ignoring case, as Windows does), the later entry wins, as extracting them in order would, and a folder wins over a file.

## Safety

-   **No zip slip.** Every extracted path is built from those safe names below the folder it's extracted to, and checked once more before anything is written. Links in an archive are extracted as plain files.
-   **Zip bombs.** Each entry is cut off at the size the archive declares, and one that holds more (or less) fails. An entry that would expand more than 1,000 times over what's stored of it is refused once it's bigger than 64 MB (deflate tops out near 1,032 times, so real files don't come close). A zip whose entries share compressed data, the overlapping-entries bomb, lists but doesn't extract. An extraction that wouldn't fit on the drive, with 1 GB to spare, doesn't start. An archive inside an archive opens up to 4 GB. Archives with more than two million entries don't open.
-   **7z headers.** A 7z's list of entries is usually compressed itself. Glint reads how big it says it unpacks to before reading it, and refuses one over 512 MB, so a crafted few-kilobyte header can't take all the memory.
-   **Tar headers.** Every header's checksum must add up. A GNU long name over 64 KB or a PAX header over 1 MB is refused: a crafted tar can say one is gigabytes long.
-   **Streams.** A compressed tar or file read whole (to list it, or to find its size) is stopped as a bomb once what comes out passes 64 MB at more than 1,000 times what's been read. xz and Zstandard streams that need more than 256 MB of memory to decompress are refused.
-   **What happens on its own stays small.** A thumbnail extracts at most 64 MB (counting what comes before the file in a solid 7z block), and a preview at most 1 GB; past that the preview says so, and nothing is extracted until you open or copy the file.
-   **Password-protected entries** show a lock and say they're password-protected; a 7z whose list of files is encrypted too says so when you open it. Glint doesn't ask for passwords yet.
-   **Damaged archives** are an error message, never a crash: every read runs under `catch_unwind`, and the tests flip and cut real archives byte by byte. Once a decompressor reports damage it isn't asked for more (an xz decoder looped forever when it was, on a file with one bad byte).
-   **Nothing writes into an archive.** Windows' own zip folders accept copies and deletes inside a zip; Glint's file operations refuse any path that goes into an archive, whatever asks.

## Where the temp copies go

Everything extracted lands in `%TEMP%\Glint\archives`:

-   `s<process id>-<time>`: this run's copies, for previews, thumbnails, opened files, archives inside archives, and copies to the other pane on their way (deleted as soon as they're copied). A lock file in it, held while Glint runs, keeps another running Glint from deleting it. It's emptied when Glint closes; what another app still has open stays until the next start.
-   `out\<time>-<n>`: copies put on the clipboard or dragged out, which other apps read after Glint has handed them over (a paste can come long after the copy, even after Glint has closed). They're deleted once they're no longer on the clipboard: when Glint next starts, or an hour after they were made, when the next one is.

## How fast

Release build, Intel Core Ultra 7 265K, NVMe SSD, Windows 11, a fresh start of the app for each run (so the archive wasn't in Glint's memory), the preview pane hidden (so nothing read the archive before `Enter`), median of 5 runs, timed by the status bar from `Enter` to the folder on screen:

| Archive | Entries | Opened (on screen) | Of which reading its list of entries |
| :--- | ---: | ---: | ---: |
| A zip of 50,000 files in 4,001 folders | 54,001 | 57 ms | 47 ms |
| The same files in a solid 7z | 54,001 | 60 ms | 52 ms |
| A folder of 500 subfolders in either, after that | 500 | 7.8–8.9 ms | 0.2 ms |

The status bar shows it for every folder in an archive: *zip · 54,001 items read in 49 ms · shown in 57 ms* the first time, *zip · read in 0.2 ms · shown in 8.8 ms* after that. In the grid, a folder of 4 photos inside a zip had its thumbnails 98 ms after opening (extracted, then made by Windows), and from a solid 7z in 279 ms (one read through the block for all of them).

## In the code

-   `src-tauri/src/archive/mod.rs`:
    -   where a path goes into an archive (`locate`), and finding one by its first bytes (`sniff`);
    -   the archives read (`opened`, by path, size and date);
    -   the commands (`archive_extract`, `archive_stage`, `archive_extract_here`, `archive_compress`, `cancel_archive_stage`, `archive_unstage`, and for first reads `archive_progress`, `cancel_archive_read` and `archive_peek`);
    -   what `list_dir`, `resolve_path`, `read_text_preview`, `folder_sizes`, `disk_usage`, `find_files` and `thumbnail` ask of an archive.
-   `read.rs`:
    -   the formats (`Format`: what a name claims, what the first bytes say, the code the UI knows each by);
    -   every format behind one interface (`Source`);
    -   the 7z header check, names and dates.
-   `stream.rs`: tars and single compressed files (`StreamSource`); the first read of a whole stream, with its progress, Stop and bomb check (`Scan`); entries read in one pass.
-   `tar.rs`: the tar header walker, with its limits.
-   `codec.rs`: the codecs by their first bytes, the sizes xz, Zstandard and LZ4 files state, Zstandard frames, fused decoders and the CRC32 check.
-   `tree.rs`: safe names, the folder tree, totals and natural order.
-   `extract.rs`: temp folders and their cleanup, and writing an entry within its declared size.
-   `compress.rs`: writing a zip (Compress to zip file).
-   `tests.rs` and `fixtures/` (made with 7-Zip 26.04: encrypted zips and 7z files, a solid 7z and a zip with non-ASCII names, a zip with names in the OEM code page): listings, nesting, previews, staging, zip slip, bombs, encrypted, damaged and real-world archives; tars in every codec (written in the tests), their links, single compressed files, Extract here, stream bombs and Stop. `cargo test --release -- --ignored --nocapture lists_50000` times a 50,000-entry zip.
-   The UI side:
    -   The UI reads a listing's archive (`Listing.archive` in `src/lib/entries.ts`: where it is in the path, its format, its entry count, how long reading it took) and the `ATTR.inArchive`, `ATTR.encrypted` and `ATTR.unreadable` bits of its entries.
    -   `src/lib/archives.ts` lists the formats by code, which extensions `Enter` opens and which "Open as archive" offers. `src/lib/explorer.ts` has the read-only guards, copying out (`extracting`, `copyOutOfArchive`, `dragFromArchive`) and opening (`openFromArchive`).
