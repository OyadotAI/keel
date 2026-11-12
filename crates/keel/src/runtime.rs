//! Persistent provider sessions. The helper hosts the official Claude SDK and Codex App Server;
//! Rust keeps ownership of checkout claims, checkpoints, stop signals and verification.
mod initialization;
#[cfg(test)]
mod tests;
use crate::{
    agent::{ChatQuery, Said},
    lock::Locked,
    serve::AppState,
    signals::Leads,
};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    process::Stdio,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{mpsc, oneshot},
};

fn processes() -> &'static Mutex<HashSet<u32>> {
    static PROCESSES: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    PROCESSES.get_or_init(Mutex::default)
}

/// A pid on the parent-death list for as long as this lives. A one-shot turn's agent is a child
/// of the daemon like a runtime's, and was on no list at all.
pub struct Tracked(u32);
impl Tracked {
    pub fn new(pid: u32) -> Self {
        if pid != 0 {
            processes().locked().insert(pid);
        }
        Self(pid)
    }
}
impl Drop for Tracked {
    fn drop(&mut self) {
        processes().locked().remove(&self.0);
    }
}

/// Parent shutdown uses process::exit, so idle provider sessions need explicit cleanup too.
pub fn stop_all() {
    for pid in processes().locked().drain() {
        crate::signals::end_tree(pid);
    }
}

type Output = mpsc::Sender<Value>;
const ANSWER_ACK_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
#[derive(Default)]
pub(crate) struct Runtimes(Mutex<HashMap<String, Arc<Runtime>>>);
type Inspections = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;
struct InspectionWait {
    pending: Inspections,
    id: String,
}
impl Drop for InspectionWait {
    fn drop(&mut self) {
        self.pending.locked().remove(&self.id);
    }
}
struct Runtime {
    pid: u32,
    input: tokio::sync::Mutex<ChildStdin>,
    output: Arc<Mutex<Option<Output>>>,
    alive: Arc<AtomicBool>,
    inspections: Inspections,
    answers: Inspections,
    inspection_lock: tokio::sync::Mutex<()>,
    browser_generation: Arc<Mutex<Option<String>>>,
}

/// Read telemetry from an existing runtime, including while no turn owns its output channel.
pub async fn inspect_context(state: &AppState, lane: &str) -> Result<Value, String> {
    // no-blocking: reads runtime handles in memory, writes async stdin, and awaits a bounded reply.
    let runtime = state
        .runtimes
        .0
        .locked()
        .get(lane)
        .cloned()
        .ok_or("No managed runtime is attached")?;
    let _held = runtime
        .inspection_lock
        .try_lock()
        .map_err(|_| "A context refresh is already pending")?;
    if !runtime.alive.load(Ordering::Acquire) {
        return Err("The runtime is disconnected".into());
    }
    let id = crate::context::request_id();
    let (tx, rx) = oneshot::channel();
    runtime.inspections.locked().insert(id.clone(), tx);
    let _wait = InspectionWait {
        pending: runtime.inspections.clone(),
        id: id.clone(),
    };
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        runtime
            .send(json!({"method":"inspect_context","id":id}))
            .await?;
        rx.await
            .map_err(|_| "The runtime disconnected".to_string())?
    })
    .await
    .map_err(|_| "Context inspection timed out".to_string())
    .and_then(|v| v)
}
impl Runtime {
    /// The helper must accept the answer into its live callback before HTTP can acknowledge it.
    async fn answer(&self, id: &str, answer: Value) -> Result<Value, String> {
        self.acked(json!({"method":"answer","id":id,"answer":answer}))
            .await
    }

    /// Send a command the helper acknowledges with `answer_ack`, and wait for its verdict. A steer
    /// needs one as an answer does: whether the words joined the turn decides whether the page
    /// shows them in it or queues them for the next.
    async fn acked(&self, mut command: Value) -> Result<Value, String> {
        let delivery = crate::context::request_id();
        command["delivery"] = json!(delivery);
        let (tx, rx) = oneshot::channel();
        self.answers.locked().insert(delivery.clone(), tx);
        let _waiting = InspectionWait {
            pending: self.answers.clone(),
            id: delivery.clone(),
        };
        answer_ack(self.send(command), rx).await
    }

    async fn send(&self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.input
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|e| e.to_string())
    }
}

