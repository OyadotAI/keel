//! Adding a skill to the project: one of Keel's vendored ones (`keel_generator::skills`), or a new
//! one generated from a sentence ([`generate`]) and installed once the person has read it.
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
    /// A generated skill's files, as the person was shown them. Empty means the form's
    /// `SKILL.md` alone.
    #[serde(default)]
    files: Vec<File>,
    #[serde(default)]
    commit: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct File {
    path: String,
    content: String,
}

#[derive(Deserialize)]
pub struct Generate {
    ask: String,
    /// Optional: the model names it when this is blank.
    #[serde(default)]
    name: String,
}

#[derive(Serialize, Debug)]
pub struct Generated {
    name: String,
    description: String,
    files: Vec<File>,
    /// What was checked before it was shown: said, because "it compiled" and "nothing could
    /// check it" must not look the same.
    check: String,
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
    let files: Vec<(String, String)> = if req.files.is_empty() {
        vec![(
            "SKILL.md".into(),
            skill_md(&req.name, description, &req.instructions),
        )]
    } else {
        // What the app sends back is what `generate` returned, but it crosses the wire, so the
        // paths are checked again: a file path is where a write lands.
        checked_files(&req.files, &req.name, true).map_err(|e| bad(&e))?
    };
    blocking(
        move || {
            let present = format!(
                ".claude/skills/{} already exists — pick another name.",
                req.name
            );
            let files: Vec<(&str, &[u8])> = files
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_bytes()))
                .collect();
            install(&state, &req.name, &files, req.commit, &present)
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the write was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

#[derive(Deserialize)]
pub struct Remove {
    /// The skill's folder, as the listing reported it (the parent of its `SKILL.md`).
    dir: String,
    #[serde(default)]
    commit: bool,
}

#[derive(Serialize, Debug)]
pub struct Removed {
    committed: Option<String>,
    note: Option<String>,
}

/// A skill the listing shows, found by its folder. Every route that touches an existing skill
/// starts here: a folder the listing did not produce is not one Keel will read, write or trash,
/// so a request cannot name `~/.ssh` and have it moved to the Trash.
struct Located {
    dir: Utf8PathBuf,
    scope: keel_workspace::Scope,
    /// The folder's name, which is what Claude Code keys a skill by.
    folder: String,
    generated: bool,
}

fn claude_home() -> Utf8PathBuf {
    keel_workspace::claude_home().unwrap_or_else(|| "/nonexistent".into())
}

fn locate(repo: &Utf8Path, home: &Utf8Path, dir: &str) -> Result<Located, Failure> {
    let mut all = keel_workspace::discover_skills(repo, home);
    all.extend(keel_workspace::plugin_skills(
        &keel_workspace::discover_plugins(home),
    ));
    let wanted = Utf8Path::new(dir);
    all.into_iter()
        .find(|s| s.path.parent() == Some(wanted))
        .and_then(|s| {
            Some(Located {
                folder: wanted.file_name()?.to_string(),
                dir: wanted.to_owned(),
                scope: s.scope,
                generated: s.generated,
            })
        })
        .ok_or((
            StatusCode::NOT_FOUND,
            "That skill is not there any more — it may have been moved or removed.".into(),
        ))
}

const PLUGIN_OWNED: &str = "A plugin's skills are replaced whenever the plugin updates — copy it to \
     Project or Personal first, or disable the plugin in Plugins.";

/// The repository-relative folder of a project skill.
fn project_rel(folder: &str) -> String {
    format!(".claude/skills/{folder}")
}

/// Move a skill's folder to the Trash — never unlinked, because a hand-written personal skill is
/// the only copy there is (`fsops::trash`). A project skill is a write into the tree a turn may be
/// writing, so it takes the writer claim like `install`, and commits its own removal and nothing
/// else when it was tracked.
pub async fn remove(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Remove>,
) -> Result<Json<Removed>, Failure> {
    blocking(
        move || {
            let root = state.repo();
            let found = locate(&root, &claude_home(), &req.dir)?;
            match found.scope {
                keel_workspace::Scope::Project => {
                    let _held =
                        crate::writes::hold(&state, &root, &format!("skill:{}", found.folder))?;
                    let done = trash_project_skill(&root, &found.folder, req.commit)?;
                    Ok(Removed {
                        committed: done.sha,
                        note: done.note,
                    })
                }
                keel_workspace::Scope::Plugin => Err((
                    StatusCode::BAD_REQUEST,
                    "A plugin's skills go with the plugin — disable or uninstall it in Plugins."
                        .into(),
                )),
                _ => {
                    crate::fsops::trash(found.dir.as_std_path())
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
                    Ok(Removed {
                        committed: None,
                        note: None,
                    })
                }
            }
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the removal was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

/// Trash `.claude/skills/<folder>` and commit its removal when it was tracked. The caller holds
/// the writer claim.
fn trash_project_skill(
    root: &Utf8Path,
    folder: &str,
    commit: bool,
) -> Result<crate::writes::Committed, Failure> {
    let rel = project_rel(folder);
    crate::writes::no_link_under(root, &format!("{rel}/SKILL.md"))
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    let tracked =
        crate::git::read(root, &["ls-files", "--", &rel]).is_some_and(|l| !l.trim().is_empty());
    crate::fsops::trash(root.join(&rel).as_std_path())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(if commit && tracked {
        crate::writes::commit_only(
            root,
            std::slice::from_ref(&rel),
            &format!("Remove skill {folder}"),
        )
    } else {
        Default::default()
    })
}

// ── Reading, revising and moving an existing skill ────────────────────────────────────────────

const MAX_SHOWN_FILES: usize = 200;

#[derive(Serialize, Debug)]
pub struct Shown {
    path: String,
    /// Absent for a binary file, or one too large to show.
    content: Option<String>,
    bytes: u64,
}

#[derive(Serialize, Debug)]
pub struct Folder {
    files: Vec<Shown>,
    /// Said when the walk stopped at `MAX_SHOWN_FILES`.
    truncated: bool,
}

#[derive(Deserialize)]
pub struct DirQuery {
    dir: String,
}

/// Every file in a skill's folder — the whole skill, not the one line the row shows. Links are
/// listed as what they are and never followed, so a folder cannot be used to read outside it.
pub async fn files(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<DirQuery>,
) -> Result<Json<Folder>, Failure> {
    blocking(
        move || {
            let found = locate(&state.repo(), &claude_home(), &q.dir)?;
            Ok(read_folder(&found.dir))
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the read was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn read_folder(dir: &Utf8Path) -> Folder {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut stack = vec![dir.to_owned()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(e.path()) else {
                continue;
            };
            let name = path.file_name().unwrap_or("");
            // `.DS_Store`, `__pycache__` and editor droppings are not part of the skill.
            if name.starts_with('.') || name == "__pycache__" {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if files.len() >= MAX_SHOWN_FILES {
                truncated = true;
                continue;
            }
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_string();
            let content = (meta.is_file() && meta.len() <= MAX_FILE_BYTES as u64)
                .then(|| std::fs::read(&path).ok())
                .flatten()
                .and_then(|b| String::from_utf8(b).ok());
            files.push(Shown {
                path: rel,
                content,
                bytes: meta.len(),
            });
        }
    }
    files.sort_by(|a, b| {
        (a.path != "SKILL.md", a.path.contains('/'), &a.path).cmp(&(
            b.path != "SKILL.md",
            b.path.contains('/'),
            &b.path,
        ))
    });
    Folder { files, truncated }
}

#[derive(Deserialize)]
pub struct Revise {
    dir: String,
    ask: String,
}

/// A change to an existing skill, described in a sentence and written by the person's own
/// `claude` — the same no-tools one-shot as [`generate`]. Returns only the files it changed or
/// added, for the pane to show as a diff; nothing is written until [`save`].
pub async fn revise(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Revise>,
) -> Result<Json<Generated>, Failure> {
    let ask = req.ask.trim().to_string();
    if ask.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Say what to change.".into()));
    }
    blocking(
        move || {
            let found = locate(&state.repo(), &claude_home(), &req.dir)?;
            if found.scope == keel_workspace::Scope::Plugin {
                return Err((StatusCode::BAD_REQUEST, PLUGIN_OWNED.into()));
            }
            let current = read_folder(&found.dir);
            let mut feedback = String::new();
            for _ in 0..2 {
                let text = ask_claude(&revision_prompt(&found.folder, &current, &ask, &feedback))?;
                match parse_revision(&text, &found.folder, &current).and_then(check_scripts) {
                    Ok(done) => return Ok(done),
                    Err(why) => feedback = why,
                }
            }
            Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Claude's change did not hold up after a second try: {feedback}"),
            ))
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the revision was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn revision_prompt(folder: &str, current: &Folder, ask: &str, feedback: &str) -> String {
    let mut prompt = format!(
        "You maintain Claude Code skills. A skill is a folder under `.claude/skills/<name>/`; \
         Claude Code reads its SKILL.md frontmatter (`name`, `description` — the description is \
         all it reads when deciding to load the skill) and follows the body, which may tell it \
         to run the skill's scripts.\n\nThis is the skill `{folder}`, every text file in it:\n\n"
    );
    for f in &current.files {
        match &f.content {
            Some(c) => prompt.push_str(&format!("<<<FILE {}>>>\n{c}\n<<<END>>>\n", f.path)),
            None => prompt.push_str(&format!(
                "({} — binary, {} bytes, not shown)\n",
                f.path, f.bytes
            )),
        }
    }
    prompt.push_str(&format!(
        "\nMake this change, and nothing beyond it:\n\n\"{ask}\"\n\n\
         Output ONLY the files you change or add, each one complete, between marker lines \
         exactly like the ones above (`<<<FILE path>>>` … `<<<END>>>`), and nothing else — no \
         prose, no code fences around the markers. Keep `name: {folder}` and any other \
         frontmatter keys you were not asked to change. Scripts print one line of JSON and read \
         credentials from the environment. Never call an LLM API from a script: Claude is the \
         LLM running the skill.\n"
    ));
    if !feedback.is_empty() {
        prompt.push_str(&format!(
            "\n## Your previous attempt was rejected — fix this and output the files again\n{}\n",
            feedback.chars().take(2000).collect::<String>()
        ));
    }
    prompt
}

/// The files a revision returned, checked; unchanged ones dropped, so the diff shows only work.
fn parse_revision(text: &str, folder: &str, current: &Folder) -> Result<Generated, String> {
    let blocks = parse_blocks(text)?;
    if blocks.is_empty() {
        return Err("There were no <<<FILE>>> blocks.".into());
    }
    let files: Vec<File> = checked_files(&blocks, folder, false)?
        .into_iter()
        .filter(|(p, c)| {
            current
                .files
                .iter()
                .find(|f| &f.path == p)
                .and_then(|f| f.content.as_deref())
                != Some(c.as_str())
        })
        .map(|(path, content)| File { path, content })
        .collect();
    if files.is_empty() {
        return Err("Every file came back unchanged — make the change that was asked.".into());
    }
    let md = files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .map(|f| f.content.clone())
        .or_else(|| {
            current
                .files
                .iter()
                .find(|f| f.path == "SKILL.md")
                .and_then(|f| f.content.clone())
        })
        .unwrap_or_default();
    let (_, description) = frontmatter(&md);
    if description.is_empty() {
        return Err("SKILL.md has no description in its frontmatter.".into());
    }
    Ok(Generated {
        name: folder.to_string(),
        description,
        files,
        check: String::new(),
    })
}

#[derive(Deserialize)]
pub struct Save {
    dir: String,
    files: Vec<File>,
    #[serde(default)]
    commit: bool,
}

/// Write the files a revision proposed and the person accepted into the skill's folder. Only
/// those files: nothing else in the folder is touched, so a binary asset the model never saw
/// cannot be lost to it. Each file is written beside itself and renamed over, so a failure
/// leaves the old one whole.
pub async fn save(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Save>,
) -> Result<Json<Installed>, Failure> {
    blocking(
        move || {
            let root = state.repo();
            let found = locate(&root, &claude_home(), &req.dir)?;
            if found.scope == keel_workspace::Scope::Plugin {
                return Err((StatusCode::BAD_REQUEST, PLUGIN_OWNED.into()));
            }
            let files = checked_files(&req.files, &found.folder, false)
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let project = found.scope == keel_workspace::Scope::Project;
            let _held = if project {
                Some(crate::writes::hold(
                    &state,
                    &root,
                    &format!("skill:{}", found.folder),
                )?)
            } else {
                None
            };
            for (path, content) in &files {
                crate::writes::no_link_under(&found.dir, path)
                    .map_err(|e| (StatusCode::CONFLICT, e))?;
                write_beside(&found.dir.join(path), content.as_bytes())
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{path}: {e}")))?;
            }
            let rel = if project {
                project_rel(&found.folder)
            } else {
                found.dir.to_string()
            };
            let (committed, note) = if project && req.commit {
                let done = crate::writes::commit_only(
                    &root,
                    std::slice::from_ref(&rel),
                    &format!("Edit skill {}", found.folder),
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
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the save was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

/// Write `path` by renaming a sibling over it, creating its folder when it is new.
fn write_beside(path: &Utf8Path, body: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Utf8Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, body)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct Move {
    dir: String,
    /// `"project"` or `"user"`.
    to: String,
    /// Leave the original where it is. Always the case for a plugin's skill.
    #[serde(default)]
    copy: bool,
    #[serde(default)]
    commit: bool,
}

#[derive(Serialize, Debug)]
pub struct Moved {
    /// The skill's folder now.
    dir: String,
    committed: Option<String>,
    note: Option<String>,
}

/// Put a skill in the other place: Project ⇄ Personal, a plugin's skill copied into either, and a
/// generated skill kept in Project — which is the one move that stays in its folder and only
/// drops the `generated-by` mark, because "Generated" is where it came from, not where it lives.
///
/// Wherever it lands it stops being marked generated: moving it is the person adopting it. The
/// copy is staged in a temporary folder beside the target and renamed into place, so a failure
/// leaves nothing half-copied; an existing skill of that name at the target is never overwritten.
pub async fn relocate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Move>,
) -> Result<Json<Moved>, Failure> {
    blocking(
        move || {
            use keel_workspace::Scope;
            let root = state.repo();
            let home = claude_home();
            let found = locate(&root, &home, &req.dir)?;
            let to = match req.to.as_str() {
                "project" => Scope::Project,
                "user" => Scope::User,
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Move it to project or user.".into(),
                    ));
                }
            };
            // Only the project side is a write into the tree a turn may be writing.
            let _held = if to == Scope::Project || found.scope == Scope::Project {
                Some(crate::writes::hold(
                    &state,
                    &root,
                    &format!("skill:{}", found.folder),
                )?)
            } else {
                None
            };
            let md = found.dir.join("SKILL.md");
            let unmarked = std::fs::read_to_string(&md)
                .map(|t| unmark(&t))
                .unwrap_or_default();

            if found.scope == to {
                if !(to == Scope::Project && found.generated) {
                    return Err((StatusCode::BAD_REQUEST, "It is already there.".into()));
                }
                write_beside(&md, unmarked.as_bytes())
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let done = if req.commit {
                    crate::writes::commit_only(
                        &root,
                        &[project_rel(&found.folder)],
                        &format!("Keep skill {}", found.folder),
                    )
                } else {
                    Default::default()
                };
                return Ok(Moved {
                    dir: found.dir.to_string(),
                    committed: done.sha,
                    note: done.note,
                });
            }

            let parent = match to {
                Scope::Project => root.join(".claude/skills"),
                _ => home.join("skills"),
            };
            if to == Scope::Project {
                crate::writes::no_link_under(
                    &root,
                    &format!("{}/SKILL.md", project_rel(&found.folder)),
                )
                .map_err(|e| (StatusCode::CONFLICT, e))?;
            }
            let target = parent.join(&found.folder);
            let where_ = if to == Scope::Project {
                "Project"
            } else {
                "Personal"
            };
            if taken(&target).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))? {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "{where_} already has a skill named {} — nothing was changed.",
                        found.folder
                    ),
                ));
            }
            copy_skill(&found.dir, &parent, &target, &unmarked)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{target}: {e}")))?;

            let copy = req.copy || found.scope == Scope::Plugin;
            let mut notes = Vec::new();
            let mut committed = None;
            if to == Scope::Project && req.commit {
                let done = crate::writes::commit_only(
                    &root,
                    &[project_rel(&found.folder)],
                    &format!("Add skill {}", found.folder),
                );
                committed = done.sha;
                notes.extend(done.note);
            }
            if !copy {
                if found.scope == Scope::Project {
                    let done = trash_project_skill(&root, &found.folder, req.commit)?;
                    committed = done.sha.or(committed);
                    notes.extend(done.note);
                } else {
                    crate::fsops::trash(found.dir.as_std_path())
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
                }
            }
            Ok(Moved {
                dir: target.to_string(),
                committed,
                note: (!notes.is_empty()).then(|| notes.join("; ")),
            })
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the move was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

/// `SKILL.md` without Keel's `generated-by` mark, and without the `metadata:` key when the mark
/// was all it held.
fn unmark(md: &str) -> String {
    md.replace("metadata:\n  generated-by: keel\n", "")
        .replace("  generated-by: keel\n", "")
}

/// Copy a skill's files — regular files and folders, never a link — into a temporary folder in
/// `parent`, then rename it to `target`. `SKILL.md` is written as `md`.
fn copy_skill(
    src: &Utf8Path,
    parent: &Utf8Path,
    target: &Utf8Path,
    md: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::Builder::new()
        .prefix(".keel-")
        .tempdir_in(parent)?;
    let mut stack = vec![(src.to_owned(), Utf8PathBuf::from(""))];
    while let Some((from, rel)) = stack.pop() {
        std::fs::create_dir_all(tmp.path().join(rel.as_str()))?;
        for e in std::fs::read_dir(&from)? {
            let e = e?;
            let Ok(path) = Utf8PathBuf::from_path_buf(e.path()) else {
                continue;
            };
            let name = path.file_name().unwrap_or("").to_string();
            if name == "__pycache__" || name == ".DS_Store" {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path)?;
            let to = rel.join(&name);
            if meta.is_dir() {
                stack.push((path, to));
            } else if meta.is_file() {
                std::fs::copy(&path, tmp.path().join(to.as_str()))?;
            }
        }
    }
    if !md.is_empty() {
        std::fs::write(tmp.path().join("SKILL.md"), md)?;
    }
    let staged = tmp.keep();
    std::fs::rename(&staged, target).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staged);
    })
}

/// How long one `claude -p` may take to write a skill. Two attempts at most, so a generate ends
/// inside ten minutes whatever the model does; the app's request waits a little longer than that.
const GENERATE_CEILING: std::time::Duration = std::time::Duration::from_secs(240);
const MAX_FILES: usize = 20;
const MAX_FILE_BYTES: usize = 256 * 1024;

/// Write a skill from a sentence with the person's own `claude`: `SKILL.md`, a `schema.json`
/// tool schema, the script and any assets. Nothing is written into the project here — the
/// result goes back to the sheet to be read, and `create` installs what was shown.
///
/// The model gets no tools (`--tools ""`, `--strict-mcp-config`) and runs in an empty temporary
/// directory, so it can only answer in text; nothing is saved as a session to show up in the
/// project's history. The scripts are parsed with `ast` and never run: executing code nobody has
/// read yet is an effect without a decision.
pub async fn generate(Json(req): Json<Generate>) -> Result<Json<Generated>, Failure> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    let ask = req.ask.trim().to_string();
    if ask.is_empty() {
        return Err(bad("Say what the skill should do."));
    }
    let name = req.name.trim().to_string();
    if !name.is_empty() && !crate::agents::valid_name(&name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens, at most 64 characters — the name becomes \
             a folder.",
        ));
    }
    blocking(
        move || {
            let mut feedback = String::new();
            for _ in 0..2 {
                let text = ask_claude(&generation_prompt(&ask, &name, &feedback))?;
                match parse_generated(&text, &name).and_then(check_scripts) {
                    Ok(done) => return Ok(done),
                    Err(why) => feedback = why,
                }
            }
            Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Claude's skill did not hold up after a second try: {feedback}"),
            ))
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the generation was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn generation_prompt(ask: &str, name: &str, feedback: &str) -> String {
    let named = if name.is_empty() {
        "Pick a kebab-case name for it (lowercase letters, digits, hyphens).".to_string()
    } else {
        format!("Its name is `{name}`.")
    };
    let mut prompt = format!(
        r#"You write Claude Code skills. A skill is a folder under `.claude/skills/<name>/` in a repository; Claude Code reads its SKILL.md frontmatter to decide when to load it, then follows the body, which tells it to run the skill's script with Bash.

Write ONE single-purpose skill that does exactly this and nothing more:

"{ask}"

{named}

## Files
- `SKILL.md` — YAML frontmatter between `---` lines with exactly two keys: `name` and `description` (one sentence saying WHEN to use it — that sentence is all Claude reads when deciding to load the skill; quote it). Then the body: an H1, one line on what it does, a "How to run" section giving the exact command `python3 .claude/skills/<name>/scripts/run.py '<json input>'` with a real example, what the output looks like, the environment variables it needs and where to get them, and a short "When to use" note.
- `schema.json` — the tool schema, as JSON: {{"name": "<snake_case>", "description": "...", "parameters": {{"type": "object", "properties": {{...typed, each with a description...}}, "required": [...]}}, "env": [{{"name": "ENV_VAR", "description": "what it is and where to get it"}}]}}. `env` lists EVERY credential the script reads, and is `[]` when there are none.
- `scripts/run.py` — Python 3, standard library only unless the job genuinely needs a package (then say so under "How to run" with the `pip install` line). Read the input with `json.loads(sys.argv[1] if len(sys.argv) > 1 else sys.stdin.read() or "{{}}")`. Validate required inputs with a clear message. Read credentials from `os.environ` — never hardcode them, never take them as input. Do the one job and print exactly ONE line of JSON to stdout: the result, or {{"error": "..."}} with exit code 1. No other prints. Use timeouts on every network call. If it writes a file, put its path in the result.
- `assets/<path>` or `references/<path>` — only if the skill genuinely needs supporting text files (templates, reference data). Usually none.

Claude itself is the LLM running the skill: never call an LLM API from the script. If the job needs judgement (summarising, rewriting, classifying), the script gathers and returns the data and SKILL.md tells Claude what to do with it.

## Output format
Output each file between marker lines, exactly like this, and nothing else — no prose, no code fences around the markers:

<<<FILE SKILL.md>>>
...contents...
<<<END>>>
<<<FILE schema.json>>>
...contents...
<<<END>>>
<<<FILE scripts/run.py>>>
...contents...
<<<END>>>
"#
    );
    if !feedback.is_empty() {
        prompt.push_str(&format!(
            "\n## Your previous attempt was rejected — fix this and output every file again\n{}\n",
            feedback.chars().take(2000).collect::<String>()
        ));
    }
    prompt
}

/// One `claude -p` with no tools, in an empty directory, answered as JSON.
fn ask_claude(prompt: &str) -> Result<String, Failure> {
    let gateway = |m: String| (StatusCode::BAD_GATEWAY, m);
    let dir = tempfile::tempdir().map_err(|e| gateway(format!("a temporary folder: {e}")))?;
    let mut command = std::process::Command::new("claude");
    command.current_dir(dir.path()).args([
        "-p",
        prompt,
        "--output-format",
        "json",
        "--tools",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
    ]);
    let out = crate::git::output_within(command, GENERATE_CEILING).map_err(|e| {
        gateway(match e.kind() {
            std::io::ErrorKind::NotFound => {
                "claude is not on PATH — install Claude Code to generate skills.".into()
            }
            _ => format!("claude {e}"),
        })
    })?;
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        result: String,
        #[serde(default)]
        is_error: bool,
    }
    let reply: Option<Reply> = serde_json::from_slice(&out.stdout).ok();
    match reply {
        Some(r) if out.status.success() && !r.is_error => Ok(r.result),
        Some(r) if !r.result.trim().is_empty() => Err(gateway(format!("claude: {}", r.result))),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            Err(gateway(format!(
                "claude exited with {}: {last}",
                out.status
            )))
        }
    }
}

