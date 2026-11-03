//! Automatic setup: in a trusted project the daemon keeps Keel's own agent team, skills and
//! suggested plugins current on its own — on open, when a writer claim frees, and when the
//! project's git state changes — and keeps one record of what it did for every window to read.
//!
//! The daemon rather than the page, because trust is read here, claims live here, and a project
//! with two windows (or none) must converge exactly once. Nothing is written in an untrusted
//! project: trust is the one decision that already means "Keel may act here without asking",
//! and it is visible and withdrawable for as long as it holds.
use crate::lock::Locked;
use crate::serve::{AppState, blocking};
use crate::team::By;
use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;
use std::sync::{Arc, Mutex, OnceLock};

/// How often a waiting pass looks again on its own, for a wait nothing else will end: a merge
/// the person finishes in their own terminal emits nothing Keel can hear.
const WAIT_TICK: std::time::Duration = std::time::Duration::from_secs(30);
/// An idle pass has nothing to look for; the tick exists so no select ever waits on nothing.
const IDLE_TICK: std::time::Duration = std::time::Duration::from_secs(3600);
/// How long one plugin install may run before it is ended: a marketplace add is a network git
/// clone, and an install that hangs would hold the root's writer claim until the daemon died.
const PLUGIN_CEILING: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// The one marketplace installed without a click. A third-party plugin ships hooks that run in
/// every later turn, and "without even knowing" is exactly how a stranger's hook would land.
const OFFICIAL: &str = "claude-plugins-official";

#[derive(Serialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Idle,
    Running,
    Waiting,
    Done,
    Failed,
}

#[derive(Serialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    #[default]
    Open,
    Click,
}

/// Why nothing is being written right now, and what it waits for.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct Waiting {
    /// `turn`, `terminal`, `keel`, a git operation's name, or `you` (your uncommitted edits).
    pub on: String,
    pub who: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct TeamDone {
    /// `added` (adoption) or `updated`.
    pub kind: &'static str,
    pub files: Vec<String>,
    pub kept: Vec<String>,
    pub yours: Vec<String>,
    pub commit: Option<String>,
    pub note: Option<String>,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct PluginDone {
    pub name: String,
    pub commit: Option<String>,
    pub error: Option<String>,
}

/// What the automatic setup is doing or last did for this daemon's project. One per daemon,
/// because one daemon is one project; its life is the project's open, which is also how long a
/// failure stays un-retried and a result stays on the chip.
#[derive(Serialize, Clone, Default, PartialEq, Debug)]
pub struct Record {
    pub root: String,
    pub phase: Phase,
    pub trigger: Trigger,
    pub step: Option<String>,
    pub waiting: Option<Waiting>,
    pub team: Option<TeamDone>,
    pub plugins: Vec<PluginDone>,
    pub error: Option<String>,
    pub at: u64,
    #[serde(skip)]
    failed_team: bool,
    /// Plugins tried this open, whatever came of it: an install that exits 0 without the index
    /// listing the plugin would otherwise be due again on every wake.
    #[serde(skip)]
    tried_plugins: Vec<String>,
}

fn record() -> &'static Mutex<Record> {
    static R: OnceLock<Mutex<Record>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Record::default()))
}

fn poke_bell() -> &'static tokio::sync::Notify {
    static P: OnceLock<tokio::sync::Notify> = OnceLock::new();
    P.get_or_init(tokio::sync::Notify::new)
}

/// Milliseconds: the page tells one result from the next by this, and two in one second happen.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Change the record and tell the windows — only when it actually changed, so a pass that does
/// nothing says nothing, and the wake it would cause ends there.
fn update(change: impl FnOnce(&mut Record)) {
    if let Some(next) = write(change) {
        crate::events::emit(
            "setup.changed",
            None,
            serde_json::to_value(&next).unwrap_or_default(),
        );
    }
}

/// Bookkeeping nobody needs to hear about — which project, who asked — and the untrusted case,
/// where nothing happened and nothing is said.
fn write(change: impl FnOnce(&mut Record)) -> Option<Record> {
    let mut r = record().locked();
    let before = r.clone();
    change(&mut r);
    if *r == before {
        return None;
    }
    r.at = now();
    Some(r.clone())
}

/// One thing a pass may do, in the order they run: the local team write before any network.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    Adopt,
    Update,
    Plugin { name: String, marketplace: String },
}

