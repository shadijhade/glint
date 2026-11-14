# Opening folders from File Explorer

Glint can open folders for the rest of Windows. These options are off until you turn them on in **Settings → Files**, and none needs administrator rights.

-   **Open in Glint from File Explorer** adds *Open in Glint* to the right-click menu of folders, drives, network shares and the empty space in a folder or on the desktop. On Windows 11 it's under **Show more options** (or Shift+right-click): Windows 11's shorter menu only lists apps that ship as packages.
-   **Open folders in Glint by default** makes that entry what a double-click runs. Double-clicking a folder in File Explorer or on the desktop opens it in Glint, and so does anything else that opens a folder the default way, such as `start C:\Projects` in a terminal. Turning it on turns the menu entry on too (the bold entry at the top of a folder's menu is the default), and turning the menu entry off turns it off.
-   **Win+E opens Glint** makes Win+E open This PC in Glint instead of File Explorer, through the key Windows reads for that shortcut. It turns the menu entry on too, and turning the menu entry off turns it off.

**What stays with File Explorer.** Only plain file-system folders change. Drives keep opening in File Explorer when double-clicked (their menu has *Open in Glint*), and so do This PC (Win+E aside, when it opens Glint), Home, the Recycle Bin, Control Panel, libraries, Network, phones and special folders such as Fonts. File Explorer's own *Open* stays on every folder's menu, `explorer.exe C:\Projects` still opens File Explorer, and the Open and Save dialogs of other apps still go into folders. Apps that show a file in its folder (*Show in folder*, *Open file location*) usually ask File Explorer for a window with the file selected, so those keep opening in File Explorer too.

**Anything Glint can't open goes to File Explorer.** Before a window shows, Glint checks what it was given. This PC and the Recycle Bin (by their `::{CLSID}` or `shell:` names), the usual known folders (`shell:Downloads`, `shell:Desktop`, `shell:My Pictures`…) and `file:///` URLs open in Glint. Other `shell:` and `::{CLSID}` locations, other URLs, a missing path, a folder it may not list and a drive with no disc are handed to File Explorer's own *Open*, which shows them as it always did (a folder you may not open gets File Explorer's *You don't currently have permission* prompt). That *Open* never comes back to Glint, even when folders open in Glint by default.

## The command line

`Glint.exe [folder or file]…` opens each folder in a new tab; a file opens its folder with the file selected. Relative paths are relative to the current folder. `this-pc:` opens This PC, and File Explorer's own switches are understood: `/select,<path>` opens the folder the item is in with the item selected, and `/e`, `/n` and `/root` are passed over. `--quick-search` brings up quick search (the taskbar button's Jump List starts Glint with it); anything else starting with `-` is ignored.

Glint runs once per user. Starting it again, from the Start menu or with folders, brings the open window to the front (restored if it was minimized) and opens those folders as new tabs at the end of the active pane, leaving Settings. A second launch hands over its folders and quits in about 80 ms. At start, a folder that's already one of the restored tabs switches to that tab instead of opening another, since an update restarts the app with the folders it was first started with.

## What it changes

Everything is per user, under `HKEY_CURRENT_USER\Software\Classes`, where `<exe>` is the installed `Glint.exe` (for Scoop, `<scoop>\apps\glint\current\Glint.exe`):

| Key | Values |
| :--- | :--- |
| `Directory\shell\Glint` | `(Default)` = `Open in Glint`, `Icon` = `<exe>,0` |
| `Directory\shell\Glint\command` | `(Default)` = `"<exe>" "%V"` |
| `Directory\Background\shell\Glint` and its `command` | the same |
| `Drive\shell\Glint` and its `command` | the same |
| `CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command` (Win+E) | `(Default)` = `"<exe>" "this-pc:"`, `DelegateExecute` = empty |

`%V` is the folder, the folder whose empty space was clicked, or the drive. Opening folders by default sets `Directory\shell` `(Default)` = `Glint`, and keeps what was there before in `Directory\shell\Glint`, `PreviousDefault` (with its type), or nothing if there was no value. Windows' own default for folders lives in `HKEY_LOCAL_MACHINE` and isn't touched.

Turning the options off puts the previous default back (unless something else has changed it since), deletes the three `Glint` keys, deletes `Directory\shell`, `Directory\Background\shell`, `Drive\shell` and the keys above them only if nothing else is in them, and tells File Explorer the associations changed. Turning Win+E off deletes its `CLSID` key when it runs a Glint; another app's (Files has the same option) is left as it is. The registry is then as it was before.

## Never a missing program

The entries run the exe where it is, so a copy that moved or was deleted would leave File Explorer unable to open folders. To prevent that:

-   **Only an installed copy can turn them on**: one the setup program installed, or one Scoop installed for your account. A Scoop copy's entries run it through the app's `current` link, which `scoop update` points at each new version, even if Glint was started from the version's own folder. In a portable copy or a dev build the switches are greyed out, and so they are in an app Scoop installed for all users (`scoop install --global`, whose uninstall couldn't reach other accounts' entries), in an older version an update left in Scoop's folder, and with Scoop's `no_junction` setting, which has no `current` link.
-   **The uninstaller removes them** (`crates/glint/installer/hooks.nsh`, before uninstalling but not when updating), if they run the copy being uninstalled, and puts back what folders opened with. Win+E's key goes too, so Win+E opens File Explorer again.
-   **So does `scoop uninstall`.** The manifest's uninstaller script (`scripts/package-manifests.mjs`) runs for `scoop update` too, but only acts when Scoop's `$cmd` is `uninstall`. It removes the entries (and Win+E's key) that run that app's `current\Glint.exe` and puts back what folders opened with. If it can't, the uninstall stops there, so Glint stays where the entries run it.
-   **Every start checks them.** If they run an exe that's gone, the installed copy points them at itself, and any other copy removes them. Win+E counts when its command runs a `Glint.exe`.

Glint knows Scoop installed it when its folder is `<scoop>\apps\<app>\<version>` with Scoop's record of the install in it (`scoop-install.json`, or `install.json` before Scoop 0.6), the app's `current` link leads to that folder, and Scoop itself is in the same `<scoop>` folder (`apps\scoop\current`), which makes it a user's own Scoop rather than the one for all users.

## Where it is in the code

-   `crates/glint-ui/src/platform/launch.rs`: sorting the command line out, handing what Glint can't open to File Explorer, and later launches (a named mutex decides which copy is first; a later copy sends its folders to the first one's hidden message window as `WM_COPYDATA`, which lets the window come to the front without pressing keys for it). Until 0.10 this was `src-tauri/src/launch.rs` and Tauri's single-instance plugin.
-   `crates/glint-core/src/verbs.rs`: the registry entries, their tests (on a scratch key, compared before and after), telling a Scoop copy apart, and the check at start. A test also runs the Scoop manifest's uninstaller script on the scratch key, as Scoop runs it.
-   `scripts/package-manifests.mjs`: the Scoop manifest, whose uninstaller script (`removeExplorerVerbs`) does what `GLINT_REMOVE_EXPLORER_VERBS` in `crates/glint/installer/hooks.nsh` does for the setup's copies.
-   `crates/glint-ui/src/explorer/requested.rs`: `open_requested` puts the folders in tabs. `crates/glint-ui/src/components/settings/integration.rs`: the three switches.

## Trying it without touching your Glint

A release build runs once per session, under one name that every release build shares, so it hands its folders to your installed Glint if that's running, and the entries it adds have the installed Glint's name. A debug build (`cargo run -p glint`) has an instance name of its own and starts next to an installed Glint, but it is not an installed copy, so the switches are greyed out in it and it cannot write the entries. Glint has no build-time identifier to change any more (until 0.10 a Tauri config overlay made a stand-in with its own name), so try changes to the entries and the uninstaller like this:

1.  Use a machine or a Windows account you can lose: the setup always installs to `%LOCALAPPDATA%\Glint` and uses the same registry keys, so a test setup would replace your own install. Build it with `npm run installer`.
2.  Install it with `/S /NS` (silent, no shortcuts).
3.  Turning the options on changes File Explorer for your account while they're on. Before you do, export `HKCU\Software\Classes\Directory` and `HKCU\Software\Classes\Drive` with `reg export`, and compare afterwards.
4.  Uninstall it with its `uninstall.exe /S`.

To try a Scoop copy the same way, make the manifest from that setup with `node scripts/package-manifests.mjs <version> <setup.exe> --out <scratch>`, point the `url` in `<scratch>/scoop/glint.json` at the local setup (`file:///C:/…/Glint_<version>_x64-setup.exe#/dl.7z`) and install it with `scoop install --no-update-scoop`. Scoop extracts the setup with 7-Zip and never runs it, so its hooks don't matter; the entries then run `<scoop>\apps\glint\current\Glint.exe`. `scoop uninstall glint` takes them out (compare the exports). Think twice before removing the 7-Zip Scoop may install first: its uninstaller deletes `HKEY_CURRENT_USER\Software\7-Zip`. Until 0.10 a stand-in with another name and identifier could be built beside the real Glint (`glint-shelltest`); the GPUI build has no such switch, which is why this needs a spare account.

Run these from an ordinary terminal. A packaged app that virtualizes the registry (some Microsoft Store apps do) keeps what the programs it starts write to `HKEY_CURRENT_USER` to itself, so File Explorer would never see the entries; start the installer from File Explorer instead.
