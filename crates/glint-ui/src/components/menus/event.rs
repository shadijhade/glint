//! What the menus tell the window when an item is chosen whose work belongs to another part of the port.
//!
//! Each variant names what was chosen and for which paths, and says which TypeScript function the part that
//! handles it calls. The host ([`MenuHost`](super::MenuHost)) emits them; the coordinator subscribes and hands
//! each to its part, or tells the person "not built yet" until the part exists. Everything that is a file
//! operation the workspace already has (Copy, Cut, Paste, Delete, Rename, Undo, Properties, Copy path) is a
//! dispatched action instead, and navigation is done by the menus themselves.

/// How `Compress to` packs (`compressEntries`' second argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressFormat {
    /// A .zip file.
    Zip,
    /// A .7z file.
    SevenZ,
    /// A .tar.zst file.
    TarZst,
}

/// Which of the media studio's tools was chosen (`openMedia`'s mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaTool {
    /// "Convert or shrink…": MP4, HEVC, AV1, to fit a size.
    Convert,
    /// "Take the sound out…".
    Audio,
    /// "Make a GIF…".
    Gif,
    /// "Make subtitles…".
    Subtitles,
    /// "Even out the volume…".
    Volume,
}

/// Which of the PDF tools was chosen (`openPdfTools`' mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfTool {
    Merge,
    Split,
    Keep,
    Rotate,
    Optimize,
    Images,
    Encrypt,
    Decrypt,
    Stamp,
}

/// Which way a cloud file is kept (`setCloudPin`'s state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudPin {
    /// Always keep on this device.
    Keep,
    /// Free up space.
    Free,
    /// No longer always kept (back to automatic).
    Auto,
}

