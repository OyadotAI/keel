//! The team's review of a branch before its pull request opens.
//!
//! The pipeline in a repository's `CLAUDE.md` says a reviewer runs before a change is done, and
//! it is prose: the session that opened OyadotAI/oya-browser#43 ran `gh pr create` with no
//! subagent at all, and the team's findings first appeared on GitHub, after the push. So Keel
//! runs the review itself — the same `ci::local` review the pull request gets in CI — whether the
//! person pressed Pull request or the agent ran `gh pr create`, and the person opens the pull
//! request from the lane afterwards, fixed or not. It advises; it never gates. Opening without it
//! is always one click, and opening stops it.
//!
//! The record is the daemon's, not a window's: an agent-started review exists before any window
//! has asked about it, two windows must see the same one, and "Keel restarted" is a review the
//! page saw running that the daemon has no record of.
//!
//! Every review waits for the tree to have no writer before it reads anything. The agent's
//! `gh pr create` arrives mid-turn, before the turn's checkpoint commits what it wrote, so a
//! review started there would read a tree that is still moving — or be refused for uncommitted
//! changes the checkpoint was about to take care of.

use crate::ci::Done;
use crate::lock::Locked;
use crate::serve::{AppState, Checkout};
use axum::extract::{Query, State};
use axum::{Json, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a review waits for the turn writing its tree to end.
const WAIT: Duration = Duration::from_secs(15 * 60);
/// The review's own ceiling: `keel review-pr`'s default.
const CEILING: Duration = Duration::from_secs(30 * 60);
/// Finished records kept, oldest dropped first. One per lane is the most a window ever reads.
const KEPT: usize = 32;

/// What the pull request will say when it opens: the person's title and draft from the form, or
/// the agent's `gh pr create` arguments. Kept with the review so Open opens exactly that.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    /// The agent's `--base`, said back to it; the review and Open use the lane's own base.
    #[serde(default)]
    pub base: String,
}

#[derive(Serialize)]
pub struct Review {
    pub lane: String,
    pub wt: Option<String>,
    /// The commit under review, once the tree has settled; empty before.
    pub sha: String,
    pub base: String,
    /// Unix seconds.
    pub started: u64,
    /// `waiting`, `checking`, `fetching`, `reviewing`, `done`.
    pub phase: &'static str,
    pub opening: Opening,
    /// The agent's `gh pr create` started it, rather than the person.
    pub by_agent: bool,
    pub outcome: Option<Done>,
}

struct Entry {
    review: Review,
    dir: Utf8PathBuf,
    stop: Arc<AtomicBool>,
}

fn reviews() -> &'static Mutex<Vec<Entry>> {
    static R: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The lane's entry, or the one nobody's lane names in the same checkout — the `monitor` rule: a
/// hook that carried no lane files its review under `""`, and whoever asks there owns it. Only
/// there: another checkout's Open would push this lane's branch under that one's title.
fn find<'a>(all: &'a mut [Entry], lane: &str, dir: &Utf8Path) -> Option<&'a mut Entry> {
    let at = all
        .iter()
        .rposition(|e| e.review.lane == lane)
        .or_else(|| {
            all.iter()
                .rposition(|e| e.review.lane.is_empty() && e.dir == dir)
        })?;
    all.get_mut(at)
}

fn running(e: &Entry) -> bool {
    e.review.outcome.is_none()
}

/// Change the lane's record and say so.
fn set(lane: &str, change: impl FnOnce(&mut Review)) {
    let wt = {
        let mut all = reviews().locked();
        let Some(e) = all.iter_mut().rev().find(|e| e.review.lane == lane) else {
            return;
        };
        change(&mut e.review);
        e.review.wt.clone()
    };
    crate::events::emit(
        "reviews.changed",
        wt.as_deref(),
        serde_json::json!({ "lane": lane }),
    );
}

/// Start the lane's review of `dir`, or say one is already running there.
pub fn start(
    state: Arc<AppState>,
    lane: &str,
    dir: Utf8PathBuf,
    opening: Opening,
    by_agent: bool,
) -> bool {
    start_with(
        state,
        lane,
        dir,
        opening,
        by_agent,
        crate::permissions::program("claude"),
    )
}

