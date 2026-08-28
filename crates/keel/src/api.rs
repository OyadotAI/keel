//! Repository browsing and the live agent session.
//!
//! The chat endpoint is the part that makes this an IDE rather than a report: it spawns the user's
//! `claude` binary in the repository and streams its `stream-json` events to the browser as SSE.

use anyhow::Result;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use crate::serve::AppState;

/// One entry in the repository tree.
#[derive(Debug, Serialize)]
pub struct Node {
    pub name: String,
    pub path: String,
    pub dir: bool,
    /// Present for directories only.
    pub children: Option<Vec<Node>>,
}

/// Walk the repository into a nested tree.
///
/// Honours `.gitignore` and skips `.git`, so the tree shows what a developer thinks of as their
/// project rather than every file on disk.
pub fn tree(root: &Utf8Path) -> Vec<Node> {
    // Collect the flat, ignore-aware path list first, then fold it into a nesting. Doing it in one
    // pass would mean re-running the ignore matcher per directory.
    let mut paths: Vec<Utf8PathBuf> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .require_git(false)
        .parents(false)
        .build();

    for entry in walker.flatten() {
        let Some(path) = Utf8Path::from_path(entry.path()) else {
            continue;
        };
        if path
            .components()
            .any(|c| matches!(c.as_str(), ".git" | ".keel" | "target" | "node_modules"))
        {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root)
            && !rel.as_str().is_empty()
        {
            paths.push(rel.to_owned());
        }
    }
    paths.sort();

    build_nodes(&paths, root)
}

/// Fold a sorted flat path list into a nested tree.
fn build_nodes(paths: &[Utf8PathBuf], root: &Utf8Path) -> Vec<Node> {
    #[derive(Default)]
    struct Dir {
        dirs: BTreeMap<String, Dir>,
        files: Vec<String>,
    }

    let mut top = Dir::default();
    for path in paths {
        let full = root.join(path);
        let mut cursor = &mut top;
        let components: Vec<&str> = path.iter().collect();
        let Some((last, parents)) = components.split_last() else {
            continue;
        };
        for part in parents {
            cursor = cursor.dirs.entry((*part).to_string()).or_default();
        }
        if full.is_dir() {
            cursor.dirs.entry((*last).to_string()).or_default();
        } else {
            cursor.files.push((*last).to_string());
        }
    }

    fn to_nodes(dir: &Dir, prefix: &str) -> Vec<Node> {
        let mut nodes: Vec<Node> = dir
            .dirs
            .iter()
            .map(|(name, child)| {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                Node {
                    name: name.clone(),
                    children: Some(to_nodes(child, &path)),
                    path,
                    dir: true,
                }
            })
            .collect();

        nodes.extend(dir.files.iter().map(|name| Node {
            name: name.clone(),
            path: if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            },
            dir: false,
            children: None,
        }));

        nodes
    }

    to_nodes(&top, "")
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct FileResponse {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

/// Read one file for the editor pane.
///
/// The path is resolved and checked to stay inside the repository. Keel binds to loopback, but a
/// traversal bug would still let any page in the user's browser read their filesystem.
/// Directories Keel is allowed to read from.
///
/// The repository, and the user's Claude Code home — skills, agents and commands live there, and
/// opening one is the point. Nothing else: a loopback server is reachable by any page in the
/// browser, so the boundary has to be explicit rather than implied by the UI never asking.
fn roots(repo: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut out = Vec::new();
    if let Ok(c) = repo.canonicalize_utf8() {
        out.push(c);
    }
    if let Some(home) = keel_workspace::claude_home()
        && let Ok(c) = home.canonicalize_utf8()
    {
        out.push(c);
    }
    out
}

/// Resolve a path and confirm it sits inside one of the allowed roots.
///
/// A path is taken as repo-relative first, then as absolute — so the UI can pass either without
/// caring which, and a `..` in a relative path still has to survive the containment check.
pub fn resolve(repo: &Utf8Path, requested: &str) -> Result<Utf8PathBuf, String> {
    let candidates = [repo.join(requested), Utf8PathBuf::from(requested)];
    let allowed = roots(repo);

    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize_utf8() else {
            continue;
        };
        if allowed.iter().any(|r| canonical.starts_with(r)) {
            return Ok(canonical);
        }
        return Err("path is outside the repository and your Claude config".to_string());
    }
    Err("no such file".to_string())
}

