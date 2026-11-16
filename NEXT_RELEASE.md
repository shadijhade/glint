# Glint: after 0.13

*0.13.0 came out on 2026-11-16 ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.0)). Nothing
is planned after it yet: this file is where the next release's brief goes.
The plan for 0.12 that was here is in git history (`git show df1245a:NEXT_RELEASE.md`), the plans for 0.9 to 0.11
before it (`git show 47c33fa:NEXT_RELEASE.md`), and 0.13's own plan, the menu and the dock that know the selection,
is [docs/gpui/aware/PLAN.md](docs/gpui/aware/PLAN.md) with its task cards beside it.*

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
made this way: built and signed on this PC (`--local`, one password), then the release, Scoop and winget by
`ship`, and the Store's package put in Partner Center by hand. Two things are left for it to need nobody, each
done once:

- **The update key, where the build is.** The repository's `TAURI_SIGNING_PRIVATE_KEY` secret has been empty
  since it was made (2026-10-22; every run says `HAS_UPDATER_KEY: false`), so GitHub builds Glint but can't sign
  the update, and `ship` stops there rather than release something installed copies can't take by themselves.
  Put the key in the secret (`Get-Content "$HOME\.tauri\shadui-ripgrep.key" -Raw | gh secret set
  TAURI_SIGNING_PRIVATE_KEY`), or build on this PC with `npm run ship -- --local`, which asks for its password.
- **The Store API's application and key**, made in Partner Center
  ([how](docs/releasing.md#sending-it-with-npm-run-store)); until then the Store's package is uploaded there by
  hand.
