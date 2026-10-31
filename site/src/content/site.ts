/*
  Everything the page says, in one place. Every time and count is the README's (benchmarks measured on an
  Intel Core Ultra 7 265K with an NVMe SSD, files in Windows' cache, the median of 5 runs, from pressing
  Enter until everything is on screen); folder sizes are the ones in screenshots/folder-sizes.png.
*/

const REPO = "https://github.com/shadijhade/glint";

export const SITE = {
  url: "https://glint.rafd.tech",
  title: "Glint, the fast file explorer for Windows",
  description:
    "Glint is a free, open-source file explorer for Windows 10 and 11. Folders open in milliseconds, every folder shows its size, and any file on any drive turns up as you type.",
  repo: REPO,
  /** The Microsoft Store's Glint, which Microsoft signs and the Store keeps up to date: the way in for most people. */
  store: "https://apps.microsoft.com/detail/9NQLRJ3HF1C9",
  /** The setup program, which can also change File Explorer's settings (Store apps can't). */
  download: `${REPO}/releases/latest/download/Glint-setup.exe`,
  latest: `${REPO}/releases/latest`,
  latestApi: "https://api.github.com/repos/shadijhade/glint/releases/latest",
  releases: `${REPO}/releases`,
  issues: `${REPO}/issues/new/choose`,
  privacy: `${REPO}/blob/master/SECURITY.md#code-signing-and-privacy-policy`,
  license: `${REPO}/blob/master/LICENSE`,
  instantSearch: `${REPO}/blob/master/docs/instant-search.md`,
  benchmarks: `${REPO}#benchmarks`,
  features: `${REPO}#features`,
  source: `${REPO}#build-from-source`,
  rafd: "https://rafd.tech/en/",
  scoop: ["scoop bucket add shadijhade https://github.com/shadijhade/scoop-bucket", "scoop install shadijhade/glint"],
  winget: "winget install 9NQLRJ3HF1C9 --source msstore",
};

/** The chapters, in order: the header, the rail and the WebGL field all read this. */
export const CHAPTERS = [
  { id: "top", label: "Glint", scene: 0 },
  { id: "open", label: "Open", scene: 1 },
  { id: "size", label: "Size", scene: 2 },
  { id: "find", label: "Find", scene: 3 },
  { id: "more", label: "More", scene: 4 },
  { id: "get", label: "Get Glint", scene: 5 },
] as const;

export const HERO = {
  lines: ["Faster than", "a blink."],
  sub: "The file explorer for Windows that doesn't make you wait.",
  body: "Glint opens folders in milliseconds, shows the size of every folder, and finds any file on any drive as you type. Free, open source, and no account.",
  stats: [
    { value: 12, decimals: 0, unit: "ms", label: "System32 on screen, all 5,092 items" },
    { value: 0.4, decimals: 1, unit: "ms", label: "Every folder in C:\\Windows sized" },
    { value: 7.7, decimals: 1, unit: "ms", label: "One file found among 3 million names" },
    { text: "MIT", label: "Free and open source, on GitHub" },
  ],
};

export const OPEN = {
  label: "Open",
  lines: ["Open before", "you blink."],
  body: "A blink lasts 100 to 400 milliseconds. Glint reads a folder in one pass, sorts it in Rust and draws only the rows you can see, so System32's 5,092 items are on screen in 12.",
  blink: { from: 100, to: 400, label: "A blink" },
  marks: [
    { ms: 6.3, where: "C:\\Windows", what: "108 items" },
    { ms: 12, where: "System32", what: "5,092 items" },
    { ms: 73, where: "WinSxS", what: "21,650 items" },
    { ms: 114, where: "A test folder", what: "100,000 files" },
  ],
  source: { text: "A blink, 0.1 to 0.4 s: BioNumbers 100706, after Schiffman, Sensation and Perception (2001)", href: "https://bionumbers.hms.harvard.edu/bionumber.aspx?id=100706" },
};

export const SIZE = {
  label: "Size",
  lines: ["Every folder", "knows its size."],
  body: "File Explorer keeps folder sizes in Properties. Glint puts them in the list as the folder opens, so you can sort by them, and draws any folder as a map of what fills it, like WizTree.",
  facts: [
    { value: 0.4, decimals: 1, unit: "ms", label: "to size every folder in C:\\Windows, from the index" },
    { value: 53, decimals: 0, unit: "ms", label: "to map all of C:, 464 GB, after the click" },
  ],
  caption: "C:\\Windows on the developer's PC, sorted by size",
  folders: [
    { name: "WinSxS", gb: 13 },
    { name: "System32", gb: 13 },
    { name: "LiveKernelReports", gb: 11 },
    { name: "assembly", gb: 6.8 },
    { name: "SystemApps", gb: 1.4 },
    { name: "SysWOW64", gb: 1.3 },
    { name: "Microsoft.NET", gb: 0.629 },
    { name: "servicing", gb: 0.41 },
    { name: "Fonts", gb: 0.404 },
    { name: "Microsoft Antimalware", gb: 0.258 },
    { name: "SystemResources", gb: 0.209 },
    { name: "Temp", gb: 0.199 },
    { name: "Speech", gb: 0.187 },
    { name: "Globalization", gb: 0.136 },
  ],
};

