//! Repository browsing and the live agent session.
//!
//! The chat endpoint is the part that makes this an IDE rather than a report: it spawns the user's
//! `claude` binary in the repository and streams its `stream-json` events to the browser as SSE.

use anyhow::Result;
use axum::{
    Json,
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

#[derive(Deserialize)]
pub struct ChatQuery {
    pub prompt: String,
    /// Resume an existing conversation rather than starting a new one.
    pub session: Option<String>,
    /// Permission mode. `plan` explores without touching anything; `acceptEdits` lets the agent
    /// edit and run commands. Anything unrecognised falls back to `plan`, because the safe default
    /// is the one that cannot change the repository.
    pub mode: Option<String>,
    /// The lane's checkout to run in. Absent means the project itself.
    pub wt: Option<String>,
    /// The directory a resumed session was launched from, when that was not the repository.
    /// `--resume` only finds a transcript in the project of the directory it runs in.
    pub cwd: Option<String>,
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
/// What Keel tells the agent about where it is.
///
/// Appended to Claude Code's own system prompt rather than replacing it: everything the CLI already
/// knows about editing code is worth keeping, and none of it covers being driven by an IDE.
///
/// The whole file is about things the agent cannot observe from inside the session. It cannot see
/// that someone is watching its diffs, that a gate runs after every turn whether it runs one or
/// not, that a refusal is a question being asked rather than a wall, or what the scan already
/// found. Everything it *can* work out by reading the repository is deliberately absent — a system
/// prompt restating what `ls` would show is tokens spent on every turn to say nothing.
fn system_prompt(repo: &Utf8Path) -> String {
    let mut out = String::from(
        "You are running inside Keel, a local IDE that drives you. This is what that changes.\n\n\
         ## What the person can see\n\n\
         Every file you write appears as a diff in the pane beside this conversation, live. They \
         also have the terminal, the project's check output, and the readiness report. So do not \
         paste back the code you just wrote, do not narrate a rename, and do not summarise a diff \
         they are already looking at. Tell them what you did and what it means for them. The diff \
         carries the rest.\n\n\
         ## Evidence, not assertion\n\n",
    );

    match crate::verify::detect(repo) {
        Some(check) => out.push_str(&format!(
            "`{}` is this project's gate, from {}. Keel runs it after every turn you take and \
             shows the person the result, so a claim that something works gets checked whether \
             you check it or not. Run it yourself first — finding out from your own run is \
             cheaper for everyone than finding out from theirs.\n\n",
            check.command, check.source
        )),
        None => out.push_str(
            "This project has no check command, so nothing contradicts you automatically. That \
             makes it more important, not less, that you run what you can and say what you \
             actually observed.\n\n",
        ),
    }

    // What is installed, which the agent finds out by running something that fails.
    let (manager, present, absent) = crate::clitools::toolchain();
    out.push_str("## What is on this machine\n\n");
    match manager {
        Some(mgr) => out.push_str(&format!(
            "`{mgr}` is installed, so a missing tool is one command away. Install what you need \
             rather than working around its absence — a workaround is a worse answer that also \
             takes longer.\n\n"
        )),
        None => out.push_str(
            "There is no package manager here, so a missing tool cannot be installed. Say what \
             is missing rather than working around it.\n\n",
        ),
    }
    out.push_str(&format!("Present: {}\n", present.join(", ")));
    if !absent.is_empty() {
        out.push_str(&format!("Not installed: {}\n", absent.join(", ")));
    }
    out.push_str(
        "\nInstalling something needs approval the first time, like any other command. That is \
         the loop working — ask once and it is remembered.\n\n",
    );

    out.push_str(
        "Never report a result you have not seen. \"The tests pass\" means you ran them and read \
         the output. If something could not be run, name it and say why rather than working \
         around the gap quietly.\n\n\
         ## Permissions\n\n\
         This session is non-interactive: nothing can prompt the person mid-turn. A command \
         outside the allowed set comes back refused, and Keel shows them that refusal with a \
         button to allow it. That is the loop working, not a failure. Say plainly what you needed \
         and stop. Do not reach for a different command that happens to be permitted — a \
         substitute they did not approve is worse than a request they can answer in one click.\n\n\
         `AskUserQuestion` works here: Keel shows the question and holds the turn until they \
         answer, and the answer arrives as the tool's result. Use it when two readings of the \
         request would lead to materially different work.\n\n\
         ## Configuring the workspace\n\n\
         Anything the person could set up from a terminal you can set up from here. A subagent is \
         a Markdown file with YAML frontmatter in `.claude/agents/` — write one directly, and make \
         its `description` say *when* to delegate to it, since that is the only part the main \
         agent reads. Skills are `.claude/skills/<name>/SKILL.md`, slash commands \
         `.claude/commands/`. MCP servers go through `claude mcp add --scope local` and \
         `claude mcp list|remove`, never by editing `.mcp.json` — Keel quarantines that file as \
         repository content, so a server written there is one the person then has to un-quarantine \
         by hand. Do not write `.claude/settings.json` or a hook: a hook is a shell command that \
         runs for whoever opens this repository next, which is why Keel quarantines it and the \
         scanner rates it Critical.\n\n\
         ## Scope\n\n\
         Do the thing that was asked. If you notice something else wrong, say so in a sentence \
         and carry on; do not fix it uninvited. Read the repository rather than asking about it — \
         ask only when two readings of the request would lead to materially different work, and \
         then ask once, at the point it matters.\n",
    );

    // The scan is Keel's own reading of this repository, and it is on screen next to the
    // conversation. An agent that has to rediscover "there are no tests" wastes a turn on
    // something the person is already looking at.
    if let Ok(ctx) = keel_scanner::RepoContext::load(repo) {
        let report = keel_scanner::scan(&ctx);
        let blocking: Vec<_> = report
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f.severity,
                    keel_scanner::Severity::Critical | keel_scanner::Severity::High
                )
            })
            .collect();
        if !blocking.is_empty() {
            out.push_str("\n## What Keel's scan already found\n\nUnfixed, and known:\n\n");
            for f in blocking {
                out.push_str(&format!("- {}\n", f.title));
            }
            out.push_str(
                "\nDo not re-diagnose these. If your task is one of them, fix it; if it is not, \
                 leave them alone and do not mention them again.\n",
            );
        }
    }

    out
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    /// A system prompt costs tokens on every single turn, so it earns its place by carrying only
    /// what the agent cannot see for itself: that its diffs are on screen, that a gate runs
    /// whether it runs one or not, that a refusal is a question rather than a wall.
    #[test]
    fn the_prompt_names_this_project_s_gate() {
        let repo = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");
        let prompt = system_prompt(repo);

        assert!(
            prompt.contains("make check"),
            "the gate is named, not implied"
        );
        assert!(prompt.contains("Keel runs it after every turn"));
        assert!(prompt.contains("non-interactive"));

        // Short enough to send every turn. Past a page it stops being read as instruction and
        // starts competing with the actual request.
        assert!(
            prompt.len() < 4_000,
            "the system prompt is {} bytes and is sent on every turn",
            prompt.len()
        );
    }

    /// The agent finds out a tool is missing by running something that fails, and then works
    /// around the gap or gives up — neither of which is installing it, which it will not think to
    /// do if it does not know there is a package manager. Reported from a fresh machine as "the
    /// chat failed to install bun".
    /// Reported as "the chat cannot create a subagent": it can — it is a file write — but nothing
    /// told it where the file goes or that configuring the workspace was its business at all.
    #[test]
    fn the_prompt_says_the_workspace_is_configurable() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let prompt = system_prompt(&root);

        assert!(prompt.contains(".claude/agents/"), "where a subagent goes");
        assert!(
            prompt.contains("claude mcp add"),
            "how an MCP server is added"
        );
        // The two paths it must not write: one is quarantined, the other is code execution.
        assert!(prompt.contains(".mcp.json"));
        assert!(prompt.contains(".claude/settings.json"));
    }

    #[test]
    fn the_prompt_says_what_is_installed() {
        let repo = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");
        let prompt = system_prompt(repo);

        assert!(prompt.contains("What is on this machine"));
        assert!(prompt.contains("Present: "), "it lists what is there");

        let (manager, present, _) = crate::clitools::toolchain();
        if let Some(mgr) = manager {
            assert!(
                prompt.contains(mgr),
                "the package manager is named, not implied"
            );
            assert!(
                prompt.contains("rather than working around"),
                "and what to do with it"
            );
        }
        for tool in present.iter().take(3) {
            assert!(prompt.contains(tool), "{tool} is installed but not listed");
        }
    }

    /// Without a gate the advice inverts: nothing contradicts the agent automatically, so saying
    /// what was actually observed matters more rather than less.
    #[test]
    #[ignore = "prints the prompt for review"]
    fn show() {
        let repo = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap();
        let target = std::env::var("KEEL_PROMPT_REPO")
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|_| repo.to_owned());
        println!("{}", system_prompt(&target));
    }

    #[test]
    fn a_project_with_no_gate_is_told_so() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let prompt = system_prompt(&root);
        assert!(prompt.contains("no check command"));
        assert!(!prompt.contains("make check"));
    }
}

