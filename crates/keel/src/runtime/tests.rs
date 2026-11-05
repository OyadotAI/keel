//! Runtime control tests do not spawn a provider or create a model turn.
use super::*;
use futures_util::StreamExt;
use std::io::{BufRead, Write};

const FIXTURE_LANE: &str = "offline-background-question";
const FIXTURE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn closing_a_one_shot_chat_retires_its_browser_binding() {
    let dir = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(dir.path()).unwrap();
    let lane = "one-shot-browser-close-fixture";
    let state = Arc::new(AppState::new(root.to_owned()));
    crate::browser::prepare(root, lane, "antigravity", "session", false).unwrap();
    assert!(crate::browser::binding_id(lane).is_some());
    let _ = control(
        State(state),
        Json(Control {
            lane: lane.into(),
            method: "close".into(),
            id: String::new(),
            answer: json!(null),
            prompt: String::new(),
        }),
    )
    .await
    .unwrap();
    assert!(crate::browser::binding_id(lane).is_none());
}

/// The child speaks only the private helper protocol; it never launches an SDK or model.
#[test]
fn offline_runtime_child() {
    if std::env::var_os("KEEL_OFFLINE_RUNTIME_FIXTURE").is_none() {
        return;
    }
    let emit = |event: &str, data: Value| {
        println!("\n{}", json!({"event":event,"data":data}));
        std::io::stdout().flush().unwrap();
    };
    for line in std::io::stdin().lock().lines() {
        let q: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match q["method"].as_str().unwrap() {
            "start" => {
                if std::env::var_os("KEEL_OFFLINE_BREAK_METADATA").is_some() {
                    let path = std::path::Path::new(".keel/CHANGELOG.md");
                    std::fs::remove_file(path).unwrap();
                    std::fs::create_dir(path).unwrap();
                    emit(
                        "msg",
                        json!({"type":"system","session_id":"metadata-fixture"}),
                    );
                }
                emit(
                    "request",
                    json!({"id":"question","kind":"question","provider":"claude","input":{"questions":[]}}),
                );
                emit(
                    "msg",
                    json!({"type":"result","result":"parent-result-fixture"}),
                );
            }
            "answer" => {
                emit(
                    "resolved",
                    json!({"id":q["id"],"answered":true,"answer":q["answer"]}),
                );
                emit("answer_ack", json!({"id":q["delivery"],"ok":true}));
            }
            "initialized" => {
                emit(
                    "initialized_fixture",
                    json!({"ready":q["launch"]["system"].is_string(),"url":q["launch"]["mcp"]["mcpServers"]["keel"]["url"]}),
                );
            }
            "steer" => {
                emit(
                    "msg",
                    json!({"type":"system","subtype":"session_state_changed","state":"idle"}),
                );
                emit("end", json!({"code":0}));
            }
            _ => std::process::exit(1),
        }
    }
}

#[tokio::test]
async fn lazy_initialization_updates_the_browser_generation_owned_by_the_helper() {
    let (_dir, state, runtime, _process) = fixture_runtime(false).await;
    let query: ChatQuery = serde_json::from_value(
        json!({"prompt":"fixture","lane":FIXTURE_LANE,"provider":"claude","mode":"plan"}),
    )
    .unwrap();
    let token = state.claim(FIXTURE_LANE, &state.repo(), false).unwrap();
    let _held = crate::serve::Held::new(state.clone(), FIXTURE_LANE.into(), token);
    let (tx, mut rx) = mpsc::channel(8);
    *runtime.output.locked() = Some(tx);
    assert!(
        initialization::respond(
            &state,
            &runtime,
            &query,
            &state.repo(),
            token,
            &json!({"id":"fixture"})
        )
        .await
        .unwrap()
    );
    let observed = tokio::time::timeout(FIXTURE_WAIT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed["event"], "initialized_fixture");
    assert_eq!(observed["data"]["ready"], true);
    let generation = runtime.browser_generation.locked().clone().unwrap();
    assert!(
        observed["data"]["url"]
            .as_str()
            .unwrap()
            .contains(&format!("binding={generation}"))
    );
    assert_eq!(
        *runtime.browser_generation.locked(),
        crate::browser::binding_id(FIXTURE_LANE)
    );
    assert!(runtime.browser_generation.locked().is_some());
    fixture_control(state, "close").await;
    assert!(crate::browser::binding_id(FIXTURE_LANE).is_none());
}

