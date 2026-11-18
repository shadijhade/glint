# Glint: after 0.13

*0.13.0 came out on 2026-11-16 ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.0)),
0.13.1, a fix for its zoom, the same day ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.1)),
and 0.13.2, updates that need nobody, that evening ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.2)).
The next release is being got ready: what it has so far is under **In the next release**.
The plan for 0.12 that was here is in git history (`git show df1245a:NEXT_RELEASE.md`), the plans for 0.9 to 0.11
before it (`git show 47c33fa:NEXT_RELEASE.md`), and 0.13's own plan, the menu and the dock that know the selection,
is [docs/gpui/aware/PLAN.md](docs/gpui/aware/PLAN.md) with its task cards beside it.*

## In the next release

### Back and Forward that remember

The user's report: a search ("Personal Stuff") listed files and folders, a folder opened from the results, and
Backspace could not get back to them (history held folders only, and opening anything ended the search). Back and
Forward now walk places, each kept as it was left (`crates/glint-ui/src/explorer/history.rs`):

- A search is a place after the folder it began in. Back from a folder opened in its results brings them back at
  once, without searching again, the folder selected and the list where it was; Back once more is the folder as it
  was before the search. Typing on refines the same place; Escape, the clear button, emptying the box or Up leave
  it for its folder, and Forward brings it back.
- Every place keeps its selection, its focused item and its scroll (a tab used to keep one scroll place).
- Only the six searches nearest the tab keep their results (200,000 rows at most together); an older one, or one
  whose results were still coming in when it was left, runs again. Results deleted, moved or renamed meanwhile are
  taken out of what comes back (a quiet look in the background).
- A folder gone from the disk is skipped with "is no longer available"; one that fails for another reason stays.
- Backspace goes up when there is nothing to go back to (`BackOrUp`). Held down, it keeps stepping back.
- The toolbar's arrows say where they go ("Back to the results for “Personal Stuff”"), read that folder ahead
  while the pointer is on them, and a right-click lists the places to jump straight to.

