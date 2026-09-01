//! Running `claude` for one turn, and turning what it says into a stream the app can draw.
//!
//! The prompt, the arguments, the two pipes, the classification of a non-zero exit, and the
//! redaction that lets a failure be reported without carrying anybody's paths in it.

use anyhow::Result;
use axum::{
    Json,
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use crate::lock::Locked;
use crate::serve::AppState;

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
    /// Extra system prompt for this turn — a persona and its evidence — so the conversation
    /// shows the ask, not the instructions behind it.
    pub system: Option<String>,
    /// The window's own id, so a question asked through `ask_user` lands in its lane.
    pub lane: Option<String>,
    /// `--model`, when the person chose one; absent means the CLI's own default.
    pub model: Option<String>,
    /// Agent runtime. Unsupported values fail rather than silently selecting another provider.
    pub provider: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct StopQuery {
    /// The conversation to stop. Absent means the project's own lane, which is what a window with
    /// no worktree is — never "all of them".
    pub lane: Option<String>,
}

/// Stop the turn running in one conversation.
///
/// This exists because closing the event stream is not stopping anything: the daemon only noticed
/// a hung-up client when the *next* line arrived, so a turn sitting quiet inside a three-minute
/// test kept running while the app said it had stopped. `AppState::interrupt` sends SIGINT to the
/// agent's process group, which is what ⌃C does in a terminal.
pub async fn stop(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StopQuery>,
) -> Json<Stopped> {
    Json(Stopped {
        stopped: state.interrupt(query.lane.as_deref().unwrap_or_default()),
    })
}

#[derive(Serialize)]
pub struct Stopped {
    /// False when there was nothing running — the turn had already finished on its own.
    pub stopped: bool,
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
    // Facts about the room, not instructions about how to behave. The agent is Claude Code —
    // the person's own `claude`, with its own judgement — and this used to be eleven
    // paragraphs telling it how to talk, when to stop and what not to say, which is exactly
    // what made the conversation read like a wrapper. What remains is what it cannot know
    // otherwise: what the person can see, what checks the work, how a refusal comes back.
    let mut out = String::from(
        "Notes from Keel, the IDE this session runs in. Facts, not instructions:\n\n         - Every file you write shows as a live diff beside this conversation; the person also          sees the terminal, the check output and the readiness report.\n",
    );

    match crate::verify::detect_all(repo).as_slice() {
        [] => out.push_str("- This project has no check command configured.\n"),
        [check] if check.dir.is_empty() => out.push_str(&format!(
            "- `{}` is this project's check command (from {}). Keel runs it after each of your turns and shows the result.\n",
            check.command, check.source
        )),
        // A folder of repositories has a gate per repository, and the agent needs to know both —
        // otherwise it runs the one it happens to find and reports the work as checked.
        checks => {
            out.push_str("- This folder holds several projects, each with its own check. Keel runs all of them after each of your turns:\n");
            for check in checks {
                out.push_str(&format!("  - `{}` in `{}`\n", check.command, check.dir));
            }
        }
    }

    // The answer to "run it", which the agent otherwise goes and rediscovers: read the package
    // manifest, look for a script, work out which package in a monorepo. Keel already found it —
    // it is what the Designer's Run button starts — and not saying so turns a two-word ask into a
    // research detour every time.
    if let Some(dev) = crate::dev::detect(repo) {
        out.push_str(&format!(
            "- `{}` starts this project's dev server{}. Keel starts it from the Designer tab and \
             points the preview at whatever URL it announces, so there is nothing to work out.\n",
            dev.command,
            if dev.dir.is_empty() {
                String::new()
            } else {
                format!(", run in `{}`", dev.dir)
            }
        ));
    }

    let (_manager, present, absent) = crate::clitools::toolchain();
    out.push_str(&format!("- On this machine: {}.", present.join(", ")));
    if !absent.is_empty() {
        out.push_str(&format!(" Not installed: {}.", absent.join(", ")));
    }
    out.push('\n');

    // Measured, twice: a turn is one `claude -p`, and the CLI kills every tracked background
    // shell at teardown — `gh run watch` was `[killed]` eight seconds after the turn ended, and
    // the person only found out six minutes on. So the hook intercepts the call and `monitor.rs`
    // runs the command in the daemon instead. The agent is told all of that in the refusal it
    // gets back; this bullet is here so it plans for it rather than discovering it.
    out.push_str(
        "- Background commands belong to Keel, not to your turn. Call `Bash` with \
         `run_in_background: true` as usual: the person is asked whether to monitor it, Keel runs \
         it outside the turn, and its output is delivered to you as a new message when it \
         finishes. Your own call comes back refused, naming the job it became — that is the \
         confirmation, not a failure. Do not poll it and do not start it again; say you are \
         watching it and end the turn. If the answer is no, do not detach it by hand instead — \
         `nohup`, `setsid`, `disown` and a trailing `&` reach the same question and get the same \
         answer, and a shell that got past it dies with the turn with nobody watching.\n\
         - Permissions: a command outside the allowed set comes back refused; Keel shows the person \
         the refusal with a button to allow it, so say what you needed.\n\
         - To ask the person a question with options, call the `ask_user` tool (server `keel`); \
         the answer is returned as its result. `AskUserQuestion` does not exist in this session.\n\
         - In plan mode `ask_user` is refused — Claude Code blocks every MCP tool there, and it is \
         not Keel doing it. So put anything you would have asked into the plan itself, as the \
         decision you made and the one you would have preferred to check. Write the plan to the \
         path your plan-mode reminder names, exactly as you normally would: Keel intercepts that \
         write, shows the plan with an Approve button, and approving starts the build as its own \
         turn in edit mode on this conversation. That write is the only way out of plan mode \
         here, so do not end the turn with the plan in your reply alone.\n\
         - MCP servers: use `claude mcp add --scope local`, not `.mcp.json` — Keel quarantines that          file as repository content. Keel also quarantines `.claude/settings.json` hooks.\n",
    );

    // A blank project ships the template catalogue as patterns; an agent that has them and
    // does not use them is designing from scratch for no reason.
    if repo.join("docs/PATTERNS.md").exists() {
        out.push_str(
            "\n## Patterns\n\n`docs/PATTERNS.md` holds reference architectures — APIs, job systems, \
             agents, control and data planes, gateways — with components, flow and the rules that \
             keep them up. Before designing a component, find the closest pattern and build to it, \
             and say which one you used.\n",
        );
    }

    // The contract: written by the review, corrected by the team, enforced from then on. It
    // is a subagent so the person can invoke it, and it is named here so every turn reads it.
    if repo.join(".claude/agents/contract.md").exists() {
        out.push_str(
            "\n## The contract\n\n`.claude/agents/contract.md` is this repository's engineering \
             contract: the invariants, the rules and the plan, first written by the staff-engineer \
             review and since corrected by the team. It outranks your defaults. Read it before \
             changing anything. When the person corrects a rule in conversation, update the \
             contract's own section so the file stays the agreement — never the review section \
             below it, which is history.\n",
        );
    }

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

/// What went wrong, as one word that can be grouped on.
///
/// Ordered, and the order is the whole trick: "no conversation found" contains "not found", and
/// a generic bucket that swallows the specific one is how a report stays useless.
fn classify(stderr: &str) -> &'static str {
    let s = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    if s.is_empty() {
        "silent"
    } else if has(&["no conversation found", "session id"]) {
        "no-such-session"
    } else if has(&[
        "invalid api key",
        "/login",
        "unauthorized",
        "authentication",
        "oauth",
        "expired",
    ]) {
        "auth"
    } else if has(&[
        "credit balance",
        "usage limit",
        "rate limit",
        "quota",
        "429",
    ]) {
        "limits"
    } else if has(&["unknown option", "unexpected argument", "unrecognized"]) {
        "bad-flag"
    } else if has(&[
        "econnrefused",
        "enotfound",
        "fetch failed",
        "socket hang up",
        "etimedout",
    ]) {
        "network"
    } else if has(&[
        "enoent",
        "no such file",
        "command not found",
        "not installed",
    ]) {
        "missing"
    } else {
        "unclassified"
    }
}

/// Strip what identifies a person or their work, keep what identifies the bug.
///
/// The same rule as `Telemetry.redact` on the app side, written without a regex crate because
/// this workspace has none and one dependency for five patterns is not a trade worth making.
/// Conservative by construction: a token is kept only when it cannot be a path, a URL, a quoted
/// name, an address, an id or anything long enough to be a secret.
fn redact(stderr: &str) -> String {
    let line = stderr.lines().next_back().unwrap_or_default();
    let mut out = String::new();
    for token in line.split_whitespace() {
        let bare = token.trim_matches(|c: char| c == ',' || c == '.' || c == ')' || c == '(');
        let identifying = bare.starts_with('\'')
            || bare.starts_with('"')
            || bare.starts_with('~')
            || bare.contains('/')
            || bare.contains('\\')
            || bare.contains('@')
            || bare.chars().count() > 24
            || (bare.chars().count() >= 8
                && bare
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_'));
        out.push_str(if identifying { "<x>" } else { bare });
        out.push(' ');
    }
    out.trim().chars().take(200).collect()
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    /// Ten issues in Sentry saying "exit 1" and nothing else. The two answers that were
    /// indistinguishable are the two most likely ones.
    #[test]
    fn a_failure_is_named_rather_than_counted() {
        for (stderr, expected) in [
            ("", "silent"),
            (
                "No conversation found with session ID: 9f2c-aa",
                "no-such-session",
            ),
            ("Invalid API key · Please run /login", "auth"),
            ("Credit balance is too low", "limits"),
            (
                "error: unknown option '--forward-subagent-text'",
                "bad-flag",
            ),
            (
                "FetchError: request failed, reason: ECONNREFUSED",
                "network",
            ),
            ("env: node: No such file or directory", "missing"),
            ("something nobody has seen before", "unclassified"),
        ] {
            assert_eq!(classify(stderr), expected, "for: {stderr}");
        }
    }

    /// The reason it used to travel as nothing at all. A git error quotes branch names, a
    /// filesystem error quotes paths, and neither may leave the machine — but the sentence
    /// saying which failure it was is exactly what makes the report worth having.
    #[test]
    fn what_identifies_the_person_never_leaves_but_the_failure_does() {
        let out = redact(
            "fatal: a branch named 'keel/add-billing' already exists in /Users/anna/Dev/acme",
        );
        for leak in ["anna", "acme", "add-billing", "/Users", "keel/"] {
            assert!(!out.contains(leak), "{leak} survived redaction: {out}");
        }
        assert!(
            out.starts_with("fatal: a branch named"),
            "the failure is gone: {out}"
        );

        // Bounded, because a stack trace pasted into an issue is not a report.
        let huge = "x ".repeat(4_000);
        assert!(redact(&huge).chars().count() <= 200);

        // A session id is an id, not a word.
        assert!(!redact("No conversation found with session ID: 9f2caa31-88").contains("9f2caa31"));
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for (p, body) in files {
            let full = root.join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        (dir, root)
    }

    #[test]
    fn the_prompt_names_this_project_s_gate() {
        let (_d, root) = repo_with(&[("Makefile", "check:\n\tcargo test\n")]);
        let prompt = system_prompt(&root);
        assert!(
            prompt.contains("`make check` is this project's check command"),
            "{prompt}"
        );
        assert!(prompt.contains("Keel runs it after each of your turns"));
    }

    /// "Run it" is two words and used to become a research detour: the agent read the manifest,
    /// hunted for a script and worked out which package in a monorepo owned it. Keel already knows
    /// — it is what the Designer's Run button starts — so it says so.
    #[test]
    fn the_prompt_names_how_to_run_the_project() {
        let (_d, root) = repo_with(&[(
            "package.json",
            r#"{"scripts":{"dev":"next dev"},"dependencies":{"next":"15"}}"#,
        )]);
        let prompt = system_prompt(&root);
        assert!(
            prompt.contains("`npm run dev` starts this project's dev server"),
            "{prompt}"
        );
    }

    /// In a monorepo the directory is half the answer, and the half that is actually annoying.
    #[test]
    fn the_prompt_names_the_directory_the_dev_server_runs_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Utf8Path::from_path(dir.path()).expect("utf8");
        std::fs::create_dir_all(root.join("dashboard")).unwrap();
        std::fs::write(
            root.join("dashboard/package.json"),
            r#"{"scripts":{"dev":"next dev"},"dependencies":{"next":"15"}}"#,
        )
        .unwrap();
        let prompt = system_prompt(root);
        assert!(prompt.contains("run in `dashboard`"), "{prompt}");
    }

    /// A repository with nothing to run says nothing, rather than inventing a command.
    #[test]
    fn a_project_with_no_dev_server_is_not_given_one() {
        let (_d, root) = repo_with(&[("README.md", "x")]);
        assert!(!system_prompt(&root).contains("dev server"));
    }

    #[test]
    fn the_prompt_is_facts_not_instructions() {
        let (_d, root) = repo_with(&[("README.md", "x")]);
        let prompt = system_prompt(&root);
        // What used to be here: eleven paragraphs on tone, scope and when to stop. The agent
        // is Claude Code with its own judgement; Keel tells it about the room and nothing else.
        for directive in [
            "Do the thing that was asked",
            "do not paste back",
            "Never report",
            "Say plainly",
            "## Scope",
        ] {
            assert!(
                !prompt.contains(directive),
                "directive survived: {directive}"
            );
        }
        assert!(prompt.contains("Facts, not instructions"));
        assert!(prompt.contains(".mcp.json") && prompt.contains(".claude/settings.json"));
        assert!(
            prompt.lines().count() < 20,
            "short: {}",
            prompt.lines().count()
        );
    }

    /// The agent promised to watch a CI run and report back, and the shell it started was killed
    /// eight seconds later when the turn ended. Keel runs it instead — but only the prompt can
    /// stop the agent planning around a background task it no longer owns.
    #[test]
    fn the_prompt_says_who_owns_a_background_command() {
        let (_d, root) = repo_with(&[("README.md", "x")]);
        let prompt = system_prompt(&root);
        assert!(
            prompt.contains("Background commands belong to Keel"),
            "{prompt}"
        );
        // The refusal it gets back is a confirmation. An agent that reads it as a failure runs
        // the job a second time in the foreground.
        assert!(
            prompt.contains("that is the confirmation, not a failure"),
            "{prompt}"
        );
    }

    #[test]
    fn the_prompt_says_what_is_installed() {
        let (_d, root) = repo_with(&[("README.md", "x")]);
        let prompt = system_prompt(&root);
        assert!(prompt.contains("On this machine:"));
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
) -> impl axum::response::IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    // The agent runs in the lane's checkout; its permissions come from the project. The two are
    // different paths on purpose, and `settings_json` below is handed the project.
    let repo = state.repo();
    let cwd = match state.checkout(query.wt.as_deref()) {
        // A resumed session runs where it was started. Validated: the repository, a parent
        // within two levels, or a subdirectory — nowhere else.
        Ok(_) if query.cwd.is_some() && query.session.is_some() => {
            match state.session_dir_checked(query.cwd.as_deref()) {
                Some(dir) => dir,
                // Resuming it here would run another project's conversation against this
                // project's files. Every path the agent already knows would be unreadable, and
                // what the person sees is the agent saying it has no access to a folder — which
                // is exactly the report that led here. Refuse, and say which project it belongs
                // to rather than pretending.
                None => {
                    let where_from = query.cwd.clone().unwrap_or_default();
                    let name = where_from
                        .rsplit('/')
                        .find(|p| !p.is_empty() && *p != ".")
                        .unwrap_or("another project")
                        .to_string();
                    tokio::spawn(async move {
                        let _ = tx
                            .send(Ok(Event::default().event("fatal").data(format!(
                                "That conversation was started in “{name}”, which is not the \
                                 project open here. Open that project and resume it there — \
                                 resuming it in this one would point it at files it has never \
                                 seen."
                            ))))
                            .await;
                    });
                    return alive(rx);
                }
            }
        }
        Ok(p) => p,
        Err(e) => {
            tokio::spawn(async move {
                let _ = tx.send(Ok(Event::default().event("fatal").data(e))).await;
            });
            return alive(rx);
        }
    };
    let port = state.port();
    let stopper = state.clone();

    tokio::spawn(async move {
        // Said before anything else, because everything else takes time. Building the system
        // prompt reads the repository, detects the gate and the dev server and scans it for
        // findings; on a large project that is seconds, and until now not one byte reached the app
        // in that window. A blank screen is what people read as "stuck".
        let _ = tx
            .send(Ok(Event::default()
                .event("starting")
                .data("reading the project")))
            .await;

        // The lane, and the right to write its checkout, before anything is spawned.
        //
        // A caller with no lane of its own gets a key nothing else can collide with. It used to
        // get `""`, which every lane-less caller shared: two of them overwrote each other's pid
        // and only one could ever be stopped.
        let lane = match query.lane.clone().filter(|l| !l.is_empty()) {
            Some(lane) => lane,
            None => {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                format!(
                    "anon-{}",
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                )
            }
        };
        // A plan turn writes nothing, so it may sit beside one that does — which is the whole
        // point of a lane for reading beside a lane that is editing.
        let writes = query.mode.as_deref() != Some("plan");
        let token = match stopper.claim(&lane, &cwd, writes) {
            Ok(token) => token,
            Err(why) => {
                let _ = tx.send(Ok(Event::default().event("fatal").data(why))).await;
                return;
            }
        };
        // Every path out of this task from here on releases it. `Guard` rather than a call at
        // each `return`, because there are five of them and the sixth would be the bug: a lane
        // left claimed is a lane that can never take another turn, which is the "never stuck"
        // failure with the worst shape — the window looks idle and every send is refused.
        let _lane_held = crate::serve::Held::new(stopper.clone(), lane.clone(), token);

        let provider = query.provider.as_deref().unwrap_or("claude");
        let mut command = if provider == "codex" {
            let mut command = Command::new("codex");
            command.current_dir(&cwd).arg("exec");
            // The sandbox and the directory on *both* branches. They used to be on the first
            // turn only, so every Codex turn after it ran with codex's own defaults: no `--cd`,
            // so a lane's turn ran outside the lane's checkout, and no `--sandbox`, so `mode`
            // stopped being enforced — including `plan`. A lane the window believed was planning
            // is exempt from the guard against two writers on one tree, and was able to write.
            //
            // The system prompt stays on the first turn alone: a resumed conversation already has
            // it, and `resume` takes the prompt as its argument.
            let sandbox = match query.mode.as_deref() {
                Some("acceptEdits") => "workspace-write",
                _ => "read-only",
            };
            command
                .arg("--json")
                .arg("--color")
                .arg("never")
                .arg("--sandbox")
                .arg(sandbox)
                .arg("--cd")
                .arg(&cwd);
            if let Some(session) = &query.session {
                command.arg("resume").arg(session).arg(&query.prompt);
            } else {
                command.arg(
                    match query
                        .system
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        Some(extra) => format!(
                            "{}\n\n{extra}\n\nTask:\n{}",
                            system_prompt(&cwd),
                            query.prompt
                        ),
                        None => format!("{}\n\nTask:\n{}", system_prompt(&cwd), query.prompt),
                    },
                );
            }
            command
        } else if provider == "claude" {
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
                    query.lane.as_deref(),
                    &cwd,
                ))
                // Keel's one MCP tool, `ask_user`; the person's own servers stay (no --strict).
                .arg("--mcp-config")
                .arg(crate::askmcp::config(port, query.lane.as_deref()))
                .arg("--append-system-prompt")
                .arg(
                    match query
                        .system
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        Some(extra) => format!("{}\n\n{extra}", system_prompt(&cwd)),
                        None => system_prompt(&cwd),
                    },
                )
                // Note what is deliberately *not* in that prompt: an instruction to avoid shell
                // expansion. It was tried, and with and without it the first command out was
                // `wc -l < a.txt; echo "exit: $?"` both times. The agent self-corrects from the
                // refusal text either way, so it bought nothing and cost tokens every turn. The fix
                // that works is in the UI, which says what an expansion refusal is.
                ;

            // A lane runs in `<repo>/.keel/worktrees/<name>`, and everything at the project root
            // is then outside the agent's working directory — including `.keel/attachments`,
            // where Keel puts the file the person just dragged into the chat. The agent asked to
            // read it and was told it had no permission, in the one flow where the person had
            // just handed it the file. Verified in a real transcript.
            if cwd != repo {
                command.arg("--add-dir").arg(&repo);
            }

            if let Some(session) = &query.session {
                command.arg("--resume").arg(session);
            }
            command
        } else {
            let _ = tx
                .send(Ok(Event::default()
                    .event("fatal")
                    .data(format!("unsupported agent provider `{provider}`"))))
                .await;
            return;
        };

        // Every provider's output is read the same way, so the pipe belongs here rather than in
        // one of the branches — which is exactly how Codex ended up without one.
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        if let Some(model) = model_arg(query.model.as_deref()) {
            command.arg("--model").arg(model);
        }

        // Its own process group, so Stop can interrupt the agent *and* whatever it started. A
        // `cargo test` the agent spawned is the case that matters: signalling only the parent
        // leaves the build running and the turn is not actually stopped.
        command.process_group(0);

        // Before the agent starts, and every time.
        //
        // `--bare` is never passed, because bare mode never reads the OAuth credentials a
        // subscription depends on — and the price of that is that the *repository's* own
        // `.claude/settings.json` loads. A hook there is a shell command that runs on the machine
        // of whoever opens the repo, which is why the scanner rates one Critical.
        //
        // `keel-harness::quarantine` existed for exactly this and was wired only to the `keel
        // trust` subcommand, which the application never runs. Opening somebody else's repository
        // and taking one turn executed their `SessionStart` hook, with no prompt: verified by
        // running a turn against a repo whose hook touched a file, and finding the file.
        //
        // Here rather than only at project-open because a hook can arrive later — a `git pull`, a
        // branch switch, a checkout the agent itself made. It is idempotent and it moves rather
        // than deletes: the file goes to `.keel/quarantine/` where the person can read it.
        if let Err(e) = keel_harness::quarantine(&cwd) {
            tracing::warn!("could not quarantine repository agent config: {e}");
            // Security-relevant and previously invisible: the repository's own hooks will run on
            // this machine. The kind travels, never the path.
            sentry::capture_message(
                "quarantine failed: repository hooks will load",
                sentry::Level::Error,
            );
            let _ = tx
                .send(Ok(Event::default().event("err").data(format!(
                    "Keel could not quarantine this repository's .claude/settings.json ({e}). \
                     Its hooks will run on your machine."
                ))))
                .await;
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                sentry::with_scope(
                    |scope| scope.set_tag("provider", provider),
                    || sentry::capture_message("could not start the agent", sentry::Level::Error),
                );
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(format!(
                        "could not start `{provider}`: {e}. Is the CLI installed and on PATH?"
                    ))))
                    .await;
                return;
            }
        };

        // Recorded before the first line is read, so a Stop arriving immediately still finds it.
        let pid = child.id().unwrap_or(0);
        if pid != 0 {
            stopper.started(&lane, token, pid);
        }

        // Drained, always, and not only to report it.
        //
        // A piped stream nobody reads fills its buffer and then *blocks the child mid-write* —
        // measured here at 64KB, past which stdout never closes again and the turn hangs with the
        // app showing "thinking" forever. Reading it concurrently is what makes that impossible.
        //
        // It is also the only place the reason for some failures is written: `--resume` against a
        // conversation that no longer exists prints "No conversation found with session ID: …"
        // here and puts nothing but `is_error` on stdout.
        let errors = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let errors = errors.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                // Undecodable bytes must not end the drain the way EOF does: stopping there
                // leaves the pipe to fill, and a full pipe blocks the child mid-write, which is
                // the wedge this whole drain exists to prevent. A reader that is actually broken
                // does end it — see `crate::lines`.
                loop {
                    match crate::lines::next(&mut lines).await {
                        crate::lines::Next::Line(line) => {
                            let mut held = errors.locked();
                            // Capped: a runaway process must not become a runaway allocation.
                            if held.len() < 16_384 {
                                held.push_str(&line);
                                held.push('\n');
                            }
                        }
                        crate::lines::Next::Skipped => continue,
                        crate::lines::Next::Done => break,
                    }
                }
            });
        }

        // Whether the turn said anything at all before it died. "Exited 1 having streamed 400
        // lines" and "exited 1 without a word" are different bugs, and the report could not tell
        // them apart.
        let mut streamed = false;
        if let Some(stdout) = child.stdout.take() {
            let mut lines = BufReader::new(stdout).lines();
            // Forward each JSONL record verbatim. Translating event shapes here would mean two
            // places to update when the CLI's stream changes; the app does the interpreting.
            //
            // `Err` is not EOF. Treating them alike truncated the turn with no `fatal`, and then
            // called `wait()` on a child still writing into a pipe nobody was draining — no
            // `done`, no `fatal`, the stream open forever. One non-UTF-8 byte was enough.
            loop {
                let line = match crate::lines::next(&mut lines).await {
                    crate::lines::Next::Line(line) => line,
                    // Said once per bad line and then carried on. This used to `continue` on
                    // *any* read error, so a reader that could not recover produced this message
                    // forever, at the speed of the loop.
                    crate::lines::Next::Skipped => {
                        let _ = tx
                            .send(Ok(Event::default()
                                .event("err")
                                .data("a line of the agent's output was not text; skipped")))
                            .await;
                        continue;
                    }
                    crate::lines::Next::Done => break,
                };
                streamed = true;
                if tx
                    .send(Ok(Event::default().event("msg").data(line)))
                    .await
                    .is_err()
                {
                    // The app disconnected — the window closed, the lane closed, the app quit.
                    // The whole tree goes, not just `claude`: `start_kill()` is `kill(pid)` on
                    // the leader, so the `cargo test` it had running would have survived it,
                    // reparented to init, with nothing left that knows it exists. That is the
                    // failure `BudgetTests` was written for, reached by a different door.
                    crate::signals::end_tree(pid);
                    return;
                }
            }
        }

        let status = child.wait().await;
        let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

        // A provider that failed and said why on stderr used to say it to nobody: the app was
        // handed `done` with a code it ignored, and drew an empty turn card. An exit of 0 is
        // silent as before — plenty of tools warn on stderr and succeed.
        if code != 0 {
            let why = errors.locked().trim().to_string();
            // Every non-zero exit is reported. It used to travel as the provider and the code and
            // nothing else, on the grounds that stderr quotes paths and branch names — which is
            // true, and left ten identical issues saying "exit 1" with no way to tell what
            // failed. So the *cause* goes instead: a name matched from the message, and the last
            // line with the identifying parts taken out by the same rule `Telemetry.redact` uses
            // on the app side. Sign-in and a spent balance are the two most likely answers here,
            // and neither was distinguishable before.
            let cause = classify(&why);
            sentry::with_scope(
                |scope| {
                    scope.set_tag("provider", provider);
                    scope.set_tag("exit", code.to_string());
                    scope.set_tag("cause", cause);
                    scope.set_tag("streamed", streamed.to_string());
                },
                || {
                    sentry::capture_message(
                        &format!("the agent exited non-zero: {cause}: {}", redact(&why)),
                        sentry::Level::Error,
                    )
                },
            );
            if why.is_empty() {
                // Silence used to be the one case the person was told nothing about: the app got
                // `done` with a code it ignores and drew an empty turn card, which is what the
                // whole `fatal` path exists to prevent.
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(format!(
                        "`{provider}` exited with code {code} and printed nothing. Run \
                         `{provider} -p hi` in a terminal — a sign-in that has expired or a spent \
                         balance fails exactly like this and says so there."
                    ))))
                    .await;
            } else {
                let tail: String = why
                    .lines()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(tail)))
                    .await;
            }
        }

        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });

    // A turn that is thinking sends nothing at all, and nothing on either side is watching.
    //
    // The app's session timeout for the stream is an hour, deliberately — a turn legitimately runs
    // for minutes. But that means a daemon that dies, or a socket that goes away, presents as
    // "thinking…" for an hour with a Stop button that signals a process nobody is reading from.
    // A comment line every fifteen seconds costs nothing, is discarded by the client's parser, and
    // turns a wedged stream into something the app can notice and say.
    alive(rx)
}