/// What is due, from the plan and the suggestions. Nothing in an untrusted project; the team
/// before plugins; a failed step not again until the next open or a click; plugins only from the
/// official marketplace and only when `plugins_ok`: a logged-in `claude` to install them, into
/// a `.claude/settings.json` that is absent or Keel's own — the CLI rewrites that file, and a
/// project's committed permissions and hooks must not be set aside and committed over unasked.
pub(crate) fn due(
    plan: &crate::team::Plan,
    suggested: &[crate::plugins::Plugin],
    rec: &Record,
    trusted: bool,
    plugins_ok: bool,
) -> Vec<Step> {
    if !trusted {
        return Vec::new();
    }
    let mut out = Vec::new();
    if !rec.failed_team {
        if !plan.present {
            out.push(Step::Adopt);
        } else if !plan.add.is_empty() || !plan.replace.is_empty() {
            out.push(Step::Update);
        }
    }
    if plugins_ok {
        out.extend(
            suggested
                .iter()
                .filter(|p| !p.installed && p.marketplace == OFFICIAL)
                .filter(|p| !rec.tried_plugins.contains(&p.name))
                .map(|p| Step::Plugin {
                    name: p.name.clone(),
                    marketplace: p.marketplace.clone(),
                }),
        );
    }
    out
}