Tests: `explorer/history_tests.rs`, the address bar's and the window's (`Backspace`, the mouse's side buttons),
and `tests/e2e`'s new `search-history` check with the real keyboard and side buttons.

### Faster where it is felt

- Sizes and counts in the list, the tiles and the status bar are written without asking Windows: the digits are
  grouped here, with the separator and the grouping Windows gives the locale (read once a run), and toFixed and
  Math.round are worked out on integers (`crates/glint-model/src/format.rs`). A size took eight calls into Windows
  and eleven allocations, on every row of every frame: 38 ns now against 552 ns (`format::tests::sizes_cost_little`),
  the same text over 200,000 sizes, and the grouping the same as Windows' own in seven locales. Region settings'
  "123456789" (no grouping) is honoured now; a change to the region settings shows from Glint's next start.

## 0.13.2: updates that need nobody

1884328. Two bugs the user met: a Glint in the tray showed a new version only after a restart (a 12-hour timer
Windows doesn't run while the PC sleeps, a check that stopped once it found a version, and a notice that faded
unseen in a hidden window), and Scoop kept listing the old version after an update from the app (only the running
copy was replaced; this PC had a Scoop copy beside the setup's). Around them, the update flow of
`crates/glint-ui/src/updater.rs`: hourly looks by the wall clock, a quiet download picked up where it broke off
(not on a metered connection) and kept across restarts, installing on quit or after a start with Windows that
nobody opened, Restart to update, Scoop copies updated through Scoop (`updater/scoop.rs`), notices that wait for
the window, Windows' own notification, and Settings → About's **Install updates automatically**.

Not tried: Windows' own notification from an installed copy, and an update installed silently on quit over a real
installed Glint (both from the dev build's dry run only: `GLINT_UPDATE_DRY_RUN`). Copies before 0.13.2 install it
the old way once (Update, then Install).

## 0.13.1: the zoom

Out on 2026-11-16, a few hours after 0.13.0: one fix on top of it (cb669ef, made on the other PC). `Ctrl` and the wheel, or a pinch, opened the folder under
the pointer and went up to the one above; that was never what the zoom was for. It changes the layout now, as one
ladder (the list, then the grid's tiles from the smallest to the biggest), and never the folder. The rest of the
motion is quieter: opening a folder is a small step, not a flight. `tests/e2e`'s `zoom-layout` holds it with the
real wheel.

Not tried: a pinch on a real touchpad (made-up events only).

## Left over from 0.13

Known when it shipped, none of it holding the release:

- **Windows 10 was not tried on a real PC.** The cause of [issue #6](https://github.com/shadijhade/glint/issues/6)
  is gone from the import tables, and `npm run check-imports` holds every release to what Windows 10 1809 has, but
  nobody has started 0.13 on Windows 10 yet.
- **The Store's copy shares its data folder with a setup copy** on the same PC, until `glint_core::host` picks
  `com.shado.glint.store` when Glint runs as a package ([docs/releasing.md](docs/releasing.md#as-an-msix-package)).
- **Suggested depends on what is known when the menu opens.** A signer's check can take longer than the 40 ms the
  menu waits, so a program right-clicked the instant the pointer arrives may get one suggestion where a moment
  later it gets two. The head catches up; the rows don't move (rule R2 of the plan).
- **Paste's words are for files only.** A picture or text on the clipboard leaves Paste a plain button.
- **Three tests fail once in a while when the whole suite runs** and pass alone:
  `dialogs::timeline::tests::another_time_window_scans_again_and_forgets_the_subfolder`,
  `core_bridge::tests::jobs_posted_from_any_thread_run_on_the_ui_thread_from_the_message_pump` and
  `core_bridge::tests::a_panicking_job_does_not_take_the_ui_down` (seen 2026-11-18; any test's `testing::init`
  replaces the process's message window).
- **Never tried for real:** the start-up note that offers to index every drive (instant search is on here), and a
  tool installed through winget's fallback.

## How a release is made now

`npm run ship -- <x.y.z>` ([docs/releasing.md](docs/releasing.md#one-command-npm-run-ship)): the checks, the version,
the build and its signatures on GitHub, the release, Scoop, winget and the Microsoft Store. 0.13.0 was the first
made this way, and 0.13.1 the second: built and signed on this PC (`--local`, one password), then the release,
Scoop and winget by `ship`, and the Store's package put in Partner Center by hand.

**Since 2026-11-16 the build needs nobody.** The update key and its password are the repository's secrets, put
there by `npm run ship -- --setup` ([docs/releasing.md](docs/releasing.md#a-release-that-needs-nobody)), which
checked them here first. The **Release check** workflow proved GitHub signs with them, and a try run of the
Release workflow built and signed 0.13.1 there in 22 minutes: its update signature and its service's signature
hold here, checked the way the app checks them. So `npm run ship -- <x.y.z>` asks for no password, from any PC
with `gh` signed in, and `--local` stays for a build on this PC.

One thing is left for a release to need nobody at all, once:

- **The Store's application.** No Microsoft Entra directory is associated with the Partner Center account yet
  (seen 2026-11-16), so first, in Partner Center: associate one, add a Microsoft Entra application with the
  Manager role, and make its key. `npm run ship -- --setup` then asks for its tenant ID, client ID and key, tries
  them against the Store and stores them; from then on `ship` sends the package through the **Store** workflow.
  Until then the package is uploaded in Partner Center's pages. `npm run store` has only ever run against a
  stand-in (`npm run test:store`): the first real run should be a look (**Actions → Store → Run workflow**, mode
  *look*), which sends nothing. The workflow itself was tried without the application: it packed 0.13.1's
  package from the released setup program, the same programs as the one packed here.

Never tried for real: a submission through the Store's API, and a release made from a build signed on GitHub
(the try run's files were checked, not released; `ship` checks every build the same way before it publishes).