/// Resolve a path that must be an existing directory.
///
/// The tree's context menu creates things *inside* a directory, and a create whose parent turned
/// out to be a file would otherwise fail later with a confusing io error.
pub fn resolve_dir(repo: &Utf8Path, requested: &str) -> Result<Utf8PathBuf, String> {
    let path = resolve(repo, requested)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("{requested} is not a directory"))
    }
}

pub fn read_file(root: &Utf8Path, requested: &str) -> Result<FileResponse, String> {
    const MAX: usize = 400_000;

    let canonical = resolve(root, requested)?;

    let bytes = std::fs::read(&canonical).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > MAX;
    let slice = if truncated { &bytes[..MAX] } else { &bytes[..] };

    let content = match std::str::from_utf8(slice) {
        Ok(text) => text.to_string(),
        Err(_) => return Err("binary file".to_string()),
    };

    Ok(FileResponse {
        path: requested.to_string(),
        content,
        truncated,
    })
}

/// Serve a file's raw bytes, for previewing images and anything else the editor cannot show as text.
///
/// Bounded to the repository like [`read_file`]. Loopback is not a substitute for that check: a
/// traversal bug would let any page in the user's browser read their filesystem.
pub fn read_raw(root: &Utf8Path, requested: &str) -> Result<(Vec<u8>, &'static str), String> {
    let canonical = root
        .join(requested)
        .canonicalize_utf8()
        .map_err(|_| "no such file".to_string())?;
    let root_canonical = root
        .canonicalize_utf8()
        .map_err(|_| "repository unavailable".to_string())?;
    if !canonical.starts_with(&root_canonical) {
        return Err("path escapes the repository".to_string());
    }

    let bytes = std::fs::read(&canonical).map_err(|e| e.to_string())?;
    let mime = match canonical
        .extension()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        // SVG is served as a download rather than inline: it is executable markup, and this is
        // repository content that nobody has reviewed.
        Some("svg") => "application/octet-stream",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    };
    Ok((bytes, mime))
}

/// The committed version of a file, for the diff editor.
///
/// Returning the baseline text and letting the editor compute the diff beats shipping pre-parsed
/// hunks: the editor's diff algorithm handles word-level highlighting, navigation and side-by-side
/// layout that a hand-rolled hunk renderer would have to reimplement badly.
pub fn read_original(root: &Utf8Path, requested: &str) -> Result<FileResponse, String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .arg("show")
        .arg(format!("HEAD:{requested}"))
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    // A file that is not in HEAD is new. An empty baseline is the honest answer — the whole file
    // then renders as added, which is exactly what happened.
    let content = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::new()
    };

    Ok(FileResponse {
        path: requested.to_string(),
        content,
        truncated: false,
    })
}

#[derive(Deserialize)]
pub struct ChatQuery {
    pub prompt: String,
    /// Resume an existing conversation rather than starting a new one.
    pub session: Option<String>,
    /// Permission mode. `plan` explores without touching anything; `acceptEdits` lets the agent
    /// edit and run commands. Anything unrecognised falls back to `plan`, because the safe default
    /// is the one that cannot change the repository.
    pub mode: Option<String>,
}