/// Whether `.claude/settings.json` is a file a plugin install may rewrite: absent, or bytes Keel
/// itself wrote on this machine. A project's own committed file is theirs.
fn settings_are_ours(root: &camino::Utf8Path) -> bool {
    match std::fs::read(root.join(".claude/settings.json")) {
        Ok(bytes) => crate::vouched::holds(root, ".claude/settings.json", &bytes),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Whether `claude` can install a plugin. Never asked in a test: the real CLI is the network and
/// this machine's account, and a test that reached it installed plugins into its scratch repos.
async fn claude_ready() -> bool {
    if cfg!(test) {
        return false;
    }
    crate::clitools::claude_ready().await
}

/// A holder's name as the chip says it: a lane is a turn, `term:` a terminal, `keel:` Keel.
fn waiting_on(holder: &str) -> Waiting {
    let on = if holder.starts_with("term:") {
        "terminal"
    } else if holder.starts_with("keel:") {
        "keel"
    } else {
        "turn"
    };
    Waiting {
        on: on.into(),
        who: Some(holder.to_string()),
        paths: Vec::new(),
    }
}

/// One pass over the project: look, then do what is due, one unit at a time, each under its
/// own writer claim. A tree somebody is writing is waited for, never written beside.
async fn pass(state: &Arc<AppState>, trigger: Trigger) {
    if !state.project_open() {
        return;
    }
    let root = state.repo();
    write(|r| {
        if r.root != root.as_str() {
            *r = Record {
                root: root.to_string(),
                ..Record::default()
            };
        }
        if trigger == Trigger::Click {
            r.failed_team = false;
            r.tried_plugins.clear();
        }
    });
    let looked = {
        let (root, state) = (root.clone(), state.clone());
        blocking(
            move || {
                let trusted = crate::permissions::trusted(&root);
                if !trusted {
                    return None;
                }
                let plan = crate::team::plan(&root, &crate::team::installed());
                // The person's own uncommitted edits, which an open never writes over: known
                // before anything is touched, so a wait on them is a wait and not a run.
                let yours: Vec<String> = plan
                    .add
                    .iter()
                    .chain(plan.replace.iter())
                    .filter(|rel| {
                        root.join(rel.as_str()).exists() && !crate::writes::clean_before(&root, rel)
                    })
                    .cloned()
                    .collect();
                Some((
                    plan,
                    crate::plugins::suggested_for(&root),
                    crate::writes::in_progress(&root),
                    state.writer_of(&root),
                    settings_are_ours(&root),
                    yours,
                ))
            },
            None,
        )
        .await
    };
    let Some((plan, suggested, in_progress, writer, settings_ours, yours)) = looked else {
        // Untrusted: nothing happens and nothing is said — unless a result was on screen from
        // before trust was withdrawn, which is cleared where it shows.
        let had = record().locked().phase != Phase::Idle;
        let reset = |r: &mut Record| {
            *r = Record {
                root: root.to_string(),
                ..Record::default()
            }
        };
        if had {
            update(reset);
        } else {
            write(reset);
        }
        return;
    };
    let rec = record().locked().clone();
    let mut steps = due(&plan, &suggested, &rec, true, settings_ours);
    // Plugins need a logged-in `claude`. Asked before anything else is decided, so a pass with
    // only the person's edits in its way waits quietly instead of running for nothing on every
    // wake — which is what it did on a machine where `claude` was not logged in.
    if steps.iter().any(|s| matches!(s, Step::Plugin { .. })) && !claude_ready().await {
        steps.retain(|s| !matches!(s, Step::Plugin { .. }));
    }
    if steps.is_empty() {
        // Nothing to do: a wait is over, a result stays until the next open.
        update(|r| {
            if r.phase == Phase::Waiting || r.phase == Phase::Running {
                r.phase = Phase::Idle;
                r.waiting = None;
                r.step = None;
            }
        });
        return;
    }
    if let Some(holder) = writer {
        update(|r| {
            r.phase = Phase::Waiting;
            r.waiting = Some(waiting_on(&holder));
            r.step = None;
        });
        return;
    }
    if let Some(op) = in_progress {
        update(|r| {
            r.phase = Phase::Waiting;
            r.waiting = Some(Waiting {
                on: op.to_string(),
                who: None,
                paths: Vec::new(),
            });
            r.step = None;
        });
        return;
    }
    // An update whose every path is theirs has nothing to write: wait for their commit, without a
    // run that would flip the chip and ring every window for nothing.
    let blocked = steps == [Step::Update]
        && plan
            .add
            .iter()
            .chain(plan.replace.iter())
            .all(|rel| yours.contains(rel));
    if blocked {
        update(|r| {
            r.phase = Phase::Waiting;
            r.waiting = Some(Waiting {
                on: "you".into(),
                who: None,
                paths: yours.clone(),
            });
            r.step = None;
            if let Some(team) = &mut r.team {
                team.yours = yours.clone();
            }
        });
        return;
    }
    update(|r| {
        r.phase = Phase::Running;
        // Who asked belongs to the pass that writes, not to the follow-up its own events wake.
        r.trigger = trigger;
        r.waiting = None;
        r.error = None;
    });
    let mut wrote = false;
    let total = steps
        .iter()
        .filter(|s| matches!(s, Step::Plugin { .. }))
        .count();
    let mut nth = 0;
    for step in steps {
        // Trust withdrawn mid-pass stops the next unit; the one in flight is one commit of
        // Keel's own paths, and undoing that would be a history rewrite nobody asked for.
        let still = {
            let root = root.clone();
            blocking(move || crate::permissions::trusted(&root), false).await
        };
        if !still {
            break;
        }
        match step {
            Step::Adopt | Step::Update => {
                let adopt = step == Step::Adopt;
                update(|r| {
                    r.step = Some(if adopt {
                        "Adding the agent team…".into()
                    } else {
                        "Updating the agent team…".into()
                    })
                });
                let (at, st) = (root.clone(), state.clone());
                let result = blocking(
                    move || {
                        if adopt {
                            crate::review::adopt_and_commit(&st, &at, true, By::Open).map(|a| {
                                TeamDone {
                                    kind: "added",
                                    files: a.written,
                                    kept: Vec::new(),
                                    yours: a.yours,
                                    commit: a.committed,
                                    note: a.note,
                                }
                            })
                        } else {
                            crate::team::update_in_place(&st, &crate::team::installed()).map(|u| {
                                TeamDone {
                                    kind: "updated",
                                    files: u.replaced.into_iter().chain(u.added).collect(),
                                    kept: u.kept,
                                    yours: u.yours,
                                    commit: u.committed,
                                    note: u.note,
                                }
                            })
                        }
                    },
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "the write was cancelled".into(),
                    )),
                )
                .await;
                match result {
                    Ok(done) => {
                        wrote |= !done.files.is_empty();
                        // Nothing written (their edit is in the way) keeps the result that was,
                        // with only the paths still waiting on them refreshed.
                        update(|r| match (&mut r.team, done.files.is_empty()) {
                            (Some(kept), true) => kept.yours = done.yours,
                            _ => r.team = Some(done),
                        });
                    }
                    // A turn started between looking and writing: wait for it, as above.
                    Err((StatusCode::CONFLICT, _)) => {
                        let (at, st) = (root.clone(), state.clone());
                        let holder = blocking(move || st.writer_of(&at), None)
                            .await
                            .unwrap_or_else(|| "turn".into());
                        update(|r| {
                            r.phase = Phase::Waiting;
                            r.waiting = Some(waiting_on(&holder));
                            r.step = None;
                        });
                        return;
                    }
                    Err((_, why)) => update(|r| {
                        r.failed_team = true;
                        r.error = Some(why);
                    }),
                }
            }
            Step::Plugin { name, marketplace } => {
                nth += 1;
                let label = format!("Installing {name} ({nth} of {total})…");
                update(|r| r.step = Some(label.clone()));
                // The SSE side has no window here; it is drained so the installer's pipe never
                // fills. The text side is what the chip shows, line by line.
                let (tx, mut rx) = tokio::sync::mpsc::channel(64);
                let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
                let (tap, mut lines) = tokio::sync::mpsc::channel::<String>(64);
                let reader = tokio::spawn(async move {
                    // The last two lines: the final one is vouched's own sentence about the
                    // settings file (which names the commit), the one before it the CLI's.
                    let (mut prev, mut last) = (String::new(), String::new());
                    while let Some(line) = lines.recv().await {
                        if !line.trim().is_empty() {
                            prev = std::mem::replace(&mut last, line);
                            let shown = format!("{label}\n{last}");
                            update(|r| r.step = Some(shown));
                        }
                    }
                    (prev, last)
                });
                // Past the ceiling the future is dropped, which ends the CLI (`kill_on_drop`) and
                // gives the writer claim back with it.
                let code = match tokio::time::timeout(
                    PLUGIN_CEILING,
                    crate::plugins::install_one(
                        state,
                        &name,
                        &marketplace,
                        true,
                        By::Open,
                        &tx,
                        Some(&tap),
                    ),
                )
                .await
                {
                    Ok(code) => code,
                    Err(_) => {
                        let _ = tap
                            .send(format!(
                                "Gave up after {} minutes.",
                                PLUGIN_CEILING.as_secs() / 60
                            ))
                            .await;
                        -1
                    }
                };
                drop(tx);
                drop(tap);
                let _ = drain.await;
                let (prev, last) = reader.await.unwrap_or_default();
                let commit = last
                    .strip_prefix("Committed .claude/settings.json (")
                    .and_then(|s| s.strip_suffix(")."))
                    .map(str::to_string);
                let settled =
                    last.starts_with("Wrote .claude") || last.starts_with("Committed .claude");
                let why = if settled && !prev.is_empty() {
                    prev
                } else {
                    last
                };
                wrote |= code == 0;
                update(|r| {
                    r.plugins.retain(|p| p.name != name);
                    r.tried_plugins.push(name.clone());
                    r.plugins.push(PluginDone {
                        name: name.clone(),
                        commit,
                        error: (code != 0).then(|| why.clone()),
                    });
                });
            }
        }
    }
    if wrote {
        for kind in ["git.changed", "tree.changed", "state.changed"] {
            crate::events::emit(kind, None, serde_json::Value::Null);
        }
    }
    update(|r| {
        r.step = None;
        let yours: Vec<String> = r.team.as_ref().map(|t| t.yours.clone()).unwrap_or_default();
        let failed = r.failed_team || r.plugins.iter().any(|p| p.error.is_some());
        r.phase = if failed {
            Phase::Failed
        } else if !yours.is_empty() {
            r.waiting = Some(Waiting {
                on: "you".into(),
                who: None,
                paths: yours,
            });
            Phase::Waiting
        } else if r.team.is_some() || !r.plugins.is_empty() {
            Phase::Done
        } else {
            Phase::Idle
        };
    });
}