struct FixtureProcess(u32);
impl Drop for FixtureProcess {
    fn drop(&mut self) {
        crate::signals::end_tree(self.0);
    }
}

async fn fixture_runtime(
    break_metadata: bool,
) -> (
    tempfile::TempDir,
    Arc<AppState>,
    Arc<Runtime>,
    FixtureProcess,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = camino::Utf8Path::from_path(dir.path()).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.hooksPath", "/nonexistent"],
        vec!["commit", "--allow-empty", "-m", "Fixture"],
        vec!["branch", "other"],
    ] {
        crate::git::run(root, &args).unwrap();
    }
    let state = Arc::new(AppState::new(root.to_owned()));
    let mut command = runtime_command(std::env::current_exe().unwrap(), "--exact".into(), root);
    command
        .args(["runtime::tests::offline_runtime_child", "--nocapture"])
        .env("KEEL_OFFLINE_RUNTIME_FIXTURE", "1");
    if break_metadata {
        command.env("KEEL_OFFLINE_BREAK_METADATA", "1");
    }
    let runtime = spawn_runtime(&state, FIXTURE_LANE, root, command)
        .await
        .unwrap();
    let guard = FixtureProcess(runtime.pid);
    (dir, state, runtime, guard)
}

async fn fixture_control(state: Arc<AppState>, method: &str) -> Value {
    control(
        State(state),
        Json(Control {
            lane: FIXTURE_LANE.into(),
            method: method.into(),
            id: "question".into(),
            answer: json!({"answers":{"choice":"custom response"}}),
            prompt: "finish".into(),
        }),
    )
    .await
    .unwrap()
    .0
}