/// Reduce a name from a pasteboard to something that can only be a file in one directory.
///
/// Basename only, a conservative character set, no leading or trailing dots, and never empty. The
/// dot trimming is what stops `..` and `....//....//x` surviving as traversal once the separators
/// are gone.
fn sanitise_attachment_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mapped: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed: String = mapped.trim_matches('.').chars().take(80).collect();
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed
    }
}

/// Take a file the person pasted or dropped, and put it where the agent can read it.
///
/// Written to disk and referenced as `@path` rather than sent as an image block. Two reasons, and
/// the second is the real one:
///
/// - `claude` is spawned as `-p <prompt>` with no stdin. Real image blocks would mean
///   `--input-format stream-json` and rewriting the whole spawn; the Read tool already returns PNG
///   and JPEG as visual content, so a path is enough.
/// - The prompt travels as a **GET query parameter** and then as an **argv value**. A large paste
///   therefore has to survive a URL length limit and then macOS's ~256 KB `ARG_MAX`, and it does
///   not. A file dodges both, which is why long text comes through here too rather than inline.
pub async fn attach(
    crate::serve::Checkout(checkout): crate::serve::Checkout,
    Query(q): Query<AttachQuery>,
    body: axum::body::Bytes,
) -> Result<Json<Attached>, (axum::http::StatusCode, String)> {
    let bad = |m: String| (axum::http::StatusCode::BAD_REQUEST, m);

    // The name comes from a browser or a pasteboard and is never trusted as a path. Only a
    // basename, only these characters, and never `..` — a `name` of `../../evil.sh` must land in
    // the attachments directory or nowhere.
    let safe = sanitise_attachment_name(&q.name);

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();

    let dir = checkout.join(".keel").join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| bad(e.to_string()))?;

    // A `.gitignore` inside the directory keeps attachments out of `git status` — and so out of the
    // Changes panel — without editing the repository's own `.gitignore`, which is the user's file
    // and not Keel's to rewrite.
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }

    let name = format!("{stamp}-{safe}");
    let path = dir.join(&name);
    let bytes = body.len();
    std::fs::write(&path, &body).map_err(|e| bad(e.to_string()))?;

    Ok(Json(Attached {
        path: format!(".keel/attachments/{name}"),
        bytes,
    }))
}