/// Which events wake a pass: trust changed, the project's own git state changed (a commit, a
/// turn's checkpoint — their edit to `CLAUDE.md` landing), or a mutating request. Never a lane's
/// worktree: the automatic setup is the root's alone.
fn wakes(e: &crate::events::Emitted) -> bool {
    matches!(e.kind, "permissions.changed" | "state.changed")
        || (e.kind == "git.changed" && e.wt.is_none())
}

/// Whether the record waits on the person's own commit. Only then does a root `tree.changed`
/// wake a pass: a commit made in a terminal emits nothing else Keel hears, and listening to every
/// save the rest of the time would run a pass per keystroke of a turn.
fn waiting_on_you() -> bool {
    let r = record().locked();
    r.phase == Phase::Waiting && r.waiting.as_ref().is_some_and(|w| w.on == "you")
}

/// The watcher: one pass on start, then one per wake, for the life of the daemon. A pass that
/// writes emits events that wake one more pass, which finds nothing due and emits nothing.
async fn run(state: Arc<AppState>) {
    let mut events = crate::events::subscribe();
    pass(&state, Trigger::Open).await;
    loop {
        // A merge finishing in a terminal emits nothing Keel hears: that wait is polled. A turn
        // ending rings `freed`; their commit shows as the root's
        // `tree.changed` (the watcher sees HEAD and the index), heard below; neither needs the clock.
        let polled = {
            let r = record().locked();
            r.phase == Phase::Waiting
                && r.waiting
                    .as_ref()
                    .is_some_and(|w| !matches!(w.on.as_str(), "you" | "turn" | "terminal" | "keel"))
        };
        let tick = tokio::time::sleep(if polled { WAIT_TICK } else { IDLE_TICK });
        let trigger = tokio::select! {
            got = events.recv() => match got {
                Ok(e) if wakes(&e) || (e.kind == "tree.changed" && e.wt.is_none() && waiting_on_you()) => Trigger::Open,
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Trigger::Open,
                Err(_) => return,
            },
            _ = state.freed.notified() => Trigger::Open,
            _ = poke_bell().notified() => Trigger::Click,
            _ = tick => Trigger::Open,
        };
        pass(&state, trigger).await;
    }
}

