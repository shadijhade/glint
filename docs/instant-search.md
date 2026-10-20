# Instant search (NTFS index)

Off until you turn it on: on This PC, with the status bar's **Turn on instant search**, or in **Settings → Instant search**. Windows then asks for administrator approval once, and a small service (`Glint instant search`) starts indexing your fixed NTFS drives. From then on:

-   **Name searches** on indexed drives answer from memory: the same query rules as fd (every word must match, `*` and `?` match the whole name, case-insensitive, same exclusions and hidden-file setting), results streamed into the same list with size, date and attributes.
-   **Folder sizes** for every folder come from the index, kept current as files change.
-   **Disk usage maps** of any folder on an indexed drive come from the index in one request: its 20,000 biggest files and folders, six levels down, with their sizes.
-   **Fallbacks are automatic.** fd answers whatever the index can't: drives that aren't indexed (network shares, FAT/exFAT USB sticks, a drive still being read), folders behind junctions, and searches with *Skip what .gitignore lists* on (only fd reads .gitignore). From This PC, indexed drives and the rest are searched together (*index + fd*). dust sizes and maps folders the index doesn't have.

## Measured

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
| Disk-usage map of C:, the 20,000 biggest items six levels down | 19 ms in the service; median 18.9 ms end to end over the pipe; 53 ms in the app from the click to the map on screen (C:\Users, 219 GB: 53 ms). On a synthetic index of 2.86 million records: 9–17 ms in the service, 4 to 32 levels down |
| A file created, grown, renamed, moved, hard-linked or deleted | in the index within ~0.3 s (the journal is checked every 250 ms) |
| The same name search with fd, home folder only (219 GB) | 8.09 s |

Measure your own with `shadui-index bench C:` (administrator) and `shadui-index search kernel32.dll --repeat 21`.

## How it works

