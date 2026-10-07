//! Persistent provider sessions. The helper hosts the official Claude SDK and Codex App Server;
//! Rust keeps ownership of checkout claims, checkpoints, stop signals and verification.
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
    sync::mpsc,
};

fn processes() -> &'static Mutex<HashSet<u32>> {
    static PROCESSES: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    PROCESSES.get_or_init(Mutex::default)
}

/// Parent shutdown uses process::exit, so idle provider sessions need explicit cleanup too.
pub fn stop_all() {
    for pid in processes().locked().drain() {
        crate::signals::end_tree(pid);
    }
}

type Output = mpsc::Sender<Value>;
#[derive(Default)]
pub(crate) struct Runtimes(Mutex<HashMap<String, Arc<Runtime>>>);
struct Runtime {
    pid: u32,
    input: tokio::sync::Mutex<ChildStdin>,
    output: Arc<Mutex<Option<Output>>>,
    alive: Arc<AtomicBool>,
    pending: Arc<Mutex<HashSet<String>>>,
}
impl Runtime {
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
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start the agent runtime: {e}"))?;
    let input = child.stdin.take().ok_or("Runtime stdin is missing")?;
    let stdout = child.stdout.take().ok_or("Runtime stdout is missing")?;
    let stderr = child.stderr.take().ok_or("Runtime stderr is missing")?;
    let alive = Arc::new(AtomicBool::new(true));
    let output: Arc<Mutex<Option<Output>>> = Default::default();
    let pending = Arc::new(Mutex::new(HashSet::new()));
    let r = Arc::new(Runtime {
        pid: child.id().unwrap_or(0),
        input: tokio::sync::Mutex::new(input),
        output: output.clone(),
        alive: alive.clone(),
        pending: pending.clone(),
    });
    let pid = r.pid;
    processes().locked().insert(pid);
    state.runtimes.0.locked().insert(lane.into(), r.clone());
    let errors = output.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let tx = errors.locked().clone();
            if let Some(tx) = tx {
                let _ = tx.send(json!({"event":"err", "data":line})).await;
            }
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(id) = record["data"]["id"].as_str() {
                match record["event"].as_str() {
                    Some("request") => {
                        pending.locked().insert(id.to_owned());
                    }
                    Some("resolved") => {
                        pending.locked().remove(id);
                    }
                    _ => {}
                }
            }
            if record["event"] == "end" {
                pending.locked().clear();
            }
            let tx = output.locked().clone();
            if let Some(tx) = tx {
                let _ = tx.send(record).await;
            }
        }
        let status = child.wait().await;
        alive.store(false, Ordering::Release);
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
        if q.method == "answer" && !r.pending.locked().remove(&q.id) {
            return Err((
                StatusCode::CONFLICT,
                "That request is no longer pending.".into(),
            ));
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
    let (tx, rx) = mpsc::channel(256);
    let out = crate::agent::translate(rx, &q);
    tokio::spawn(async move {
        if let Err(error) = run(state, q, &tx).await {
            emit(&tx, "fatal", json!(error)).await;
        }
    });
    crate::agent::alive(out)
}

async fn run(state: Arc<AppState>, q: ChatQuery, tx: &Tx) -> Result<(), String> {
    let lane = q.lane.as_deref().ok_or("A lane is required")?;
    let cwd = state.checkout(q.wt.as_deref())?;
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
    let (begun, system) = tokio::task::spawn_blocking(move || {
        let keep = |rel: &str, bytes: &[u8]| crate::vouched::holds(&checkout, rel, bytes);
        keel_harness::quarantine_keeping(&checkout, keep).map_err(|e| e.to_string())?;
        Ok::<_, String>((
            crate::turns::begin(&checkout),
            crate::agent::system_prompt(&checkout),
        ))
    })
    .await
    .map_err(|e| e.to_string())??;
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
    r.send(json!({"method":"start", "query": {
        "lane":lane, "provider":provider, "cwd":cwd, "repo":repo, "prompt":q.prompt, "session":q.session,
        "mode":q.mode, "model":q.model, "attachments":q.attachments, "system":system, "settings":settings,
        "mcp":serde_json::from_str::<Value>(&crate::askmcp::config(state.port(), Some(lane))).unwrap_or(Value::Null), "executable":crate::permissions::program(provider).to_string_lossy()
    }})).await?;
    let mut session = q.session.clone();
    let mut usage = None;
    let mut code = -1;
    let mut error = None;
    let mut forward = None;
    while let Some(record) = records.recv().await {
        let data = record["data"].clone();
        let name = record["event"].as_str().unwrap_or("");
        if name == "msg" {
            if let Some(id) = data["session_id"].as_str() {
                session = Some(id.into());
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
            state.keyed(lane, token, session, &key);
            let mut bus = crate::turns::subscribe(session);
            crate::turns::emit(&repo, session, &key, started.clone());
            let tx = tx.clone();
            let key = key.clone();
            forward = Some(tokio::spawn(async move {
                while let Ok(fact) = bus.recv().await {
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
    *r.output.locked() = None;
    state.finished(lane, token);
    let stopped = state.was_interrupted(lane, token) || code == -1;
    if stopped {
        emit(tx, "#stopped", Value::Null).await;
    }
    emit(tx, "done", json!(code)).await;
    if let Some(session) = session {
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
            || true,
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