async fn answer_ack(
    send: impl std::future::Future<Output = Result<(), String>>,
    rx: oneshot::Receiver<Result<Value, String>>,
) -> Result<Value, String> {
    tokio::time::timeout(ANSWER_ACK_WAIT, async {
        send.await?;
        rx.await
            .map_err(|_| "The runtime disconnected before acknowledging the answer.".to_string())?
    })
    .await
    .map_err(|_| "Answer delivery was not confirmed. Retry the same answer.".to_string())?
}

fn runtime_command(
    node: std::path::PathBuf,
    helper: std::path::PathBuf,
    cwd: &camino::Utf8Path,
) -> Command {
    let mut command = Command::new(node);
    command
        .arg(helper)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in crate::agent::INHERITED {
        command.env_remove(name);
    }
    command.lead_group();
    command.env("MCP_TOOL_TIMEOUT", crate::askmcp::tool_timeout());
    command
}

fn helper_paths() -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = executable
        .parent()
        .ok_or("The executable has no directory")?;
    let node = dir.join(if cfg!(windows) {
        "keel-node.exe"
    } else {
        "keel-node"
    });
    for helper in [
        dir.join("runtime/keel-runtime.mjs"),
        dir.join("../Resources/runtime/keel-runtime.mjs"),
    ] {
        if node.is_file() && helper.is_file() {
            return Ok((node, helper));
        }
    }
    #[cfg(debug_assertions)]
    {
        let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../desktop/src-tauri/runtime/keel-runtime.mjs");
        if helper.is_file() {
            return Ok((crate::permissions::program("node").into(), helper));
        }
    }
    Err(
        "The packaged agent runtime is missing. Rebuild Keel with the runtime packaging step."
            .into(),
    )
}

async fn runtime(
    state: &AppState,
    lane: &str,
    cwd: &camino::Utf8Path,
) -> Result<Arc<Runtime>, String> {
    if let Some(existing) = state
        .runtimes
        .0
        .locked()
        .get(lane)
        .filter(|r| r.alive.load(Ordering::Acquire))
    {
        return Ok(existing.clone());
    }
    let (node, helper) = tokio::task::spawn_blocking(helper_paths)
        .await
        .map_err(|e| e.to_string())??;
    spawn_runtime(state, lane, cwd, runtime_command(node, helper, cwd)).await
}

// Keep process ownership and protocol forwarding identical for packaged helpers and offline tests.
async fn spawn_runtime(
    state: &AppState,
    lane: &str,
    cwd: &camino::Utf8Path,
    mut command: Command,
) -> Result<Arc<Runtime>, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start the agent runtime: {e}"))?;
    let input = child.stdin.take().ok_or("Runtime stdin is missing")?;
    let stdout = child.stdout.take().ok_or("Runtime stdout is missing")?;
    let stderr = child.stderr.take().ok_or("Runtime stderr is missing")?;
    let alive = Arc::new(AtomicBool::new(true));
    let output: Arc<Mutex<Option<Output>>> = Default::default();
    let inspections = Arc::new(Mutex::new(HashMap::<
        String,
        oneshot::Sender<Result<Value, String>>,
    >::new()));
    let r = Arc::new(Runtime {
        pid: child.id().unwrap_or(0),
        input: tokio::sync::Mutex::new(input),
        output: output.clone(),
        alive: alive.clone(),
        inspections: inspections.clone(),
        inspection_lock: tokio::sync::Mutex::new(()),
        answers: Default::default(),
        browser_generation: Arc::new(Mutex::new(crate::browser::binding_id(lane))),
    });
    let answers = r.answers.clone();
    let pid = r.pid;
    let browser_lane = lane.to_string();
    let browser_generation = r.browser_generation.clone();
    processes().locked().insert(pid);
    state.runtimes.0.locked().insert(lane.into(), r.clone());
    let errors = output.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        // `lines::next`: undecodable bytes are skipped, not taken as the end of the pipe — a
        // helper whose stderr stopped being read fills it and blocks mid-write.
        loop {
            let line = match crate::lines::next(&mut lines).await {
                crate::lines::Next::Line(line) => line,
                crate::lines::Next::Skipped => continue,
                crate::lines::Next::Done => break,
            };
            let tx = errors.locked().clone();
            if let Some(tx) = tx {
                let _ = tx.send(json!({"event":"err", "data":line})).await;
            }
        }
    });
    let context_repo = cwd.to_owned();
    let context_lane = lane.to_owned();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        // A turn's whole output is this pipe: one undecodable byte must not end it and leave the
        // turn waiting on an `end` that was never going to be read.
        loop {
            let line = match crate::lines::next(&mut lines).await {
                crate::lines::Next::Line(line) => line,
                crate::lines::Next::Skipped => continue,
                crate::lines::Next::Done => break,
            };
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if record["event"] == "context" {
                crate::context::receive(&context_repo, &context_lane, record["data"].clone());
                continue;
            }
            if record["event"] == "answer_ack" {
                let data = &record["data"];
                if let Some(id) = data["id"].as_str()
                    && let Some(tx) = answers.locked().remove(id)
                {
                    let _ = tx.send(Ok(data.clone()));
                }
                continue;
            }
            if record["event"] == "inspection" {
                let data = &record["data"];
                if let Some(id) = data["id"].as_str() {
                    let pending = inspections.locked().remove(id);
                    if let Some(tx) = pending {
                        let _ = tx.send(if let Some(error) = data["error"].as_str() {
                            Err(error.to_owned())
                        } else {
                            Ok(data["snapshot"].clone())
                        });
                    }
                }
                continue;
            }
            let tx = output.locked().clone();
            if let Some(tx) = tx {
                let _ = tx.send(record).await;
            }
        }
        let status = child.wait().await;
        alive.store(false, Ordering::Release);
        crate::browser::release(&browser_lane, browser_generation.locked().take().as_deref());
        inspections.locked().clear();
        answers.locked().clear();
        processes().locked().remove(&pid);
        let tx = output.locked().take();
        if let Some(tx) = tx {
            let code = status
                .ok()
                .and_then(|s| s.code())
                .filter(|c| *c != 0)
                .unwrap_or(1);
            let _ = tx.send(json!({"event":"fatal", "data":"The agent runtime exited unexpectedly. You can retry in this lane."})).await;
            let _ = tx.send(json!({"event":"end", "data":{"code":code}})).await;
        }
    });
    Ok(r)
}