#[derive(serde::Deserialize)]
pub struct AttachQuery {
    pub name: String,
}

#[derive(Serialize)]
pub struct Attached {
    /// Repo-relative, because that is the form `@path` mentions take.
    pub path: String,
    pub bytes: usize,
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    // The agent runs in the lane's checkout; its permissions come from the project. The two are
    // different paths on purpose, and `settings_json` below is handed the project.
    let repo = state.repo();
    let cwd = match state.checkout(query.wt.as_deref()) {
        // A resumed session runs where it was started. Validated: the repository, a parent
        // within two levels, or a subdirectory — nowhere else.
        Ok(_) if query.cwd.is_some() && query.session.is_some() => {
            state.session_dir(query.cwd.as_deref())
        }
        Ok(p) => p,
        Err(e) => {
            tokio::spawn(async move {
                let _ = tx.send(Ok(Event::default().event("fatal").data(e))).await;
            });
            return Sse::new(ReceiverStream::new(rx));
        }
    };
    let port = state.port();

    tokio::spawn(async move {
        let mut command = Command::new("claude");
        command
            .current_dir(&cwd)
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
            .arg(crate::permissions::settings_json(
                &repo,
                port,
                query.session.as_deref(),
            ))
            .arg("--append-system-prompt")
            .arg(system_prompt(&cwd))
            // Note what is deliberately *not* in that prompt: an instruction to avoid shell
            // expansion. It was tried, and with and without it the first command out was
            // `wc -l < a.txt; echo "exit: $?"` both times. The agent self-corrects from the
            // refusal text either way, so it bought nothing and cost tokens every turn. The fix
            // that works is in the UI, which says what an expansion refusal is.
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

#[derive(Debug, Default, Serialize)]
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

/// Run git and keep what it said when it fails.
///
/// [`git`] drops stderr, which is right for a status read and wrong for an action: "could not
/// discard" with no reason is the kind of message people screenshot and send to you.
fn git_run(root: &Utf8Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    let why = why.trim();
    Err(if why.is_empty() {
        "git refused, without saying why".into()
    } else {
        why.to_string()
    })
}

/// Whether git is tracking this path. An untracked file has no version to be restored to.
fn tracked(root: &Utf8Path, path: &str) -> bool {
    git_run(root, &["ls-files", "--error-unmatch", "--", path]).is_ok()
}

/// Stage, unstage, or throw away one file's changes.
///
/// Discarding is the one that matters — reviewing a diff and deciding against it is most of what
/// the Changes panel is for, and doing it in the terminal means retyping a path you are already
/// looking at. It is also the only one that destroys anything, so an untracked file goes to the
/// Trash rather than being deleted: git has no copy of it, and "discard" should not mean "gone".
/// Turn a directory into a git repository.
///
/// Offered where the absence is noticed rather than reported as an error: a new project is not a
/// broken one, and "not a git repository" is a thing to fix in a click, not a thing to be told.
///
/// Only `git init`. No first commit, no author config, no `.gitignore` guessed from the language —
/// each of those is a decision that belongs to whoever owns the project, and doing them silently
/// is how a tool ends up in someone's history with an opinion they never had.
pub fn git_init(root: &Utf8Path) -> Result<(), String> {
    if root.join(".git").exists() {
        return Err("This is already a git repository.".into());
    }
    git_run(root, &["init"]).map(|_| ())
}

pub fn git_act(
    root: &Utf8Path,
    action: &str,
    path: &str,
    hunk: Option<usize>,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("no file".into());
    }
    // Resolved against the repository, so a path cannot climb out of it.
    let target = resolve(root, path)?;

    match action {
        "stage" => git_run(root, &["add", "--", path]).map(|_| ()),
        "unstage" => git_run(root, &["restore", "--staged", "--", path]).map(|_| ()),
        // One hunk, not the file. Reviewing a four-hunk edit and wanting three of them is the
        // ordinary case, and "discard the file and ask again" throws away the three.
        "discard-hunk" => {
            let n = hunk.ok_or("which hunk?")?;
            let raw = git_run(root, &["diff", "--no-color", "-U3", "--", path])?;
            let patch = one_hunk(&raw, n).ok_or(format!("no hunk {n} in {path}"))?;
            let mut child = std::process::Command::new("git")
                .current_dir(root)
                .args(["apply", "-R", "--recount", "--unidiff-zero", "-"])
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            child
                .stdin
                .take()
                .ok_or("no stdin")?
                .write_all(patch.as_bytes())
                .map_err(|e| e.to_string())?;
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
        "discard" => {
            if tracked(root, path) {
                git_run(root, &["restore", "--staged", "--worktree", "--", path]).map(|_| ())
            } else {
                crate::fsops::trash(target.as_std_path())
            }
        }
        _ => Err(format!("unknown action: {action}")),
    }
}

/// The file header and the `n`th `@@` block of a unified diff, as a patch of its own.
fn one_hunk(raw: &str, n: usize) -> Option<String> {
    let mut header = String::new();
    let mut hunks: Vec<String> = Vec::new();
    for line in raw.lines() {
        if line.starts_with("@@") {
            hunks.push(format!("{line}\n"));
        } else if let Some(last) = hunks.last_mut() {
            last.push_str(line);
            last.push('\n');
        } else {
            header.push_str(line);
            header.push('\n');
        }
    }
    hunks.get(n).map(|h| header + h)
}

/// One commit, for the list beside the working tree.
#[derive(Serialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    /// Seconds since the epoch.
    pub when: i64,
    pub files: u32,
    /// Whether the upstream branch has it. `true` when there is no upstream at all is a lie, so
    /// that case is `false` too: nothing has been pushed anywhere.
    pub pushed: bool,
}

/// The last `n` commits on the current branch.
///
/// A working tree that only ever grows is what makes people nervous about an agent; a list of
/// small commits beside it is what makes the same work look like progress.
pub fn git_log(root: &Utf8Path, n: usize) -> Vec<Commit> {
    let Some(raw) = git(
        root,
        &[
            "log",
            &format!("-{n}"),
            "--format=%h%x1f%s%x1f%ct",
            "--shortstat",
        ],
    ) else {
        return Vec::new();
    };
    // What the upstream does not have yet. No upstream means nothing is pushed.
    let unpushed: std::collections::HashSet<String> = git(root, &["rev-list", "@{u}..HEAD"])
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    let has_upstream = git(root, &["rev-parse", "--abbrev-ref", "@{u}"]).is_some();
    let full: Vec<String> = git(root, &["log", &format!("-{n}"), "--format=%H"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();

    // Headers carry the separator; the stat line for a commit follows its header, blank lines
    // between, and the next header comes straight after the stat.
    let mut out: Vec<Commit> = Vec::new();
    for line in raw.lines() {
        if line.contains('\x1f') {
            let mut f = line.split('\x1f');
            let (Some(sha), Some(subject), Some(when)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let long = full.get(out.len()).cloned().unwrap_or_default();
            out.push(Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
                when: when.parse().unwrap_or(0),
                files: 0,
                pushed: has_upstream && !unpushed.contains(&long),
            });
        } else if line.contains("changed")
            && let Some(last) = out.last_mut()
            && let Some(n) = line.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            last.files = n;
        }
    }
    out
}

/// One commit's diff, file by file, in the shape the working-tree diff already has.
pub fn git_commit_diff(root: &Utf8Path, sha: &str) -> Result<Vec<DiffResponse>, String> {
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() > 40 {
        return Err("not a commit id".into());
    }
    let files = git_run(root, &["show", "--format=", "--name-only", sha])?;
    Ok(files
        .lines()
        .filter(|f| !f.is_empty())
        .map(|path| {
            let raw = git(
                root,
                &["show", "--format=", "--no-color", "-U3", sha, "--", path],
            )
            .unwrap_or_default();
            DiffResponse {
                path: path.to_string(),
                hunks: parse_hunks(&raw),
                untracked: false,
            }
        })
        .collect())
}

/// Send the branch to its remote, setting the upstream on the first push.
pub fn git_push(root: &Utf8Path) -> Result<String, String> {
    git_run(root, &["push", "-u", "origin", "HEAD"])
}

// ── branches and remotes: the rest of a git client ───────────────────────────

#[derive(Serialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Last commit's subject, for the list.
    pub subject: String,
}

#[derive(Serialize)]
pub struct Branches {
    pub current: Option<String>,
    pub local: Vec<Branch>,
    /// `origin/feature`, without the ones a local branch already tracks.
    pub remote: Vec<String>,
    pub remotes: Vec<String>,
    pub staged: u32,
    pub unstaged: u32,
}

/// Every branch, with how far each local one is from its upstream.
pub fn git_branches(root: &Utf8Path) -> Branches {
    let current = git(root, &["branch", "--show-current"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let raw = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%1f%(upstream:short)%1f%(upstream:track)%1f%(subject)",
            "refs/heads",
        ],
    )
    .unwrap_or_default();
    let mut tracked = std::collections::HashSet::new();
    let local: Vec<Branch> = raw
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\x1f');
            let name = f.next()?.to_string();
            let upstream = f.next().filter(|u| !u.is_empty()).map(str::to_string);
            let track = f.next().unwrap_or_default();
            let subject = f.next().unwrap_or_default().to_string();
            if let Some(u) = &upstream {
                tracked.insert(u.clone());
            }
            let count = |key: &str| -> u32 {
                track
                    .split(|c: char| !c.is_alphanumeric())
                    .collect::<Vec<_>>()
                    .windows(2)
                    .find(|w| w[0] == key)
                    .and_then(|w| w[1].parse().ok())
                    .unwrap_or(0)
            };
            Some(Branch {
                current: Some(&name) == current.as_ref(),
                ahead: count("ahead"),
                behind: count("behind"),
                name,
                upstream,
                subject,
            })
        })
        .collect();
    let remote: Vec<String> = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .unwrap_or_default()
    .lines()
    .map(str::to_string)
    .filter(|r| !r.ends_with("/HEAD") && !tracked.contains(r))
    .collect();
    let remotes: Vec<String> = git(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let status = git(root, &["status", "--porcelain"]).unwrap_or_default();
    let staged = status
        .lines()
        .filter(|l| l.len() > 1 && !l.starts_with([' ', '?']))
        .count() as u32;
    let unstaged = status
        .lines()
        .filter(|l| l.len() > 1 && (l.as_bytes()[1] != b' ' || l.starts_with("??")))
        .count() as u32;
    Branches {
        current,
        local,
        remote,
        remotes,
        staged,
        unstaged,
    }
}

fn valid_branch(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c == '~' || c == '^' || c == ':')
    {
        return Err(format!("{name:?} is not a branch name"));
    }
    Ok(())
}

