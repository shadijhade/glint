# Remote connections

Glint opens an S3-compatible bucket, a WebDAV server or an SFTP server as a folder of its own: under **Remote** in the sidebar, in a tab like any other, at a path of the form `remote:<id>\folder\file`. It's read-only there: Glint looks, previews, opens and copies out, and never changes anything on the remote.

## Adding one

**Add a connection** (under **Remote** in the sidebar, or in `Ctrl+K`) asks for what Glint needs to reach it. **Test** tries it as it's typed and says what's at its top, or why it didn't connect; **Add and open** keeps it and opens it.

| Kind | Address | Its secret |
| :--- | :--- | :--- |
| Amazon S3 | empty, with the bucket's region (`eu-west-1`) | the secret access key of the access key ID |
| Cloudflare R2 | `https://<account id>.r2.cloudflarestorage.com`, region `auto` | an R2 API token's secret access key |
| Backblaze B2 | `https://s3.<region>.backblazeb2.com` (the bucket's endpoint) | the application key, its key ID as the access key ID |
| MinIO, Wasabi, others | the service's S3 endpoint | as the service gives it |
| A public bucket | as above | none: leave the access key ID empty |
| WebDAV (Nextcloud, ownCloud, a NAS, dufs) | the WebDAV URL, such as `https://cloud.example.com/remote.php/dav/files/you/` | the password (an app password, where the service has them) |
| SFTP (a NAS, a server you reach with SSH) | the server's name or address (a `Host` from `~/.ssh/config` will do), and its port | a password or a key's passphrase, or nothing: ssh then signs in with your SSH agent or your keys |

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