#[derive(Deserialize)]
pub struct Control {
    lane: String,
    method: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    answer: Value,
    #[serde(default)]
    prompt: String,
}
pub async fn control(
    State(state): State<Arc<AppState>>,
    Json(q): Json<Control>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !["answer", "steer", "interrupt", "close"].contains(&q.method.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "Unknown session control".into()));
    }
    let r = state.runtimes.0.locked().get(&q.lane).cloned();
    let Some(r) = r else {
        return if q.method == "close" {
            // An Antigravity turn is a one-shot `agy`, not a runtime: closing its lane must still
            // end it, or it runs on with nothing on screen.
            let browser_generation = crate::browser::binding_id(&q.lane);
            state.interrupt(&q.lane);
            crate::browser::release(&q.lane, browser_generation.as_deref());
            Ok(Json(json!({"ok":true})))
        } else {
            Err((
                StatusCode::NOT_FOUND,
                "The session is no longer running".into(),
            ))
        };
    };
    if q.method == "close" {
        let pid = r.pid;
        tokio::task::spawn_blocking(move || crate::signals::end_tree(pid))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for _ in 0..100 {
            if !r.alive.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        if r.alive.load(Ordering::Acquire) {
            return Err((
                StatusCode::CONFLICT,
                "The session is still closing. Try again shortly.".into(),
            ));
        }
        state.runtimes.0.locked().remove(&q.lane);
    } else {
        if q.method == "answer" {
            return r
                .answer(&q.id, q.answer)
                .await
                .map(Json)
                .map_err(|e| (StatusCode::BAD_GATEWAY, e));
        }
        if q.method == "steer" {
            return r
                .acked(json!({"method":"steer","prompt":q.prompt}))
                .await
                .map(Json)
                .map_err(|e| (StatusCode::BAD_GATEWAY, e));
        }
        r.send(json!({"method":q.method, "id":q.id, "answer":q.answer, "prompt":q.prompt}))
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    }
    Ok(Json(json!({"ok":true})))
}

type Tx = mpsc::Sender<Result<Said, Infallible>>;
async fn emit(tx: &Tx, name: &'static str, data: Value) {
    let _ = tx
        .send(Ok(Said(
            name,
            match data {
                Value::String(s) => s,
                other => other.to_string(),
            },
        )))
        .await;
}

pub async fn chat(state: Arc<AppState>, q: ChatQuery) -> axum::response::Response {
    // no-blocking: wire channels only; the runner offloads filesystem preparation.
    // A turn per `agy -p`: the one-shot path already owns the claim, quarantine, the process
    // group, Stop and `turns::finish`, and the Rust decoder reads agy's stream.
    if q.provider.as_deref() == Some("antigravity") {
        use axum::response::IntoResponse;
        return crate::agent::chat(axum::extract::State(state), axum::extract::Query(q))
            .await
            .into_response();
    }
    let (tx, rx) = mpsc::channel(256);
    let out = crate::agent::translate(rx, &q);
    tokio::spawn(async move {
        if let Err(error) = run(state, q, &tx).await {
            emit(&tx, "fatal", json!(error)).await;
        }
    });
    crate::agent::alive(out)
}

