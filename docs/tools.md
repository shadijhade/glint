# Tools

Glint gets much of its speed and reach from command-line tools, most of them from the Rust world. Three come with it. The rest install when a feature first needs one: that feature shows an **Install** card, and **Settings → Tools** lists every tool with its version, what it does in Glint, where it came from, and **Install**, **Update** or **Remove**.

## Bundled

| Tool | In Glint |
| :--- | :--- |
| [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg.exe`) | Searching inside files (Text mode) |
| [fd](https://github.com/sharkdp/fd) (`fd.exe`) | Finding files by name where instant search doesn't |
| [pdu](https://github.com/KSXGitHub/parallel-disk-usage) (`pdu.exe`) | Folder sizes and disk-usage maps where instant search doesn't |

They're the official Windows x64 releases, unmodified, installed next to `Glint.exe`. If one is missing (a security tool quarantined it, a portable copy), Glint uses one installed on the PC instead, and **Install** puts it back: its official release first, the very build Glint ships, else winget. The installer checks for them once it has copied them, and puts back one that's gone.

## Installed when a feature needs them

| Tool | In Glint | From (in that order) |
| :--- | :--- | :--- |
| [Czkawka](https://github.com/qarmin/czkawka) CLI | Clean up's look-alike pictures, videos and music, broken files and wrong extensions | winget `qarmin.czkawka.cli`, else its release, 12.0.2 |
| [FFmpeg](https://ffmpeg.org/) | Reading videos for Clean up's look-alike videos | winget `Gyan.FFmpeg.Essentials`, else its release, 9.0.2 (with ffprobe) |
| [Difftastic](https://github.com/Wilfred/difftastic) | Compare two files, and a changed file's **Changes**: code compared by its structure | winget `Wilfred.difftastic`, else its release, 0.72.0 |
| [ast-grep](https://github.com/ast-grep/ast-grep) | Code patterns in Text mode (`Alt+A`), found and rewritten by their syntax | winget `ast-grep.ast-grep`, else its release, 0.51.0 |
| [jaq](https://github.com/01mf02/jaq) | jq filters over JSON, YAML and TOML in their preview | its GitHub release, v3.1.1 |
| [Magika](https://github.com/google/magika) | A file's real type in the File X-Ray, told by its content | winget `Google.Magika`, else its release, 1.1.0 |
| [YARA-X](https://github.com/VirusTotal/yara-x) | **Check with YARA rules**, from a rules folder chosen in Settings → Check | winget `VirusTotal.YARA-X`, else its release, 1.22.1 |
| [Kingfisher](https://github.com/mongodb/kingfisher) | **Look for passwords and keys**, and a look before **Compress to zip file** | its GitHub release, v2.14.0 |
| [rimage](https://github.com/vlad-salone/rimage) | **Convert pictures** to JPEG, PNG, WebP or AVIF | its GitHub release, v0.14.0 |
| [onefetch](https://github.com/o2sh/onefetch) | A Git repository's card in its preview | its GitHub release, 3.1.0 |
| [tokei](https://github.com/XAMPPRocky/tokei) | Lines of code by language on that card | winget `XAMPPRocky.Tokei`, else its release, 12.1.2 |
| [zoxide](https://github.com/ajeetdsouza/zoxide) | Folder history shared with your terminals (Settings → History) | winget `ajeetdsouza.zoxide`, else its release, 0.10.0 |
| [just](https://github.com/casey/just) | A justfile's recipes in the **Run** menu | winget `Casey.Just`, else its release, 1.59.0 |
| [uv](https://github.com/astral-sh/uv) | pyproject.toml's scripts in **Run**, and Python scripts in **Scripts** | winget `astral-sh.uv`, else its release, 0.13.6 |
| [Bun](https://bun.sh/) | TypeScript and JavaScript scripts in **Scripts** (Node.js works too, for JavaScript) | winget `Oven-sh.Bun`, else its release, 1.4.4 (the baseline build, for any x64 processor) |
| [Nushell](https://www.nushell.sh/) | A shell for the terminal, and Nushell scripts in **Scripts** | winget `Nushell.Nushell`, else its release, 0.116.1 |
| [sendme](https://github.com/n0-computer/sendme) | **Send to another PC** and **Receive from another PC** | winget `n0-computer.sendme`, else its release, 0.37.1 |
| [dufs](https://github.com/sigoden/dufs) | **Share on this network** | winget `sigoden.Dufs`, else its release, 0.47.0 |

Meaning search's model isn't a program, but it's listed with them: Model2Vec's [potion-base-8M](https://huggingface.co/minishlab/potion-base-8M) (30 MB), downloaded from Hugging Face the first time it's asked for (or from [a copy on Glint's GitHub releases](https://github.com/shadijhade/glint/releases/tag/potion-base-8M-bf8b056) where Hugging Face can't be reached), at a pinned revision and checked against its SHA-256 checksums.

**Or all at once, while installing Glint.** The installer's **Extra tools** page lists them with instant search, grouped by what they're for, each with what it is and what Glint uses it for. The ones you tick install once Glint is in place, the same way as below: Glint installs them (`Glint.exe tools install <id>`), with how each is going on the installer's status line, and the last page says if one didn't. One that's on the PC already is greyed out and left as it is. A silent install picks them with `/TOOLS=difftastic,jaq` (the ids in [`tools.rs`](../src-tauri/src/tools.rs), and `meaning` for the model), and turns on instant search with `/INSTANTSEARCH`.

## Where they go

Every tool installs on every PC, with winget or without it: each has its own release pinned, and Glint tries the next way when one can't (winget isn't on the PC, as on Windows Server, LTSC and some managed PCs, or it fails, or its install was declined).

-   **With winget**, where the PC has it and it has the tool: `winget install --id <id> --exact --source winget --scope user`, silently, with its progress in the card and a **Stop**. That puts the tool in your account's winget folder (`%LOCALAPPDATA%\Microsoft\WinGet`), on your `PATH`, where terminals find it too. A package that can't install per user installs the way it does for everyone, which may ask for administrator approval.
-   **From its GitHub release** (where there's no winget, when winget can't, and for tools that aren't on winget or not at the version Glint is tested with): downloaded over HTTPS from the URL pinned in [`src-tauri/src/tools.rs`](../src-tauri/src/tools.rs), through the PC's proxy if it has one (a download that can't connect, or breaks off, is tried again over IPv4 alone, which gets through on networks whose IPv6 is broken), checked against the SHA-256 pinned beside it (a download that doesn't match is thrown away), and only the program (and anything it needs beside it) taken out, into `%LOCALAPPDATA%\com.shado.shadui-ripgrep\tools\<tool>\`. **Update** in Settings → Tools appears when Glint pins a newer version. `cargo test --lib -- --ignored pinned_releases` downloads and unpacks every pinned release, to check new pins.
-   **Already on the PC**: Glint uses a copy it finds on `PATH`, in winget's links and packages, Scoop's shims, Cargo's `bin` or Chocolatey's `bin`, and installs nothing.

Glint builds each tool's arguments itself, from typed options (the UI never passes flags through), runs it without a shell or a window, reads its JSON where it has one, and stops it with **Stop** or when the tab moves on to something else.

## Removing them

-   **Downloaded by Glint**: **Remove** in Settings → Tools deletes its folder; the model's row has its own **Remove**.
-   **Installed with winget**: Glint leaves those alone, since you may use them outside Glint too. Remove one with `winget uninstall --id <id>`, or in **Settings → Apps → Installed apps**.
-   **Everything Glint keeps**: its uninstaller's *Delete the application data* removes `%LOCALAPPDATA%\com.shado.shadui-ripgrep`: the tools it downloaded, the model, your Scripts folder, the folder rules' log and SFTP host keys you trusted in Glint.
