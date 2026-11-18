//! What to put forward: the rows of a menu that fit what it is for, by rule and by habit.
//!
//! Every rule gives one key a score and a reason; what the person did before adds a bonus. Rows the menu does not
//! have (`available`) are never put forward, so a suggestion is always a copy of a row that is there (R3).

use super::facts::Facts;
use super::habits::{Habits, Place};
use super::probed::{Origin, Probed, Signer};
use super::shape::Shape;

/// Why a row is put forward.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    /// It is the plain next step for this kind of thing: no words needed.
    Plain,
    /// An app has the file open: its name.
    InUse(String),
    /// It was downloaded: the site, "" when unknown.
    FromInternet(String),
    /// Nobody signed it.
    Unsigned,
    /// A build left this many bytes in the folder.
    BuildOutput(u64),
    /// The person often does this with this kind of thing.
    Usual,
    /// Where the person usually puts this kind of thing.
    UsualPlace,
}

/// A row put forward.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    /// The row's key (`Run::key` in glint-ui).
    pub key: String,
    pub why: Why,
    pub score: f64,
}

const MB: u64 = 1024 * 1024;
/// The least a row must score to be put forward.
const LEAST: f64 = 2.5;
/// The most a habit can add to a row's score.
const MOST_BONUS: f64 = 8.0;
/// A habit counts only after this many uses.
const USES_FOR_BONUS: u32 = 3;
/// A habit this heavy is worth saying aloud ("You often do this…").
const BONUS_TO_SAY: f64 = 2.0;

/// The rules that hold, in the table's order of PLAN.md's card A3: a key, its score, and its reason.
fn rules(facts: &Facts, probed: Option<&Probed>, available: &[&str]) -> Vec<(&'static str, f64, Why)> {
    let shape = facts.shape;
    let mut out = Vec::new();
    // The origin when the file is marked as from the internet.
    let blocked: Option<&Origin> = probed.and_then(|p| p.origin.as_ref()).filter(|o| o.blocked);
    let host = || blocked.map(|o| o.host.clone()).unwrap_or_default();
    let is_runnable = matches!(shape, Shape::Program | Shape::Installer | Shape::Script);

    // 1
    if let Some(name) = probed.and_then(|p| p.locked_by.as_ref()).and_then(|names| names.first()) {
        out.push(("do:locks", 9.5, Why::InUse(name.clone())));
    }
    // 2: an archive that was extracted already is opened where it went before it is extracted again.
    if shape == Shape::Archive && !facts.fixed {
        let extracted = matches!(probed.and_then(|p| p.extracted.as_ref()), Some(Some(_)));
        if extracted {
            out.push(("do:open-extracted", 9.2, Why::Plain));
        }
        out.push(("do:extract-here", 9.0, Why::Plain));
    }
    // 3
    if shape == Shape::DiskImage && !facts.fixed {
        out.push(("do:mount", 9.0, Why::Plain));
    }
    // 4
    if is_runnable && blocked.is_some() {
        out.push(("do:defender", 8.5, Why::FromInternet(host())));
    }
    // 5: the menu has a Compare row only when there are two files to compare.
    if available.contains(&"do:compare") {
        out.push(("do:compare", 8.0, Why::Plain));
    }
    // 6
    if matches!(shape, Shape::Program | Shape::Installer)
        && matches!(probed.and_then(|p| p.signer.as_ref()), Some(Signer::Unsigned))
    {
        out.push(("do:run-in-sandbox", 7.5, Why::Unsigned));
    }
    // 7: a folder selected, or the folder that is open (its menu with nothing selected; `probed` is its own).
    if matches!(shape, Shape::Folder | Shape::Nothing) {
        if let Some(inside) = probed.and_then(|p| p.inside.as_ref()) {
            if inside.done && inside.junk >= 500 * MB {
                out.push(("do:cleanup", 7.5, Why::BuildOutput(inside.junk)));
            }
        }
    }
    // 8
    if shape == Shape::Pdfs {
        out.push(("do:pdf:merge", 7.0, Why::Plain));
    }
    // 9
    if shape == Shape::Picture && facts.first_name.starts_with("screenshot") {
        out.push(("do:copy-text-in", 6.5, Why::Plain));
    }
    // 10
    if shape == Shape::Video && facts.bytes >= 200 * MB {
        out.push(("do:make-smaller", 6.0, Why::Plain));
    }
    // 11
    if shape == Shape::Data && facts.count >= 2 {
        out.push(("do:query-together", 6.0, Why::Plain));
    }
    // 12
    if shape == Shape::Pictures {
        out.push(("do:convert-pictures", 5.0, Why::Plain));
    }
    // 13
    if blocked.is_some() && !is_runnable {
        out.push(("do:unblock", 5.0, Why::FromInternet(host())));
    }
    // 14
    if matches!(shape, Shape::Song | Shape::Songs) {
        out.push(("do:edit-songs", 4.0, Why::Plain));
    }
    // 15
    if matches!(shape, Shape::Document | Shape::Documents) {
        out.push(("do:convert-document", 3.0, Why::Plain));
    }
    // 16: the row is there only with two panes, and can be chosen only when the other can take items.
    if !facts.fixed {
        out.push(("do:copy-other", 2.5, Why::Plain));
    }
    out
}

