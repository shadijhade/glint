# Glint: after 0.13

*0.13.0 was released on 2026-11-15. Nothing is planned here yet: this file is where the next release's brief goes.
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