export const FIND = {
  label: "Find",
  lines: ["Any file. Any drive.", "As you type."],
  body: "Turn on instant search and a small service reads each NTFS drive's file table once, the way Everything does, then follows every change. Every kernel32.dll on three drives, among more than 3 million names, turns up in 7.7 ms.",
  placeholder: "Search 9,474 of Windows' own file names",
  start: "kernel",
  tries: ["kernel32", "*.sys", "shell", "img*.jpg", "notepad"],
  note: "This demo searches 9,474 of Windows' own file names, right here in your browser. Glint searches every name on your drives.",
  facts: [
    { value: 2.4, decimals: 1, unit: "s", label: "to index a C: of 2.9 million entries" },
    { value: 0.3, decimals: 1, unit: "s", label: "for a change on disk to show up" },
    { value: 0.25, decimals: 2, unit: "s", label: "to be ready again after a restart" },
  ],
};

export const MORE = {
  label: "More",
  lines: ["What File Explorer", "leaves out."],
  cards: [
    { shot: "disk-usage", title: "A map of your drive", text: "Every folder and file as a block the size of the space it takes, like WizTree.", alt: "The disk usage map of C:\\Windows: every folder and file as a block sized by how much space it takes" },
    { shot: "dual-pane", title: "Tabs, and two panes", text: "Copy and move between two folders, or compare them and sync.", alt: "Two panes side by side, each with its own tabs" },
    { shot: "grid-view", title: "Thumbnails that never wait", text: "1,200 photos on screen in 9.3 ms, thumbnails from Windows' own cache.", alt: "Wallpapers found by name and shown as a grid of thumbnails" },
    { shot: "quick-look", title: "Quick Look on Space", text: "Any file full size without opening it: code, pictures, PDFs, Office files.", alt: "Quick Look showing a Rust file with syntax highlighting" },
    { shot: "text-search", title: "Search inside files", text: "ripgrep in any folder, with regular expressions and replace.", alt: "ripgrep results with a syntax-highlighted preview" },
    { shot: "command-palette", title: "Ctrl+K for everything", text: "Jump to the folders you use most, or run any command, by typing.", alt: "The command palette with recent folders and commands" },
  ],
  also: ["Archives that open like folders", "Git status in every folder", "A terminal under the files", "Checksums and File X-Ray", "Batch rename with Undo", "Duplicates and Clean up", "Send files to another PC", "S3, WebDAV and SFTP folders"],
};

export const GET = {
  label: "Get Glint",
  lines: ["Try it on your", "slowest folder."],
  body: "It installs for your account in a few seconds, without administrator rights, and keeps itself up to date.",
  requirements: "Windows 10 or 11, 64-bit",
  trust: [
    { icon: "engine", text: "Windows' own file engine" },
    { icon: "lock", text: "No telemetry, no account" },
    { icon: "shield", text: "Signed, checked updates" },
    { icon: "code", text: "Open source, MIT" },
  ],
  smartscreen:
    "Microsoft signs the Store's copy. The installer isn't code-signed yet, so Windows may say it protected your PC: choose More info, then Run anyway.",
  ways: [
    { title: "winget", commands: [SITE.winget], note: "Installs the Microsoft Store's Glint, which the Store keeps up to date." },
    { title: "Scoop", commands: SITE.scoop, note: "Checks the download against its hash and keeps Glint up to date." },
  ],
  faq: [
    { q: "Is Glint free?", a: "Yes. It's open source under the MIT license, with no ads and no account." },
    { q: "The Microsoft Store or the installer?", a: "Either: it's the same Glint. Microsoft signs the Store's copy, so Windows installs it without a warning, and the Store keeps it up to date. The installer's copy updates itself from GitHub, and it can also add Open in Glint to File Explorer's menu, open folders in Glint by default and make Win+E open Glint, which Windows doesn't let Store apps do." },
    { q: "Why does Windows say it protected my PC?", a: "Glint's installer isn't code-signed yet, and Windows warns about programs it hasn't seen many people run. Choose More info, then Run anyway. All of Glint's code is on GitHub, every update is signed and checked before it installs, and each download's SHA-256 is on its release page. On a PC with Smart App Control on, Windows doesn't run unsigned programs at all: get Glint from the Microsoft Store there, which Microsoft signs." },
    { q: "Does it collect any data?", a: "No. There's no telemetry and no analytics: on its own, Glint only asks GitHub whether there's a new version." },
    { q: "Does it replace File Explorer?", a: "No, it runs next to it, and copying and drag and drop work between the two. Instant search, the index behind the fastest searches, is off until you turn it on. To remove Glint, uninstall it from Settings → Apps." },
  ],
};

export const FOOTER = {
  line: "A free innovation from Rafd.",
  sub: "Rafd builds technology that flows. Some of it, we give away.",
};
