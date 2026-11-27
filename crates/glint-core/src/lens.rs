//! Cheap answers for the folder head's smart buttons: whether a folder has a history to scrub (Time Machine).
//! Each asks the sources of [`crate::time_machine`] for one hit instead of the whole timeline, and none is meant
//! for the UI thread.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// The shadow copies of each drive (by its letter, upper case), asked once per session: listing them can take a
/// moment (the service, or VSS itself), and a new one a minute ago isn't worth asking again for.
static SHADOWS: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);

/// Whether a shadow copy of `folder`'s drive holds the folder.
fn in_a_shadow_copy(folder: &str) -> bool {
    let Ok(norm) = crate::fsx::normalize(folder) else {
        return false;
    };
    let Some(drive) = norm.get(..2).filter(|d| d.ends_with(':')) else {
        return false;
    };
    let drive = drive.to_ascii_uppercase();
    let rest = norm[2..].trim_start_matches('\\');
    let known = SHADOWS.lock().unwrap_or_else(|e| e.into_inner()).get(&drive).cloned();
    let roots = known.unwrap_or_else(|| {
        let roots: Vec<String> = crate::versions::snapshots()
            .into_iter()
            .filter(|s| s.drive.eq_ignore_ascii_case(&drive))
            .map(|s| s.root)
            .collect();
        SHADOWS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(drive.clone(), roots.clone());
        roots
    });
    roots
        .iter()
        .any(|root| crate::fsx::stat(&format!("{root}{rest}")).is_some_and(|s| s.dir))
}

/// Whether `folder` has something for Time Machine to scrub: a Git repository with a commit that holds it, a
/// backup plan that covers it, or a shadow copy that has it. (The change journal isn't asked: it is the slowest
/// source and rarely the only one.)
pub fn has_history(folder: &str) -> bool {
    crate::git::folder_has_commits(folder) || crate::backup::has_plan_for(folder) || in_a_shadow_copy(folder)
}