/// The stream, with a heartbeat.
///
/// A turn that is thinking sends nothing at all, and nothing on either side is watching. The app's
/// timeout for this one stream is an hour, deliberately — a turn legitimately runs for minutes.
/// But that means a daemon that dies, or a socket that goes away, presents as "thinking…" for an
/// hour behind a Stop button that signals a process nobody is reading from. A comment line every
/// fifteen seconds costs nothing, is discarded by the client's parser, and turns a wedged stream
/// into something the app can notice and say.
///
/// Every return from `chat` goes through here, including the ones that only carry a `fatal`.
fn alive(rx: tokio::sync::mpsc::Receiver<Result<Event, Infallible>>) -> axum::response::Response {
    use axum::response::IntoResponse;
    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
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

/// The `--model` argument, if there is one to pass.
///
/// Both CLIs take `--model`; what differs is which names they answer to, and the picker offers
/// each lane its own provider's list. Empty means "whatever the CLI's own config says", which is
/// how somebody with `model = "gpt-5.6-sol"` in `~/.codex/config.toml` keeps it.
///
/// The character filter is the older half and stays: a model name is a word, and anything else
/// arriving on that argument is somebody putting a shell fragment where a model goes. `.` is
/// allowed because real names have it (`claude-sonnet-4.5`, `gpt-5.6-sol`).
fn model_arg(model: Option<&str>) -> Option<&str> {
    model.filter(|m| {
        !m.is_empty()
            && m.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    })
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    /// The other half of the same mistake: a pipe that is created and never read.
    ///
    /// A child blocks mid-write once a pipe nobody drains fills its buffer — measured on macOS at
    /// somewhere past 64KB. `stderr` was piped and read by nobody, so a provider that said enough
    /// on it stopped being able to write to *stdout* as well, `next_line()` waited forever, and
    /// `child.wait()` was never reached. The turn never ended and the app sat on "thinking" with
    /// no error, no exit code and nothing to stop.
    ///
    /// The test spawns exactly that shape and proves the reader must be concurrent: draining
    /// stdout alone never sees the last line.
    #[tokio::test]
    async fn an_undrained_stderr_wedges_the_child() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("echo first; yes padding | head -c 300000 >&2; echo last")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("could not spawn `sh`");

        // Exactly what the handler used to do: stdout to EOF, stderr untouched.
        let stdout = child.stdout.take().expect("piped");
        let read_everything = async {
            let mut lines = BufReader::new(stdout).lines();
            let mut seen = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                seen.push(line);
            }
            seen
        };
        let wedged = tokio::time::timeout(std::time::Duration::from_secs(5), read_everything).await;
        let _ = child.start_kill();
        assert!(
            wedged.is_err(),
            "a child writing 300KB to an undrained stderr reached EOF on stdout — if this now \
             passes the platform buffers it, and the concurrent drain is still what makes the \
             guarantee rather than the buffer size"
        );
    }

    /// The bug that made Codex do nothing at all.
    ///
    /// `child.stdout` is `Some` only when the command was configured with `Stdio::piped()`, and
    /// that call lived inside the `claude` branch — so Codex's JSONL went to the daemon's own
    /// stdout, `if let Some(stdout)` never ran, no `msg` event was ever sent, and the app watched
    /// a turn start and then end with nothing in between. Nothing failed; nothing appeared.
    ///
    /// This is the shape of the mistake rather than the call site: a provider branch that forgets
    /// the pipe produces a child whose output nobody can read.
    #[tokio::test]
    async fn a_child_without_a_piped_stdout_cannot_be_read() {
        let mut unpiped = Command::new("echo");
        unpiped.arg("hello");
        let mut child = unpiped.spawn().expect("could not spawn `echo`");
        assert!(
            child.stdout.take().is_none(),
            "a command with no `Stdio::piped()` still handed back a readable stdout — \
             if this ever passes, the guard below is testing nothing"
        );
        let _ = child.wait().await;

        let mut piped = Command::new("echo");
        piped.arg("hello").stdout(Stdio::piped());
        let mut child = piped.spawn().expect("could not spawn `echo`");
        assert!(
            child.stdout.take().is_some(),
            "the pipe every provider's output is read through is missing"
        );
        let _ = child.wait().await;
    }
    /// Real model names from both CLIs survive, including the dots and dashes they contain.
    #[test]
    fn a_real_model_name_is_passed_through() {
        assert_eq!(model_arg(Some("opus")), Some("opus"));
        assert_eq!(model_arg(Some("gpt-5-codex")), Some("gpt-5-codex"));
        assert_eq!(
            model_arg(Some("claude-sonnet-4.5")),
            Some("claude-sonnet-4.5")
        );
        assert_eq!(model_arg(Some("gpt-5.6-sol")), Some("gpt-5.6-sol"));
    }

    /// Empty means "leave it to the CLI's own config", which is the default and the only correct
    /// answer for somebody running a model this list has never heard of.
    #[test]
    fn no_model_means_the_cli_decides() {
        assert_eq!(model_arg(None), None);
        assert_eq!(model_arg(Some("")), None);
    }

    /// A model name is a word. Anything else on that argument is somebody putting a shell
    /// fragment where a model goes.
    #[test]
    fn a_model_name_that_is_not_a_name_is_dropped() {
        assert_eq!(model_arg(Some("opus; rm -rf /")), None);
        assert_eq!(model_arg(Some("../../etc/passwd")), None);
        assert_eq!(model_arg(Some("$(whoami)")), None);
    }
}
