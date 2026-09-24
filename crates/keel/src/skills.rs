//! Adding a skill to the project: one of Keel's vendored ones (`keel_generator::skills`), or a new
//! one the person writes in a form.
//!
//! Both go through [`install`], because both have the same three hazards. **Overwriting** — a
//! folder under `.claude/skills` may be the team's own, so an existing name is refused, and the
//! files are written into a temporary folder and renamed into place, so a failed write leaves
//! nothing that would read as "already added" next time. **The sweep** — the checkpoint after a
//! turn is `git add -A`, so untracked skill files would ride into the next turn's commit under
//! that turn's prompt; the write therefore takes the working tree's writer claim like a turn
//! does, and commits its own path and nothing else. **The shim** — whether `python3` exists is
//! read from `PATH`, never by running it: on a Mac without the Command Line Tools,
//! `/usr/bin/python3` is a stub that opens an installer window.
//!
//! Skills go to the project root, not a lane's checkout: that is where `discover_skills` reads
//! and where the panel lists them, and a skill is repository configuration, like permissions.

use crate::serve::{AppState, blocking};
use crate::writes::{Failure, taken};
use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct Entry {
    id: &'static str,
    description: &'static str,
    license: &'static str,
    author: &'static str,
    files: usize,
    bytes: usize,
    script: Option<&'static str>,
    /// `"project"` or `"user"` when a folder of that name already exists — a personal skill
    /// shadows a project one, so the row has to say which it found.
    present: Option<&'static str>,
    /// Whether the script's interpreter is on `PATH`. Only meaningful when `script` is set.
    python: bool,
}