/// Switch, create, or delete a branch. Delete is `-d`: a branch git will not delete safely is
/// one whose commits would vanish, and that is not a panel button's decision to make.
pub fn git_branch_act(root: &Utf8Path, action: &str, name: &str) -> Result<String, String> {
    valid_branch(name)?;
    match action {
        // `switch` refuses when it would lose work, which is the behaviour a button needs.
        "checkout" => {
            if name.contains('/')
                && git(
                    root,
                    &["rev-parse", "--verify", &format!("refs/heads/{name}")],
                )
                .is_none()
            {
                // A remote branch: make the tracking branch on the way.
                let short = name.split_once('/').map(|x| x.1).unwrap_or(name);
                git_run(root, &["switch", "--track", "-c", short, name])
            } else {
                git_run(root, &["switch", name])
            }
        }
        "create" => git_run(root, &["switch", "-c", name]),
        "delete" => git_run(root, &["branch", "-d", name]),
        _ => Err(format!("unknown branch action: {action}")),
    }
}

/// Fetch, pull, or push.
pub fn git_remote_act(root: &Utf8Path, action: &str) -> Result<String, String> {
    match action {
        "fetch" => git_run(root, &["fetch", "--all", "--prune"]),
        // `--ff-only`: a merge commit nobody asked for is not a pull, and a conflict is a
        // decision for a person with a terminal.
        "pull" => git_run(root, &["pull", "--ff-only"]),
        "push" => git_push(root),
        _ => Err(format!("unknown remote action: {action}")),
    }
}

