//! Moving a project into the Keel workspace: one same-disk rename, and everything keyed by the
//! project's path carried to where it went.
//!
//! The person decides it (`POST`), after Keel says what it would do or why it will not (`GET`).
//! The rename is the first write and everything before it is a check, so a refusal leaves the
//! project exactly where it was. A rename within one disk is atomic whatever the size, and a shell
//! whose working directory is inside follows the folder. What the rename does not carry — git's
//! absolute lane paths, Claude Code's per-path history folders, Keel's trust store, its vouched
//! records, its known sessions — is carried after it, each named in the reply if it failed.
use crate::serve::{AppState, Held, blocking};
use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use serde::Serialize;
use std::sync::Arc;

/// Why a project will not be moved, by code: the page owns the sentences, since only it knows a
/// lane by its title.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "code", rename_all = "lowercase")]
pub enum Refusal {
    /// Already in the workspace.
    Inside,
    /// The folder holds the workspace itself.
    Contains,
    Link {
        target: String,
    },
    Worktree {
        main: String,
    },
    Submodule {
        parent: String,
    },
    Nested {
        root: String,
    },
    Taken {
        to: String,
    },
    Volume,
    Turn {
        lane: String,
    },
    Terminal,
    /// Claude Code running in this folder outside Keel.
    Live,
    Dev,
    Job {
        command: String,
    },
    Workspace {
        error: String,
    },
    Rename {
        error: String,
    },
    Repair {
        error: String,
    },
}

/// Where a move would take the project, and why not when it would not.
#[derive(Serialize, Debug)]
pub struct Plan {
    pub from: String,
    pub to: String,
    pub refusal: Option<Refusal>,
}

/// A follow-up that did not land; the project moved regardless.
#[derive(Serialize, Debug)]
pub struct Warning {
    pub step: &'static str,
    pub error: String,
}

#[derive(Serialize, Debug)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub warnings: Vec<Warning>,
}

/// The places a move reads and writes beyond the project itself, so tests use scratch ones.
pub struct Roots {
    pub workspace: Utf8PathBuf,
    pub claude: Option<Utf8PathBuf>,
    pub moved: Option<Utf8PathBuf>,
}

impl Roots {
    fn local() -> Result<Self, String> {
        let workspace = keel_workspace::locations::projects()?;
        Ok(Self {
            workspace: Utf8PathBuf::from_path_buf(workspace)
                .map_err(|_| "The workspace path is not text.".to_string())?,
            claude: keel_workspace::claude_home(),
            moved: keel_workspace::home().map(|h| h.join(keel_workspace::moved::FILE)),
        })
    }
}

/// The nearest folder of `path` that exists, resolved, with the rest put back: the workspace may
/// not have been made yet, and its real place is what a same-disk check and a comparison need.
fn resolved(path: &Utf8Path) -> Utf8PathBuf {
    let mut rest = Vec::new();
    let mut at = path.to_owned();
    loop {
        if let Ok(real) = at.real() {
            return rest
                .iter()
                .rev()
                .fold(real, |p: Utf8PathBuf, part| p.join(part));
        }
        match (at.file_name().map(str::to_string), at.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                at = parent.to_owned();
            }
            _ => return path.to_owned(),
        }
    }
}

/// Every check that needs nothing but the folder and the workspace.
fn structure(from: &Utf8Path, workspace: &Utf8Path) -> (Utf8PathBuf, Result<Utf8PathBuf, Refusal>) {
    let to_of = |real: &Utf8Path| resolved(workspace).join(real.file_name().unwrap_or("project"));
    if let Ok(target) = std::fs::read_link(from) {
        let target = target.to_string_lossy().into_owned();
        return (to_of(from), Err(Refusal::Link { target }));
    }
    let real = from.real().unwrap_or_else(|_| from.to_owned());
    let to = to_of(&real);
    let ws = resolved(workspace);
    if real.starts_with(&ws) {
        return (to, Err(Refusal::Inside));
    }
    if ws.starts_with(&real) {
        return (to, Err(Refusal::Contains));
    }
    if let Some(refusal) = repository(&real) {
        return (to, Err(refusal));
    }
    if std::fs::symlink_metadata(&to).is_ok() {
        return (to.clone(), Err(Refusal::Taken { to: to.to_string() }));
    }
    if !same_disk(&real, &to) {
        return (to, Err(Refusal::Volume));
    }
    (to.clone(), Ok(to))
}

