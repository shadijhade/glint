//! Project buttons: what a folder's project files say can be run there, from the folder up to its
//! repository's root (a few levels at most): package.json's scripts (with the runner its lock file
//! names), justfile recipes (as just lists them), Cargo's usual commands, and pyproject.toml's
//! scripts through uv. The UI runs one in the terminal pane, typed at a shell's prompt.

use crate::fsx::blocking;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How far up from the folder Glint looks, without a repository to stop at.
const MAX_UP: usize = 6;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub name: String,
    /// What's typed at the prompt, in the group's folder.
    pub command: String,
    /// What it does: the script's own command line, a recipe's comment.
    pub detail: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroup {
    pub folder: String,
    /// "npm", "pnpm", "yarn", "bun", "just", "cargo" or "uv".
    pub source: String,
    pub tasks: Vec<Task>,
    /// The tool these need that isn't installed (its id in Settings → Tools), if any.
    pub missing: Option<String>,
}

fn task(name: &str, command: String, detail: &str) -> Task {
    Task { name: name.to_string(), command, detail: detail.to_string() }
}

/// A script's or a recipe's name as typed: quoted when it has a space.
fn word(name: &str) -> String {
    if name.contains(' ') {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

/// The runner a JavaScript project's lock file names, looked for in `dir` and up to `root`.
fn js_runner(dir: &Path, root: &Path) -> &'static str {
    for at in dir.ancestors() {
        for (lock, runner) in [("bun.lock", "bun"), ("bun.lockb", "bun"), ("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("package-lock.json", "npm")] {
            if at.join(lock).is_file() {
                return runner;
            }
        }
        if at == root {
            break;
        }
    }
    "npm"
}

fn npm_scripts(dir: &Path, root: &Path) -> Option<TaskGroup> {
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("package.json")).ok()?).ok()?;
    let runner = js_runner(dir, root);
    let run = if runner == "npm" { "npm run" } else { runner };
    let tasks: Vec<Task> = json["scripts"].as_object()?.iter().filter_map(|(name, line)| Some(task(name, format!("{run} {}", word(name)), line.as_str()?))).collect();
    (!tasks.is_empty()).then(|| TaskGroup { folder: dir.to_string_lossy().into_owned(), source: runner.into(), tasks, missing: None })
}

fn cargo_tasks(dir: &Path) -> Option<TaskGroup> {
    dir.join("Cargo.toml").is_file().then(|| TaskGroup {
        folder: dir.to_string_lossy().into_owned(),
        source: "cargo".into(),
        tasks: [("build", "Builds it"), ("run", "Builds and runs it"), ("test", "Runs its tests"), ("check", "Checks it compiles, quickly"), ("clippy", "Lints it")]
            .iter()
            .map(|(name, detail)| task(name, format!("cargo {name}"), detail))
            .collect(),
        missing: None,
    })
}

/// just's recipes from `just --dump --dump-format json`: the public ones, with their comments.
fn just_recipes(json: &str) -> Vec<Task> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let mut tasks: Vec<Task> = value["recipes"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, r)| !r["private"].as_bool().unwrap_or(false))
        .map(|(name, r)| task(name, format!("just {}", word(name)), r["doc"].as_str().unwrap_or("")))
        .collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    tasks
}