/// Stage or unstage everything.
pub fn git_stage_all(root: &Utf8Path, stage: bool) -> Result<(), String> {
    if stage {
        git_run(root, &["add", "-A"]).map(|_| ())
    } else {
        git_run(root, &["reset", "-q"]).map(|_| ())
    }
}

/// Commit what is staged, and only that.
pub fn git_commit_staged(root: &Utf8Path, message: &str) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("a commit needs a message".into());
    }
    if git(root, &["diff", "--cached", "--quiet"]).is_some() {
        return Err("Nothing is staged. Stage files first, or commit everything.".into());
    }
    git_run(root, &["commit", "-q", "-m", message]).map(|_| ())
}

/// Take the last commit apart, keeping its changes in the working tree.
///
/// `--soft`, never `--hard`: undoing a commit Keel made must not undo the work in it.
pub fn git_uncommit(root: &Utf8Path) -> Result<(), String> {
    let count = git(root, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if count < 2 {
        return Err("This is the first commit; there is nothing to go back to.".into());
    }
    git_run(root, &["reset", "--soft", "HEAD~1"]).map(|_| ())
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
mod attach_tests {
    /// The name comes from a pasteboard or a browser, so it is a string and not a path.
    ///
    /// Everything that could climb out of the attachments directory has to end up inside it or
    /// nowhere. Asserted on the sanitiser rather than through the handler because the property is
    /// about the name, and a test that needs an HTTP server to check a string is a test people
    /// stop running.
    #[test]
    fn a_pasted_name_cannot_escape_the_attachments_directory() {
        for hostile in [
            "../../evil.sh",
            "/etc/passwd",
            "..\\..\\windows",
            "....//....//x",
            "",
            ".",
            "..",
        ] {
            let safe = super::sanitise_attachment_name(hostile);
            assert!(
                !safe.contains('/'),
                "{hostile:?} kept a separator: {safe:?}"
            );
            assert!(
                !safe.contains('\\'),
                "{hostile:?} kept a separator: {safe:?}"
            );
            assert!(
                !safe.contains(".."),
                "{hostile:?} kept a traversal: {safe:?}"
            );
            assert!(!safe.is_empty(), "{hostile:?} sanitised to nothing");
        }
    }

    /// An ordinary name survives recognisably. A sanitiser that turns `shot.png` into `--------`
    /// is safe and useless.
    #[test]
    fn an_ordinary_name_is_left_alone() {
        assert_eq!(super::sanitise_attachment_name("shot.png"), "shot.png");
        assert_eq!(
            super::sanitise_attachment_name("paste-4213-chars.txt"),
            "paste-4213-chars.txt"
        );
        assert_eq!(super::sanitise_attachment_name("a b.png"), "a-b.png");
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
            paths.contains(&".github/workflows/a.yml")
                && paths.contains(&".github/workflows/b.yml"),
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

    /// Discard is the only panel action that destroys anything, so it is the one worth a test:
    /// the file has to come back exactly as it was committed, and staging has to be reversible.
    #[test]
    fn a_file_can_be_staged_unstaged_and_put_back() {
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
        std::fs::write(root.join("a.txt"), "committed\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        let staged = |root: &Utf8Path| {
            git_status(root)
                .changes
                .iter()
                .find(|c| c.path == "a.txt")
                .map(|c| c.staged)
        };
        assert_eq!(staged(&root), Some(false), "an edit starts unstaged");

        git_act(&root, "stage", "a.txt", None).expect("stage");
        assert_eq!(staged(&root), Some(true));
        git_act(&root, "unstage", "a.txt", None).expect("unstage");
        assert_eq!(staged(&root), Some(false));

        git_act(&root, "discard", "a.txt", None).expect("discard");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "committed\n"
        );
        assert!(
            git_status(&root).changes.is_empty(),
            "the working tree is clean again"
        );

        // A path that is not in the repository never reaches git.
        assert!(git_act(&root, "discard", "../../etc/hosts", None).is_err());
        assert!(git_act(&root, "nonsense", "a.txt", None).is_err());
    }

    /// Branches: create, switch, list with tracking; stage-all and a staged-only commit.
    #[test]
    fn branches_can_be_made_switched_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "--quiet", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        git_branch_act(&root, "create", "feature").unwrap();
        let b = git_branches(&root);
        assert_eq!(b.current.as_deref(), Some("feature"));
        assert_eq!(b.local.len(), 2);
        assert!(b.local.iter().any(|x| x.name == "main" && !x.current));

        std::fs::write(root.join("b.txt"), "x\n").unwrap();
        assert_eq!(git_branches(&root).unstaged, 1);
        assert!(git_commit_staged(&root, "nothing staged").is_err());
        git_stage_all(&root, true).unwrap();
        assert_eq!(git_branches(&root).staged, 1);
        git_commit_staged(&root, "add b").unwrap();
        assert_eq!(git_branches(&root).staged, 0);

        git_branch_act(&root, "checkout", "main").unwrap();
        assert!(!root.join("b.txt").exists(), "switched");
        assert!(
            git_branch_act(&root, "delete", "feature").is_err(),
            "unmerged: -d refuses"
        );
        assert!(git_branch_act(&root, "create", "bad name").is_err());
        assert!(git_branch_act(&root, "create", "-x").is_err());
    }

    /// The log reads back what was committed, and undoing keeps the work.
    #[test]
    fn the_log_lists_commits_and_uncommit_keeps_the_changes() {
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
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);
        assert!(git_uncommit(&root).is_err(), "the first commit stays");

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("b.txt"), "new\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "turn 1: add b"]);

        let log = git_log(&root, 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "turn 1: add b");
        assert_eq!(log[0].files, 2);
        assert!(log[0].when > 0);

        git_uncommit(&root).unwrap();
        assert_eq!(git_log(&root, 10).len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("b.txt")).unwrap(),
            "new\n",
            "the work survives"
        );
    }

    /// The header's numbers, not the header's `@@`.
    #[test]
    fn hunk_headers_number_both_sides() {
        let raw = "@@ -48,6 +48,14 @@ describe(\"x\", () => {\n a\n+b\n c\n@@ -7 +7,2 @@\n x\n+y\n";
        let hunks = parse_hunks(raw);
        assert_eq!(hunks[0].lines[0].old, Some(48));
        assert_eq!(hunks[0].lines[0].new, Some(48));
        assert_eq!(hunks[0].lines[1].new, Some(49));
        assert_eq!(hunks[0].lines[2].old, Some(49));
        assert_eq!(hunks[1].lines[0].old, Some(7));
        assert_eq!(hunks[1].lines[1].new, Some(8));
    }

    /// Discarding one hunk leaves the other. The reason the action exists at all.
    #[test]
    fn one_hunk_can_be_discarded_on_its_own() {
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
        let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("a.txt"), &body).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "seed"]);

        // Two edits far enough apart to be two hunks.
        let edited = body
            .replace("line 2\n", "LINE 2\n")
            .replace("line 28\n", "LINE 28\n");
        std::fs::write(root.join("a.txt"), &edited).unwrap();
        assert_eq!(git_diff(&root, "a.txt").hunks.len(), 2);

        git_act(&root, "discard-hunk", "a.txt", Some(0)).expect("discard the first");
        let now = std::fs::read_to_string(root.join("a.txt")).unwrap();
        assert!(now.contains("line 2\n"), "the first edit is gone");
        assert!(now.contains("LINE 28\n"), "the second survives");
        assert_eq!(git_diff(&root, "a.txt").hunks.len(), 1);

        assert!(
            git_act(&root, "discard-hunk", "a.txt", Some(5)).is_err(),
            "no such hunk"
        );
        assert!(git_act(&root, "discard-hunk", "a.txt", None).is_err());
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

#[derive(Default, Serialize)]
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

    DiffResponse {
        path: path.to_string(),
        hunks: parse_hunks(&raw),
        untracked,
    }
}

/// `@@` blocks with numbered lines, from any unified diff.
fn parse_hunks(raw: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);

    for line in raw.lines() {
        if line.starts_with("@@") {
            // @@ -old,count +new,count @@ …  — the numbers only. The `@@` used to be the first
            // token, failed to parse, and every hunk's old side started at line 1.
            let nums: Vec<u32> = line
                .split_once(" @@")
                .map(|(head, _)| head)
                .unwrap_or(line)
                .split(['-', '+', ',', ' ', '@'])
                .filter_map(|s| s.parse().ok())
                .collect();
            old_no = nums.first().copied().unwrap_or(1);
            // A hunk with one old line has no count: `-7 +7,2`. Old is first, new is the one
            // after old's count when there is one.
            let old_has_count = line
                .split_whitespace()
                .nth(1)
                .is_some_and(|t| t.contains(','));
            new_no = nums
                .get(if old_has_count { 2 } else { 1 })
                .copied()
                .unwrap_or(1);
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
    hunks
}