/// A folder that belongs to another repository stays with it: a linked worktree, a submodule, a
/// folder inside a checkout. A folder that is not a repository at all may move.
fn repository(real: &Utf8Path) -> Option<Refusal> {
    let ask = |args: &[&str]| {
        crate::git::trimmed(real, args)
            .ok()
            .filter(|s| !s.is_empty())
    };
    let top = ask(&["rev-parse", "--show-toplevel"])?;
    let top = Utf8Path::new(&top)
        .real()
        .unwrap_or_else(|_| top.clone().into());
    if let Some(parent) = ask(&["rev-parse", "--show-superproject-working-tree"]) {
        return Some(Refusal::Submodule { parent });
    }
    if top != real {
        return Some(Refusal::Nested {
            root: top.to_string(),
        });
    }
    let common = ask(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = Utf8Path::new(&common)
        .real()
        .unwrap_or_else(|_| common.clone().into());
    (common != real.join(".git")).then(|| Refusal::Worktree {
        main: common.parent().unwrap_or(&common).to_string(),
    })
}

#[cfg(unix)]
fn same_disk(from: &Utf8Path, to: &Utf8Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let device = |p: &Utf8Path| {
        p.ancestors()
            .find_map(|a| std::fs::metadata(a).ok())
            .map(|m| m.dev())
    };
    device(from) == device(to)
}

/// The drive or share each is on: Windows' volume serial is not stable through std, and the
/// rename's own error is the backstop either way.
#[cfg(windows)]
fn same_disk(from: &Utf8Path, to: &Utf8Path) -> bool {
    let prefix = |p: &Utf8Path| {
        p.components()
            .next()
            .map(|c| c.as_str().to_ascii_lowercase())
    };
    prefix(from) == prefix(to)
}

/// Anything running in the folder: each is refused by name, since a move under it would leave it
/// writing into a path that no longer exists.
fn activity(state: &AppState, real: &Utf8Path) -> Option<Refusal> {
    if let Some(holder) = state.holder_under(real) {
        return Some(if holder.starts_with("term:") {
            Refusal::Terminal
        } else {
            Refusal::Turn { lane: holder }
        });
    }
    if crate::term::open_count() > 0 {
        return Some(Refusal::Terminal);
    }
    if crate::dev::running_in(real) {
        return Some(Refusal::Dev);
    }
    // Jobs and the dev server are the daemon's, and the daemon is this project's; the folder check
    // is still the right question, since a job names where it runs.
    let in_project = |dir: &str| {
        let d = Utf8Path::new(dir);
        d.real().unwrap_or_else(|_| d.to_owned()).starts_with(real)
    };
    if let Some(job) = crate::monitor::list(None)
        .into_iter()
        .find(|j| j.finished.is_none() && in_project(&j.dir))
    {
        return Some(Refusal::Job {
            command: job.command.chars().take(40).collect(),
        });
    }
    None
}

/// Claude Code running in this folder outside Keel: it would go on filing its transcript under
/// the old path's key, and the history folder renamed under it would be made again.
fn live_claude(claude: Option<&Utf8Path>, real: &Utf8Path) -> bool {
    let Some(dir) = claude.map(|c| c.join("sessions")) else {
        return false;
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .any(|v| {
            let alive = v["pid"]
                .as_i64()
                .is_some_and(|p| keel_workspace::pid_alive(p as i32));
            let here = v["cwd"].as_str().is_some_and(|c| {
                let c = Utf8Path::new(c);
                c.real().unwrap_or_else(|_| c.to_owned()).starts_with(real)
            });
            alive && here
        })
}

/// What a move of `from` would do, without doing any of it.
pub fn plan(state: &AppState, from: &Utf8Path, roots: &Roots) -> Plan {
    let (to, checked) = structure(from, &roots.workspace);
    let real = from.real().unwrap_or_else(|_| from.to_owned());
    let refusal = checked
        .err()
        .or_else(|| activity(state, &real))
        .or_else(|| live_claude(roots.claude.as_deref(), &real).then_some(Refusal::Live));
    Plan {
        from: from.to_string(),
        to: to.to_string(),
        refusal,
    }
}

/// The rename's error, as one of the refusals a person can act on.
fn refusal_of(error: &std::io::Error) -> Refusal {
    match error.kind() {
        std::io::ErrorKind::CrossesDevices => Refusal::Volume,
        std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists => {
            Refusal::Taken { to: String::new() }
        }
        // ERROR_NOT_SAME_DEVICE: Windows says a cross-volume rename this way.
        _ if cfg!(windows) && error.raw_os_error() == Some(17) => Refusal::Volume,
        _ => Refusal::Rename {
            error: error.to_string(),
        },
    }
}

/// Move `from` into the workspace, or say why not. Every check runs again under the claims.
pub fn run(state: &Arc<AppState>, from: &Utf8Path, roots: &Roots) -> Result<Moved, Refusal> {
    let first = plan(state, from, roots);
    if let Some(refusal) = first.refusal {
        return Err(refusal);
    }
    let real = from.real().map_err(|e| Refusal::Rename {
        error: e.to_string(),
    })?;
    let to = Utf8PathBuf::from(first.to);
    let lanes = crate::worktree::names(&real);
    // Every tree held, so nothing starts while the folder is between places.
    let mut held = Vec::new();
    for (key, tree) in
        std::iter::once(("keel:move".to_string(), real.clone())).chain(lanes.iter().map(|l| {
            (
                format!("keel:move:{l}"),
                real.join(crate::worktree::DIR).join(l),
            )
        }))
    {
        let token = state
            .reserve_checkout(&key, &tree)
            .map_err(|_| Refusal::Turn {
                lane: String::new(),
            })?;
        held.push(Held::new(state.clone(), key, token));
    }
    if let Some(refusal) = activity_beyond(state, &real) {
        return Err(refusal);
    }
    let old_key = crate::permissions::key_of(&real).ok();
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Refusal::Workspace {
            error: e.to_string(),
        })?;
    }
    std::fs::rename(&real, &to).map_err(|e| refusal_of(&e))?;
    // Lanes relinked, or the folder goes back: a lane whose admin entry git prunes is lost.
    if let Err(error) = crate::worktree::repair(&to) {
        let _ = std::fs::rename(&to, &real);
        let _ = crate::worktree::repair(&real);
        return Err(Refusal::Repair { error });
    }
    drop(held);
    let mut warnings = Vec::new();
    let mut note = |step: &'static str, r: Result<(), String>| {
        if let Err(error) = r {
            warnings.push(Warning { step, error });
        }
    };
    if let Some(key) = old_key {
        note("trust", crate::permissions::carry(&key, &to));
    }
    note("vouched", crate::vouched::relocate(&real, &to));
    let spellings: Vec<String> =
        [from.to_string(), real.to_string()]
            .into_iter()
            .fold(Vec::new(), |mut v, s| {
                if !v.contains(&s) {
                    v.push(s);
                }
                v
            });
    note("sessions", crate::names::relocate(&spellings, &to));
    if let Some(record) = &roots.moved {
        note("record", remember(record, &spellings, &to));
    }
    if let Some(claude) = &roots.claude {
        note("history", history(claude, &spellings, &to, &lanes));
    }
    Ok(Moved {
        from: from.to_string(),
        to: to.to_string(),
        warnings,
    })
}