/// Started once per daemon, beside the event watchers.
pub fn watch(state: Arc<AppState>) {
    tokio::spawn(run(state));
}

/// `GET /api/setup`: the record, for a window connecting or reconnecting.
pub async fn api_status() -> Json<Record> {
    // no-blocking: a clone of what is in memory.
    Json(record().locked().clone())
}

/// `POST /api/setup/run`: the pass again, now, with failures forgotten — the chip's Retry and
/// every button a trusted project still shows. Returns at once; the result is an event.
pub async fn api_run() -> Json<Record> {
    // no-blocking: rings a bell the watcher task sleeps on.
    poke_bell().notify_one();
    Json(record().locked().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::{Utf8Path, Utf8PathBuf};
    use keel_generator::team::Agent;

    fn repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(d.path().into()).unwrap();
        crate::git::run(&root, &["init", "-q"]).unwrap();
        for (k, v) in [("user.name", "t"), ("user.email", "t@t")] {
            crate::git::run(&root, &["config", k, v]).unwrap();
        }
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "init"]).unwrap();
        (d, root)
    }

    fn log(root: &Utf8Path) -> Vec<String> {
        crate::git::run(root, &["log", "--format=%s"])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn plugin(name: &str, marketplace: &str, installed: bool) -> crate::plugins::Plugin {
        crate::plugins::Plugin {
            name: name.into(),
            description: String::new(),
            author: None,
            category: None,
            homepage: None,
            marketplace: marketplace.into(),
            installed,
            reason: None,
        }
    }

    /// The record is one per process; tests of the pass take turns on it. An async lock, since
    /// a std guard must not be held across the awaits inside a pass.
    async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
        static S: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let guard = S.lock().await;
        // A fresh record per test: the last one's "waiting" is not this one's.
        *record().locked() = Record::default();
        guard
    }

    #[test]
    fn due_skips_failed_plugins_third_party_and_an_untrusted_or_logged_out_machine() {
        let plan = crate::team::Plan {
            present: true,
            add: vec![".claude/agents/pm.md".into()],
            ..Default::default()
        };
        let suggested = vec![
            plugin("code-review", OFFICIAL, false),
            plugin("ponytail", "ponytail", false),
            plugin("github", OFFICIAL, true),
            plugin("security-guidance", OFFICIAL, false),
        ];
        let mut rec = Record::default();
        rec.tried_plugins.push("security-guidance".into());
        let steps = due(&plan, &suggested, &rec, true, true);
        assert_eq!(
            steps,
            vec![
                Step::Update,
                Step::Plugin {
                    name: "code-review".into(),
                    marketplace: OFFICIAL.into()
                }
            ]
        );
        assert!(due(&plan, &suggested, &rec, false, true).is_empty());
        assert_eq!(
            due(&plan, &suggested, &rec, true, false),
            vec![Step::Update]
        );
        rec.failed_team = true;
        assert_eq!(
            due(&plan, &suggested, &rec, true, false),
            Vec::<Step>::new()
        );
        let fresh = crate::team::Plan::default();
        assert_eq!(
            due(&fresh, &[], &Record::default(), true, false),
            vec![Step::Adopt]
        );
    }

    #[tokio::test]
    async fn an_untrusted_project_is_never_written() {
        let _s = serial().await;
        let (_d, root) = repo();
        let state = Arc::new(AppState::new(root.clone()));
        let mut bus = crate::events::subscribe();
        pass(&state, Trigger::Open).await;
        assert!(!root.join(".claude/agents").exists());
        assert_eq!(log(&root), vec!["init".to_string()]);
        assert_eq!(record().locked().phase, Phase::Idle);
        assert!(bus.try_recv().is_err(), "nothing happened, nothing said");
    }

    #[tokio::test]
    async fn a_trusted_project_gets_the_team_on_open_and_the_commit_says_so() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let mut bus = crate::events::subscribe();
        pass(&state, Trigger::Open).await;
        assert!(root.join(".claude/agents/pm.md").is_file());
        let head = log(&root);
        assert_eq!(head[0], "Add the agent team");
        let body = crate::git::run(&root, &["log", "-1", "--format=%b"]).unwrap();
        assert!(
            body.starts_with("Keel wrote this on project open, in a trusted project."),
            "{body}"
        );
        assert!(body.contains(".claude/agents/pm.md"));
        let rec = record().locked().clone();
        assert_eq!(rec.phase, Phase::Done, "{rec:?}");
        assert_eq!(rec.team.as_ref().map(|t| t.kind), Some("added"));
        assert!(rec.team.as_ref().unwrap().commit.is_some());
        let mut kinds = Vec::new();
        while let Ok(e) = bus.try_recv() {
            kinds.push(e.kind);
        }
        assert!(kinds.contains(&"setup.changed"), "{kinds:?}");
        assert!(kinds.contains(&"git.changed"), "{kinds:?}");
        // Current now: a second pass writes nothing and says nothing.
        pass(&state, Trigger::Open).await;
        assert_eq!(log(&root).len(), 2);
        assert!(bus.try_recv().is_err());
        assert_eq!(record().locked().phase, Phase::Done);
    }

    #[tokio::test]
    async fn a_busy_tree_waits_and_never_gets_a_lane() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        state.claim_terminal("s", &root).unwrap();
        pass(&state, Trigger::Open).await;
        assert!(!root.join(".claude/agents").exists());
        assert!(!root.join(".keel/worktrees").exists());
        let rec = record().locked().clone();
        assert_eq!(rec.phase, Phase::Waiting, "{rec:?}");
        assert_eq!(
            rec.waiting.as_ref().map(|w| w.on.as_str()),
            Some("terminal")
        );
        // The release rings the bell the watcher sleeps on, and the next pass lands it.
        state.release_terminal_except(&std::collections::HashSet::new());
        tokio::time::timeout(std::time::Duration::from_secs(1), state.freed.notified())
            .await
            .expect("a released claim wakes the setup watcher");
        pass(&state, Trigger::Open).await;
        assert_eq!(log(&root)[0], "Add the agent team");
        assert_eq!(record().locked().phase, Phase::Done);
    }

    #[tokio::test]
    async fn a_dirty_instructions_file_is_left_to_the_person() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine\n").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "instructions"]).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine, mid-edit\n").unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        pass(&state, Trigger::Open).await;
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "# mine, mid-edit\n"
        );
        assert!(root.join(".claude/agents/pm.md").is_file());
        assert_eq!(log(&root)[0], "Add the agent team");
        let rec = record().locked().clone();
        assert_eq!(rec.phase, Phase::Waiting, "{rec:?}");
        assert_eq!(rec.waiting.as_ref().map(|w| w.on.as_str()), Some("you"));
        assert_eq!(
            rec.waiting.as_ref().unwrap().paths,
            vec!["CLAUDE.md".to_string()]
        );
        // Their commit lands; the next pass appends Keel's sections and commits them alone.
        crate::git::run(&root, &["commit", "-qam", "my edit"]).unwrap();
        pass(&state, Trigger::Open).await;
        let text = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(text.starts_with("# mine, mid-edit\n"), "{text}");
        assert!(text.contains("docs/PRODUCTION.md"), "{text}");
        let rec = record().locked().clone();
        assert_eq!(rec.phase, Phase::Done, "{rec:?}");
        assert_eq!(log(&root)[0], "Update the agent team");
    }

    /// The watcher on a project it cannot finish — their `CLAUDE.md` is mid-edit — settles after
    /// one pass. It used to wake itself: the pass's own writer hold rang the bell it then slept on.
    #[tokio::test]
    async fn the_watcher_settles_when_it_cannot_write() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine\n").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "instructions"]).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine, mid-edit\n").unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let watcher = tokio::spawn(run(state.clone()));
        // The first pass adopts (a scan and a commit: seconds on a loaded machine) and lands on
        // "waiting on you". From there the watcher must go quiet.
        let settled = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            while record().locked().phase != Phase::Waiting {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(settled.is_ok(), "the first pass never reached waiting");
        let mut bus = crate::events::subscribe();
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        watcher.abort();
        let mut changes = 0;
        while let Ok(e) = bus.try_recv() {
            if e.kind == "setup.changed" {
                changes += 1;
            }
        }
        // Its own events wake at most one more pass, which writes nothing and says nothing.
        assert!(
            changes <= 2,
            "{changes} setup.changed events after settling"
        );
        let rec = record().locked().clone();
        assert_eq!(rec.waiting.as_ref().map(|w| w.on.as_str()), Some("you"));
        assert_eq!(
            rec.team.as_ref().map(|t| t.kind),
            Some("added"),
            "the adoption stays on record"
        );
    }

    /// Their commit made in a terminal ends the wait on its own: the watcher hears the root's
    /// `tree.changed` while it waits on them. QA found it waiting 75 s and counting.
    #[tokio::test]
    async fn a_terminal_commit_ends_the_wait_on_you() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine\n").unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "instructions"]).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# mine, mid-edit\n").unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let watcher = tokio::spawn(run(state.clone()));
        let reached = |phase: Phase| async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while record().locked().phase != phase {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await
            .is_ok()
        };
        assert!(reached(Phase::Waiting).await, "never waited on them");
        crate::git::run(&root, &["commit", "-qam", "my edit"]).unwrap();
        // The tree watcher's own event, as it emits it after the commit.
        crate::events::emit("tree.changed", None, serde_json::Value::Null);
        assert!(
            reached(Phase::Done).await,
            "{:?}",
            record().locked().clone()
        );
        watcher.abort();
        assert_eq!(log(&root)[0], "Update the agent team");
    }

    #[tokio::test]
    async fn a_merge_in_progress_writes_nothing() {
        let _s = serial().await;
        let (_d, root) = repo();
        crate::permissions::set_trusted(&root, true).unwrap();
        let git_dir = crate::git::trimmed(&root, &["rev-parse", "--git-dir"]).unwrap();
        std::fs::write(root.join(git_dir).join("MERGE_HEAD"), "0".repeat(40)).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        pass(&state, Trigger::Open).await;
        assert!(!root.join(".claude/agents").exists());
        let rec = record().locked().clone();
        assert_eq!(rec.phase, Phase::Waiting, "{rec:?}");
        assert_eq!(rec.waiting.as_ref().map(|w| w.on.as_str()), Some("merge"));
    }

    #[test]
    fn the_commit_body_names_who_asked_and_caps_the_list() {
        let many: Vec<String> = (0..45).map(|n| format!("f{n}")).collect();
        let open = crate::team::message(
            By::Open,
            "Add the agent team",
            "Add the team",
            &[("Files", &many)],
        );
        assert!(open.starts_with("Add the agent team\n\nKeel wrote this on project open"));
        assert!(open.contains("  f39\n  … and 5 more\n"), "{open}");
        assert!(!open.contains("f40"));
        let click = crate::team::message(By::Click, "Update the agent team", "Update team", &[]);
        assert!(click.contains("when you clicked Update team."));
        let _ = Agent::Claude;
    }
}
