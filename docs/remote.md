# Remote connections

Glint opens Google Drive, OneDrive, Dropbox and 60 more clouds ([through rclone](#clouds-through-rclone)), an S3-compatible bucket, a WebDAV server or an SFTP server as a folder of its own: under **Remote** in the sidebar, in a tab like any other, at a path of the form `remote:<id>\folder\file`. It's read-only there: Glint looks, previews, opens and copies out, and never changes anything on the remote. A folder of yours can be kept in sync with a cloud by a [folder rule](#keeping-a-folder-in-sync).

A **Windows share** (a NAS or another PC, over SMB) is the exception: it opens at its own path, read and write, through Windows itself ([below](#a-windows-share-smb)).

## Adding one

**Add a connection** (under **Remote** in the sidebar, or in `Ctrl+K`) starts at **A cloud** ([below](#clouds-through-rclone)); for the other kinds it asks for what Glint needs to reach it. **Test** tries it as it's typed and says what's at its top, or why it didn't connect; **Add and open** keeps it and opens it.

| Kind | Address | Its secret |
| :--- | :--- | :--- |
| Amazon S3 | empty, with the bucket's region (`eu-west-1`) | the secret access key of the access key ID |
| Cloudflare R2 | `https://<account id>.r2.cloudflarestorage.com`, region `auto` | an R2 API token's secret access key |
| Backblaze B2 | `https://s3.<region>.backblazeb2.com` (the bucket's endpoint) | the application key, its key ID as the access key ID |
| MinIO, Wasabi, others | the service's S3 endpoint | as the service gives it |
| A public bucket | as above | none: leave the access key ID empty |
| WebDAV (Nextcloud, ownCloud, a NAS, dufs) | the WebDAV URL, such as `https://cloud.example.com/remote.php/dav/files/you/` | the password (an app password, where the service has them) |
| SFTP (a NAS, a server you reach with SSH) | the server's name or address (a `Host` from `~/.ssh/config` will do), and its port | a password or a key's passphrase, or nothing: ssh then signs in with your SSH agent or your keys |
| Windows share (SMB: a NAS, another PC) | the share, or a folder in it: `\\192.168.1.6\share` (`smb://192.168.1.6/share` will do), with a user name | its password, or nothing: Windows then signs in as you |

**Folder in it** opens the connection at a folder below its top, such as `photos/2024` (for SFTP, below the folder it signs in to, or from `/` when it starts with one). Each connection gets an id made from its first name (`photos-backup`), which stays when it's renamed, so tabs and pinned folders keep working.

The connection itself is kept in Glint's settings without its secret. The secret access key or password is kept by Windows Credential Manager, as a generic credential named `Glint/remote/<id>` (it's in Control Panel → Credential Manager → Windows Credentials), and it goes when the connection is removed. **Remove the connection** (its place's right-click menu, or the connection's dialog) also sends the tabs showing it to This PC.

## Browsing

A folder on a remote lists what's directly in it: for S3, the objects and the "folders" its keys make with `/` (ListObjectsV2, a thousand at a time, up to 200,000 in a folder); for WebDAV, a `PROPFIND` of depth 1; for SFTP, `ls -la` (its dates are to the minute). Names come from the remote, so each is shown as Windows would take it (a key like `2024-07-14T10:30:05.log` shows as `2024-07-14T10_30_05.log`, and `notes.txt` beside `Notes.txt` as `notes (2).txt`), while Glint asks the remote for it by its own name.

Nothing watches a remote: `F5` reads the folder again.

## Previews, opening and copying out

- **Previews** of text read only as far as they show (a range of the file); a picture, a video, a PDF or anything else that's previewed is downloaded first, up to 100 MB, and kept for the rest of the session.
- **Open** downloads the file (with how far it is, and Stop) into Glint's temp folder, read-only, and opens that copy: what's changed in it doesn't go back to the remote.
- **Copy** (`Ctrl+C`, then paste anywhere), **Copy to the other pane** and dragging out download first, folders with everything in them, then hand the copies to Windows' file engine.
- **Thumbnails** of pictures in the grid are off unless the connection's **Thumbnails of pictures** is on: each one shown is then downloaded (up to 16 MB), which a paid bucket charges for.

What's downloaded goes where files extracted from archives go (in Glint's temp folder), and goes as they do: a copy handed to another app once it's an hour old and off the clipboard, the rest when Glint closes or at its next start.

## What doesn't happen there

Nothing is renamed, deleted, moved, pasted, made or uploaded on a remote. Folder sizes, disk usage, search inside files, Git, the terminal, scripts, File X-Ray, Defender and the other tools leave remotes out, as they do archives. An archive on a remote is a file there: copy it out to look inside.

## How Glint talks to them

Glint's own clients do it, over the HTTPS (rustls) that Glint already has for its updates: S3 requests are signed with AWS Signature Version 4 (or sent unsigned, for a public bucket), and WebDAV ones carry basic authentication. Amazon's own endpoints are addressed with the bucket in the host name; every other service's with the bucket in the path, which all of them take. A connection gives up after 15 seconds without an answer, and a download after 60 seconds without data.

WebDAV servers that ask for digest authentication aren't supported yet.

## SFTP

SFTP goes through Windows' own OpenSSH client (`sftp.exe`, which Windows 10 and 11 have; if yours doesn't, add **OpenSSH Client** in Settings → System → Optional features). So it signs in as `ssh` does for you: `~/.ssh/config` applies (`Host` names, `ProxyJump`, `IdentityFile`), and so do your SSH agent and your keys. A password, or a key's passphrase, is kept in Credential Manager like the other secrets, and reaches ssh only when ssh asks for it: ssh runs Glint itself as its `SSH_ASKPASS`, which answers a password or passphrase prompt and nothing else. It's never on a command line or in a file.

Host keys are checked against `~/.ssh/known_hosts` and Glint's own list (`known_hosts` in its data folder); ssh isn't allowed to change either. A server neither knows isn't connected to: **Test** in its connection shows its key's fingerprint, and **Trust it and test again** adds it to Glint's list. Whoever runs the server can tell you whether the fingerprint is its (`ssh-keygen -lf` on the server's key). A key that changes after that is refused.

Windows' OpenSSH can't share one connection between runs, so each listing, preview or download signs in again (well under a second on a network nearby); copying a folder out downloads all its files in one run. A text preview stops downloading once there's enough of the file to show.

## A Windows share (SMB)

**Windows share (SMB)** in **Add a connection** takes a share's address, a name for it, a user name and a password. Glint signs in to the share with Windows' own SMB client, the one File Explorer uses, and then the share is a folder like any on this PC: at its own path (`\\192.168.1.6\share`), read and write, with copying, moving, renaming, deleting, previews, thumbnails, search and every app's Open. It sits in the sidebar with the drives, below the Recycle Bin, by its name, and its tooltip shows its path; its right-click menu has **Edit the connection…** and **Remove the connection…**.

The sign-in has no drive letter and no place in File Explorer, and Windows doesn't restore it at the next logon: Glint signs in again the first time it lists, opens or changes something there in a session. The user name is in Glint's settings; the password is in Credential Manager like the other secrets (`Glint/remote/<id>`). With neither, Windows signs in as you, or with what it already keeps for that server. A NAS's own account can be written `NAS\name`. Removing the connection signs out of it, and so does changing its user name or password, so the next use signs in as it says now.

### When Windows won't open a share

Glint carries on where Windows' SMB client stops, on every Windows version, without changing Windows' settings. These apply to any `\\server\share` Glint opens, pinned, typed or a connection's:

-   **Sign in right there.** A share that wants a user name and password (one with no connection yet, or whose password Glint keeps no longer works) opens on a page that says so, with **Sign in…**. That's the connection dialog, made for the share or the one it has: saved, the share opens, and Glint signs in by itself from then on. A share that knows your account but doesn't let it in offers **Sign in as someone else…**; a locked, turned-off or expired account is said as such.
-   **A refused password isn't tried again** on every folder Glint opens: Glint waits until the user name or password changes, since a NAS may lock an account after a few wrong ones.
-   **Another account on the same server.** Windows lets a session reach a server with one account at a time, so a share File Explorer (or a mapped drive) already reached as someone else used to open as that account. Windows tells servers apart by the name they're reached by, so Glint now signs in through an address the server's name has (`\\nas\photos` is opened as `\\192.168.1.6\photos`) and opens the share there, as the connection's account. A connection is for its share by any name of its server.
-   **Read-only through rclone.** A server that lets in guests only (refused by Windows 10 and 11 Pro and Enterprise, and by every edition since Windows 11 24H2), one that doesn't sign what it sends (required since 24H2), or one Windows already reaches as someone else under every name, offers **Open read-only**: the share's connection then goes through rclone's own SMB client, at `remote:<id>` like the other remotes, read-only. A guest-only share opens at once; otherwise the connection dialog asks for the account, and installs rclone when it isn't there (**Read-only, through rclone** in the dialog switches it either way). One that refuses rclone's sign-in offers **Sign in again…** as a share opened through Windows does. Signing in with an account on the NAS, or turning on SMB signing there, keeps the share read and write.
-   **A sleeping NAS.** Once a share has opened, Glint notes its server's address and, on this PC's network, its network card (in `shares.json` in Glint's data folder). When the server later doesn't answer, Glint says so within about a second (a tab, a sidebar row or a path typed in the address bar), rather than after Windows' wait of half a minute or more, offers **Wake** (a Wake-on-LAN packet) and opens the folder as soon as the server answers, checking every few seconds for ten minutes.

## Clouds through rclone

[rclone](https://rclone.org) reaches more than 60 storage services: Google Drive, OneDrive, Dropbox, Box, pCloud, MEGA, Proton Drive, iCloud Drive, Google Photos, Backblaze B2, Azure, Google Cloud Storage, any S3-compatible storage, SFTP, FTP, SMB, WebDAV and more. It's optional: **Settings → Clouds** (or Tools) installs it, and so does the installer's **Extra tools** page; an rclone already on the PATH is used. Then **Add a connection** starts at **A cloud**:

-   **Already in rclone** lists the remotes in rclone's configuration that aren't in the sidebar yet: one click tries one and adds it.
-   **Or connect another**: pick a service, give it a name, fill in what it asks for first (a server's address, an account's user name and password…), then **Sign in** or **Connect**. Glint asks rclone's own setup questions one at a time, as `rclone config` would in a terminal. A service you sign in to in a browser (Google Drive, OneDrive, Dropbox, Box…) opens its own sign-in page in your browser: you sign in there and allow rclone, and Glint carries on by itself. Glint never sees that password.
-   Google is retiring the app rclone signs in to Google Drive with: **My own app (client ID)** takes yours ([how to make one](https://rclone.org/drive/#making-your-own-client-id)).

A remote is kept in rclone's own configuration file (Settings → Clouds says where, as `rclone config file` does), with its sign-in or password the way rclone keeps them (a password obscured): rclone in your terminals has the same remotes, and Glint's settings only have the remote's name. Stopping half-way takes the half-made remote out again. An encrypted configuration is unlocked with its password, asked for once each time Glint runs. **Remove the connection** asks whether to sign out as well (the remote and its sign-in leave rclone's configuration); Settings → Clouds lists every remote, puts one in the sidebar, or signs out of it.

A cloud opens read-only, like the other remotes: Glint lists folders, previews (a text file only as far as it shows), opens and copies out, and changes nothing there. Where the service says how full it is, the sidebar shows it, as it does a drive's.

**How Glint talks to rclone.** One `rclone rcd`, rclone's remote control, runs while Glint needs it: listening only on 127.0.0.1, on a port of its own, with a password made for that run, in a Windows job that ends it with Glint. Listings are its `operations/list`, a file's bytes come from its `--rc-serve` (ranges and all), and setting a remote up is `config/create` and `config/update` in non-interactive mode. A browser sign-in is `rclone authorize` with `--auth-no-open-browser`: Glint opens the page it gives and answers rclone's question with what it returns.

### Keeping a folder in sync

A folder rule whose **Then** is **Keep it in sync with a cloud** keeps its folder in sync with a folder on one of your clouds:

| How | What runs | What it does |
| :--- | :--- | :--- |
| Copy there | `rclone copy` | What's new or changed here goes to the cloud. Nothing there is deleted. |
| Mirror there | `rclone sync` | The cloud's folder becomes what's here: what's deleted here is deleted there. |
| Both ways | `rclone bisync` | Changes on either side go to the other. Where a file changed on both, the newer wins and the other is kept as `name.conflict1`. Its first run (or one after its filters change) makes the record later runs compare with (`--resync`, the newer of each file winning). |

The rule's names, sizes, age and subfolders become rclone's filters (`--include`, `--min-size`, `--min-age`, `--max-depth 1`). It runs a minute after Glint starts, half a minute after files in the folder stop changing (what its own run changes doesn't count), and as often as asked, from every 15 minutes to once a day: changes made on the cloud come in then. **What would it do?** is rclone's dry run: each file it would copy up or down, or delete. Each run that changes something, or fails, goes in **What they did** ("3 up, 1 down"), without Undo.
