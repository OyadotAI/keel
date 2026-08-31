//! The repository as a tree of files, and reading one of them.
//!
//! Split out of the old `api.rs`, which had grown to hold four unrelated things: this, the agent
//! invocation, the whole git client, and the frontend importer analysis. Nothing here knows about
//! git or about `claude`; it is paths, and what is allowed to be read.

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
        // Asked of the path *inside* the checkout: a lane lives at
        // `<project>/.keel/worktrees/<name>`, and matching the absolute path skipped every file it
        // held — an isolated lane showed an empty file tree and an empty mention picker.
        if let Ok(rel) = path.strip_prefix(root)
            && !rel.as_str().is_empty()
            && !rel
                .components()
                .any(|c| matches!(c.as_str(), ".git" | ".keel" | "target" | "node_modules"))
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