/// Run `claude` in the repository and stream its events to the browser.
///
/// # Permissions
///
/// The UI selects between `plan` (explore and propose, no writes) and `acceptEdits` (real file
/// access, runs commands). An unrecognised value falls back to `plan`: the safe default is the one
/// that cannot change the repository.
///
/// `acceptEdits` is **not** the locked-down surface described in `docs/guardrails.md` —
/// that surface depends on Keel's MCP server, which is a tool catalog with no implementation behind
/// it yet. Until it exists, an agent restricted to Keel tools would have no tools at all and could
/// do nothing. The UI states this plainly rather than implying a containment that is not there.
pub async fn chat(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    let repo = state.repo();

    tokio::spawn(async move {
        let mut command = Command::new("claude");
        command
            .current_dir(&repo)
            .arg("-p")
            .arg(&query.prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--include-partial-messages")
            .arg("--permission-mode")
            .arg(match query.mode.as_deref() {
                Some("acceptEdits") => "acceptEdits",
                _ => "plan",
            })
            // acceptEdits covers file writes but not arbitrary shell, and headless has nobody to
            // ask. These are the rules the user approved in the IDE.
            .arg("--settings")
            .arg(crate::permissions::settings_json(&repo))
            // No `--append-system-prompt` telling the agent to avoid shell expansion. It was tried:
            // with and without the instruction, the first command out was `wc -l < a.txt; echo
            // "exit: $?"` both times. The agent self-corrects from the refusal text either way, so
            // the instruction bought nothing and cost tokens on every turn. The fix that works is
            // in the UI, which now says what an expansion refusal is instead of showing nothing.

            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(session) = &query.session {
            command.arg("--resume").arg(session);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(format!(
                        "could not start `claude`: {e}. Is the CLI installed and on PATH?"
                    ))))
                    .await;
                return;
            }
        };

        if let Some(stdout) = child.stdout.take() {
            let mut lines = BufReader::new(stdout).lines();
            // Forward each JSONL record verbatim. Translating event shapes here would mean two
            // places to update when the CLI's stream changes; the browser does the interpreting.
            while let Ok(Some(line)) = lines.next_line().await {
                if tx
                    .send(Ok(Event::default().event("msg").data(line)))
                    .await
                    .is_err()
                {
                    // The browser disconnected. Stop the run rather than leaving it orphaned.
                    let _ = child.start_kill();
                    return;
                }
            }
        }

        let status = child.wait().await;
        let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });

    Sse::new(ReceiverStream::new(rx))
}

// ── git ──────────────────────────────────────────────────────────────────────

/// One file with uncommitted changes.
#[derive(Debug, Serialize)]
pub struct Change {
    pub path: String,
    /// Two-character porcelain code, e.g. ` M`, `??`, `A `.
    pub status: String,
    /// Human label for the status.
    pub label: String,
    pub staged: bool,
}

#[derive(Debug, Serialize)]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub changes: Vec<Change>,
}

fn git(root: &Utf8Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Uncommitted changes, which after an agent run is the answer to "what did it just do".
pub fn git_status(root: &Utf8Path) -> GitStatus {
    // `-uall` rather than the default. Without it git collapses an untracked directory to a single
    // entry ending in `/` — `.github/` instead of the three files under it — which is useless in a
    // list you click to open a file, and rendered as a row with no name at all, because the
    // basename of "a/b/" is the empty string.
    let Some(raw) = git(root, &["status", "--porcelain=v1", "-z", "-uall"]) else {
        return GitStatus {
            is_repo: false,
            branch: None,
            changes: Vec::new(),
        };
    };

    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|b| b.trim().to_string());

    // NUL-separated so paths containing spaces or quotes survive intact.
    let changes = raw
        .split('\0')
        .filter(|entry| entry.len() > 3)
        .map(|entry| {
            let (status, path) = entry.split_at(2);
            let staged = !status.starts_with([' ', '?']);
            Change {
                // A submodule still arrives with a trailing slash, and nothing downstream should
                // have to know that.
                path: path.trim_start().trim_end_matches('/').to_string(),
                status: status.to_string(),
                label: label_for(status).to_string(),
                staged,
            }
        })
        .collect();

    GitStatus {
        is_repo: true,
        branch,
        changes,
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;

    /// `git status --porcelain` reports an untracked *directory* as one entry ending in `/`, so a
    /// new folder of twelve files was one row whose basename was the empty string — a blank line
    /// in the changes list that opened nothing.
    #[test]
    fn untracked_directories_are_listed_as_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("seed"), "x").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/a.yml"), "a").unwrap();
        std::fs::write(root.join(".github/workflows/b.yml"), "b").unwrap();

        let status = git_status(&root);
        let paths: Vec<_> = status.changes.iter().map(|c| c.path.as_str()).collect();

        assert!(
            paths.contains(&".github/workflows/a.yml") && paths.contains(&".github/workflows/b.yml"),
            "expected the files, got {paths:?}"
        );
        for path in &paths {
            assert!(!path.ends_with('/'), "{path} is a directory, not a file");
            assert!(
                !path.rsplit('/').next().unwrap_or_default().is_empty(),
                "{path} has no basename, so its row would render blank"
            );
        }
    }
}