fn start_with(
    state: Arc<AppState>,
    lane: &str,
    dir: Utf8PathBuf,
    opening: Opening,
    by_agent: bool,
    program: std::ffi::OsString,
) -> bool {
    let root = state.repo();
    let wt = dir
        .strip_prefix(root.join(crate::worktree::DIR))
        .ok()
        .map(|n| n.as_str().to_string());
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut all = reviews().locked();
        if let Some(e) = all
            .iter_mut()
            .find(|e| running(e) && (e.review.lane == lane || e.dir == dir))
        {
            // The agent asked while the person's review ran: what it would have opened is kept,
            // so Open uses its words rather than `--fill`.
            if by_agent && e.review.opening == Opening::default() {
                e.review.opening = opening;
            }
            return false;
        }
        all.retain(|e| e.review.lane != lane);
        while all.len() >= KEPT {
            let Some(oldest) = all.iter().position(|e| !running(e)) else {
                break;
            };
            all.remove(oldest);
        }
        all.push(Entry {
            review: Review {
                lane: lane.to_string(),
                wt: wt.clone(),
                sha: String::new(),
                base: String::new(),
                started: now(),
                phase: "waiting",
                opening,
                by_agent,
                outcome: None,
            },
            dir: dir.clone(),
            stop: stop.clone(),
        });
    }
    crate::events::emit(
        "reviews.changed",
        wt.as_deref(),
        serde_json::json!({ "lane": lane }),
    );
    let lane = lane.to_string();
    // Its own thread, not the blocking pool: it lives for minutes, mostly waiting. A thread that
    // cannot start ends the record at once, or it would sit at "waiting" with nothing behind it.
    let ended = lane.clone();
    let spawned = std::thread::Builder::new().spawn(move || {
        let opening = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&state, &lane, &dir, wt.as_deref(), &program, &stop)
        }));
        let done = opening.unwrap_or_else(|_| Done::None {
            cause: "other",
            line: "The review stopped unexpectedly, inside Keel.".into(),
            took: 0,
            cost: None,
        });
        set(&lane, |r| {
            r.phase = "done";
            r.outcome = Some(done);
        });
    });
    if let Err(e) = spawned {
        set(&ended, |r| {
            r.phase = "done";
            r.outcome = Some(none(
                "start",
                format!("Keel could not start the review: {e}."),
                Instant::now(),
            ));
        });
    }
    true
}

fn none(cause: &'static str, line: String, since: Instant) -> Done {
    Done::None {
        cause,
        line,
        took: since.elapsed().as_secs(),
        cost: None,
    }
}