/// How long a runtime has to end a turn it was asked to interrupt before the turn's process group
/// gets SIGINT. The SDK and the App Server answer an interrupt in well under a second; a helper
/// still running at this point is wedged, and a Stop that does nothing is the worse failure.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Stop a lane's turn the way Esc does in the CLI: the runtime interrupts the turn and stays up,
/// with its session, its background agents and its warm process. SIGINT to the helper's group
/// ended all three, and the next send resumed cold and replayed the transcript over the view.
/// A lane with no live runtime (Antigravity's one-shot `agy`, a turn still preparing) is
/// signalled as before. Returns whether there was a turn to stop.
pub async fn stop(state: &Arc<AppState>, lane: &str) -> bool {
    let runtime = state
        .runtimes
        .0
        .locked()
        .get(lane)
        .filter(|r| r.alive.load(Ordering::Acquire))
        .cloned();
    let Some(runtime) = runtime else {
        return state.interrupt(lane) || state.stop_settling(lane);
    };
    let Some((token, attached)) = state.mark_interrupted(lane) else {
        return state.stop_settling(lane);
    };
    // Still preparing: `run` checks `was_interrupted` before it hands the runtime a turn.
    if !attached {
        return true;
    }
    if runtime.send(json!({"method":"interrupt"})).await.is_err() {
        state.signal_if_running(lane, token);
        return true;
    }
    let (state, lane) = (state.clone(), lane.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(STOP_GRACE).await;
        state.signal_if_running(&lane, token);
    });
    true
}

/// Only daemon-owned runtimes may be signalled by a task handoff.
pub(crate) fn owned_pid(state: &AppState, lane: &str) -> Option<u32> {
    state
        .runtimes
        .0
        .locked()
        .get(lane)
        .filter(|r| r.alive.load(Ordering::Acquire))
        .map(|r| r.pid)
}

/// An idle managed provider still owns its session and may retain children.
pub(crate) fn has_session(state: &AppState, lane: &str) -> bool {
    state
        .runtimes
        .0
        .locked()
        .get(lane)
        .is_some_and(|r| r.alive.load(Ordering::Acquire))
}

