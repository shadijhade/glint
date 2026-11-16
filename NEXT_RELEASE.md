# Glint: after 0.13

*0.13.0 came out on 2026-11-16 ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.0)), and
0.13.1, a fix for its zoom, the same day ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.1)).
Nothing is planned after them yet: this file is where the next release's brief goes.
The plan for 0.12 that was here is in git history (`git show df1245a:NEXT_RELEASE.md`), the plans for 0.9 to 0.11
before it (`git show 47c33fa:NEXT_RELEASE.md`), and 0.13's own plan, the menu and the dock that know the selection,
is [docs/gpui/aware/PLAN.md](docs/gpui/aware/PLAN.md) with its task cards beside it.*

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
- **Two tests fail once in a while when the whole suite runs** and pass alone:
  `dialogs::timeline::tests::another_time_window_scans_again_and_forgets_the_subfolder` and
  `core_bridge::tests::jobs_posted_from_any_thread_run_on_the_ui_thread_from_the_message_pump` (any test's
  `testing::init` replaces the process's message window).
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