fn run(
    state: &AppState,
    lane: &str,
    dir: &Utf8Path,
    wt: Option<&str>,
    program: &std::ffi::OsStr,
    stop: &AtomicBool,
) -> Done {
    let since = Instant::now();
    let cancelled = || none("cancelled", "The review was cancelled.".into(), since);
    while state.writer_of(dir).is_some() {
        if stop.load(Ordering::Relaxed) {
            return cancelled();
        }
        if since.elapsed() > WAIT {
            return none(
                "wait",
                format!(
                    "The turn did not end within {} minutes, so the review did not start.",
                    WAIT.as_secs() / 60
                ),
                since,
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    set(lane, |r| r.phase = "checking");
    if let Err(why) = crate::pr::precheck(dir) {
        return none("refused", why, since);
    }
    let sha = crate::git::trimmed(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    // Never the agent's `--base`: it could point the review at a branch with nothing on it and
    // get "Ship" over an empty diff, while Open opens against the default. The lane's own base,
    // or the remote's default — where the pull request will actually go.
    let branch = if let Some(name) = wt {
        crate::worktree::base_of(&state.repo(), name)
    } else {
        remote_default(dir)
    };
    set(lane, |r| {
        r.phase = "fetching";
        r.sha = sha;
        r.base = branch.clone();
    });
    // Best effort: an offline machine still has the last fetch, and the base is checked below.
    // No hooks: `reference-transaction` fires on the ref update, and it is the branch's code.
    let mut fetch = crate::git::command(dir);
    fetch
        .args(crate::git::AUTOMATIC)
        .args(["fetch", "--quiet", "origin", &branch]);
    let _ = crate::git::output_until(fetch, Duration::from_secs(60), stop);
    if stop.load(Ordering::Relaxed) {
        return cancelled();
    }
    let verifies = |r: &str| {
        crate::git::trimmed(dir, &["rev-parse", "--verify", &format!("{r}^{{commit}}")]).is_ok()
    };
    let remote = format!("origin/{branch}");
    let base = if verifies(&remote) {
        remote
    } else if verifies(&branch) {
        branch
    } else {
        return none(
            "base",
            format!("`{branch}` is not in this checkout and fetching it failed."),
            since,
        );
    };
    set(lane, |r| {
        r.phase = "reviewing";
        r.base = base.clone();
    });
    let pr = {
        let all = reviews().locked();
        let opening = all
            .iter()
            .rev()
            .find(|e| e.review.lane == lane)
            .map(|e| e.review.opening.clone())
            .unwrap_or_default();
        serde_json::json!({ "title": opening.title, "body": opening.body })
    };
    let env: Vec<(String, String)> = std::env::vars().collect();
    crate::ci::local(dir, &base, &pr, program, &env, CEILING, stop)
}

/// The branch `origin`'s HEAD points at, else `main`.
fn remote_default(dir: &Utf8Path) -> String {
    crate::git::trimmed(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|r| r.strip_prefix("origin/").map(str::to_string))
    .filter(|b| !b.is_empty())
    .unwrap_or_else(|| "main".into())
}

/// Stop the lane's review.
pub fn stop(lane: &str, dir: &Utf8Path) {
    if let Some(e) = find(&mut reviews().locked(), lane, dir).filter(|e| running(e)) {
        e.stop.store(true, Ordering::Relaxed);
    }
}

/// Put away the lane's finished review, for every window: the form comes back, and a reconnect
/// does not bring the review back with it.
pub fn dismiss(lane: &str, dir: &Utf8Path) {
    let wt = {
        let mut all = reviews().locked();
        let Some(at) = all.iter().rposition(|e| {
            !running(e) && (e.review.lane == lane || (e.review.lane.is_empty() && e.dir == dir))
        }) else {
            return;
        };
        all.remove(at).review.wt
    };
    crate::events::emit(
        "reviews.changed",
        wt.as_deref(),
        serde_json::json!({ "lane": lane }),
    );
}

/// Stop whatever review is running in `dir`: opening its pull request answers it.
pub fn stop_for(dir: &Utf8Path) {
    for e in reviews().locked().iter().filter(|e| running(e)) {
        if e.dir == dir {
            e.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Every review, on the way out. Waits briefly, so each one's scratch worktree is removed before
/// the process exits rather than left registered in the project's `.git`.
pub fn stop_all() {
    for e in reviews().locked().iter() {
        e.stop.store(true, Ordering::Relaxed);
    }
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until && reviews().locked().iter().any(running) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

// --- The agent's `gh pr create` -----------------------------------------------------------------

/// Whether a shell command opens a pull request: `gh pr create` or `gh pr new`, however it is
/// reached — `cd web && gh pr create`, `GH_REPO=a/b gh pr new`, `env gh pr create`. Quoted text
/// and heredoc bodies are data, so `echo "gh pr create"` is not one.
pub fn opens_pull_request(command: &str) -> bool {
    let command = crate::approve::without_heredocs(command);
    let mut bare = String::with_capacity(command.len());
    let (mut single, mut double) = (false, false);
    for c in command.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ if single || double => {}
            _ => bare.push(c),
        }
    }
    bare.split(['\n', ';', '|', '&', '(', ')'])
        .any(|segment| gh_pr_create(segment).is_some())
}

/// Where in `segment`'s words `pr create` starts, when its program is `gh`.
fn gh_pr_create(segment: &str) -> Option<usize> {
    if crate::approve::program_of(segment).as_deref() != Some("gh") {
        return None;
    }
    let words: Vec<&str> = segment.split_whitespace().collect();
    let gh = words
        .iter()
        .position(|w| w.rsplit('/').next() == Some("gh"))?;
    (words.get(gh + 1) == Some(&"pr") && matches!(words.get(gh + 2), Some(&"create" | &"new")))
        .then_some(gh + 3)
}

/// What the agent's `gh pr create` asked for. Anything it cannot read is left empty, which the
/// person's Open fills from the commits (`--fill`), as the button always has.
pub fn opening(command: &str) -> Opening {
    let mut out = Opening::default();
    let (command, bodies) = lift_heredocs(command);
    let Some(words) = shlex::split(&command) else {
        return out;
    };
    let put_back = |v: String| {
        bodies
            .iter()
            .enumerate()
            .fold(v, |v, (n, body)| v.replace(&placeholder(n), body))
    };
    let Some(at) = words.windows(3).position(|w| {
        w[0].rsplit('/').next() == Some("gh")
            && w[1] == "pr"
            && matches!(w[2].as_str(), "create" | "new")
    }) else {
        return out;
    };
    let mut args = words[at + 3..]
        .iter()
        .take_while(|w| !matches!(w.as_str(), "&&" | "||" | ";" | "|"));
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .unwrap_or_default()
        };
        match flag {
            "--title" | "-t" => out.title = put_back(value()),
            "--body" | "-b" => out.body = put_back(value()),
            "--base" | "-B" => out.base = value(),
            "--draft" | "-d" => out.draft = true,
            _ => {}
        }
    }
    out
}

fn placeholder(n: usize) -> String {
    format!("\u{1}keel-heredoc-{n}\u{1}")
}

/// Every `$(cat <<'EOF' … EOF)` — the form Claude Code writes a pull request body in — taken out
/// of the command before it is split into words, each replaced by a placeholder. Inside `$(…)` a
/// `"` starts a new quoting context, which a word splitter reads as the end of the outer string:
/// a body that quoted a phrase came back cut off at the phrase, beginning `$(cat <<'EOF'`.
fn lift_heredocs(command: &str) -> (String, Vec<String>) {
    const OPEN: &str = "$(cat <<";
    let mut out = String::with_capacity(command.len());
    let mut bodies = Vec::new();
    let mut rest = command;
    while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        let lifted = after.split_once('\n').and_then(|(head, tail)| {
            let end = head
                .trim()
                .trim_start_matches('-')
                .trim_matches(['\'', '"', '\\']);
            if end.is_empty() {
                return None;
            }
            let mut consumed = 0;
            for line in tail.split_inclusive('\n') {
                if line.trim() == end {
                    let body = &tail[..consumed];
                    let close = tail[consumed + line.len()..]
                        .trim_start()
                        .strip_prefix(')')?;
                    let used = after.len() - close.len();
                    return Some((body.trim_end_matches('\n').to_string(), used));
                }
                consumed += line.len();
            }
            None
        });
        match lifted {
            Some((body, used)) => {
                out.push_str(&rest[..at]);
                out.push_str(&placeholder(bodies.len()));
                bodies.push(body);
                rest = &after[used..];
            }
            None => {
                out.push_str(&rest[..at + OPEN.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, bodies)
}

/// The refusal the agent's `gh pr create` gets when Keel has started the review instead.
pub const STARTED: &str = "Not opened: Keel is reviewing this branch with the project's team \
    before the pull request opens, outside this turn. Nothing was pushed and no pull request \
    exists. The findings will appear in the lane, and the person opens the pull request from \
    there, with your title and description, or after fixing what the review finds. Do not run \
    `gh pr create` again, and do not open the pull request another way (`gh api`, `git push` and \
    a browser included). Do not say the pull request is open. Say that Keel is reviewing the \
    branch before the pull request opens, and end your turn.";

/// The refusal when a review of this branch is already running.
pub const RUNNING: &str = "Not opened: Keel is already reviewing this branch before its pull \
    request opens. Nothing was pushed. The person opens it from the lane when the review \
    finishes. Do not run `gh pr create` again or open it another way. Say that the review is \
    running and end your turn.";

/// The hook's answer to the agent's `gh pr create` in `dir`: the review starts, the call does not.
pub fn refuse(state: Arc<AppState>, lane: &str, dir: Utf8PathBuf, command: &str) -> &'static str {
    if start(state, lane, dir, opening(command), true) {
        STARTED
    } else {
        RUNNING
    }
}

// --- Handlers -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LaneQuery {
    #[serde(default)]
    pub lane: String,
}

#[derive(Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub lane: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub draft: bool,
}

/// The lane's review, and where its checkout stands now, so a moved HEAD reads as stale.
pub async fn api_get(
    State(_state): State<Arc<AppState>>,
    Checkout(dir): Checkout,
    Query(q): Query<LaneQuery>,
) -> Json<serde_json::Value> {
    let review = {
        let mut all = reviews().locked();
        find(&mut all, &q.lane, &dir).and_then(|e| serde_json::to_value(&e.review).ok())
    };
    let head = crate::serve::blocking(
        move || crate::git::trimmed(&dir, &["rev-parse", "HEAD"]).unwrap_or_default(),
        String::new(),
    )
    .await;
    Json(serde_json::json!({ "review": review, "head": head }))
}

pub async fn api_start(
    State(state): State<Arc<AppState>>,
    Checkout(dir): Checkout,
    Json(q): Json<StartBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // no-blocking: inserts a record and spawns the thread that does the work.
    let opening = Opening {
        title: q.title,
        draft: q.draft,
        ..Opening::default()
    };
    if start(state, &q.lane, dir, opening, false) {
        Ok(Json(serde_json::json!({ "started": true })))
    } else {
        Err((
            StatusCode::CONFLICT,
            "A review of this branch is already running.".into(),
        ))
    }
}

pub async fn api_stop(
    Checkout(dir): Checkout,
    Json(q): Json<LaneQuery>,
) -> Json<serde_json::Value> {
    // no-blocking: sets a flag the review's own thread reads.
    stop(&q.lane, &dir);
    Json(serde_json::json!({ "stopped": true }))
}

pub async fn api_dismiss(
    Checkout(dir): Checkout,
    Json(q): Json<LaneQuery>,
) -> Json<serde_json::Value> {
    // no-blocking: removes a record from memory.
    dismiss(&q.lane, &dir);
    Json(serde_json::json!({ "dismissed": true }))
}

// Unix, as `ci.rs`'s are: the stub `claude` is a shell script.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A project on branch `pr`, one commit ahead of `main`, with no remote.
    fn project() -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(d.path().canonicalize().unwrap()).unwrap();
        let git = |args: &[&str]| crate::git::run(&root, args).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "pr"]);
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        git(&["commit", "-qam", "change"]);
        (d, root)
    }

    /// A `claude` that runs `body`. Outside the project, so the tree it reviews stays clean.
    fn stub(dir: &Utf8Path, body: &str) -> std::ffi::OsString {
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.into_std_path_buf().into_os_string()
    }

    const SHIP: &str = r#"echo '{"is_error":false,"result":"","structured_output":{"verdict":"ship","reason":"","summary":"ok","findings":[],"report":"","stages":[{"name":"pm","ran":true},{"name":"principal","ran":true},{"name":"reviewer","ran":true},{"name":"qa","ran":true}]}}'"#;

    fn outcome(lane: &str, within: Duration) -> Option<serde_json::Value> {
        let until = Instant::now() + within;
        while Instant::now() < until {
            let all = reviews().locked();
            if let Some(e) = all.iter().rev().find(|e| e.review.lane == lane)
                && let Some(o) = &e.review.outcome
            {
                return serde_json::to_value(o).ok();
            }
            drop(all);
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn phase(lane: &str) -> &'static str {
        reviews()
            .locked()
            .iter()
            .rev()
            .find(|e| e.review.lane == lane)
            .map_or("", |e| e.review.phase)
    }

    /// The agent's `gh pr create` arrives mid-turn. The review reads nothing until that turn has
    /// let go of the tree — or it would review files the checkpoint has not committed yet.
    #[test]
    fn a_review_waits_for_the_writer() {
        let (_d, root) = project();
        let bin = tempfile::tempdir().unwrap();
        let bin = Utf8PathBuf::from_path_buf(bin.path().into()).unwrap();
        let ran = bin.join("ran");
        let program = stub(&bin, &format!("touch {ran}\n{SHIP}"));
        let state = Arc::new(AppState::new(root.clone()));
        let token = state.claim("writing", &root, true).unwrap();
        assert!(start_with(
            state.clone(),
            "wait-lane",
            root.clone(),
            Opening::default(),
            false,
            program
        ));
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(phase("wait-lane"), "waiting");
        assert!(
            !ran.exists(),
            "nothing was reviewed while the turn held the tree"
        );
        state.release("writing", token);
        let done = outcome("wait-lane", Duration::from_secs(20)).expect("finished");
        assert_eq!(done["kind"], "verdict", "{done}");
        assert_eq!(done["verdict"], "ship");
        assert!(ran.exists());
    }

    /// The branch's own git hooks never run. A husky `post-checkout` the agent wrote fired on the
    /// review's `worktree add`, as the person, on a review the agent's `gh pr create` set off.
    #[test]
    fn a_review_runs_none_of_the_branch_s_hooks() {
        let (_d, root) = project();
        let bin = tempfile::tempdir().unwrap();
        let bin = Utf8PathBuf::from_path_buf(bin.path().into()).unwrap();
        let marker = bin.join("hook-ran");
        let hooks = root.join(".husky");
        std::fs::create_dir_all(&hooks).unwrap();
        for hook in ["post-checkout", "reference-transaction"] {
            let path = hooks.join(hook);
            std::fs::write(&path, format!("#!/bin/sh\ntouch {marker}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let git = |args: &[&str]| crate::git::run(&root, args).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "hooks"]);
        // After the commit, which would fire `reference-transaction` itself.
        git(&["config", "core.hooksPath", ".husky"]);
        let state = Arc::new(AppState::new(root.clone()));
        assert!(start_with(
            state,
            "hook-lane",
            root.clone(),
            Opening::default(),
            false,
            stub(&bin, SHIP)
        ));
        let done = outcome("hook-lane", Duration::from_secs(20)).expect("finished");
        assert_eq!(done["kind"], "verdict", "{done}");
        assert!(!marker.exists(), "a hook from the branch under review ran");
    }

    /// Stop ends the agent and its scratch worktree, and the record says it was cancelled.
    #[test]
    fn stopping_ends_claude_and_the_worktree() {
        let (_d, root) = project();
        let bin = tempfile::tempdir().unwrap();
        let bin = Utf8PathBuf::from_path_buf(bin.path().into()).unwrap();
        let program = stub(&bin, "sleep 30");
        let state = Arc::new(AppState::new(root.clone()));
        assert!(start_with(
            state,
            "stop-lane",
            root.clone(),
            Opening::default(),
            false,
            program
        ));
        let until = Instant::now() + Duration::from_secs(10);
        while phase("stop-lane") != "reviewing" && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        stop("stop-lane", &root);
        let done = outcome("stop-lane", Duration::from_secs(5)).expect("stopped promptly");
        assert_eq!(done["cause"], "cancelled", "{done}");
        let list = crate::git::run(&root, &["worktree", "list"]).unwrap();
        assert_eq!(
            list.lines().count(),
            1,
            "the scratch worktree is gone: {list}"
        );
    }

    /// Every way a review can end leaves a record that says which, never one stuck running.
    #[test]
    fn a_tree_that_cannot_open_a_pull_request_ends_with_the_reason() {
        let (_d, root) = project();
        std::fs::write(root.join("a.txt"), "dirty\n").unwrap();
        let bin = tempfile::tempdir().unwrap();
        let bin = Utf8PathBuf::from_path_buf(bin.path().into()).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        assert!(start_with(
            state.clone(),
            "dirty-lane",
            root.clone(),
            Opening::default(),
            false,
            stub(&bin, SHIP)
        ));
        let done = outcome("dirty-lane", Duration::from_secs(10)).expect("finished");
        assert_eq!(done["cause"], "refused", "{done}");
        assert!(done["line"].as_str().unwrap().contains("uncommitted"));
        // And a second start while one runs is refused rather than doubled.
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        let token = state.claim("writing", &root, true).unwrap();
        assert!(start_with(
            state.clone(),
            "dirty-lane",
            root.clone(),
            Opening::default(),
            false,
            stub(&bin, SHIP)
        ));
        assert!(!start_with(
            state.clone(),
            "dirty-lane",
            root.clone(),
            Opening::default(),
            false,
            stub(&bin, SHIP)
        ));
        stop("dirty-lane", &root);
        state.release("writing", token);
        assert_eq!(
            outcome("dirty-lane", Duration::from_secs(5)).expect("ended")["cause"],
            "cancelled"
        );
    }

    /// The shapes an agent writes it in are caught, and the things that merely mention it are not.
    #[test]
    fn a_pull_request_is_recognised_however_it_is_opened() {
        for c in [
            "gh pr create",
            "cd web && gh pr create -t x",
            "GH_REPO=a/b gh pr new --fill",
            "env gh pr create",
            "git push -u origin HEAD && gh pr create --title \"x\" --body \"$(cat <<'EOF'\nbody & more\nEOF\n)\"",
            "/opt/homebrew/bin/gh pr create",
            "cat <<EOF>body.md\nhi\nEOF\ngh pr create --body-file body.md",
            "git commit -m \"use << for shifts\"\ngh pr create --fill",
        ] {
            assert!(opens_pull_request(c), "missed: {c}");
        }
        for c in [
            "echo \"gh pr create\"",
            "gh pr view 43",
            "gh pr list",
            "grep -r 'gh pr create' .",
            "cat <<EOF > notes.md\ngh pr create\nEOF",
        ] {
            assert!(!opens_pull_request(c), "false positive: {c}");
        }
    }

    /// The agent's title, body, base and draft are what the person's Open will use.
    #[test]
    fn the_agent_s_title_and_body_survive() {
        let c = "gh pr create --title \"Keep the key\" --draft -B release --body \"$(cat <<'EOF'\n## What\n\nFixes it.\nEOF\n)\"";
        assert_eq!(
            opening(c),
            Opening {
                title: "Keep the key".into(),
                body: "## What\n\nFixes it.".into(),
                draft: true,
                base: "release".into(),
            }
        );
        assert_eq!(
            opening("gh pr create --title=x"),
            Opening {
                title: "x".into(),
                ..Opening::default()
            }
        );
        assert_eq!(
            opening("gh pr create --title \"unclosed"),
            Opening::default()
        );
        // A body that quotes a phrase, and has flag-shaped words in it: whole, and not flags.
        let c = "git push -u origin HEAD && gh pr create --title \"Refusal\" --body \"$(cat <<'EOF'\n## What\n\nThe refusal now says \"do not run it\" -d -B x and stops.\nEOF\n)\"";
        assert_eq!(
            opening(c),
            Opening {
                title: "Refusal".into(),
                body: "## What\n\nThe refusal now says \"do not run it\" -d -B x and stops.".into(),
                ..Opening::default()
            }
        );
    }

    /// The agent is told what happened and what not to do instead: the route it would otherwise
    /// find around a refusal is exactly the one this exists to close.
    #[test]
    fn the_refusal_closes_the_other_doors() {
        for reason in [STARTED, RUNNING] {
            assert!(reason.starts_with("Not opened"));
            assert!(reason.contains("Nothing was pushed"));
            assert!(reason.contains("Do not run `gh pr create` again"));
            assert!(reason.contains("end your turn"));
        }
        assert!(STARTED.contains("`gh api`") && STARTED.contains("`git push`"));
        assert!(STARTED.contains("Do not say the pull request is open"));
    }
}