-   **One read of the MFT.** The service reads each volume's Master File Table straight off the disk in 8 MB sequential chunks (parsed in parallel while the next chunk is read), the way Everything and WizTree do. One pass gives every name (hard links included), its folder, size, last-write time and attributes; `FSCTL_ENUM_USN_DATA` alone would give names but no sizes or dates.
-   **The USN journal** keeps it current: every 250 ms the service reads new journal records and re-reads each file record they touched through NTFS itself (`FSCTL_GET_NTFS_FILE_RECORD`), so creates, renames, moves, size changes, hard links and deletes converge on what's really on disk however late or often a change is seen. Folder totals are updated along each changed file's folder chain. If the journal wraps while the service isn't running, it reads the MFT again (seconds).
-   **Snapshots** of each volume's index are saved after indexing, every 30 minutes if anything changed, and when the service stops, so a restart is a load plus a short journal replay.
-   **The index** keeps, per volume, a record per MFT entry (size, date, attributes) and a link per name to its parent folder, with names in one UTF-8 arena (a lowercase copy only for names that aren't ASCII). A search is a parallel scan over every name, then a walk up each match's folders for the scope, hidden and exclusion rules.
-   **A disk-usage map** walks the folder's subfolders biggest first. Once it has as many items as the map can hold, anything smaller than the smallest of them can't make the map, so neither it nor anything inside it is looked at; one parallel pass over the names then picks up the files in the folders it opened. The biggest items, with every folder they're in, go back in one binary frame, each folder's items together and largest first. A service from before maps closes the pipe on that request, and the app says the service needs updating while dust measures instead.

## Security model

-   **Nothing runs elevated unless you turn it on.** Turning it on runs the bundled `shadui-index.exe install` behind the UAC prompt; turning it off runs `shadui-index.exe uninstall` the same way. Uninstalling the app removes the service too (one more prompt, only if it was on).
-   **Where it runs.** Reading a raw volume requires administrator rights, so the service runs as LocalSystem, but with every privilege removed except `SeChangeNotify`, `SeBackup` and `SeManageVolume`. It's copied to `C:\Program Files\ShadUI Ripgrep\`, which only administrators can change (a LocalSystem service must never run a file its users could swap), and registered with a quoted path, delayed start, and restart on failure. It only reads volumes; the one change it can make is turning on a drive's change journal if it was off (Windows' default size, 32 MB), as Everything does.
-   **Who can ask it.** It answers on the named pipe `\\.\pipe\shadui-ripgrep-index-v1`, whose ACL lets in only SYSTEM, administrators and the accounts that turned it on, and refuses remote clients. Even those accounts can't create an instance of the pipe, so no other program can pose as the service, and the app also refuses a pipe not owned by SYSTEM or Administrators (a program could otherwise take the name while the service is stopped and feed the app made-up results).
-   **What it exposes.** Like Everything, the index knows the name, size, date and attributes of every file on the indexed drives, **including files your account can't open** (other users' profiles, `System Volume Information`…), and anyone allowed on the pipe can search them or map a folder's disk usage, which returns the same four things for the biggest items below it. It never reads or returns file contents. On a shared PC, only turn it on for accounts that may see all file names. Its snapshots hold the same names, so they live in LocalSystem's own profile (`%SystemRoot%\System32\config\systemprofile\AppData\Local\ShadUI Ripgrep\Index`), readable only by SYSTEM and administrators, and are deleted when it's turned off.
-   **Installing per user.** The app installs per user by default, so the copy of `shadui-index.exe` the UAC prompt runs sits in your own folder, where a program running as you could swap it before you click. Install the app per machine if that matters to you.

## Why our own service

Before writing it, these were evaluated (October 2026):

| Option | What it offers | Why not |
| :--- | :--- | :--- |
| [goz](https://github.com/mustafaahci/goz) (MIT): daemon `gozd` + `goz --json` client over a named pipe | Fast, multi-volume, live journal tailing, sizes and dates via `FSCTL_QUERY_FILE_LAYOUT`, a careful pipe ACL | No folder-size totals, pipe open to every authenticated user, no snapshot (re-enumerates on each start), ~20 ms per query to spawn its CLI or a dependency on its internal wire format, and a three-month-old project to run as LocalSystem |
| [everyfind](https://github.com/kyo5uke/everyfind) (MIT/Apache-2.0): `efd` + `ef` | Snapshots, disk usage from the index | One volume per daemon, no dates, allocated rather than real sizes, terminal-first |
| [UFFS](https://github.com/skyllc-ai/UltraFastFileSearch) (MPL-2.0) | Raw MFT reading, daemon, 40+ filters | Very large, a separate access-broker service, its own clients in mind |
| [usn-journal-rs](https://crates.io/crates/usn-journal-rs) / [ntfs-reader](https://crates.io/crates/ntfs-reader) crates | Journal reading; a full raw-MFT reader | `FSCTL_ENUM_USN_DATA` has no sizes or dates; ntfs-reader keeps the whole MFT in memory (~1 KiB per record, ~3 GB here) or reads it twice |

What the app needed (folder totals, its exact query rules, results streamed with metadata, a pipe only the opting-in user can open, fast restarts, few dependencies in a SYSTEM service) came out simpler as a purpose-built service of about 4,400 lines of Rust (plus 1,050 of tests) in [`src-tauri/index`](../src-tauri/index), with ideas taken from those projects: goz's pipe ACL and server-owner check, everyfind's snapshot and polling tail, Everything's single MFT pass.

## For admins and developers

```text
shadui-index status                      what the running service has indexed
shadui-index search QUERY [--in PATH]... [--max N] [--hidden] [--repeat N] [--print | --long]
shadui-index sizes PATH
shadui-index usage PATH [--depth N] [--max N] [--repeat N]
shadui-index install [--allow-sid SID]   (administrator) install or update, and allow a user
shadui-index uninstall                   (administrator) remove it with its snapshots
shadui-index run [--volume C:]...        (administrator) run in a console instead, logging to it
shadui-index bench [C:]...               (administrator) index drives in-process and time everything
```

The NSIS installer accepts `/INSTANTSEARCH` to turn it on during a (silent) install. The service logs to `service.log` next to its snapshots.

**Updates.** The app and the service share one version (`[workspace.package]` in `src-tauri/Cargo.toml`). When the app updates itself, the installer runs in update mode, so its uninstall hook leaves the service alone and it keeps running the version it had; the app then says *Instant search has an update* once, and Settings shows *Update* until it's done, which runs the same `install` (one administrator approval). See [releasing.md](releasing.md).

Tested on a real install: turning it on and off from Settings, indexing, snapshot restarts, live changes, the fallbacks; and, with a test copy of the app, an in-app update that leaves the running service alone and then offers its update; and updating the running service in place (from the disk-usage map's *Update instant search*: one prompt, then it stopped, was replaced and was ready again from its snapshot in 0.28 s). Not yet exercised: the installer hooks at install/uninstall time (the installer builds with them). Not indexed: ReFS, FAT and exFAT drives, removable drives, and volumes without a drive letter; fd covers them.