fn just_tasks(dir: &Path) -> Option<TaskGroup> {
    let file = ["justfile", "Justfile", ".justfile"].iter().map(|n| dir.join(n)).find(|p| p.is_file())?;
    let folder = dir.to_string_lossy().into_owned();
    let Ok(mut just) = crate::tools::command("just") else {
        return Some(TaskGroup { folder, source: "just".into(), tasks: Vec::new(), missing: Some("just".into()) });
    };
    let out = just.arg("--justfile").arg(&file).args(["--dump", "--dump-format", "json"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let tasks = just_recipes(&String::from_utf8_lossy(&out.stdout));
    (!tasks.is_empty()).then_some(TaskGroup { folder, source: "just".into(), tasks, missing: None })
}

/// pyproject.toml's `[project.scripts]`, each run through uv, after `uv sync`.
fn uv_tasks(dir: &Path) -> Option<TaskGroup> {
    let text = std::fs::read_to_string(dir.join("pyproject.toml")).ok()?;
    let mut tasks = vec![task("sync", "uv sync".into(), "Installs what it needs")];
    let mut in_scripts = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_scripts = line == "[project.scripts]";
        } else if let Some((name, target)) = line.split_once('=').filter(|_| in_scripts) {
            let name = name.trim().trim_matches('"');
            if !name.is_empty() && !name.starts_with('#') {
                tasks.push(task(name, format!("uv run {}", word(name)), target.trim().trim_matches('"')));
            }
        }
    }
    let missing = crate::tools::command("uv").is_err().then(|| "uv".to_string());
    Some(TaskGroup { folder: dir.to_string_lossy().into_owned(), source: "uv".into(), tasks, missing })
}

/// The folders looked in: `folder`, then each above it up to its repository's root (or a few levels).
fn folders_up(folder: &Path) -> Vec<PathBuf> {
    let root = gix::discover(folder).ok().and_then(|repo| repo.workdir().map(Path::to_path_buf));
    let mut found = Vec::new();
    for at in folder.ancestors().take(MAX_UP + 1) {
        found.push(at.to_path_buf());
        if root.as_deref().is_some_and(|r| r.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(at.to_string_lossy().trim_end_matches(['\\', '/']))) {
            break;
        }
        // Outside a repository, only the folder itself.
        if root.is_none() {
            break;
        }
    }
    found
}

/// What can be run in `folder` and above it, nearest first.
#[tauri::command]
pub async fn project_tasks(folder: String) -> Result<Vec<TaskGroup>, String> {
    blocking(move || {
        let folders = folders_up(Path::new(&folder));
        let root = folders.last().cloned().unwrap_or_default();
        let mut groups = Vec::new();
        for dir in &folders {
            groups.extend(npm_scripts(dir, &root));
            groups.extend(just_tasks(dir));
            groups.extend(cargo_tasks(dir));
            groups.extend(uv_tasks(dir));
        }
        Ok(groups)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_projects_can_run() {
        let dir = std::env::temp_dir().join(format!("glint-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::write(dir.join("web").join("package.json"), r#"{"name":"web","scripts":{"dev":"vite","build:all":"tsc && vite build","my task":"echo"}}"#).unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();
        std::fs::write(dir.join("pyproject.toml"), "[project]\nname = \"tool\"\n\n[project.scripts]\nserve = \"tool.main:serve\"\n\"two words\" = \"tool:x\"\n\n[tool.ruff]\nline = 1\n").unwrap();

        // The lock file above the package names its runner.
        let web = npm_scripts(&dir.join("web"), &dir).unwrap();
        assert_eq!(web.source, "pnpm");
        assert_eq!(web.tasks.iter().map(|t| t.command.as_str()).collect::<Vec<_>>(), ["pnpm build:all", "pnpm dev", "pnpm \"my task\""]);
        assert_eq!(web.tasks[0].detail, "tsc && vite build");
        assert_eq!(cargo_tasks(&dir).unwrap().tasks[0].command, "cargo build");
        assert!(cargo_tasks(&dir.join("web")).is_none());
        let uv = uv_tasks(&dir).unwrap();
        assert_eq!(uv.tasks.iter().map(|t| t.command.as_str()).collect::<Vec<_>>(), ["uv sync", "uv run serve", "uv run \"two words\""]);
        assert_eq!(js_runner(&dir.join("web"), &dir.join("web")), "npm", "not above the root");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_justs_public_recipes() {
        let json = r#"{"recipes":{"test":{"name":"test","doc":"Runs the tests","private":false},"_helper":{"private":true},"build":{"doc":null,"private":false}}}"#;
        let tasks = just_recipes(json);
        assert_eq!(tasks.iter().map(|t| (t.command.as_str(), t.detail.as_str())).collect::<Vec<_>>(), [("just build", ""), ("just test", "Runs the tests")]);
        assert!(just_recipes("not json").is_empty());
    }
}