/// What a menu asked the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEvent {
    // ---- Covering the window ----
    /// A menu is now on screen over part of the window. A native preview in an owned pop-up draws above GPUI,
    /// so the window hides it now (`platform::preview_host::hide(claimant)`).
    Shown,
    /// The menu was closed, chosen from or not: the window shows the preview again (`preview_host::reveal`).
    Closed,

    // ---- The Recycle Bin ----
    /// "Restore" (`restoreRecycled`): these entries go back where they were deleted from.
    Restore { paths: Vec<String> },
    /// "Empty the Recycle Bin" (`emptyRecycleBin`); the window confirms.
    EmptyRecycleBin,

    // ---- Opening and showing ----
    /// "Paste into folder" / "Paste" on a folder that isn't the one shown (`paste(folder)`).
    PasteInto { folder: String },
    /// "Paste with the copy queue" (`pasteQueued(folder)`).
    PasteQueued { folder: String },
    /// "Add to basket" (Ctrl+Shift+B): these items join what the basket holds.
    BasketAdd { paths: Vec<String> },
    /// The basket's "Move here": what it holds moves into the folder shown.
    BasketMove,
    /// The basket's "Copy here".
    BasketCopy,
    /// The basket's "Zip": what it holds into one zip file.
    BasketZip,
    /// The basket's "Batch rename".
    BasketRename,
    /// The basket's "Empty the basket".
    BasketEmpty,
    /// An item taken out of the basket.
    BasketRemove { path: String },
    /// "Paste as link": what is on the clipboard linked into the folder (junction, hardlink or shortcut).
    PasteAsLink { folder: String },
    /// "Extract here" (`extractHere`).
    ExtractHere { path: String },
    /// "Mount" a disk image (`mountImage`).
    Mount { path: String },
    /// "Open with its app" on an archive (`openFile`, or `openArchivedWith` when it is inside another archive).
    OpenWithItsApp { path: String, extract_first: bool },
    /// "Open with <editor>" (`openPath(path, editorPath)`); `extract_first` for a file in an archive or on a remote.
    OpenWithEditor {
        path: String,
        editor: String,
        extract_first: bool,
    },
    /// "Open with…" on a file in an archive or on a remote: extract it, then show Windows' picker.
    OpenWithPicker { path: String },
    /// "Run in Windows Sandbox" (`runInSandbox`).
    RunInSandbox { path: String },
    /// "Previous versions…" of a file (`openVersions`), or "Previous versions of this folder".
    PreviousVersions { path: String },
    /// "Open in new window" (`openInNewWindow`).
    OpenInNewWindow { path: String },
    /// "Open file location" (`revealInApp`): the folder the item is in, with it selected.
    OpenFileLocation { path: String },
    /// "Remove from Recent" (`forgetRecent`).
    ForgetRecent { paths: Vec<String> },
    /// "Show in File Explorer" (`revealItemInDir`).
    RevealInFileExplorer { paths: Vec<String> },
    /// "Show the archive in File Explorer": the archive file on disk.
    RevealArchive { archive: String },
    /// "Search here" on a folder: it opens, with the search box ready (names, and inside the files).
    SearchText { folder: String },

    // ---- The other pane, comparing ----
    /// "Copy to the other pane" / "Move to the other pane" (`copyToOtherPane`).
    CopyToOtherPane { paths: Vec<String>, moving: bool },
    /// "Compare…" two files (`compareFiles`): left then right as the panes have them.
    Compare { left: String, right: String },

    // ---- Tags ----
    /// A color tag toggled (`setTags(paths, add, remove)`).
    SetTags {
        paths: Vec<String>,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// "Edit tags…" (`useTagsEdit.setState({ paths })`).
    EditTags { paths: Vec<String> },

    // ---- Making and changing files ----
    /// "Folder sizes" (`calcSizes`).
    FolderSizes,
    /// "New > Text document" (`newItem(false)`).
    NewTextDocument,
    /// "New > <kind>" (`newOfKind`): the kind's extension with its dot.
    NewOfKind { ext: String },
    /// "Rename items in this folder…": batch rename with every row of the folder (`openBatchRename(rows, rows)`).
    BatchRenameFolder,
    /// "Compress to" a format (`compressEntries`, after `secretsCheckFirst`).
    Compress { paths: Vec<String>, format: CompressFormat },
    /// "Compress to > In place…" (`openCompressInPlace`).
    CompressInPlace { paths: Vec<String> },
    /// "Compress to > With a password…" (`useCompressAsk`, after `secretsCheckFirst`).
    CompressWithPassword { paths: Vec<String> },
    /// "Make smaller…" (`openShrink`).
    MakeSmaller { paths: Vec<String> },
    /// "Back up…" a folder (`openBackup`).
    BackUp { folder: String },
    /// "Convert picture…" (`convertPictures`).
    ConvertPictures { paths: Vec<String> },
    /// One of the media studio's tools (`openMedia`).
    Media { paths: Vec<String>, tool: MediaTool },
    /// "Edit song details…" (`openSongs`).
    EditSongs { paths: Vec<String> },
    /// "Convert document…" (`openDocConvert`).
    ConvertDocument { paths: Vec<String> },
    /// "Query N files together…" (`queryTogether`).
    QueryTogether { paths: Vec<String> },
    /// "Convert data…" (`convertData`).
    ConvertData { paths: Vec<String> },
    /// A PDF tool (`openPdfTools`).
    PdfTools { paths: Vec<String>, tool: PdfTool },
    /// "Set as desktop background" (`setWallpaper`).
    SetWallpaper { path: String },
    /// "Rotate right" / "Rotate left" (`rotatePictures`).
    Rotate { paths: Vec<String>, clockwise: bool },
    /// "Find similar colors": opens the OKLab Color Wheel anchored on this picture's dominant color.
    MatchColor { path: String },
    /// Opens or closes the OKLab Color Wheel filter bar in the active tab.
    ToggleColorWheel,
    /// "Always keep on this device" / "Free up space" (`cloudPin`).
    CloudPin { paths: Vec<String>, state: CloudPin },

    // ---- Inspecting ----
    /// "File X-Ray" (`inspectXRay`).
    XRay { path: String },
    /// "Check files in this folder": every file the checksum file at `path` lists, checked in the background.
    CheckSums { path: String },
    /// "Recent changes" in a folder (`inspectTimeline`).
    Timeline { folder: String },
    /// "Time Machine scrubber" in a folder (shadow copies, Git commits, backups, journal).
    OpenTimeMachine { folder: String },
    /// "Moved and deleted here" (`openHistory`).
    History { folder: String },
    /// "What's locking this?" (`inspectLocks`).
    Locks { paths: Vec<String> },
    /// "Scan with Microsoft Defender" (`scanWithDefender`).
    Defender { paths: Vec<String> },

    // ---- Cleaning up and sharing ----
    /// "Find duplicate files" (`inspectDuplicates`).
    Duplicates { folder: String },
    /// "Clean up…" (`openCleanup`).
    Cleanup { folder: String },
    /// "A rule for this folder…" (`openRules`).
    Rule { folder: String },
    /// "Remove personal info…" (`openMetadata(…, "scrub")`).
    RemovePersonalInfo { paths: Vec<String> },
    /// "Change when it was taken…" (`openMetadata(…, "date")`).
    ChangeDateTaken { paths: Vec<String> },
    /// "Share…": the Share dialog, with these files to send to a device nearby or through Windows (`openNearby`).
    Share { paths: Vec<String> },
    /// "Copy permalink": copies an unbreakable `glint://` NTFS/ReFS file-ID URI for `path`.
    CopyPermalink { path: String },

    // ---- Scripts ----
    /// A script of the Scripts folder, run on these paths in `cwd` (`runScript`): the script's file.
    RunScript {
        script: String,
        cwd: String,
        paths: Vec<String>,
    },
    /// A script whose tool isn't installed: install it (`installTool`): the tool's id.
    InstallTool { tool: String },
    /// "Open the Scripts folder" (`openScriptsFolder`).
    OpenScriptsFolder,

    // ---- Connections ----
    /// "Edit the connection…" (`editRemote`).
    EditConnection { id: String },
    /// "Remove the connection…" (`removeRemote`).
    RemoveConnection { id: String },
    /// "Show in File Explorer, on demand" (`showOnDemand`).
    ShowOnDemand { id: String, name: String },
    /// "Stop showing in File Explorer" (`hideOnDemand`).
    HideOnDemand { id: String },
    /// Mounts an archive, search, commit, or backup snapshot as a live CfAPI portal folder.
    PinPortal {
        name: String,
        source: glint_core::portals::PortalSource,
    },
    /// Unmounts an active CfAPI live portal folder.
    UnpinPortal { id: String },

    // ---- What Glint knows (docs/gpui/aware) ----
    /// "Unblock": takes Windows' "downloaded" mark off these files.
    Unblock { paths: Vec<String> },
    /// "Copy the text in it": the text Windows reads in a picture, to the clipboard.
    CopyTextIn { path: String },
    /// "Move or copy to" a folder used before: these items go to `folder`.
    SendTo {
        paths: Vec<String>,
        folder: String,
        moving: bool,
    },
    /// "Choose a folder…" under "Move or copy to": asks where, then as [`MenuEvent::SendTo`].
    ChooseSendTo { paths: Vec<String>, moving: bool },
}