/// The `<<<FILE path>>>` … `<<<END>>>` blocks, checked, with the frontmatter's name and
/// description read out. Markers rather than code fences, because a `SKILL.md` is full of fences.
fn parse_generated(text: &str, wanted: &str) -> Result<Generated, String> {
    let files = parse_blocks(text)?;
    let md = files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .ok_or("There was no SKILL.md block.")?;
    let (name, description) = frontmatter(&md.content);
    let name = if wanted.is_empty() {
        name
    } else {
        wanted.to_string()
    };
    if !crate::agents::valid_name(&name) {
        return Err(format!("The name `{name}` is not kebab-case."));
    }
    if description.is_empty() {
        return Err("SKILL.md has no description in its frontmatter.".into());
    }
    let files = checked_files(&files, &name, true)?
        .into_iter()
        .map(|(path, content)| File {
            content: if path == "SKILL.md" {
                mark_generated(&content)
            } else {
                content
            },
            path,
        })
        .collect();
    Ok(Generated {
        name,
        description,
        files,
        check: String::new(),
    })
}

fn parse_blocks(text: &str) -> Result<Vec<File>, String> {
    let mut files = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<<<FILE ") {
        let after = &rest[start + 8..];
        let Some(close) = after.find(">>>") else {
            break;
        };
        let path = after[..close].trim().to_string();
        let body = after[close + 3..]
            .strip_prefix('\n')
            .unwrap_or(&after[close + 3..]);
        let Some(end) = body.find("<<<END>>>") else {
            return Err(format!("{path} has no <<<END>>> line."));
        };
        files.push(File {
            path,
            content: body[..end].trim_end().to_string() + "\n",
        });
        rest = &body[end + 9..];
    }
    Ok(files)
}

