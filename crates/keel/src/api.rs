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
pub fn read_file(root: &Utf8Path, requested: &str) -> Result<FileResponse, String> {
    const MAX: usize = 400_000;

    let joined = root.join(requested);
    let canonical = joined
        .canonicalize_utf8()
        .map_err(|_| "no such file".to_string())?;
    let root_canonical = root
        .canonicalize_utf8()
        .map_err(|_| "repository unavailable".to_string())?;

    if !canonical.starts_with(&root_canonical) {
        return Err("path escapes the repository".to_string());
    }

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

#[derive(Deserialize)]
pub struct ChatQuery {
    pub prompt: String,
    /// Resume an existing conversation rather than starting a new one.
    pub session: Option<String>,
}

/// Run `claude` in the repository and stream its events to the browser.
///
/// # Permissions
///
/// This spawns with `--permission-mode acceptEdits`, which means the agent has real file access and
/// can run commands. That is **not** the locked-down surface described in `docs/guardrails.md` —
/// that surface depends on Keel's MCP server, which is a tool catalog with no implementation behind
/// it yet. Until it exists, an agent restricted to Keel tools would have no tools at all and could
/// do nothing. The UI states this plainly rather than implying a containment that is not there.
pub async fn chat(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    let repo = state.repo.clone();

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
            .arg("acceptEdits")
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
                if tx.send(Ok(Event::default().event("msg").data(line))).await.is_err() {
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