/// The rows to put forward, best first, at most `most`. `available` are the keys of the menu's rows that can be
/// chosen, in the menu's order; `probed` is the one target's answers (None for several targets).
pub fn suggest(
    facts: &Facts,
    probed: Option<&Probed>,
    available: &[&str],
    habits: &Habits,
    today: u32,
    most: usize,
) -> Vec<Pick> {
    if most == 0 {
        return Vec::new();
    }
    let rules = rules(facts, probed, available);
    let mut seen: Vec<&str> = Vec::new();
    let mut kept: Vec<Pick> = Vec::new();
    for &key in available {
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if is_basic(key) {
            continue;
        }
        let rule = rules.iter().find(|(k, _, _)| *k == key);
        let uses = habits.action_uses(facts.shape, key);
        let weight = habits.action_weight(facts.shape, key, today);
        let bonus = if uses >= USES_FOR_BONUS { weight.min(MOST_BONUS) } else { 0.0 };
        let score = rule.map_or(0.0, |(_, s, _)| *s) + bonus;
        let why = match rule {
            Some((_, _, why)) if *why != Why::Plain => why.clone(),
            _ if bonus >= BONUS_TO_SAY => Why::Usual,
            _ => Why::Plain,
        };
        if score >= LEAST {
            kept.push(Pick { key: key.to_string(), why, score });
        }
    }
    // A stable sort keeps the menu's order among equal scores.
    kept.sort_by(|a, b| b.score.total_cmp(&a.score));
    kept.truncate(most);
    kept
}

/// A row everyone knows where to find (Cut, Copy, Delete, Properties…): never put forward.
pub fn is_basic(key: &str) -> bool {
    key.starts_with("cmd:")
        || matches!(
            key,
            "task:toggle-pin"
                | "task:windows-menu"
                | "task:copy-text"
                | "task:show-properties"
                | "task:navigate"
                | "task:refresh-drives"
                | "task:get-access"
                | "do:tags"
                | "do:edit-tags"
                | "do:paste-into"
                | "do:paste-queued"
                | "do:open-scripts-folder"
                | "do:reveal-in-file-explorer"
                | "do:reveal-archive"
                | "do:forget-recent"
                | "do:restore"
                | "do:empty-recycle-bin"
                | "do:install-tool"
                | "do:batch-rename-folder"
                | "do:copy-to-choose"
                | "do:move-to-choose"
        )
}

/// The folder this kind of thing usually goes to, if one clearly is: used three times or more, and with six
/// tenths or more of the weight.
pub fn usual_place(places: &[Place]) -> Option<&Place> {
    places.iter().find(|p| p.uses >= 3 && p.share >= 0.6)
}

#[cfg(test)]
mod tests {
    use super::super::habits::Use;
    use super::super::probed::Inside;
    use super::*;

    const TODAY: u32 = 20_000;

    fn facts(shape: Shape) -> Facts {
        Facts { shape, count: 1, ..Facts::default() }
    }