fn label_for(status: &str) -> &'static str {
    match status.trim() {
        "M" | "MM" => "modified",
        "A" => "added",
        "D" => "deleted",
        "R" => "renamed",
        "??" => "untracked",
        "C" => "copied",
        "U" | "UU" => "conflicted",
        _ => "changed",
    }
}

#[derive(Serialize)]
pub struct DiffResponse {
    pub path: String,
    pub hunks: Vec<Hunk>,
    /// True when the file is untracked, so there is no baseline to diff against.
    pub untracked: bool,
}

#[derive(Serialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize)]
pub struct DiffLine {
    /// `add`, `del`, or `ctx`.
    pub kind: &'static str,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// Parse `git diff` for one path into hunks the browser can render side by side with line numbers.
pub fn git_diff(root: &Utf8Path, path: &str) -> DiffResponse {
    // An untracked file has no baseline; show it as entirely added rather than an empty diff.
    let untracked = git(root, &["ls-files", "--error-unmatch", path]).is_none();

    let raw = if untracked {
        std::fs::read_to_string(root.join(path))
            .map(|content| {
                let n = content.lines().count();
                format!(
                    "@@ -0,0 +1,{n} @@\n{}",
                    content
                        .lines()
                        .map(|l| format!("+{l}\n"))
                        .collect::<String>()
                )
            })
            .unwrap_or_default()
    } else {
        git(root, &["diff", "--no-color", "-U3", "--", path]).unwrap_or_default()
    };

    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);

    for line in raw.lines() {
        if line.starts_with("@@") {
            // @@ -old,count +new,count @@
            let nums: Vec<&str> = line
                .split(['-', '+', ',', ' '])
                .filter(|s| !s.is_empty())
                .collect();
            old_no = nums.first().and_then(|s| s.parse().ok()).unwrap_or(1);
            new_no = nums.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
            hunks.push(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };

        let (kind, text) = match line.chars().next() {
            Some('+') => ("add", &line[1..]),
            Some('-') => ("del", &line[1..]),
            Some(' ') => ("ctx", &line[1..]),
            _ => continue,
        };

        let (old, new) = match kind {
            "add" => (None, Some(new_no)),
            "del" => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if kind != "add" {
            old_no += 1;
        }
        if kind != "del" {
            new_no += 1;
        }

        hunk.lines.push(DiffLine {
            kind,
            old,
            new,
            text: text.to_string(),
        });
    }

    DiffResponse {
        path: path.to_string(),
        hunks,
        untracked,
    }
}

#[derive(Deserialize)]
pub struct SaveRequest {
    pub path: String,
    pub content: String,
}

/// Write a file from the editor, bounded to the repository like [`read_file`].
pub fn write_file(root: &Utf8Path, req: &SaveRequest) -> Result<(), String> {
    // An existing file resolves directly; a new one is checked by its parent, since the file itself
    // cannot be canonicalised until it exists.
    let target = match resolve(root, &req.path) {
        Ok(p) => p,
        Err(_) => {
            let joined = root.join(&req.path);
            let parent = joined.parent().ok_or("invalid path")?;
            let parent = parent
                .canonicalize_utf8()
                .map_err(|_| "no such directory".to_string())?;
            if !roots(root).iter().any(|r| parent.starts_with(r)) {
                return Err("path is outside the repository and your Claude config".to_string());
            }
            parent.join(joined.file_name().ok_or("invalid path")?)
        }
    };

    std::fs::write(&target, &req.content).map_err(|e| e.to_string())
}
