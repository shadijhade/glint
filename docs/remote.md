# Remote connections

Glint opens an S3-compatible bucket or a WebDAV server as a folder of its own: under **Remote** in the sidebar, in a tab like any other, at a path of the form `remote:<id>\folder\file`. It's read-only there: Glint looks, previews, opens and copies out, and never changes anything on the remote.

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

**Folder in it** opens the connection at a folder below its top, such as `photos/2024`. Each connection gets an id made from its first name (`photos-backup`), which stays when it's renamed, so tabs and pinned folders keep working.

The connection itself is kept in Glint's settings without its secret. The secret access key or password is kept by Windows Credential Manager, as a generic credential named `Glint/remote/<id>` (it's in Control Panel → Credential Manager → Windows Credentials), and it goes when the connection is removed. **Remove the connection** (its place's right-click menu, or the connection's dialog) also sends the tabs showing it to This PC.

## Browsing

A folder on a remote lists what's directly in it: for S3, the objects and the "folders" its keys make with `/` (ListObjectsV2, a thousand at a time, up to 200,000 in a folder); for WebDAV, a `PROPFIND` of depth 1. Names come from the remote, so each is shown as Windows would take it (a key like `2024-07-14T10:30:05.log` shows as `2024-07-14T10_30_05.log`, and `notes.txt` beside `Notes.txt` as `notes (2).txt`), while Glint asks the remote for it by its own name.

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