async fn fixture_stream(state: Arc<AppState>) -> mpsc::UnboundedReceiver<String> {
    let query = serde_json::from_value(json!({"prompt":"Offline fixture", "lane":FIXTURE_LANE,"provider":"claude","mode":"plan","auto_commit":false})).unwrap();
    let mut body = chat(state, query).await.into_body().into_data_stream();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(bytes) = body.next().await {
            if tx
                .send(String::from_utf8(bytes.unwrap().to_vec()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    rx
}

async fn fixture_until(stream: &mut mpsc::UnboundedReceiver<String>, fragment: &str) {
    tokio::time::timeout(FIXTURE_WAIT, async {
        while let Some(record) = stream.recv().await {
            assert!(!record.contains("event: fatal"), "{record}");
            if record.contains(fragment) {
                return;
            }
        }
        panic!("Stream closed before {fragment}");
    })
    .await
    .unwrap();
}

async fn fixture_branch_blocked(state: &Arc<AppState>, runtime: &Runtime) {
    assert!(
        runtime.output.locked().is_some(),
        "the turn lost its output owner"
    );
    let other = Arc::new(AppState::new(state.repo()));
    let error = crate::serve::fixture_switch_branch(other, state.repo())
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(
        crate::repo::git_branches(&state.repo())
            .unwrap()
            .current
            .as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn a_background_question_keeps_its_output_and_branch_protection_until_completion() {
    let (_dir, state, runtime, _process) = fixture_runtime(false).await;
    let mut stream = fixture_stream(state.clone()).await;
    fixture_until(&mut stream, "parent-result-fixture").await;
    fixture_branch_blocked(&state, &runtime).await;
    assert_eq!(fixture_control(state.clone(), "answer").await["ok"], true);
    fixture_until(&mut stream, "event: resolved").await;
    fixture_branch_blocked(&state, &runtime).await;
    fixture_control(state.clone(), "steer").await;
    fixture_until(&mut stream, "event: done").await;
    tokio::time::timeout(FIXTURE_WAIT, async {
        while stream.recv().await.is_some() {}
    })
    .await
    .unwrap();
    assert!(runtime.output.locked().is_none());
    let _ = crate::serve::fixture_switch_branch(state.clone(), state.repo())
        .await
        .unwrap();
    assert_eq!(fixture_control(state, "close").await["ok"], true);
    assert!(!runtime.alive.load(Ordering::Acquire));
}

#[tokio::test]
async fn stopping_a_background_question_releases_its_output_and_branch_protection() {
    let (_dir, state, runtime, _process) = fixture_runtime(false).await;
    let mut stream = fixture_stream(state.clone()).await;
    fixture_until(&mut stream, "parent-result-fixture").await;
    fixture_branch_blocked(&state, &runtime).await;
    assert!(state.interrupt(FIXTURE_LANE));
    // A real Stop terminates this owned helper. Process exit must end forwarding and release Held.
    tokio::time::timeout(FIXTURE_WAIT, async {
        while stream.recv().await.is_some() {}
    })
    .await
    .unwrap();
    assert!(!runtime.alive.load(Ordering::Acquire));
    assert!(runtime.output.locked().is_none());
    let _ = crate::serve::fixture_switch_branch(state.clone(), state.repo())
        .await
        .unwrap();
}

#[tokio::test]
async fn metadata_failure_during_a_turn_keeps_checkout_protection_until_completion() {
    let (_dir, state, runtime, _process) = fixture_runtime(true).await;
    let mut stream = fixture_stream(state.clone()).await;
    fixture_until(&mut stream, "couldn’t update shared task").await;
    fixture_until(&mut stream, "parent-result-fixture").await;
    fixture_branch_blocked(&state, &runtime).await;
    fixture_control(state.clone(), "steer").await;
    fixture_until(&mut stream, "event: done").await;
    tokio::time::timeout(FIXTURE_WAIT, async {
        while stream.recv().await.is_some() {}
    })
    .await
    .unwrap();
    assert!(runtime.output.locked().is_none());
    let _ = crate::serve::fixture_switch_branch(state.clone(), state.repo())
        .await
        .unwrap();
    assert_eq!(fixture_control(state, "close").await["ok"], true);
}

#[test]
fn sdk_runtime_gives_mcp_questions_the_full_keel_lifetime() {
    let command = runtime_command(
        "node".into(),
        "fixture.mjs".into(),
        camino::Utf8Path::new("."),
    );
    let timeout = command
        .as_std()
        .get_envs()
        .find(|(name, _)| *name == "MCP_TOOL_TIMEOUT")
        .and_then(|(_, value)| value)
        .unwrap()
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(timeout > crate::approve::WAIT.as_millis() as u64);
}

#[tokio::test(start_paused = true)]
async fn a_written_answer_without_a_helper_receipt_is_never_accepted() {
    let (_tx, rx) = oneshot::channel();
    let delivery = tokio::spawn(answer_ack(async { Ok(()) }, rx));
    tokio::task::yield_now().await;
    assert!(!delivery.is_finished());
    tokio::time::advance(ANSWER_ACK_WAIT + std::time::Duration::from_secs(1)).await;
    assert!(
        delivery
            .await
            .unwrap()
            .unwrap_err()
            .contains("not confirmed")
    );
}

#[tokio::test]
async fn helper_acceptance_and_rejection_are_returned_without_inventing_success() {
    for ok in [true, false] {
        let (tx, rx) = oneshot::channel();
        tx.send(Ok(json!({"ok":ok}))).unwrap();
        assert_eq!(answer_ack(async { Ok(()) }, rx).await.unwrap()["ok"], ok);
    }
    let (tx, rx) = oneshot::channel();
    tx.send(Ok(json!({"ok":true}))).unwrap();
    assert_eq!(
        answer_ack(async { Err("broken pipe".into()) }, rx)
            .await
            .unwrap_err(),
        "broken pipe"
    );
}
