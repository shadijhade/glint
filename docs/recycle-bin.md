# The Recycle Bin

Glint shows the Recycle Bin as a place of its own: under the drives in the sidebar, as `Recycle Bin` typed in the address bar, or as `shell:RecycleBinFolder`, File Explorer's name for it. It lists what your account deleted on every drive, as File Explorer's Recycle Bin does, with the name each item had, the folder it was deleted from (**Deleted from**), when (**Date deleted**, which sorts as dates do), and its size. A folder's size is everything that was in it.

## Where it comes from

Windows keeps each deleted item as a file or folder named `$R…` in the hidden `$Recycle.Bin\<your account's SID>` folder of its drive, next to a `$I…` record with the same ending, which holds the path it was deleted from, when, and its size. Glint asks the shell for the Recycle Bin's items, every drive's (and those of a folder like Documents that's kept on another drive or a server), then reads each one's `$I` record: version 1 from Windows Vista to 8.1, version 2 from Windows 10 on. An item without a readable record isn't listed; File Explorer leaves it out too.

Nothing watches the Recycle Bin. Glint reads it again after it deletes, restores or empties anything, when its window comes back to the front, and with `F5`.

## Restoring

**Restore** moves each item from its `$R` path back into the folder it was deleted from, under its name, through Windows' own file engine, as Undo after a delete always has. A name that's taken there gets Windows' *Replace or Skip* dialog. If the folder it was deleted from is gone, it's made again first, as File Explorer does. The item's `$I` record goes once it's back. **Undo** (`Ctrl+Z`) recycles what was restored.

## Deleting for good

`Delete` in the Recycle Bin, or **Delete for good**, asks first in Glint's own dialog, naming what goes (Windows' dialog would name each item by its `$R` file). Then each `$R` file or folder is deleted through Windows' file engine, and its `$I` record after it. Nothing takes this back.

**Empty the Recycle Bin** is Windows' own: Windows asks, shows its progress and empties every drive's bin. Glint then reads what's left.

## What doesn't happen there

What's in the Recycle Bin is restored or deleted for good, and nothing else. It isn't opened (`Enter` shows it in Quick Look), renamed, cut, copied or dragged out where it is, since it would land named as Windows keeps it (`$RXXXXXX.ext`). Nothing is pasted or made in the Recycle Bin, and files dropped on it are deleted, as in File Explorer. Folder sizes, disk usage, Git, text search and the terminal leave it out, and it isn't added to folder history.