/// `name` and `description` from a `SKILL.md`'s frontmatter, unquoted. Blank when absent.
fn frontmatter(md: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let Some(head) = md.strip_prefix("---\n") else {
        return (name, description);
    };
    for line in head.lines().take_while(|l| l.trim() != "---") {
        let value = |v: &str| {
            let v = v.trim();
            v.strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v)
                .replace("\\\"", "\"")
        };
        if let Some(v) = line.strip_prefix("name:") {
            name = value(v);
        } else if let Some(v) = line.strip_prefix("description:") {
            description = value(v);
        }
    }
    (name, description)
}

/// Paths a skill's files may take, with `SKILL.md`'s `name:` set to the folder it goes in. Every
/// path is relative and climbs nowhere; this is the check that stands between a model's output
/// and a write. A `new` skill must have a `SKILL.md` and keeps to the folders a skill has; an
/// edit to an existing one may touch any file already shaped however its author shaped it.
fn checked_files(files: &[File], name: &str, new: bool) -> Result<Vec<(String, String)>, String> {
    if files.len() > MAX_FILES {
        return Err(format!("{} files is more than a skill needs.", files.len()));
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for f in files {
        let path = Utf8Path::new(&f.path);
        let plain = path
            .components()
            .all(|c| matches!(c, camino::Utf8Component::Normal(_)));
        let placed = !new
            || matches!(f.path.as_str(), "SKILL.md" | "schema.json")
            || ["scripts/", "assets/", "references/"]
                .iter()
                .any(|d| f.path.starts_with(d));
        let hidden = path.components().any(|c| c.as_str().starts_with('.'));
        if f.path.is_empty() || !plain || !placed || hidden {
            return Err(format!("`{}` is not a place a skill keeps files.", f.path));
        }
        if f.content.len() > MAX_FILE_BYTES {
            return Err(format!("{} is larger than 256 KB.", f.path));
        }
        if out.iter().any(|(p, _)| p == &f.path) {
            return Err(format!("{} appears twice.", f.path));
        }
        let content = if f.path == "SKILL.md" {
            set_name(&f.content, name)
        } else {
            f.content.clone()
        };
        out.push((f.path.clone(), content));
    }
    if new && !out.iter().any(|(p, _)| p == "SKILL.md") {
        return Err("There is no SKILL.md.".into());
    }
    if let Some((_, schema)) = out.iter().find(|(p, _)| p == "schema.json")
        && let Err(e) = serde_json::from_str::<serde_json::Value>(schema)
    {
        return Err(format!("schema.json is not JSON: {e}"));
    }
    Ok(out)
}

/// The frontmatter's `name:` line, pointed at the folder the skill is installed as.
fn set_name(md: &str, name: &str) -> String {
    let mut in_head = false;
    let mut out = String::with_capacity(md.len());
    for (i, line) in md.lines().enumerate() {
        if line.trim() == "---" {
            in_head = i == 0;
        }
        if in_head && line.starts_with("name:") {
            out.push_str(&format!("name: {name}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The mark the panel's Generated tab reads, after the `name:` line. Skipped when the model wrote
/// its own `metadata:`, because a second one is a duplicate key and breaks the whole file.
fn mark_generated(md: &str) -> String {
    if md.contains("generated-by: keel") || md.lines().any(|l| l.starts_with("metadata:")) {
        return md.to_string();
    }
    match md.find("\nname:") {
        Some(at) => {
            let end = md[at + 1..].find('\n').map_or(md.len(), |e| at + 1 + e + 1);
            format!(
                "{}metadata:\n  generated-by: keel\n{}",
                &md[..end],
                &md[end..]
            )
        }
        None => md.to_string(),
    }
}

/// Parse every Python file with `ast` — a syntax error goes back to the model — without running
/// any of them. `python3` is found on `PATH`, never run to find out (see `python3_available`).
fn check_scripts(mut done: Generated) -> Result<Generated, String> {
    let scripts: Vec<&File> = done
        .files
        .iter()
        .filter(|f| f.path.ends_with(".py"))
        .collect();
    if scripts.is_empty() {
        done.check = "No scripts to check.".into();
        return Ok(done);
    }
    if !python3_available() {
        done.check = "python3 is not on PATH, so the scripts were not checked.".into();
        return Ok(done);
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new("python3");
    command.current_dir(dir.path()).args([
        "-c",
        "import ast,sys\nfor p in sys.argv[1:]: ast.parse(open(p).read(), p)",
    ]);
    for (i, f) in scripts.iter().enumerate() {
        let local = dir.path().join(format!("{i}.py"));
        std::fs::write(&local, &f.content).map_err(|e| e.to_string())?;
        command.arg(&local);
    }
    let out = crate::git::output_within(command, std::time::Duration::from_secs(15))
        .map_err(|e| format!("python3 {e}"))?;
    if !out.status.success() {
        // The temporary names mean nothing to the model; the file's own path does.
        let mut err = String::from_utf8_lossy(&out.stderr).into_owned();
        for (i, f) in scripts.iter().enumerate() {
            err = err.replace(
                &dir.path().join(format!("{i}.py")).display().to_string(),
                &f.path,
            );
        }
        return Err(format!("A script does not parse:\n{err}"));
    }
    done.check = format!(
        "{} script{} parsed with python3 — not run.",
        scripts.len(),
        if scripts.len() == 1 { "" } else { "s" }
    );
    Ok(done)
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
            files: Vec::new(),
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

    const REPLY: &str = "Here you go.\n<<<FILE SKILL.md>>>\n---\nname: wrong-name\ndescription: \"Use when: pricing a coin\"\n---\n\n# Price\n```bash\npython3 x\n```\n<<<END>>>\n<<<FILE schema.json>>>\n{\"name\": \"price\", \"parameters\": {}, \"env\": []}\n<<<END>>>\n<<<FILE scripts/run.py>>>\nprint(1)\n<<<END>>>\n";

    #[test]
    fn a_generated_reply_parses_into_its_files_and_takes_the_asked_name() {
        let g = parse_generated(REPLY, "coin-price").unwrap();
        assert_eq!(g.name, "coin-price");
        assert_eq!(g.description, "Use when: pricing a coin");
        let paths: Vec<_> = g.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["SKILL.md", "schema.json", "scripts/run.py"]);
        assert!(
            g.files[0]
                .content
                .contains("name: coin-price\nmetadata:\n  generated-by: keel\n")
        );
        assert!(
            g.files[0].content.contains("```bash"),
            "fences inside a file survive"
        );
        // Unnamed, the model's own name stands.
        assert_eq!(parse_generated(REPLY, "").unwrap().name, "wrong-name");
    }

    #[test]
    fn generated_paths_cannot_leave_the_skill() {
        for bad in [
            "../x",
            "/etc/x",
            "scripts/../../x",
            "run.py",
            "",
            ".git/config",
        ] {
            let files = [
                File {
                    path: "SKILL.md".into(),
                    content: "---\nname: a\n---\n".into(),
                },
                File {
                    path: bad.into(),
                    content: "x".into(),
                },
            ];
            assert!(checked_files(&files, "a", true).is_err(), "{bad}");
        }
        let broken = [
            File {
                path: "SKILL.md".into(),
                content: "x".into(),
            },
            File {
                path: "schema.json".into(),
                content: "{".into(),
            },
        ];
        assert!(checked_files(&broken, "a", true).is_err());
        assert!(
            checked_files(&[], "a", true).is_err(),
            "SKILL.md is required"
        );
    }

    #[test]
    fn a_script_that_does_not_parse_is_sent_back_and_one_that_does_is_not_run() {
        if !python3_available() {
            return;
        }
        let mut g = parse_generated(REPLY, "coin-price").unwrap();
        g.files[2].content = "open('ran', 'w')\n".into();
        let ok = check_scripts(g).unwrap();
        assert!(ok.check.contains("not run"), "{}", ok.check);
        let mut g = parse_generated(REPLY, "coin-price").unwrap();
        g.files[2].content = "def (:\n".into();
        let err = check_scripts(g).unwrap_err();
        assert!(err.contains("scripts/run.py"), "{err}");
    }

    #[test]
    fn a_generated_skill_installs_every_file() {
        let (_d, state, root) = repo();
        let g = parse_generated(REPLY, "coin-price").unwrap();
        let files: Vec<(&str, &[u8])> = g
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.content.as_bytes()))
            .collect();
        install(&state, "coin-price", &files, false, "taken").unwrap();
        assert!(
            root.join(".claude/skills/coin-price/scripts/run.py")
                .is_file()
        );
        let found = keel_workspace::discover_skills(&root, &root.join("no-home"));
        assert_eq!(found[0].name, "coin-price");
        assert!(found[0].generated);
    }

    #[tokio::test]
    async fn removing_a_project_skill_commits_its_removal_and_nothing_else() {
        let (_d, state, root) = repo();
        install(&state, "x", ONE, true, "taken").unwrap();
        std::fs::write(root.join("mine.txt"), "mine").unwrap();
        let ask = |dir: String| Remove { dir, commit: true };
        let dir = root.join(".claude/skills/x").to_string();
        let done = remove(State(state.clone()), Json(ask(dir.clone())))
            .await
            .unwrap();
        assert!(done.committed.is_some(), "{:?}", done.note);
        assert!(!root.join(".claude/skills/x").exists());
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert_eq!(status.trim(), "?? mine.txt");
        let again = remove(State(state), Json(ask(dir))).await.unwrap_err();
        assert_eq!(again.0, StatusCode::NOT_FOUND);
    }

    /// Only a folder the listing produced can be read, written or trashed.
    #[tokio::test]
    async fn a_folder_the_listing_did_not_produce_is_refused() {
        let (_d, state, root) = repo();
        std::fs::create_dir_all(root.join("secret")).unwrap();
        std::fs::write(root.join("secret/SKILL.md"), "---\nname: s\n---\n").unwrap();
        let dir = root.join("secret").to_string();
        let err = remove(
            State(state.clone()),
            Json(Remove {
                dir: dir.clone(),
                commit: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert!(root.join("secret/SKILL.md").exists());
        let q = axum::extract::Query(DirQuery { dir });
        assert!(files(State(state), q).await.is_err());
    }

    #[tokio::test]
    async fn a_skill_is_read_whole_and_saved_file_by_file() {
        let (_d, state, root) = repo();
        let g = parse_generated(REPLY, "coin-price").unwrap();
        let written: Vec<(&str, &[u8])> = g
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.content.as_bytes()))
            .collect();
        install(&state, "coin-price", &written, false, "taken").unwrap();
        let dir = root.join(".claude/skills/coin-price");
        std::fs::write(dir.join("logo.png"), [0u8, 159, 146, 150]).unwrap();

        let q = axum::extract::Query(DirQuery {
            dir: dir.to_string(),
        });
        let shown = files(State(state.clone()), q).await.unwrap();
        let paths: Vec<_> = shown.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["SKILL.md", "logo.png", "schema.json", "scripts/run.py"]
        );
        assert!(
            shown.files[1].content.is_none(),
            "binary is listed, not shown"
        );

        // A revision returns only what changed, and saving it leaves the rest alone.
        let current = read_folder(&dir);
        let text = "<<<FILE scripts/run.py>>>\nprint(2)\n<<<END>>>\n<<<FILE schema.json>>>\n{\"name\": \"price\", \"parameters\": {}, \"env\": []}\n<<<END>>>";
        let r = parse_revision(text, "coin-price", &current).unwrap();
        assert_eq!(r.files.len(), 1, "schema.json came back unchanged");
        assert!(parse_revision("no blocks", "coin-price", &current).is_err());
        let saved = save(
            State(state),
            Json(Save {
                dir: dir.to_string(),
                files: r.files,
                commit: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(saved.files, 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("scripts/run.py")).unwrap(),
            "print(2)\n"
        );
        assert_eq!(
            std::fs::read(dir.join("logo.png")).unwrap(),
            [0u8, 159, 146, 150]
        );
    }

    #[tokio::test]
    async fn keeping_a_generated_skill_drops_its_mark_in_place() {
        let (_d, state, root) = repo();
        let g = parse_generated(REPLY, "coin-price").unwrap();
        let written: Vec<(&str, &[u8])> = g
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.content.as_bytes()))
            .collect();
        install(&state, "coin-price", &written, true, "taken").unwrap();
        let dir = root.join(".claude/skills/coin-price");
        let moved = relocate(
            State(state),
            Json(Move {
                dir: dir.to_string(),
                to: "project".into(),
                copy: false,
                commit: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(moved.dir, dir.to_string());
        assert!(moved.committed.is_some(), "{:?}", moved.note);
        let found = keel_workspace::discover_skills(&root, &root.join("no-home"));
        assert!(!found[0].generated);
        let md = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(!md.contains("metadata:"), "{md}");
    }

    #[test]
    fn a_copied_skill_arrives_whole_without_links() {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(d.path()).unwrap();
        let src = root.join("src/x");
        std::fs::create_dir_all(src.join("scripts")).unwrap();
        std::fs::write(src.join("SKILL.md"), "old").unwrap();
        std::fs::write(src.join("scripts/run.py"), "print(1)").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", src.join("hosts")).unwrap();
        let parent = root.join("dst");
        copy_skill(&src, &parent, &parent.join("x"), "new").unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.join("x/SKILL.md")).unwrap(),
            "new"
        );
        assert!(parent.join("x/scripts/run.py").is_file());
        assert!(!parent.join("x/hosts").exists());
        assert_eq!(
            std::fs::read_dir(&parent).unwrap().count(),
            1,
            "no staging left"
        );
        assert_eq!(
            unmark("---\nname: x\nmetadata:\n  generated-by: keel\n---\n"),
            "---\nname: x\n---\n"
        );
    }
}
