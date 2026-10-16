# Opening folders from File Explorer

Glint can open folders for the rest of Windows. Both options are off until you turn them on in **Settings → Files**, and neither needs administrator rights.

-   **Open in Glint from File Explorer** adds *Open in Glint* to the right-click menu of folders, drives, network shares and the empty space in a folder or on the desktop. On Windows 11 it's under **Show more options** (or Shift+right-click): Windows 11's shorter menu only lists apps that ship as packages.
-   **Open folders in Glint by default** makes that entry what a double-click runs. Double-clicking a folder in File Explorer or on the desktop opens it in Glint, and so does anything else that opens a folder the default way, such as `start C:\Projects` in a terminal. Turning it on turns the menu entry on too (the bold entry at the top of a folder's menu is the default), and turning the menu entry off turns it off.

**What stays with File Explorer.** Only plain file-system folders change. Drives keep opening in File Explorer when double-clicked (their menu has *Open in Glint*), and so do This PC, Home, the Recycle Bin, Control Panel, libraries, Network, phones and special folders such as Fonts. File Explorer's own *Open* stays on every folder's menu, `explorer.exe C:\Projects` still opens File Explorer, and the Open and Save dialogs of other apps still go into folders. Apps that show a file in its folder (*Show in folder*, *Open file location*) usually ask File Explorer for a window with the file selected, so those keep opening in File Explorer too.

**Anything Glint can't open goes to File Explorer.** Before a window shows, Glint checks what it was given. `shell:` and `::{CLSID}` locations, other URLs, a missing path, a folder it may not list and a drive with no disc are handed to File Explorer's own *Open*, which shows them as it always did (a folder you may not open gets File Explorer's *You don't currently have permission* prompt). That *Open* never comes back to Glint, even when folders open in Glint by default.

## The command line

`Glint.exe [folder or file]…` opens each folder in a new tab; a file opens its folder with the file selected. Relative paths are relative to the current folder, and anything starting with `-` is ignored.

Glint runs once per user. Starting it again, from the Start menu or with folders, brings the open window to the front (restored if it was minimized) and opens those folders as new tabs at the end of the active pane, leaving Text mode and Settings. A second launch hands over its folders and quits in about 80 ms. At start, a folder that's already one of the restored tabs switches to that tab instead of opening another, since an update restarts the app with the folders it was first started with.

## What it changes

Everything is per user, under `HKEY_CURRENT_USER\Software\Classes`, where `<exe>` is the installed `Glint.exe`:

| Key | Values |
| :--- | :--- |
| `Directory\shell\Glint` | `(Default)` = `Open in Glint`, `Icon` = `<exe>,0` |
| `Directory\shell\Glint\command` | `(Default)` = `"<exe>" "%V"` |
| `Directory\Background\shell\Glint` and its `command` | the same |
| `Drive\shell\Glint` and its `command` | the same |

`%V` is the folder, the folder whose empty space was clicked, or the drive. Opening folders by default sets `Directory\shell` `(Default)` = `Glint`, and keeps what was there before in `Directory\shell\Glint`, `PreviousDefault` (with its type), or nothing if there was no value. Windows' own default for folders lives in `HKEY_LOCAL_MACHINE` and isn't touched.

Turning the options off puts the previous default back (unless something else has changed it since), deletes the three `Glint` keys, deletes `Directory\shell`, `Directory\Background\shell`, `Drive\shell` and the keys above them only if nothing else is in them, and tells File Explorer the associations changed. The registry is then as it was before.

## Never a missing program

The entries run the exe where it is, so a copy that moved or was deleted would leave File Explorer unable to open folders. To prevent that:

-   **Only a copy the setup program installed can turn them on.** In a Scoop or portable copy, or a dev build, the switches are greyed out.
-   **The uninstaller removes them** (`src-tauri/installer/hooks.nsh`, before uninstalling but not when updating), if they run the copy being uninstalled, and puts back what folders opened with.
-   **Every start checks them.** If they run an exe that's gone, the installed copy points them at itself, and any other copy removes them.

## Where it is in the code

-   `src-tauri/src/launch.rs`: sorting the command line out, handing what Glint can't open to File Explorer, and later launches (through [tauri-plugin-single-instance](https://v2.tauri.app/plugin/single-instance/), which lets the window come to the front without pressing keys for it).
-   `src-tauri/src/verbs.rs`: the registry entries, their tests (on a scratch key, compared before and after), and the check at start.
-   `src/lib/explorer.ts`: `initExplorer` and `openRequested` put the folders in tabs. `src/components/SettingsView.tsx`: the two switches.

## Trying it without touching your Glint

Release builds run once per app identifier (debug builds don't, so `npm run tauri dev` still starts next to an installed Glint). A release build with Glint's own identifier hands its folders to your installed Glint if it's running, and adds menu entries with the same name. So try changes with a stand-in that has its own name and identifier:

1.  Write a config overlay, `<scratch>/standin.json`:

    ```json
    {
      "productName": "glint-shelltest",
      "mainBinaryName": "glint-shelltest",
      "identifier": "com.shado.shadui-ripgrep.shelltest",
      "bundle": { "windows": { "nsis": { "installerHooks": "<scratch>/hooks-standin.nsh" } } }
    }
    ```

    where `hooks-standin.nsh` is the `GLINT_REMOVE_VERB` and `GLINT_REMOVE_EXPLORER_VERBS` macros copied from `src-tauri/installer/hooks.nsh`, plus an `NSIS_HOOK_PREUNINSTALL` that inserts `GLINT_REMOVE_EXPLORER_VERBS` when `$UpdateMode <> 1`. Leave out the rest of the real hooks: they would remove an old ShadUI Ripgrep and the instant search service.
2.  `npm run tauri build -- --bundles nsis --config <scratch>/standin.json`, then install it with `/S /NS` (silent, no shortcuts). It goes to `%LOCALAPPDATA%\glint-shelltest`, and its entries are named `glint-shelltest`, apart from Glint's.
3.  Turning the options on changes File Explorer for your account while they're on. Before you do, export `HKCU\Software\Classes\Directory` and `HKCU\Software\Classes\Drive` with `reg export`, and compare afterwards.
4.  Uninstall it with its `uninstall.exe /S` and delete `%LOCALAPPDATA%\com.shado.shadui-ripgrep.shelltest`.

Run these from an ordinary terminal. A packaged app that virtualizes the registry (some Microsoft Store apps do) keeps what the programs it starts write to `HKEY_CURRENT_USER` to itself, so File Explorer would never see the entries; start the stand-in and its installer from File Explorer instead.