/// The activity checks again, now that the move holds its own claims: those are not a turn.
fn activity_beyond(state: &AppState, real: &Utf8Path) -> Option<Refusal> {
    match activity(state, real) {
        Some(Refusal::Turn { lane }) if lane.starts_with("keel:move") => None,
        other => other,
    }
}

/// Append this move to the device's record of former paths, under its file lock.
fn remember(record: &Utf8Path, from: &[String], to: &Utf8Path) -> Result<(), String> {
    if let Some(dir) = record.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::writes::locked(record.as_std_path(), || {
        let mut all = keel_workspace::moved::records_at(record);
        all.push(keel_workspace::moved::Move {
            from: from.to_vec(),
            to: to.to_string(),
        });
        let body = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        crate::writes::replace(record.as_std_path(), &body)
    })?
}

/// Claude Code's history for the project and each lane, renamed to the new path's key. Each key
/// is computed from a whole path, never matched by prefix: `foo`'s key is the start of
/// `foo-bar`'s, and a prefix rename would take another project's conversations with it.
fn history(
    claude: &Utf8Path,
    from: &[String],
    to: &Utf8Path,
    lanes: &[String],
) -> Result<(), String> {
    let projects = claude.join("projects");
    let trees = std::iter::once(String::new()).chain(
        lanes
            .iter()
            .map(|l| format!("{}/{l}", crate::worktree::DIR)),
    );
    let mut failed = Vec::new();
    for rel in trees {
        let under = |base: &Utf8Path| {
            if rel.is_empty() {
                base.to_owned()
            } else {
                base.join(&rel)
            }
        };
        let new = projects.join(keel_workspace::project_key(&under(to)));
        for old in from
            .iter()
            .map(|f| projects.join(keel_workspace::project_key(&under(Utf8Path::new(f)))))
        {
            if old == new || !old.is_dir() {
                continue;
            }
            if let Err(e) = merge(&old, &new) {
                failed.push(format!("{old}: {e}"));
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// `old` becomes `new`; when `new` already exists, each entry moves in, and none is overwritten.
fn merge(old: &Utf8Path, new: &Utf8Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(new).is_err() {
        return std::fs::rename(old, new);
    }
    for entry in std::fs::read_dir(old)?.flatten() {
        let target = new.as_std_path().join(entry.file_name());
        if std::fs::symlink_metadata(&target).is_err() {
            std::fs::rename(entry.path(), target)?;
        }
    }
    let _ = std::fs::remove_dir(old);
    Ok(())
}

/// The folder as the person opened it. The daemon serves its resolved path, so a project opened
/// through a link would otherwise be moved out from under the link, which would then point at
/// nothing — the one place the page's spelling decides anything.
#[derive(serde::Deserialize)]
pub struct Opened {
    pub opened: Option<String>,
}

/// A refusal for a link the person opened, before anything else is looked at.
fn opened_link(opened: Option<&str>) -> Option<Refusal> {
    let path = Utf8Path::new(opened?);
    let target = std::fs::read_link(path).ok()?;
    Some(Refusal::Link {
        target: target.to_string_lossy().into_owned(),
    })
}

/// `GET /api/project/move`: where this project would go, or why it will not.
pub async fn api_plan(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<Opened>,
) -> Result<Json<Plan>, (StatusCode, String)> {
    blocking(
        move || {
            let roots = Roots::local().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            let mut plan = plan(&state, &state.repo(), &roots);
            if let Some(link) = opened_link(q.opened.as_deref()) {
                plan.refusal = Some(link);
            }
            Ok(Json(plan))
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the check was cancelled".into(),
        )),
    )
    .await
}

/// `POST /api/project/move`: move it. A refusal is a 409 carrying its code; the app closes this
/// daemon and opens one at the new path after a move.
pub async fn api_move(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<Opened>,
) -> Result<Json<Moved>, (StatusCode, Json<Refusal>)> {
    blocking(
        move || {
            if let Some(link) = opened_link(q.opened.as_deref()) {
                return Err((StatusCode::CONFLICT, Json(link)));
            }
            let roots = Roots::local().map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(Refusal::Workspace { error }),
                )
            })?;
            run(&state, &state.repo(), &roots)
                .map(Json)
                .map_err(|r| (StatusCode::CONFLICT, Json(r)))
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Refusal::Rename {
                error: "the move was cancelled".into(),
            }),
        )),
    )
    .await
}

#[cfg(test)]
mod tests;
