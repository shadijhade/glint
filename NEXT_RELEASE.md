# Glint: next release plan

*Part 1 (reviewed and built 2026-11-09) is on master as 373b14e, not released. Part 2 was planned the same day against 373b14e and each tool's current release, because the release should carry more features and fixes first.*

## Part 2 (planned 2026-11-09)

### Build status (part 2, started 2026-11-09)

- **G9 pins**: fd 10.5.2 (bundled binary replaced), ast-grep 0.51.1, YARA-X 1.23.0, Kingfisher 2.17.0, rclone 1.76.0, uv 0.13.7, jaq 3.1.2, onefetch 3.2.0; all eight downloaded and unpacked by `pinned_releases` (which now takes `TOOLS=id,id`). THIRD_PARTY_CRATES.md is left for release prep (cargo-about isn't installed on this PC).
- **New tools** in tools.rs, the installer's Extra tools page (rows 27–32, group 7 Backups) and docs/tools.md: ab-av1 0.11.7, libjxl 0.12.0 (cjxl + djxl), Ghostscript 10.08.0 (its NSIS installer opened by 7-Zip, installed first when missing; Program Files\gs found too), pandoc 3.13, Typst 0.15.1, restic 0.19.1. All six downloaded and unpacked by `pinned_releases`.
- **G1 Make smaller**: shrink.rs + ShrinkDialog.tsx (menu, Ctrl+K, Back to JPEG for .jxl). On real files: a 94 MB 1080p clip → 4.0 MB at VMAF 95 (H.264, 18 s) or 4.3 MB (HEVC); a 3.6 MB JPEG → 953 KB (MozJPEG 80) or 3.2 MB JPEG XL that djxl turns back into the identical file; a 3.2 MB BMP → 102 KB PNG; a 3.7 MB Word file → 405 KB; a 4.7 MB picture PDF → 435 KB (150 dpi). Dates modified kept; nothing bigger kept. HDR videos keep their colours as HEVC/AV1 (10-bit, colour tags passed on) and are refused as H.264.
- **Fix found on the way**: Convert pictures failed ("rimage made nothing") for names with extra dots (`Screenshot 10.15.32.png`): rimage keeps every dot, convert.rs's lookup cut them.
- **G2 Convert documents**: docconvert.rs + DocConvertDialog.tsx (menu, Ctrl+K), .typ previews (TypstView.tsx, cached PDFs) and Export as PDF for .typ. PDFs are two steps (pandoc → Typst source with its pictures in the work folder → `typst compile --root work`), since Typst refuses pandoc's absolute picture paths. Mostly-Arabic or Hebrew documents get `lang`/`dir=rtl` (checked: the PDF reads right to left). Word, HTML and Markdown (with an Arabic name) into PDF, Word, Markdown, HTML and EPUB all pass.
- **G1 + G2 in the real app** (test copy, its own commands over CDP): 15 of 15 checks passed, Ghostscript installed for real through its installer and 7-Zip; both dialogs checked on screen from Ctrl+K.
- **G4 books**: foliate-js vendored at commit 78914ae (src/vendor/foliate, MIT, SOURCE.md says what changed: zip.js from `@zip.js/zip.js` 2.24.0, PDF branch dropped); BookView.tsx in the preview for EPUB, MOBI, AZW3, FB2, FBZ, CBZ (loaded only when one is shown): contents, text size, dark pages, the place remembered, Read aloud with local voices only. CBR not done.
- **G5**: songtags.rs (lofty: title, artist, album, album artist, track, total, year, genre, cover; copies take the songs' places, so Undo restores them; tested writing and clearing on a WAV with an Arabic artist) + SongDialog.tsx (one song or many; titles and tracks from file names; number the tracks); Media's **Even volume** (two-pass loudnorm, linear; a −48 LUFS recording came out at −16.0); a waveform under the audio player (decoded in the page, click to seek).
- **G6**: SmallPreviews.tsx (lazy): vCard and iCalendar cards; .eml (postal-mime 4.1.4) and Outlook .msg (@kenjiuno/msgreader 1.29.0) with a sandboxed frame whose policy blocks everything from the internet, inline pictures kept, attachments opened through attachments.rs (saved to temp with the internet zone's mark); Krita/OpenRaster/Procreate/Sketch pictures (zip.js); Graphviz (@hpcc-js/wasm-graphviz 1.30.0; Word 97 .dot templates told apart); Mermaid files.
- **G3 backups**: backup.rs (restic; plans in the data folder's backups.json, one repository "Glint backup" per chosen place, password in Credential Manager under Glint/backup/<place>, `cat config` exit codes 10/12 tell new from wrong-password; backup with JSON progress, skipped open files listed, exit 3 still a backup; then `forget --prune` 7/4/12 and a monthly `check`; Open/Compare/Restore/Restore a copy through `restore <snapshot>:<folder>` with `--include`, since `dump` writes to stdout; daily/weekly runs from a scheduler thread while Glint runs, with a notice) + BackupDialog.tsx (a drive or folder, or a cloud from rclone's remotes) and a "Glint's backups" section in Previous versions. Tested for real with restic 0.19.1: set up, two backups (the second adding only the changed file), versions, Open of a file and a folder with an Arabic name, a wrong password refused. Restore through the UI not tried yet.
- **G7**: scanned PDFs into the content index (ocr::pdf_pages: pages drawn by Windows' PDF renderer, read by Windows OCR, up to 50 pages and 200 MB, only when the PDF has no text and pictures are on; tested on a JPEG-only PDF); files dropped on a paired PC in the sidebar are sent to it (Place's `dropAs="device:<id>"`, "Send to <PC>" on the drag). E15 (GPU/NPU) not done.
- **Part 2 in the real app** (fresh test build): song details read, written from the name and read back (Arabic artist); Even volume; a backup set up, run twice (3 new, then 1 changed), two versions, a restored copy of a file and of a folder, Open from a backup, a wrong password refused; an attachment saved with the internet zone's mark: 13 of 14 (the miss was the harness reading Tauri's progress channel raw). On screen: EPUB and CBZ in the reader, contacts, an invite, an email (inline picture shown, the tracking picture not loaded, the attachment listed), an OpenRaster picture, Graphviz, Mermaid, the song's waveform with its new details, and the Back up, Song details and Even volume dialogs. After the lazy row lookup, applying a 100,000-file listing went 33 → 13 ms: that folder 82 → 68 ms on screen (README: 114), WinSxS 51 → 48 (73), System32 12.
- **Checks**: cargo test 312 passed (16 ignored real-file ones run separately: shrinks_real_files, converts_real_documents, backs_up_and_restores_for_real, sort_speed_on_real_folders, pinned_releases), clippy -D warnings clean, both .check.ts, vite build. README's next-version list and docs/tools.md updated; Arabic added for the new menu items. Left for release prep: THIRD_PARTY_CRATES.md (cargo-about), the README benchmark table, and the hardware checks.
- **G8 speed, measured**: sort_entries moves small records with a 32-byte key head and makes keys on several threads for big folders: WinSxS's sort 8.5 → 3.1 ms, 100,000 files 6.8 → 6.1 ms (moving the entries is the rest), identical order (property test at 25,000 names). The page's profile (marks list:start/arrived/decoded/apply/shown, kept for the bench) showed the path → row map, built on every listing, cost 20–30 ms per 100,000 rows (V8 flattening freshly joined path strings); it's now worked out only when a row is looked up. Before that fix: 100,000 files 82 ms on screen (README: 114), WinSxS 51 (73), System32 12. S3 (lazy dialogs) and S4 (thumbnails) not done: thumbnails already run in-process on up to 8 threads through Windows' photo provider, which also fills Windows' cache, so a faster decoder would have to bypass it.

### The idea

Part 1 made Glint **do things to files**: media, PDFs, data, metadata, disk images, encryption, phones. Part 2 finishes the jobs people still leave File Explorer for: **make files smaller**, **turn one kind of document into another**, **keep safe copies**, **read books**, **fix song details**. It also makes a 100,000-file folder open nearly as fast as a small one, and clears what's owed before a release: tool pins, notices, and the checks only real hardware can do.

Part 1's seven rules still apply: no terminal, installed on first use, in-process when small, nothing leaves the PC unasked, undoable, one engine per job, measured.

What Glint already does was checked first, so nothing here repeats it. Sandbox, file locks, batch rename with photo and song details, checksums, resizing and converting pictures, replace in files, folder compare and sync, the terminal, Clean up (Czkawka), Magika, Kingfisher, YARA-X, zoxide and meaning search are all in. There are no open GitHub issues.

### Today's bench (this PC, release build of 373b14e)

| Measure | In Rust | On screen (README) |
| :--- | ---: | ---: |
| A folder of 100,000 files: enumerate / sort / encode | 31 / 14 / 1.7 ms = 40 ms (3.9 MB sent) | 114 ms |
| `C:\Windows\WinSxS`, 25,228 items | 11.6 / 11.7 / 1.3 ms = 23 ms | 73 ms |
| Typo search `kernal32`, 3 drives, through the installed 0.9.0 service | 100 ms | |
| Start: main 864 KB + vendor 281 KB of script, with about two dozen dialogs inside (a quarter of the page's source) | | launch to first folder 441 ms |

Big folders spend about two-thirds of their time in the page, decoding and drawing. WinSxS's sort is half of its time in Rust. Typo search won't feel part 1's E16 until the service updates.

### Core scope (recommended)

#### G1. Make smaller

One **Make smaller** for any selection, from the menu and `Ctrl+K`. It lists each file with the engine it will use and the saving (estimated from samples for videos, real as each one finishes), then saves beside the originals or replaces them (originals to the Recycle Bin, and Undo brings them back).

- **Videos: looks the same, smaller.** [ab-av1](https://github.com/alexheretic/ab-av1) 0.11.7 (MIT, one `ab-av1.exe`) encodes short samples to find the lowest quality setting whose score stays above a target, then encodes the whole video with FFmpeg. The score is XPSNR, a filter built into every FFmpeg since 7.1 (so Gyan's essentials build works), or VMAF where the build has libvmaf. Progress comes as JSON lines (`--stdout-format json`). To check at build time: how it drives NVENC, AMF and Quick Sync, which Media already finds.
- **Photos.** *Same JPEG, smaller*: MozJPEG through rimage (already a tool), never bigger, details kept. *JPEG XL, lossless*: `cjxl` from [libjxl](https://github.com/libjxl/libjxl) 0.12.0 (BSD-3-Clause) repacks a JPEG about 20% smaller, and **Back to JPEG** (`djxl`) gives back the very same file. Windows 11 opens JPEG XL (Microsoft's JPEG XL extension, and Photos), and Chrome decodes it by default from version 155.
- **PNGs**: OxiPNG through rimage, lossless.
- **PDFs full of pictures**: [Ghostscript](https://www.ghostscript.com) 10.08.0 rewrites them with pictures at 150 dpi (*for screens and mail*) or 300 dpi (*for print*). pdfcpu's optimize stays the lossless choice. Ghostscript is AGPL-3.0, so it runs as a separate program downloaded from Artifex's own release, in the Job Object with `-dSAFER`. Its installer is NSIS; to check at build time whether 7-Zip unpacks it (nothing in it runs), as Glint does with 7-Zip's own installer. pdfcpu can't do this job: its `images update` only swaps a picture for one of the same size, and not one with a mask.
- **Word, PowerPoint and Excel files**: their pictures recompressed (PowerPoint's *Compress Pictures*, for all three, many files at once), by Glint's zip code and rimage.
- **Folders**: the existing Compress, offered in the same dialog.

#### G2. Convert documents

[pandoc](https://pandoc.org) 3.13 (GPL-2.0-or-later, run as a separate program; winget `JohnMacFarlane.Pandoc`), with [Typst](https://typst.app) 0.15.1 (Apache-2.0; winget `Typst.Typst`) making its PDFs.

- Right-click → **Convert document**: Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF, reStructuredText, Org and Jupyter notebooks into one another, to **PDF** without LaTeX or Word, and Markdown to **PowerPoint**.
- pandoc runs with `--sandbox`, so it reads only the files it's given and nothing from the network. Typst compiles pandoc's output; a document that imports Typst packages is refused, since those download (rule 4).
- The dialog says what's kept (text, headings, lists, tables, pictures, footnotes, links) and what isn't (Word's exact page layout). An exact-look PDF of a Word file stays Word's or Windows' printing job.
- `.typ` files get a preview and **Export as PDF**. This moves up from part 1's *If there's room*, since Typst comes anyway.

#### G3. Back up with restic

[restic](https://restic.net) 0.19.1 (BSD-2-Clause; winget `restic.restic`).

- Right-click a folder → **Back up…** to a USB drive, a network folder, or any cloud in Remote (restic's `rclone:` backend, pointed at Glint's rclone with `-o rclone.program`). Encrypted and deduplicated, each backup adding only what changed. The password goes into Windows Credential Manager, with a reminder to keep a copy elsewhere too: without it, the backup can't be read.
- Backups appear in the Previous versions panel beside shadow copies, with the same **Open**, **Compare**, **Restore** and **Restore a copy** (`snapshots`, `ls` and `dump` with `--json`, `restore --include`).
- They can run on a schedule through folder rules (daily or weekly), keeping 7 daily, 4 weekly and 12 monthly backups (`forget --prune`), with a monthly `check`. The folder shows when it was last backed up.
- Not in the first version: files an app holds open (Outlook's PST) need a shadow copy and administrator rights (`--use-fs-snapshot`), so they're listed as skipped. Part 1's verdict stands: restic, not rustic, while rustic calls itself beta.

#### G4. Books and comics

[foliate-js](https://github.com/johnfactotum/foliate-js) (MIT, plain JavaScript, bundled and loaded only when needed).

- EPUB, Kindle's MOBI and AZW3, FB2, and comics (CBZ; CBR through Glint's RAR reader) in the preview and a reading view. It has pages or scrolling, the contents, search, text size and dark pages, and remembers the page you stopped at. Glint's content index already reads EPUB text.
- Windows hasn't had a built-in e-book reader since the old Edge, which read EPUB, was replaced in 2020. Books with DRM can't open, and the preview says so.
- **Read aloud** with Windows' own voices, through the page's speech API. Only voices on this PC are offered; Edge's online voices are left out (rule 4).

#### G5. Song details and sound

No new tools: [lofty](https://github.com/Serial-ATA/lofty-rs), already used to read tags, and FFmpeg.

- **Edit song details**: title, artist, album, track, year, genre and cover art, for MP3, FLAC, M4A, Ogg, Opus and WAV. Edit one song or many (fill in from file names, number the tracks), with Undo. ExifTool only reads MP3 tags; lofty writes them.
- **Even out the volume**: two-pass EBU R128 (`loudnorm`) in Media, for podcasts, lectures, and music gathered from different sources.
- The audio preview gets a **waveform**, and the trim handles that videos got in part 1.

#### G6. Previews people run into

Small readers, no new tools:

- Contacts (`.vcf`) and calendar invites (`.ics`) as cards.
- Emails (`.eml`, and Outlook's `.msg` read from its compound file), with their attachments listed and openable.
- The pictures that Krita (`.kra`), OpenRaster (`.ora`), Procreate and Sketch files keep inside them.
- Graphviz (`.dot`, `.gv`) drawn in the page (@hpcc-js/wasm-graphviz, Apache-2.0), and Mermaid files (`.mmd`) through the Markdown preview's Mermaid.

#### G7. What part 1 left

- **Scanned PDFs searchable**: for PDFs with no text in them, pages are drawn by Windows' PDF renderer (as thumbs.rs does) and read by Windows OCR into the content index, in the background below normal priority.
- **Drop onto a device** in the sidebar. Send to <PC> is there; the drop target isn't.
- **E15, AI on the GPU and NPU**, with photos batched (part 1's text below). It's what makes *Remove background* and searching for moments in videos practical later.

#### G8. Speed

- **S1. The first screen first.** Rust already sorts, so it can send the rows that fit on screen in a small first message and the rest after. The page draws them at once, decodes the rest into typed arrays, and makes row objects only for rows on screen, instead of 100,000 objects up front. Target: 100,000 files on screen in about 50 ms (114 now), WinSxS in about 35 (73 now).
- **S2. Sorting.** Each name's sort key (case-folded, numbers compared as numbers) is worked out once, with a parallel sort above about 10,000 items. Target: WinSxS's sort from 11.7 ms to under 4.
- **S3. A lighter start.** The dialogs load when first opened (`React.lazy`, fetched while the app is idle), not with the main script. Launch to first folder is measured before and after, and only a measured gain is claimed.
- **S4. Thumbnails of new photos** (242 ms for the 30 on screen, about 220 a second): Windows' cache stays first, then JPEGs are decoded at reduced size by WIC on a pool of threads, and thumbnails are sent without BMP's uncompressed bulk. Target: twice as fast, on the README's 1,200 photos.
- **S5. Ship part 1's E16.** Typo search falls from ~100 ms to ~10–25 ms only once the index service updates, so the release carries the new service and its self-update gets tried for real (G9).

#### G9. Fixes and finish

- **With you, on real hardware**: part 1's F17 list (the service updating itself under the SCM, Previous versions through it, two PCs paired, the MCP confirm dialogs clicked through), plus LocalSend with a real phone both ways and Send to my PC between two PCs.
- **Tool pins behind their latest release**: fd 10.5.0 → 10.5.2 (bundled), ast-grep 0.51.1, YARA-X 1.23.0, Kingfisher 2.17.0, rclone 1.76.0, uv 0.13.7, jaq 3.1.2, onefetch 3.2.0; then `pinned_releases` to check them.
- **Before the release**: third-party notices regenerated for the new crates (age, hyper and their dependencies) and three.js, and the README's benchmark table re-measured with the bench.

### If there's room

Part 1's list still stands (fq, Remove background, SigLIP 2, phones over MTP, editing zips in place, Paste as link, the history slider, osv-scanner, mergiraf, lazygit and scc, writable remotes), minus Typst, which is now in G2. New to it:

| Feature | Engine | Why it isn't core |
| :--- | :--- | :--- |
| **Glint as a viewer**: pictures, videos and PDFs opened from the desktop or another app straight into Quick Look | Glint (file associations, with the tray process already running) | Needs Default apps registration in the installer and the Store package, and a window of its own |
| **Save as audio** for text, Markdown and e-books | Windows' voices | Read aloud (G4) covers most of it |
| **Picture to SVG** (logos, scanned drawings) | [vtracer](https://github.com/visioncortex/vtracer) (MIT) | A small audience |

### Still to decide: local AI actions

Part 1's bet, refreshed. llama.cpp's daily builds (b12224, 2026-11-09) include Windows CPU, Vulkan, CUDA, OpenVINO, SYCL and ROCm builds. New: if **Ollama** or **LM Studio** is already on the PC (both serve an OpenAI-style API on the PC itself), Glint uses it and its models instead of downloading another 1–3 GB. The rest is unchanged: Summarize, Ask (answers grounded in the content index), Suggest names and Translate, with a small Apache-2.0 model chosen against an Arabic test set first.

### Cut or later

| Idea | Verdict | Why |
| :--- | :--- | :--- |
| yt-dlp | Cut | Downloading from video sites breaks most of their terms, and puts the Store listing at risk |
| LibreOffice for Office → PDF | Cut | About 350 MB; pandoc and Typst cover structure, and Word or Windows' printing covers the exact look |
| Tesseract, OCRmyPDF | Cut | Tesseract has no official Windows build and OCRmyPDF is a Python stack; Windows OCR already does the reading |
| Syncthing | Cut | Folder rules (rclone) and devices (iroh) already sync |
| jpegli | Later | It left libjxl in 0.12 for google/jpegli, which has no releases yet; MozJPEG covers the job now |
| Kokoro or Piper voices | Later | Windows' voices read aloud, Arabic included, with nothing to download |

### Suggested order

1. G9's pins and notices; the hardware checks whenever two PCs and a phone are at hand.
2. G1 Make smaller and G2 Convert documents: the most visible, on proven tools.
3. G8 speed (S1–S4), measured before and after.
4. G4 books, G5 songs, G6 previews.
5. G3 backups: the most new surface, and backups need the most testing (real USB drives, a cloud, restores done for real).
6. G7.

### Decisions for you

1. **Scope**: part 2's core as written?
2. **Backups (G3)**: in this release, or a release of their own?
3. **Local AI actions**: this release, the next, or not at all?
4. **Version** (still open from part 1): 1.0 once the hardware checks pass, or 0.10?

### Facts checked (2026-11-09)

- [pandoc 3.13](https://github.com/jgm/pandoc/releases/tag/3.13), 2026-10-26, Windows zip and MSI; `--sandbox` limits reading to the files given ([MANUAL](https://github.com/jgm/pandoc/blob/main/MANUAL.txt)); Typst has been a `--pdf-engine` since pandoc 3.1.2 ([release](https://newreleases.io/project/github/jgm/pandoc/release/3.1.2))
- [Typst 0.15.1](https://github.com/typst/typst/releases/tag/v0.15.1), 2026-07-17, Windows x64 zip
- [restic 0.19.1](https://restic.net/blog/2026-07-05/restic-0.19.1-released/), 2026-07-05, `restic_0.19.1_windows_amd64.zip`; `-o rclone.program` ([docs](https://restic.readthedocs.io/en/stable/030_preparing_a_new_repo.html))
- [ab-av1 0.11.7](https://github.com/alexheretic/ab-av1/releases/tag/v0.11.7), 2026-08-21, MIT, `ab-av1.exe`; XPSNR since 0.9.0, JSON lines since 0.11.5
- [libjxl 0.12.0](https://github.com/libjxl/libjxl/releases/tag/v0.12.0), 2026-07-01, `jxl-x64-windows-static.zip`; jpegli moved to google/jpegli
- [Chrome ships JPEG XL decoding from Chrome 155](https://developer.chrome.com/blog/jpeg-xl-in-chrome) (2026-10-06); Windows 11 opens JPEG XL with Microsoft's extension ([The Register](https://www.theregister.com/2026/01/14/google_rekindles_relationship_with_jilted/))
- [Ghostscript 10.08.0](https://github.com/ArtifexSoftware/ghostpdl-downloads/releases/tag/gs10080), 2026-09-08, `gs10080w64.exe`
- [pdfcpu `images update`](https://pdfcpu.io/images/images_update): same width and height only, no soft mask or alpha
- [foliate-js](https://github.com/johnfactotum/foliate-js): MIT; EPUB, MOBI, KF8 (AZW3), FB2, CBZ
- [llama.cpp b12224](https://github.com/ggml-org/llama.cpp/releases/tag/b12224), 2026-11-09: Windows CPU, Vulkan, CUDA, OpenVINO, SYCL and ROCm builds
- This PC's FFmpeg (Gyan 9.0.2 full) has `xpsnr`, `libvmaf`, `loudnorm`, `showwavespic`, `whisper` and SVT-AV1. whisper.cpp's pin (b5454) is v1.9.5's build, so it's current.
- Pins against the latest releases: each repository's GitHub releases, 2026-11-09

---

## Part 1 (built 2026-11-09, not released)

### Build status (2026-11-09)

Pushed to master as 373b14e (not released):

- **E12 bench harness**: `Glint.exe bench [--copy N] [--runs N] [folders]` (bench.rs) and `npm run bench [-- --start]` (scripts/bench.mjs, results in bench-results/, compared run to run; `--start` launches Glint and times launch → first folder painted).
- **A1 media studio**: media_tools.rs + MediaDialog/MediaTools: trim (copied or exact), convert and shrink (H.264/HEVC/AV1, hardware encoders found once a session), sound alone, GIF/WebP, subtitles (whisper.cpp), and a playable copy for videos WebView2 can't play.
- **A2 PDF tools**: pdf_tools.rs + PdfDialog (pdfcpu 0.16.4, long flags, `--offline`, passwords passed by file).
- **A3 DuckDB 2.0.0**: duck.rs (Query in place, Profile tab, Query together, Convert data), sandboxed by `allowed_paths`, `enable_external_access=false`, `lock_configuration`, and `query()`.
- **A4 Remove personal info**: metadata.rs + MetadataDialog (ExifTool 13.61 from SourceForge; Office files cleaned in Rust; PDFs rewritten by pdfcpu after).
- **A5 disk images**: archive/image.rs (7-Zip 26.06, unpacked from its own installer by Glint's 7z reader; Program Files' 7-Zip found too).
- **A6 Encrypt**: vault.rs (age 0.12.1, passphrase or age1 keys; folders as .tar.zst.age).
- **B7**: Group by (list view, header menu and Ctrl+K; groups.ts), the Columns view (Ctrl+Shift+9: the folder above, this one, what's in the focused folder; ← and →), and live counts on saved searches.
- **B8**: notebooks, 3D models (three.js 0.187, loaded on demand), Markdown → PDF (print.html + print.rs, WebView2 PrintToPdf).
- **C9**: LocalSend (localsend.rs, Nearby dialog: scan by multicast and subnet, send, receive while open) and Send to my PC (iroh Push in devices.rs).
- **C10**: six read-only MCP tools (the MCP process now finds Glint's data folder itself).
- **D11**: OCR text of pictures in the content index (Settings switch, on by default). *Image-only PDFs not OCRed yet.*
- **E13**: the copy queue copies files under 1 MB eight at a time (conflicts asked first). **E14**: the last session's folders read at start (startup.rs).
- **E16 typo search** (index crate, query.rs): before any word layout or edit table, a name is ruled out if it lacks more of the word's characters than the typos allowed (a branch-free 64-bit mask) or has no word starting with the word's first or second letter. On 3.4M real names from C:\: 44–106 ms → 8–24 ms per typo search (as typed: 3–7 ms); on the crate's 3M synthetic names: 22–29 → 4–15 ms. Every name's answer is checked identical with and without the checks (`real_names_close_matches`, NAMES=file), plus a 200,000-case random property test. Reaches users when the service updates.
- *Not built this round*: E15 (Windows ML / DirectML for photo AI).

Verified on the real app (a test copy with its own identifier, driven over CDP): 44 of 45 end-to-end checks passed at first (trim, convert, shrink to size, sound, GIF, a playable copy of an AVI; PDF merge, split, keep, encrypt, wrong password, decrypt, stamp; DuckDB query, profile, join, convert, and its refusals of writes and other files; metadata read, scrub, date shift; age encrypt/decrypt, wrong passphrase, a folder; Markdown → PDF; 7-Zip listing an installer; Nearby scan; index counts). The one failure, 7-Zip unpacked from its installer, is fixed (the signature also appears in the installer's own code) and passed on the rebuilt copy. Group by and Columns checked on screen; the MCP tools answered over stdio. 289 Rust tests, clippy -D warnings, both .check.ts and the frontend build pass.

#### What the bench says (this PC, release build)

| Measure | Result |
| :--- | :--- |
| System32 (5,097 items) in Rust: enumerate / sort / encode | 1.5 / 0.8 / 0.1 ms, 2.3 ms in all |
| Index name search, 3 drives | 5–7 ms |
| Index typo search, 3 drives (ranked), through the installed 0.9.0 service | 82–98 ms (in-process on 3.4M real names after E16: 8–24 ms) |
| 5,000 small files copied one at a time / 16 at a time | 1.86 s / 0.95 s |
| Launch to first folder painted (test copy) | 441 ms; 445 vs 437 ms with and without the start-up read-ahead |

So: the README's 12 ms for System32 is ~2 ms of Rust and ~10 ms of IPC, decoding and drawing in the page, which makes the draft's 3.1 (NtQueryDirectoryFileEx, arena, u64 sort keys) worth at most ~1–2 ms; the page side is where folder time goes. Typo search, at ~80–100 ms end to end, was the clearest engine target: done (E16 above). The copy pool halves small-file copies. The start-up read-ahead saves nothing for a normal folder (the listing is ~2 ms against ~440 ms of WebView2 starting); it can only help a first folder that's slow to read (a big or network one).

### The idea

0.9.5 was about **finding and seeing** files: photo, speech and content search, previous versions, devices. This release is about **doing things to files without leaving Glint**: trim a video, merge PDFs, query a 5 GB Parquet file, strip a photo's location, open an ISO, send to a phone. Each is one right-click away, backed by the best open-source engine for the job, installed on first use. Alongside it, speed work aimed at the waits people actually notice.

Every new feature follows these rules (the draft's five, sharpened):

1. **No terminal.** The feature lives where the file is: its menu, the preview, Quick Look, `Ctrl+K`. Arguments are built in Rust from typed options ([tools.rs](src-tauri/src/tools.rs)), never from raw UI text.
2. **Installed on first use** through tools.rs (winget, else the pinned GitHub release checked by SHA-256), listed in Settings → Tools and on the installer's Extra tools page.
3. **In-process when it's small and pure Rust** (age, resvg, like zip, 7z and tantivy today); a separate program when it's big, not Rust, or a crash would hurt (FFmpeg, DuckDB, 7-Zip), run in the Job Object ([proc.rs](src-tauri/src/proc.rs)) with limits.
4. **Nothing leaves the PC unless the user asks.** A tool that goes online by default (DuckDB's extension autoload, osv-scanner's API, Typst's package downloads, fetching SSH keys from GitHub) runs offline, or asks first each time it would go online.
5. **Undoable.** Anything that writes puts its result next to the original or recycles the original, as Convert pictures does, and Undo takes it back as one step.
6. **One engine per job.** No second tool for what an installed one already does (DuckDB's `SUMMARIZE` instead of qsv).
7. **Measured.** Every speed claim comes from the bench harness (E12), and the README's benchmark table is re-measured with it.

### Where things stand

- **24 tools in tools.rs today:** ripgrep, fd and pdu (bundled); Czkawka, Difftastic, FFmpeg, Magika, c2patool, whisper.cpp, YARA-X, Kingfisher, ast-grep, jaq, zoxide, rimage, onefetch, tokei, just, uv, Bun, sendme, dufs, rclone and Nushell.
- **Still untried for real from 0.9.5:** Previous versions through the real service, devices between two real PCs, the MCP confirm dialogs clicked through, the service updating itself under the SCM, ReFS indexing (no ReFS volume here), odr.exe registration (not on this Windows build).
- **The draft's code facts hold:** [fsx.rs](src-tauri/src/fsx.rs) lists with `FindFirstFileExW` + `FIND_FIRST_EX_LARGE_FETCH`; [shared.rs](src-tauri/src/shared.rs) creates a shared buffer per post over 256 KB; [ioring.rs](src-tauri/src/ioring.rs) creates a ring per call (used by dedup.rs and media.rs); [sql.rs](src-tauri/src/sql.rs) caps loads at 256 MB and 1M rows; [photos.rs](src-tauri/src/photos.rs) runs CLIP on the CPU one picture per run; [thumbs.rs](src-tauri/src/thumbs.rs) sends BMPs; [mcp.rs](src-tauri/src/mcp.rs) has 9 tools.
- **Not there yet:** Group by, a columns (Miller) view, phones and cameras (MTP), video conversion ([convert.rs](src-tauri/src/convert.rs) is pictures only), creating links, exporting Markdown, 3D model or notebook previews, pushing files to a paired device, OCR text in the content index. The copy queue ([queue.rs](src-tauri/src/queue.rs)) runs one worker, file by file, with `CopyFileEx`.

### Core scope (recommended)

#### A. Do it in Glint

**A1. Media studio.** FFmpeg and whisper.cpp, both already tools.

- **Trim:** handles on the preview's and Quick Look's scrubber. **Save trim** cuts without re-encoding (`-c copy`, at keyframes, well under a second) or frame-exact (re-encoding only the cut).
- **Convert and shrink…:** "fit under 10 / 25 / 50 MB" works out the bitrate from the length; hardware encoders when the PC has them (NVENC, AMF and Quick Sync are all in Gyan's essentials build), x264 otherwise. Extract audio (MP3, M4A, Opus, FLAC). A clip to GIF (palettegen) or animated WebP.
- **Every video plays in Quick Look:** what WebView2 can't play (AVI, WMV, FLV, MPEG-TS, MKV with codecs it lacks) is remuxed on the fly to fragmented MP4 (`-c copy`), and transcoded only when the codec itself won't play.
- **Make subtitles:** a video's speech as an `.srt` beside it (whisper-cli `-osrt`), in its own language or translated to English (`-tr`).
- Where: a new `media_tools.rs` beside convert.rs; [scrub.rs](src-tauri/src/scrub.rs) (Media Foundation frames) gives the trim bar its pictures. Gyan's essentials build has no SVT-AV1 or dav1d (full build only), so AV1 encoding is hardware-only unless a full build is found.

**A2. PDF tools.** [pdfcpu](https://github.com/pdfcpu/pdfcpu) (Apache-2.0, Go, one exe; v0.16.2 checked).

- Merge (in selection order, drag to reorder), split by ranges or into single pages, extract, rotate or delete pages, optimize (bytes saved shown), extract images, add or remove passwords, text and image stamps.
- Results land beside the originals and Undo recycles them. pdfcpu's JSON output drives the dialogs; page thumbnails come from Windows' PDF renderer, which thumbs.rs already uses.

**A3. Big data files.** [DuckDB 2.0 LTS](https://duckdb.org/2026/10/21/announcing-duckdb-200) (MIT, released 2026-10-21).

- Once installed, **Query** reads CSV, TSV, JSON(L) and Parquet in place at any size, so the 256 MB / 1M-row caps go away for them (workbooks keep calamine → SQLite).
- **Query together:** several selected data files as tables named after their stems, joins across them.
- **Profile** tab: `SUMMARIZE` gives each column's type, min and max, approximate distinct count, mean, spread, quartiles and nulls, plus `histogram()` bars. This replaces the draft's qsv.
- **Convert data…:** CSV ⇄ Parquet (zstd) ⇄ JSONL with `COPY … TO`, optionally partitioned by a column.
- Safety to match sql.rs's read-only authorizer: `-readonly`, `autoinstall_known_extensions=false`, `autoload_known_extensions=false`, `enable_external_access=false` with `allowed_directories` set to the files' folders, then `lock_configuration=true` (check the names against the 2.0 docs). It runs in the Job Object with today's 10-second and memory limits.

**A4. Remove personal metadata.** [ExifTool](https://exiftool.org/) (winget `OliverBetz.ExifTool`).

- Photos, videos, PDFs and Office files: it shows exactly which tags go (GPS, serial numbers, owner, software, edit history) and keeps orientation and color profile; the result goes beside the original, or in place with Undo.
- Edit Date taken, title, artist and album, and a PDF's title and author, from the preview's details.

**A5. Disk images and installers open like folders.** 7-Zip 26.x (LGPL-2.1 with the unRAR restriction; 26.05 reported 2026-10-21).

- ISO, IMG, VHD/VHDX, VMDK, QCOW2, DMG, WIM/ESD, ext4, APFS, HFS and squashfs images, NSIS installers, DEB, RPM, CPIO, CHM: browse, preview, copy out. See what an installer holds without running it.
- Glint's own readers stay first for zip, 7z, RAR, cab, tar and msi (faster, already hardened); 7-Zip only takes what they can't read, through `l -slt` listings and `x` of the chosen entries, in the Job Object.
- 7-Zip fixes archive-parsing vulnerabilities often (26.03 and 26.04 this year), so its pin has to follow its releases.
- Install route to settle: winget's `7zip.7zip` installs for all users (UAC). The official x64 installer is a 7z self-extractor, which Glint's own 7z reader may be able to unpack.

**A6. Encrypt with age.** The [`age`](https://github.com/str4d/rage) crate, in-process (MIT/Apache-2.0, from rage's repository).

- Right-click → **Encrypt…** with a passphrase or an SSH public key (a file, or a GitHub user's published keys, after asking); folders become `.tar.zst.age`.
- `.age` files open like archives after the passphrase or private key, and work with age and rage on any OS. The passphrase can be kept in Windows Credential Manager ([credentials.rs](src-tauri/src/credentials.rs)).
- Replaces the draft's rage.exe: same format, nothing to download.

#### B. Explorer and previews

**B7. Group by, a Columns view, live smart folders** (no new tools).

- Group by Kind, Date (Today, Yesterday, Earlier this week…), Size, Type, Git status or Tag: collapsible, each group with its total, in the list, the grid and search results.
- Columns view (Miller): parent, current and child folders side by side with the preview last; the arrow keys move between them. `Ctrl+Shift+1/3/4` are still free and mean icon sizes in File Explorer, so they should pick the grid's tile sizes; Columns gets a key of its own.
- Saved searches show a live count and open as virtual folders, kept fresh by the index.

**B8. Previews File Explorer lost or never had** (no new tools).

- **3D models** (STL, OBJ, GLB/glTF, 3MF, PLY) drawn in the page with three.js (MIT, bundled, loaded only when needed). Microsoft deprecated 3D Viewer and [pulled it from the Store on 2026-07-01](https://learn.microsoft.com/windows/whats-new/deprecated-features).
- **Jupyter notebooks** (.ipynb) with their outputs: text, pictures and Markdown cells (comrak); nothing in them runs.
- **Export as PDF** for Markdown, through WebView2's own PrintToPdf in a hidden window: exactly what the preview shows, math and Mermaid included. No Typst or pandoc needed for this.

#### C. Connected

**C9. Phones and computers nearby.** The [LocalSend protocol](https://github.com/localsend/protocol), built in.

- Glint shows up in the LocalSend apps (Android, iOS, macOS, Linux, Windows) on the same network, and they show up in Glint: send a selection to a phone, receive into Downloads with a toast. No account, no internet, HTTPS between devices.
- Built from the open spec. The existing Rust crates (`localsend` by LocalSend's author, last updated 2025-01; `localsend-rs`, 2026-01) are references, not dependencies, until one proves complete. Needs the same firewall handling as Share on this network.
- **Drop to device** for paired PCs (iroh, [devices.rs](src-tauri/src/devices.rs)): drag onto a device in the sidebar, or **Send to <PC>**, into its Inbox folder with a toast there. Today devices only browse, read-only.

**C10. More for AI agents** (mcp.rs, no new tools). Read-only `search_content` (tantivy), `search_photos` (`shows:`), `search_speech` (`says:`), `search_code` (ast-grep), `query_data` (sql.rs, or DuckDB when installed) and `compare_files` (Difftastic), each with the same folder scope and limits as today's tools.

#### D. Search

**D11. Words in screenshots and scans.** [ocr.rs](src-tauri/src/ocr.rs) (Windows OCR) feeds the content index ([content.rs](src-tauri/src/content.rs)): pictures and image-only PDFs in indexed folders become searchable by the text in them, read in the background below normal priority. Today OCR runs only for Copy text and Documents search's *Images too*.

#### E. Speed you can feel

**E12. A bench harness first.** `npm run bench` (or a Rust bench binary) replays the README's table on this PC (folder open, name search, folder sizes, thumbnails, archive open, copies) plus cold start, from fresh starts, and writes JSON to compare runs. Each item below states its before and after from it.

**E13. Copying many small files.** The copy queue is one worker copying file by file. A pool of workers for small files (and unbuffered I/O for big ones) is where Glint can beat File Explorer by a margin people feel. Compare with Explorer and `robocopy /MT` on 50,000 small files.

**E14. Cold start.** Read the last folder while WebView2 starts and hand it over when the page mounts (the draft's 3.7). The target comes from measuring launch to first folder painted; the draft's "under 40 ms" can't be met while WebView2's own processes take longer than that to start.

**E15. AI on GPU and NPU.** [Windows ML](https://github.com/microsoft/WindowsML) (generally available since 2025-09; on Windows 11 24H2 and later, Windows downloads and updates the NPU and GPU execution providers) for `shows:` and meaning search, with a DirectML build of ONNX Runtime as the fallback for older Windows, and pictures batched (the draft's 16 per run; today it's 1). This corrects the draft: [DirectML](https://learn.microsoft.com/windows/ai/directml) is in sustained engineering, and ONNX Runtime's DirectML provider uses DirectML 1.15.2, a redistributable rather than Windows' own copy, so it's an extra download, not "zero". Speech stays on the CPU: whisper.cpp's official Windows releases have no Vulkan build to pin.

**E16. Engine work where the harness points.** The draft's 3.1 (`NtQueryDirectoryFileEx` into one arena, `u64` sort prefixes), 3.2 (a persistent shared-buffer ring) and 3.4 (64-bit name masks in the index for fuzzy search), each only once the profile shows its stage is a real share of a wait. A difference under one frame (~16 ms) isn't visible, so System32's 12 ms isn't a target; 100,000 files (114 ms) and fuzzy search (about 30 ms in the service) are.

#### F. Finish what 0.9.5 started

**F17.** Before release, run the untried list for real: Previous versions through the updated service, two real PCs paired, the MCP confirm dialogs clicked through, the service updating itself under the SCM, ReFS once a volume is available, odr.exe once Windows ships it.

### If there's room (next in line)

| Feature | Engine | Why it isn't core |
| :--- | :--- | :--- |
| **Structure** tab for binary formats (MP4, PNG, ELF, Wasm, PCAP, Protobuf…), the hex view highlighting each field's bytes | [fq](https://github.com/wader/fq) v0.18.0 (MIT, Go) | A narrower audience; the hex sync is real UI work |
| `.typ` previews and PDF export | [Typst](https://typst.app/docs/changelog/0.15.1/) 0.15.1 (Apache-2.0, winget `Typst.Typst`) | Small audience; previews must block package downloads (rule 4) |
| **Remove background**, in batches, to transparent PNGs | BiRefNet-lite (MIT, 115 MB fp16 ONNX) on the existing ONNX Runtime | A big model download; Photos already does single pictures |
| `shows:` in Arabic and other languages, and moments inside videos | SigLIP 2 (Apache-2.0, multilingual Gemma tokenizer) on the same runtime; video frames from scrub.rs | Needs a re-index and model testing |
| Phones and cameras (MTP) as places | Windows Portable Devices / the shell namespace | Not a path-based file system: a new kind of location, like remotes |
| Edit zip archives in place (paste, delete, rename inside) | the zip crate's raw copy of untouched entries | 7z can't do the same (solid blocks), so zip only at first |
| **Paste as link** (hard link, symbolic link, junction) | Win32 | Small, for power users |
| File history slider in the Changes tab, a structural diff at each commit | gix (already used) + Difftastic | UI work; developers only |
| Dependency vulnerabilities on the repository card | [osv-scanner](https://google.github.io/osv-scanner/usage/offline-mode) `--offline` with its downloaded databases | Only offline mode fits rule 4, and the database download needs a consent step |
| Resolve Git conflicts by structure; lazygit in the terminal pane; scc for code stats | mergiraf (GPL-3.0, run as a separate program; winget `Mergiraf.Mergiraf`), lazygit (MIT), scc (MIT) | Developer niceties. scc would replace tokei, which is stuck at 12.1.2 because its later releases ship no programs |
| Remotes writable from File Explorer and other apps | Cloud Files write-back in [ondemand.rs](src-tauri/src/ondemand.rs) | Needs change detection, upload, in-sync states and conflict handling; the data-loss risk calls for a release of its own |

On writable remotes, the draft named the dehydrate callback, which is about freeing space. Edits arrive as writes to hydrated placeholders that Glint must notice, upload and then mark in sync (`CfSetInSyncState`); renames and deletes come as `NOTIFY_RENAME` and `NOTIFY_DELETE` callbacks.

### A bet to decide on: local AI actions

Moved to part 2's *Still to decide: local AI actions*, with what's changed since.

### Cut, replaced or later

| Draft item | Verdict | Why |
| :--- | :--- | :--- |
| qsv | Cut | DuckDB's `SUMMARIZE` and `COPY` cover profiling, de-duplicating and splitting: one install, not two |
| rage.exe | Replaced | The `age` crate in-process: same format, no download |
| resvg.exe | Replaced | The resvg crate in-process for SVG → PNG and multi-size ICO |
| capa | Later | A large bundled Python program whose analysis takes a while per file; YARA-X, Magika, Defender and the PE preview cover most of what people need. If it comes, it's a button in the File X-Ray, never automatic |
| rustic snapshots | Later | Its README still calls it beta and not for production backups (0.11.3 checked), and backups can't be the experimental part. When this comes, restic (the same repository format, mature) is the safer engine |
| git-cliff, Jujutsu | Later | Small audiences |
| Persistent IoRing (3.3) | Cut unless measured | ioring.rs already batches with IoRing; keeping a ring alive saves microseconds per call, nowhere near the 2.5–4× claimed |
| 8 KB shared-memory threshold (3.2) | Measure | Below some size the main-thread hop costs more than JSON; the harness finds the crossover |
| `ImageBitmap` worker for thumbnails (3.6) | Measure | First find where the 71 ms goes: shell lookups, decoding or painting |
| whisper.cpp on the GPU | Later | No official Windows Vulkan build to pin |

### Suggested order

1. Bench harness (E12) and the 0.9.5 finish list (F17): every later claim is then honest, and the debt is cleared.
2. Media studio (A1) and PDF tools (A2): the most visible additions, on proven tools.
3. DuckDB (A3), metadata (A4), 7-Zip (A5), age (A6).
4. Group by and Columns (B7), previews (B8), OCR in the index (D11).
5. LocalSend and Drop to device (C9), MCP (C10).
6. Speed items E13–E16 as the harness directs; then re-measure the README's table.

### Decisions for you

Answered 2026-11-09: the core scope was built ("Yes, start building"). Local AI actions and the version are still open, in part 2's decisions.

### Facts checked (2026-11-09)

- DuckDB 2.0.0 LTS released 2026-10-21, CLI available at launch: [announcement](https://duckdb.org/2026/10/21/announcing-duckdb-200)
- Windows ML generally available since 2025-09, Windows-managed execution providers, hardware ones on 24H2+: [microsoft/WindowsML](https://github.com/microsoft/WindowsML); DirectML in sustained engineering: [DirectML docs](https://learn.microsoft.com/windows/ai/directml); ONNX Runtime's DirectML provider on DirectML 1.15.2: [ORT docs](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)
- osv-scanner's `--offline` sends nothing once its databases are downloaded: [offline mode](https://google.github.io/osv-scanner/usage/offline-mode)
- LocalSend protocol spec: [localsend/protocol](https://github.com/localsend/protocol); Rust crates: [localsend](https://lib.rs/crates/localsend), [localsend-rs](https://github.com/CrossCopy/localsend-rs)
- 7-Zip 26.05 (2026-10-21) reported, not yet confirmed on 7-zip.org: [Impress Watch](https://forest.watch.impress.co.jp/docs/news/2150176.html)
- Typst 0.15.1 (2026-07-17): [changelog](https://typst.app/docs/changelog/0.15.1/); fq v0.18.0: [releases](https://github.com/wader/fq/releases); pdfcpu v0.16.2: [release notes](https://newreleases.io/project/github/pdfcpu/pdfcpu/release/v0.16.2)
- rustic 0.11.3, README still "beta": [release](https://newreleases.io/project/github/rustic-rs/rustic/release/v0.11.3)
- Gyan's FFmpeg 9.0.2 (2026-09-19): NVENC, AMF and libvpl in every build, SVT-AV1 and dav1d in the full build only: [gyan.dev](https://www.gyan.dev/ffmpeg/builds/)
- 3D Viewer deprecated, removed from the Store 2026-07-01: [deprecated features](https://learn.microsoft.com/windows/whats-new/deprecated-features)
- SigLIP 2 uses a multilingual (Gemma) tokenizer: [paper](https://arxiv.org/abs/2502.14786); BiRefNet-lite ONNX is MIT, 224 MB fp32 / 115 MB fp16: [onnx-community](https://huggingface.co/onnx-community/BiRefNet_lite-ONNX)
- Foundry Local's CLI is under Microsoft's license terms (the SDK is MIT): [winget listing](https://winget.ragerworks.com/package/Microsoft.FoundryLocal)
- llama.cpp's official releases include a Windows x64 Vulkan build: [b9196 notes](https://knightli.com/en/2026/05/18/llama-cpp-windows-cuda-vulkan-gguf/)