    fn all() -> Vec<&'static str> {
        vec![
            "do:locks",
            "do:extract-here",
            "do:open-extracted",
            "do:mount",
            "do:defender",
            "do:compare",
            "do:run-in-sandbox",
            "do:cleanup",
            "do:pdf:merge",
            "do:copy-text-in",
            "do:make-smaller",
            "do:query-together",
            "do:convert-pictures",
            "do:unblock",
            "do:edit-songs",
            "do:convert-document",
            "do:copy-other",
            "cmd:open",
            "cmd:delete",
            "do:x-ray",
            "do:compress:zip",
        ]
    }

    fn probed() -> Probed {
        Probed::default()
    }

    fn blocked(host: &str) -> Origin {
        Origin { host: host.into(), blocked: true }
    }

    fn inside(junk: u64, done: bool) -> Inside {
        Inside { files: 10, folders: 2, bytes: junk, junk, done, from_index: false }
    }

    fn habit(h: &mut Habits, shape: Shape, key: &str, n: u32, day: u32) {
        h.actions.entry(shape.key().to_string()).or_default().insert(key.to_string(), Use { n, day });
    }

    /// Suggests with no habits. Compare is in the menu only for the shape that has two files to compare (`Files`),
    /// as in the real menu, so the other tests see the rule they test first.
    fn run(f: &Facts, p: Option<&Probed>, available: &[&str]) -> Vec<Pick> {
        let menu: Vec<&str> =
            available.iter().copied().filter(|k| *k != "do:compare" || f.shape == Shape::Files).collect();
        suggest(f, p, &menu, &Habits::default(), TODAY, 10)
    }

    fn first(picks: &[Pick]) -> Option<(&str, f64, &Why)> {
        picks.first().map(|p| (p.key.as_str(), p.score, &p.why))
    }

    fn has(picks: &[Pick], key: &str) -> bool {
        picks.iter().any(|p| p.key == key)
    }

    #[test]
    fn a_file_somebody_has_open_offers_the_locks_first() {
        let p = Probed { locked_by: Some(vec!["Word".into(), "Excel".into()]), ..probed() };
        let picks = run(&facts(Shape::File), Some(&p), &all());
        assert_eq!(first(&picks), Some(("do:locks", 9.5, &Why::InUse("Word".into()))));
        let p = Probed { locked_by: Some(Vec::new()), ..probed() };
        assert!(!has(&run(&facts(Shape::File), Some(&p), &all()), "do:locks"));
        assert!(!has(&run(&facts(Shape::File), None, &all()), "do:locks"));
    }

    #[test]
    fn an_archive_offers_extract_here_unless_it_is_fixed() {
        let picks = run(&facts(Shape::Archive), None, &all());
        assert_eq!(first(&picks), Some(("do:extract-here", 9.0, &Why::Plain)));
        let fixed = Facts { fixed: true, ..facts(Shape::Archive) };
        assert!(!has(&run(&fixed, None, &all()), "do:extract-here"));
    }

    #[test]
    fn an_archive_that_was_extracted_offers_its_folder_before_extracting_again() {
        let done = Probed { extracted: Some(Some("C:\\x\\pack".into())), ..probed() };
        let picks = run(&facts(Shape::Archive), Some(&done), &all());
        assert_eq!(first(&picks), Some(("do:open-extracted", 9.2, &Why::Plain)));
        assert_eq!(picks.get(1).map(|p| p.key.as_str()), Some("do:extract-here"), "extracting stays on offer");
        let not_yet = Probed { extracted: Some(None), ..probed() };
        for p in [Some(&not_yet), None] {
            let picks = run(&facts(Shape::Archive), p, &all());
            assert_eq!(first(&picks), Some(("do:extract-here", 9.0, &Why::Plain)));
            assert!(!has(&picks, "do:open-extracted"));
        }
        // The row has to be in the menu, and an archive in an archive can't be extracted.
        let without: Vec<&str> = all().into_iter().filter(|k| *k != "do:open-extracted").collect();
        assert!(!has(&run(&facts(Shape::Archive), Some(&done), &without), "do:open-extracted"));
        let fixed = Facts { fixed: true, ..facts(Shape::Archive) };
        assert!(!has(&run(&fixed, Some(&done), &all()), "do:open-extracted"));
    }

    #[test]
    fn a_disk_image_offers_mount_unless_it_is_fixed() {
        let picks = run(&facts(Shape::DiskImage), None, &all());
        assert_eq!(first(&picks), Some(("do:mount", 9.0, &Why::Plain)));
        let fixed = Facts { fixed: true, ..facts(Shape::DiskImage) };
        assert!(!has(&run(&fixed, None, &all()), "do:mount"));
    }

    #[test]
    fn a_downloaded_program_offers_defender_and_says_where_from() {
        let p = Probed { origin: Some(blocked("github.com")), ..probed() };
        for shape in [Shape::Program, Shape::Installer, Shape::Script] {
            let picks = run(&facts(shape), Some(&p), &all());
            assert_eq!(first(&picks), Some(("do:defender", 8.5, &Why::FromInternet("github.com".into()))));
        }
        let home = Probed { origin: Some(Origin { host: "x.com".into(), blocked: false }), ..probed() };
        assert!(!has(&run(&facts(Shape::Program), Some(&home), &all()), "do:defender"));
        assert!(!has(&run(&facts(Shape::Picture), Some(&p), &all()), "do:defender"));
    }

    #[test]
    fn compare_is_offered_when_the_menu_has_it() {
        let picks = run(&facts(Shape::Files), None, &all());
        assert_eq!(first(&picks), Some(("do:compare", 8.0, &Why::Plain)));
        let without: Vec<&str> = all().into_iter().filter(|k| *k != "do:compare").collect();
        assert!(!has(&run(&facts(Shape::Files), None, &without), "do:compare"));
    }

    #[test]
    fn an_unsigned_program_offers_the_sandbox() {
        let p = Probed { signer: Some(Signer::Unsigned), ..probed() };
        for shape in [Shape::Program, Shape::Installer] {
            let picks = run(&facts(shape), Some(&p), &all());
            assert_eq!(first(&picks), Some(("do:run-in-sandbox", 7.5, &Why::Unsigned)));
        }
        let signed = Probed { signer: Some(Signer::Signed("Acme".into())), ..probed() };
        assert!(!has(&run(&facts(Shape::Program), Some(&signed), &all()), "do:run-in-sandbox"));
        assert!(!has(&run(&facts(Shape::Script), Some(&p), &all()), "do:run-in-sandbox"));
    }

    #[test]
    fn the_open_folders_own_menu_offers_cleanup_for_its_build_output_too() {
        // Nothing selected: the menu is the open folder's, and `probed` is what is known about that folder.
        let p = Probed { inside: Some(inside(2_000 * MB, true)), ..probed() };
        let picks = run(&facts(Shape::Nothing), Some(&p), &all());
        assert_eq!(first(&picks), Some(("do:cleanup", 7.5, &Why::BuildOutput(2_000 * MB))));
        assert!(!has(&run(&facts(Shape::Nothing), None, &all()), "do:cleanup"), "nothing known, nothing offered");
        let small = Probed { inside: Some(inside(10 * MB, true)), ..probed() };
        assert!(!has(&run(&facts(Shape::Nothing), Some(&small), &all()), "do:cleanup"));
    }

    #[test]
    fn a_folder_full_of_build_output_offers_cleanup() {
        let p = Probed { inside: Some(inside(500 * MB, true)), ..probed() };
        let picks = run(&facts(Shape::Folder), Some(&p), &all());
        assert_eq!(first(&picks), Some(("do:cleanup", 7.5, &Why::BuildOutput(500 * MB))));
        let less = Probed { inside: Some(inside(500 * MB - 1, true)), ..probed() };
        assert!(!has(&run(&facts(Shape::Folder), Some(&less), &all()), "do:cleanup"));
        let counting = Probed { inside: Some(inside(900 * MB, false)), ..probed() };
        assert!(!has(&run(&facts(Shape::Folder), Some(&counting), &all()), "do:cleanup"));
    }

    #[test]
    fn several_pdfs_offer_merge() {
        let picks = run(&facts(Shape::Pdfs), None, &all());
        assert_eq!(first(&picks), Some(("do:pdf:merge", 7.0, &Why::Plain)));
        assert!(!has(&run(&facts(Shape::Pdf), None, &all()), "do:pdf:merge"));
    }

    #[test]
    fn a_screenshot_offers_the_text_in_it() {
        let f = Facts { first_name: "screenshot 2026-11-15.png".into(), ..facts(Shape::Picture) };
        assert_eq!(first(&run(&f, None, &all())), Some(("do:copy-text-in", 6.5, &Why::Plain)));
        let other = Facts { first_name: "holiday.png".into(), ..facts(Shape::Picture) };
        assert!(!has(&run(&other, None, &all()), "do:copy-text-in"));
    }

    #[test]
    fn a_big_video_offers_make_smaller() {
        let f = Facts { bytes: 200 * MB, ..facts(Shape::Video) };
        assert_eq!(first(&run(&f, None, &all())), Some(("do:make-smaller", 6.0, &Why::Plain)));
        let small = Facts { bytes: 199 * MB, ..facts(Shape::Video) };
        assert!(!has(&run(&small, None, &all()), "do:make-smaller"));
    }

    #[test]
    fn two_data_files_offer_to_be_queried_together() {
        let f = Facts { count: 2, ..facts(Shape::Data) };
        assert_eq!(first(&run(&f, None, &all())), Some(("do:query-together", 6.0, &Why::Plain)));
        assert!(!has(&run(&facts(Shape::Data), None, &all()), "do:query-together"));
    }

    #[test]
    fn several_pictures_offer_conversion() {
        let picks = run(&facts(Shape::Pictures), None, &all());
        assert_eq!(first(&picks), Some(("do:convert-pictures", 5.0, &Why::Plain)));
        assert!(!has(&run(&facts(Shape::Picture), None, &all()), "do:convert-pictures"));
    }

    #[test]
    fn a_downloaded_picture_offers_unblock() {
        let p = Probed { origin: Some(blocked("example.org")), ..probed() };
        let picks = run(&facts(Shape::Picture), Some(&p), &all());
        assert_eq!(first(&picks), Some(("do:unblock", 5.0, &Why::FromInternet("example.org".into()))));
        let unblocked = Probed { origin: Some(Origin { host: "example.org".into(), blocked: false }), ..probed() };
        assert!(!has(&run(&facts(Shape::Picture), Some(&unblocked), &all()), "do:unblock"));
    }

    #[test]
    fn songs_offer_the_tag_editor() {
        for shape in [Shape::Song, Shape::Songs] {
            assert_eq!(first(&run(&facts(shape), None, &all())), Some(("do:edit-songs", 4.0, &Why::Plain)));
        }
        assert!(!has(&run(&facts(Shape::Video), None, &all()), "do:edit-songs"));
    }

    #[test]
    fn documents_offer_conversion() {
        for shape in [Shape::Document, Shape::Documents] {
            let picks = run(&facts(shape), None, &all());
            assert_eq!(first(&picks), Some(("do:convert-document", 3.0, &Why::Plain)));
        }
        assert!(!has(&run(&facts(Shape::Pdf), None, &all()), "do:convert-document"));
    }

    #[test]
    fn copy_to_the_other_pane_is_offered_unless_fixed_or_missing() {
        let picks = run(&facts(Shape::File), None, &all());
        assert_eq!(first(&picks), Some(("do:copy-other", 2.5, &Why::Plain)));
        let fixed = Facts { fixed: true, ..facts(Shape::File) };
        assert!(!has(&run(&fixed, None, &all()), "do:copy-other"));
        let without: Vec<&str> = all().into_iter().filter(|k| *k != "do:copy-other").collect();
        assert!(!has(&run(&facts(Shape::File), None, &without), "do:copy-other"));
    }

    #[test]
    fn a_locked_archive_offers_locks_before_extract() {
        let p = Probed { locked_by: Some(vec!["7-Zip".into()]), ..probed() };
        let picks = run(&facts(Shape::Archive), Some(&p), &all());
        let keys: Vec<&str> = picks.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(&keys[..2], ["do:locks", "do:extract-here"]);
    }

    #[test]
    fn a_blocked_unsigned_program_offers_defender_then_sandbox_and_not_unblock() {
        let p = Probed { origin: Some(blocked("evil.example")), signer: Some(Signer::Unsigned), ..probed() };
        let picks = run(&facts(Shape::Program), Some(&p), &all());
        let keys: Vec<&str> = picks.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(&keys[..2], ["do:defender", "do:run-in-sandbox"]);
        assert!(!keys.contains(&"do:unblock"));
    }

    #[test]
    fn nothing_basic_is_ever_returned() {
        let mut h = Habits::default();
        habit(&mut h, Shape::File, "cmd:delete", 100, TODAY);
        habit(&mut h, Shape::File, "task:toggle-pin", 100, TODAY);
        let picks = suggest(&facts(Shape::File), None, &all(), &h, TODAY, 10);
        assert!(picks.iter().all(|p| !is_basic(&p.key)));
        assert!(!has(&picks, "cmd:delete"));
        assert!(is_basic("cmd:anything"));
        assert!(is_basic("do:move-to-choose"));
        assert!(!is_basic("do:x-ray"));
    }

    #[test]
    fn a_key_the_menu_does_not_have_is_not_returned() {
        let p = Probed { locked_by: Some(vec!["Word".into()]), ..probed() };
        let picks = run(&facts(Shape::Archive), Some(&p), &["cmd:open", "do:x-ray"]);
        assert!(picks.is_empty());
    }

    #[test]
    fn two_uses_give_no_bonus_and_three_add_their_weight_up_to_eight() {
        let f = facts(Shape::File);
        let avail = ["cmd:open", "do:x-ray"];
        let mut h = Habits::default();
        habit(&mut h, Shape::File, "do:x-ray", 2, TODAY);
        assert!(suggest(&f, None, &avail, &h, TODAY, 3).is_empty());
        habit(&mut h, Shape::File, "do:x-ray", 3, TODAY);
        let picks = suggest(&f, None, &avail, &h, TODAY, 3);
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].score, 3.0);
        assert_eq!(picks[0].why, Why::Usual);
        habit(&mut h, Shape::File, "do:x-ray", 50, TODAY);
        assert_eq!(suggest(&f, None, &avail, &h, TODAY, 3)[0].score, 8.0);
    }

    #[test]
    fn a_rules_reason_wins_over_usual_and_a_plain_rule_with_a_habit_says_usual() {
        let mut h = Habits::default();
        habit(&mut h, Shape::File, "do:locks", 5, TODAY);
        habit(&mut h, Shape::File, "do:copy-other", 2, TODAY);
        let p = Probed { locked_by: Some(vec!["Word".into()]), ..probed() };
        let picks = suggest(&facts(Shape::File), Some(&p), &all(), &h, TODAY, 5);
        let locks = picks.iter().find(|p| p.key == "do:locks");
        assert_eq!(locks.map(|p| (&p.why, p.score)), Some((&Why::InUse("Word".into()), 14.5)));

        let mut h = Habits::default();
        habit(&mut h, Shape::Archive, "do:extract-here", 2, TODAY);
        habit(&mut h, Shape::Picture, "do:copy-other", 4, TODAY);
        let picks = suggest(&facts(Shape::Picture), None, &all(), &h, TODAY, 5);
        let other = picks.iter().find(|p| p.key == "do:copy-other");
        assert_eq!(other.map(|p| (&p.why, p.score)), Some((&Why::Usual, 6.5)));
    }

    #[test]
    fn old_habits_fade_below_the_bar() {
        let mut h = Habits::default();
        habit(&mut h, Shape::File, "do:x-ray", 3, TODAY - 90);
        assert!(suggest(&facts(Shape::File), None, &["do:x-ray"], &h, TODAY, 3).is_empty());
    }

    #[test]
    fn equal_scores_keep_the_menus_order_and_most_cuts_the_list() {
        let mut h = Habits::default();
        habit(&mut h, Shape::File, "do:a", 3, TODAY);
        habit(&mut h, Shape::File, "do:b", 3, TODAY);
        habit(&mut h, Shape::File, "do:c", 3, TODAY);
        let avail = ["do:c", "do:a", "do:b"];
        let f = facts(Shape::File);
        let keys: Vec<String> = suggest(&f, None, &avail, &h, TODAY, 10).into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["do:c", "do:a", "do:b"]);
        assert_eq!(suggest(&f, None, &avail, &h, TODAY, 2).len(), 2);
        assert!(suggest(&f, None, &avail, &h, TODAY, 0).is_empty());
    }

    #[test]
    fn the_folders_own_menu_has_no_rule_but_offers_a_habit() {
        let f = Facts { shape: Shape::Nothing, ..Facts::default() };
        assert!(suggest(&f, None, &["task:open-terminal", "do:x-ray"], &Habits::default(), TODAY, 3).is_empty());
        let mut h = Habits::default();
        habit(&mut h, Shape::Nothing, "task:open-terminal", 5, TODAY);
        let picks = suggest(&f, None, &["task:open-terminal", "do:x-ray"], &h, TODAY, 3);
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].key, "task:open-terminal");
        assert_eq!(picks[0].why, Why::Usual);
    }

    fn place(uses: u32, share: f64) -> Place {
        Place { folder: r"C:\Invoices".into(), uses, weight: uses as f64, share }
    }

    #[test]
    fn a_usual_place_needs_three_uses_and_six_tenths_of_the_weight() {
        assert!(usual_place(&[place(3, 0.6)]).is_some());
        assert!(usual_place(&[place(2, 0.9)]).is_none());
        assert!(usual_place(&[place(10, 0.5)]).is_none());
        assert!(usual_place(&[]).is_none());
        let places = [place(2, 0.9), place(4, 0.7)];
        assert_eq!(usual_place(&places).map(|p| p.uses), Some(4));
    }
}