#[derive(Serialize)]
pub struct Catalog {
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
pub struct Add {
    id: String,
    #[serde(default)]
    commit: bool,
}

#[derive(Deserialize)]
pub struct Create {
    name: String,
    description: String,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    commit: bool,
}

#[derive(Serialize, Debug)]
pub struct Installed {
    /// Repository-relative.
    path: String,
    files: usize,
    /// The short sha of the commit that holds it, when one was made.
    committed: Option<String>,
    /// Why it was not committed, when a commit was asked for and did not happen.
    note: Option<String>,
}

pub async fn catalog(State(state): State<Arc<AppState>>) -> Result<Json<Catalog>, Failure> {
    let root = state.repo();
    blocking(
        move || read_catalog(&root),
        Err("the catalog read was cancelled".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub async fn add(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Add>,
) -> Result<Json<Installed>, Failure> {
    let Some(skill) = keel_generator::skills::find(&req.id) else {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Keel has no skill named {}.", req.id),
        ));
    };
    let files: Vec<(&'static str, &'static [u8])> = skill
        .files
        .iter()
        .map(|(p, b)| (*p, b.as_bytes()))
        .collect();
    blocking(
        move || {
            let present = format!(
                "{} is already in this project — nothing was changed.",
                skill.id
            );
            install(&state, skill.id, &files, req.commit, &present)
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the write was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Create>,
) -> Result<Json<Installed>, Failure> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    if !crate::agents::valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens, at most 64 characters — the name becomes \
             a folder.",
        ));
    }
    let description = req.description.trim();
    if description.is_empty() {
        return Err(bad(
            "Say when to use it: that is the only thing Claude reads when deciding whether to \
             load the skill.",
        ));
    }
    let body = skill_md(&req.name, description, &req.instructions);
    blocking(
        move || {
            let present = format!(
                ".claude/skills/{} already exists — pick another name.",
                req.name
            );
            install(
                &state,
                &req.name,
                &[("SKILL.md", body.as_bytes())],
                req.commit,
                &present,
            )
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the write was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn skill_md(name: &str, description: &str, instructions: &str) -> String {
    let body = if instructions.trim().is_empty() {
        // Said rather than left blank: a skill with no body loads and then tells Claude nothing.
        "Describe what to do when this skill applies: the steps, the commands, the files to read, \
         and what to check before calling it done.\n"
            .to_string()
    } else {
        format!("{}\n", instructions.trim())
    };
    format!(
        "---\nname: {name}\ndescription: {}\n---\n\n{body}",
        crate::agents::yaml(description)
    )
}

fn read_catalog(root: &Utf8Path) -> Result<Catalog, String> {
    let home = std::env::var("HOME").ok().map(Utf8PathBuf::from);
    let python = python3_available();
    let mut entries = Vec::new();
    for v in keel_generator::skills::catalog() {
        let project = root.join(".claude/skills").join(v.id);
        let user = home.as_ref().map(|h| h.join(".claude/skills").join(v.id));
        let present = if taken(&project)? {
            Some("project")
        } else if let Some(u) = user
            && taken(&u)?
        {
            Some("user")
        } else {
            None
        };
        entries.push(Entry {
            id: v.id,
            description: v.description,
            license: v.license,
            author: v.author,
            files: v.files.len(),
            bytes: v.bytes(),
            script: v.script,
            present,
            python,
        });
    }
    Ok(Catalog { entries })
}

/// Whether a working `python3` is on `PATH`, without running one (see `usable_on_path`).
fn python3_available() -> bool {
    crate::permissions::usable_on_path("python3")
}

/// Write `files` into `.claude/skills/<name>/` atomically, under the root's writer claim, and
/// commit that path alone when `commit` is set. `taken` is the refusal when the folder exists.
fn install(
    state: &Arc<AppState>,
    name: &str,
    files: &[(&str, &[u8])],
    commit: bool,
    taken_message: &str,
) -> Result<Installed, Failure> {
    let failed = |m: String| (StatusCode::INTERNAL_SERVER_ERROR, m);
    let root = state.repo();
    let _held = crate::writes::hold(state, &root, &format!("skill:{name}"))?;

    let rel = format!(".claude/skills/{name}");
    crate::writes::no_link_under(&root, &format!("{rel}/SKILL.md"))
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    let dir = root.join(".claude/skills");
    let target = dir.join(name);
    if taken(&target).map_err(failed)? {
        return Err((StatusCode::CONFLICT, taken_message.to_string()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{dir}: {e}")))?;
    // Same directory, same filesystem, so the rename is atomic; removed on drop on every
    // error path, panic included.
    let tmp = tempfile::Builder::new()
        .prefix(".keel-")
        .tempdir_in(&dir)
        .map_err(|e| failed(format!("{dir}: {e}")))?;
    for (path, body) in files {
        let dest = tmp.path().join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| failed(format!("{rel}/{path}: {e}")))?;
        }
        std::fs::write(&dest, body).map_err(|e| failed(format!("{rel}/{path}: {e}")))?;
    }
    let staged = tmp.keep();
    // `rename` onto an existing non-empty directory fails, so a folder that appeared since the
    // check above is not overwritten either.
    if let Err(e) = std::fs::rename(&staged, &target) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(failed(format!("{rel}: {e}")));
    }

    let (committed, note) = if commit {
        let done = crate::writes::commit_only(
            &root,
            std::slice::from_ref(&rel),
            &format!("Add skill {name}"),
        );
        (done.sha, done.note)
    } else {
        (None, None)
    };
    Ok(Installed {
        path: rel,
        files: files.len(),
        committed,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::Held;

    fn repo() -> (tempfile::TempDir, Arc<AppState>, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            crate::git::run(&root, args).unwrap();
        }
        (dir, Arc::new(AppState::new(root.clone())), root)
    }

    const ONE: &[(&str, &[u8])] = &[("SKILL.md", b"---\nname: x\n---\n")];

    #[test]
    fn add_never_overwrites() {
        let (_d, state, root) = repo();
        let mine = root.join(".claude/skills/x/SKILL.md");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::write(&mine, "ours").unwrap();
        let err = install(&state, "x", ONE, false, "taken").unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "ours");
    }

    /// A path whose parent is a file cannot be written; what was written before it must go too,
    /// or the next check reads a half-written folder as "already added".
    #[test]
    fn a_failed_write_leaves_nothing() {
        let (_d, state, root) = repo();
        let files: &[(&str, &[u8])] = &[("a", b"1"), ("a/b", b"2")];
        assert!(install(&state, "x", files, false, "taken").is_err());
        let left: Vec<_> = std::fs::read_dir(root.join(".claude/skills"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn add_waits_for_the_writer() {
        let (_d, state, root) = repo();
        let token = state.claim("lane", &root, true).unwrap();
        let held = Held::new(state.clone(), "lane".into(), token);
        let err = install(&state, "x", ONE, false, "taken").unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.starts_with("A turn is editing"), "{}", err.1);
        assert!(!root.join(".claude/skills/x").exists());
        drop(held);
        install(&state, "x", ONE, false, "taken").unwrap();
        // And its own claim is given back: a second skill is not refused by the first.
        install(&state, "y", ONE, false, "taken").unwrap();
    }

    #[test]
    fn add_commits_only_its_own_path() {
        let (_d, state, root) = repo();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        crate::git::run(&root, &["add", "a.txt"]).unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        let done = install(&state, "x", ONE, true, "taken").unwrap();
        assert!(done.committed.is_some(), "{done:?}");
        let shown = crate::git::run(&root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert_eq!(shown.trim(), ".claude/skills/x/SKILL.md");
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert!(status.contains("A  a.txt"), "{status}");
        assert!(status.contains("?? b.txt"), "{status}");
    }

    /// A folder that is not a repository still gets its skill; the commit that could not happen
    /// is a note, not a failure.
    #[test]
    fn outside_a_repository_it_writes_and_says_why_it_did_not_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        let state = Arc::new(AppState::new(root.clone()));
        let done = install(&state, "x", ONE, true, "taken").unwrap();
        assert!(done.committed.is_none() && done.note.is_some(), "{done:?}");
        assert!(root.join(".claude/skills/x/SKILL.md").is_file());
    }

    #[test]
    fn a_created_skill_is_listed_with_its_description() {
        let (_d, state, root) = repo();
        let md = skill_md("notes", "Use this: for notes", "");
        install(
            &state,
            "notes",
            &[("SKILL.md", md.as_bytes())],
            false,
            "taken",
        )
        .unwrap();
        let found = keel_workspace::discover_skills(&root, &root.join("no-home"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "notes");
        assert_eq!(found[0].description.as_deref(), Some("Use this: for notes"));
    }

    #[tokio::test]
    async fn create_refuses_taken_and_climbing_names() {
        let (_d, state, root) = repo();
        let ask = |name: &str| Create {
            name: name.into(),
            description: "when".into(),
            instructions: String::new(),
            commit: false,
        };
        for bad in ["../x", "Foo", "a/b", "-x", ""] {
            let err = create(State(state.clone()), Json(ask(bad)))
                .await
                .unwrap_err();
            assert_eq!(err.0, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert!(!root.join("x").exists() && !root.join(".claude").exists());
        let made = create(State(state.clone()), Json(ask("notes")))
            .await
            .unwrap();
        assert_eq!(made.path, ".claude/skills/notes");
        let again = create(State(state.clone()), Json(ask("notes")))
            .await
            .unwrap_err();
        assert_eq!(again.0, StatusCode::CONFLICT);
        let blank = Create {
            description: "  ".into(),
            ..ask("other")
        };
        assert!(create(State(state), Json(blank)).await.is_err());
    }

    #[test]
    fn the_vendored_skill_installs_whole_and_its_script_runs() {
        let (_d, state, root) = repo();
        let v = keel_generator::skills::find("ui-ux-pro-max").unwrap();
        let files: Vec<(&str, &[u8])> = v.files.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        let done = install(&state, v.id, &files, true, "taken").unwrap();
        assert_eq!(done.files, v.files.len());
        let catalog = read_catalog(&root).unwrap();
        assert_eq!(catalog.entries[0].present, Some("project"));
        if python3_available() {
            let out = std::process::Command::new("python3")
                .current_dir(&root)
                .args([
                    ".claude/skills/ui-ux-pro-max/scripts/search.py",
                    "saas dashboard",
                    "--design-system",
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(String::from_utf8_lossy(&out.stdout).contains("DESIGN SYSTEM"));
        }
    }
}