async fn run(state: Arc<AppState>, q: ChatQuery, tx: &Tx) -> Result<(), String> {
    let lane = q.lane.as_deref().ok_or("A lane is required")?;
    let cwd = state.checkout(q.wt.as_deref())?;
    let (reference_state, reference_query, reference_cwd) = (state.clone(), q.clone(), cwd.clone());
    let provider_prompt = tokio::task::spawn_blocking(move || {
        crate::references::provider_prompt(&reference_state, &reference_query, &reference_cwd)
    })
    .await
    .map_err(|e| e.to_string())??;
    let provider_prompt = crate::agent::with_shown_attachments(provider_prompt, &q.attachments);
    let repo = state.repo();
    let provider = q.provider.as_deref().unwrap_or("claude");
    if !["claude", "codex"].contains(&provider) {
        return Err("Unsupported agent provider".into());
    }
    if provider == "claude"
        && let Some(session) = q.session.as_deref()
        && let Some(home) = keel_workspace::claude_home()
        && keel_workspace::status(&home, session).is_some()
        && !state
            .runtimes
            .0
            .locked()
            .get(lane)
            .is_some_and(|r| r.alive.load(Ordering::Acquire))
    {
        return Err(
            "This conversation is still open outside Keel. Stop it there before resuming it here."
                .into(),
        );
    }
    let writes = q.mode.as_deref() == Some("acceptEdits");
    let token = state.claim(lane, &cwd, writes)?;
    let _held = crate::serve::Held::new(state.clone(), lane.into(), token);
    emit(tx, "starting", json!("Preparing the workspace")).await;
    let checkout = cwd.clone();
    let previous_session = q.session.clone();
    let selected_provider = provider.to_owned();
    let task_repo = repo.clone();
    let task_lane = lane.to_string();
    let task_prompt = q.prompt.clone();
    let task_wt = q.wt.clone();
    let task_session = q.session.clone().unwrap_or_else(|| lane.to_string());
    let begun = tokio::task::spawn_blocking(move || {
        crate::tasks::register(
            &task_repo,
            crate::tasks::Source {
                lane: task_lane.clone(),
                provider: selected_provider.clone(),
                session: task_session.clone(),
                checkout: checkout.to_string(),
                wt: task_wt,
            },
        )?;
        crate::guidance::prepare(&task_repo)?;
        crate::tasks::input(&task_repo, &selected_provider, &task_session, &task_prompt)?;
        if let Some(session) = previous_session {
            crate::context::prime(&checkout, &selected_provider, &session);
        }
        let keep = |rel: &str, bytes: &[u8]| crate::vouched::holds(&checkout, rel, bytes);
        keel_harness::quarantine_keeping(&checkout, keep).map_err(|e| e.to_string())?;
        Ok::<_, String>(crate::turns::begin(&checkout))
    })
    .await
    .map_err(|e| e.to_string())??;
    // The helper drives the agent's own CLI, so a missing one is installed here too — this is the
    // path Claude Code and Codex chat lanes take, and without it only Antigravity's installed.
    // A lane whose helper is already up got past this on its first turn; asking again only costs
    // a PATH walk per turn (and, on a runner with no agent, ran an installer under a live helper).
    if !has_session(&state, lane)
        && !crate::agent::install_missing(provider, &state, lane, token, tx).await
    {
        return Ok(());
    }
    if state.was_interrupted(lane, token) {
        emit(tx, "done", json!(-1)).await;
        return Ok(());
    }
    let r = runtime(&state, lane, &cwd).await?;
    let (output, mut records) = mpsc::channel(256);
    *r.output.locked() = Some(output);
    state.started(lane, token, r.pid);
    let key = format!(
        "keel-{}-{token}",
        crate::turns::now().replace([':', '.'], "-")
    );
    let started = crate::turns::Fact::Started {
        started: begun.started.clone(),
        snapshot: begun.snapshot.clone(),
        prompt: q.prompt.chars().take(200).collect(),
    };
    emit(
        tx,
        "fact",
        serde_json::to_value(crate::turns::Emitted {
            turn: Some(key.clone()),
            at: begun.started.clone(),
            fact: started.clone(),
        })
        .map_err(|e| e.to_string())?,
    )
    .await;
    let settings: Value = serde_json::from_str(&crate::permissions::settings_json(
        &repo,
        state.port(),
        q.session.as_deref(),
        Some(lane),
        &cwd,
    ))
    .map_err(|e| e.to_string())?;
    let mut supplied_guidance = false;
    r.send(json!({"method":"start", "query": {
        "lane":lane, "provider":provider, "cwd":cwd, "repo":repo, "prompt":provider_prompt, "session":q.session,
        "mode":q.mode, "model":q.model, "effort":crate::agent::effort_arg(q.effort.as_deref()), "attachments":q.attachments, "settings":settings, "port":state.port(),
        "executable":crate::permissions::program(provider).to_string_lossy(),
    }})).await?;
    let run_usage = crate::license::run(repo.clone()).await;
    let mut session = q.session.clone();
    let mut usage = None;
    let mut code = -1;
    let mut error = None;
    let mut forward = None;
    while let Some(record) = records.recv().await {
        let data = record["data"].clone();
        let name = record["event"].as_str().unwrap_or("");
        if name == "initialize" {
            supplied_guidance = initialization::respond(&state, &r, &q, &cwd, token, &data).await?;
            continue;
        }
        if name == "msg" {
            if let Some(id) = data["session_id"].as_str() {
                session = Some(id.into());
            }
            // Claude's `result` and the helper's `turn.completed` for Codex: the same token counts
            // (Codex reports no price). Reading only the first left Codex chat turns with no usage.
            if data["type"] == "turn.completed" {
                let u = &data["usage"];
                // Codex counts cached tokens inside its input; the decoder subtracts them too.
                let cached = u["cached_input_tokens"].as_u64().unwrap_or(0);
                usage = Some(crate::turns::Usage {
                    input: u["input_tokens"]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_sub(cached),
                    output: u["output_tokens"].as_u64().unwrap_or(0),
                    cache_read: cached,
                    cache_write: 0,
                    cost_usd: None,
                });
            }
            if data["type"] == "result" {
                let u = &data["usage"];
                usage = Some(crate::turns::Usage {
                    input: u["input_tokens"].as_u64().unwrap_or(0),
                    output: u["output_tokens"].as_u64().unwrap_or(0),
                    cache_read: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                    cache_write: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                    cost_usd: data["total_cost_usd"].as_f64(),
                });
            }
        }
        if name == "turn"
            && let Some(frames) = data.as_array()
        {
            for f in frames {
                if f["op"] == "session" {
                    session = f["id"].as_str().map(str::to_string);
                }
            }
        }
        if forward.is_none()
            && let Some(session) = &session
        {
            let (task_repo, task_lane, task_session, task_provider) = (
                repo.clone(),
                lane.to_string(),
                session.clone(),
                provider.to_string(),
            );
            let binding = crate::serve::in_blocking(move || {
                crate::tasks::session(&task_repo, &task_lane, &task_session)?;
                if !supplied_guidance {
                    return Ok(());
                }
                crate::guidance::acknowledge(
                    &task_repo,
                    &task_provider,
                    &task_session,
                    "Managed native instruction initialization",
                )
            })
            .await;
            if let Err(error) = binding {
                emit(tx, "err", json!(format!(
                    "Keel couldn’t update shared task or browser session metadata: {error}. The agent is still running; stop it before retrying the handoff."
                ))).await;
            }
            state.keyed(lane, token, session, &key);
            let mut bus = crate::turns::subscribe(session);
            {
                // A file write under the store's lock: off the executor.
                let (repo, session, key, started) =
                    (repo.clone(), session.clone(), key.clone(), started.clone());
                crate::serve::blocking(
                    move || {
                        crate::turns::emit(&repo, &session, &key, started);
                    },
                    (),
                )
                .await;
            }
            let tx = tx.clone();
            let key = key.clone();
            forward = Some(tokio::spawn(async move {
                loop {
                    // A burst of facts can outrun this reader; missing some is better than
                    // missing the gate, the commit and the end, which `while let Ok` did.
                    let fact = match bus.recv().await {
                        Ok(fact) => fact,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    if fact.turn.as_deref() != Some(&key) {
                        continue;
                    }
                    let end = matches!(fact.fact, crate::turns::Fact::Ended { .. });
                    emit(
                        &tx,
                        "fact",
                        serde_json::to_value(&*fact).unwrap_or(Value::Null),
                    )
                    .await;
                    if end {
                        break;
                    }
                }
            }));
        }
        match name {
            "end" => {
                code = data["code"].as_i64().unwrap_or(-1) as i32;
                break;
            }
            "msg" => emit(tx, "msg", data).await,
            "turn" => emit(tx, "turn", data).await,
            "snapshot" => {
                emit(tx, "snapshot", data).await;
                emit(
                    tx,
                    "fact",
                    serde_json::to_value(crate::turns::Emitted {
                        turn: Some(key.clone()),
                        at: begun.started.clone(),
                        fact: started.clone(),
                    })
                    .map_err(|e| e.to_string())?,
                )
                .await;
            }
            "request" => emit(tx, "request", data).await,
            "resolved" => emit(tx, "resolved", data).await,
            "capabilities" => emit(tx, "capabilities", data).await,
            "fatal" => {
                if !state.was_interrupted(lane, token) {
                    error = data.as_str().map(str::to_string);
                    emit(tx, "fatal", data).await;
                }
            }
            "err" => tracing::debug!("agent runtime: {}", data),
            _ => {}
        }
    }
    drop(run_usage);
    *r.output.locked() = None;
    state.finished(lane, token);
    let stopped = state.was_interrupted(lane, token) || code == -1;
    if stopped {
        emit(tx, "#stopped", Value::Null).await;
    }
    emit(tx, "done", json!(code)).await;
    if let Some(session) = session {
        let stop_seen = state.clone();
        let stop_lane = lane.to_owned();
        crate::turns::finish(
            &state,
            crate::turns::Finish {
                repo,
                checkout: cwd,
                session,
                turn: key,
                prompt: q.prompt,
                begun,
                writes,
                usage,
                failed: if stopped || code == 0 {
                    None
                } else {
                    Some(crate::turns::Failed {
                        code,
                        cause: "agent".into(),
                        tail: error.unwrap_or_else(|| "The agent did not finish the turn".into()),
                    })
                },
                expect_design: false,
                auto_commit: q.auto_commit.unwrap_or(false),
                refused: None,
            },
            // Stop during the checks ends them, and nothing is committed (`stop_settling`).
            move || !stop_seen.checks_stopped(&stop_lane, token),
            || true,
        )
        .await;
    }
    if let Some(forward) = forward {
        let abort = forward.abort_handle();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), forward).await;
        abort.abort();
    }
    Ok(())
}
