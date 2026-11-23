//! Public adapter behavior and authenticated session retirement use isolated loopback fixtures.

use super::*;
#[test]
fn only_the_exact_owned_tab_missing_error_means_closed() {
    const OWNED_TAB: u64 = 22;
    assert!(tab_gone("Tab 22 not found", OWNED_TAB));
    for why in [
        "Tab 23 not found",
        "Tab 220 not found",
        "switch timed out",
        "Local Oya disconnected",
        "Not switched: that tab belongs to the person or another agent.",
        "Error: Tab 22 not found",
        "Tab 22 not found during navigation",
        super::super::connection::RESTARTED,
    ] {
        assert!(!tab_gone(why, OWNED_TAB), "{why}");
    }
}
#[test]
fn typing_keeps_the_read_unless_it_submits() {
    assert!(read_survives_typing(false));
    assert!(!read_survives_typing(true));
}
#[test]
fn element_actions_require_a_fresh_page_analysis() {
    assert!(require_analysis(false).is_err());
    assert!(require_analysis(true).is_ok());
}
#[tokio::test]
async fn disconnected_local_browser_never_uses_an_embedded_or_personal_tab() {
    let mut page = Page::default();
    assert!(
        page.run(Op::Read)
            .await
            .unwrap_err()
            .contains("not connected")
    );
    assert!(page.show().await.unwrap_err().contains("no owned"));
}
/// Live: fills a form from one read in the installed Oya, including a field typed by its old
/// id after a select revealed a new field above it. Needs Oya running and `KEEL_LIVE_FORM`
/// naming a page shaped like the one this asserts on.
#[tokio::test]
#[ignore = "needs a running Oya Browser"]
async fn live_form_fills_from_one_read() {
    let url = std::env::var("KEEL_LIVE_FORM").expect("KEEL_LIVE_FORM");
    let installation = super::super::detection::installed()
        .unwrap()
        .expect("Oya installed");
    let project = camino::Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let connection = super::super::connection::discover(&project, &installation).unwrap();
    let mut page = Page::default();
    page.connect(Client::new(connection).unwrap())
        .await
        .unwrap();
    page.run(Op::Open { url }).await.unwrap();
    page.run(Op::Read).await.unwrap();
    let fields = [
        (1, "Ada"),
        (2, "Lovelace"),
        (3, "ada@example.com"),
        (4, "5550100"),
        (5, "Business"),
        (6, "London"),
    ];
    for (target, text) in fields {
        let typed = page
            .run(Op::Type {
                target,
                text: text.into(),
                submit: false,
            })
            .await;
        assert!(typed.is_ok(), "field {target}: {typed:?}");
    }
    let after = page.run(Op::Read).await.unwrap().text;
    println!("{after}");
    for want in [
        r#""First name" = "Ada""#,
        r#""Phone" = "5550100""#,
        r#""Account type" = "Business""#,
        r#""City" = "London""#,
    ] {
        assert!(after.contains(want), "missing {want}");
    }
    assert!(
        after.contains(r#"input:text "Company"]"#),
        "Company stays empty"
    );
    let tab = page.tab.unwrap();
    let _ = page
        .client
        .as_mut()
        .unwrap()
        .tool("close_tab", json!({"tab_id": tab}))
        .await;
}

#[cfg(any(target_os = "macos", windows))]
#[path = "local_recovery_tests.rs"]
mod recovery;

#[cfg(any(target_os = "macos", windows))]
mod retirement {
    use super::super::*;
    use axum::{Json, extract::State, routing::post};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;

    /// Scripted tool calls exercise the real client without accessing an installed browser.
    pub(super) type Step = (
        &'static str,
        serde_json::Value,
        Result<serde_json::Value, String>,
    );

    /// Session state shared by retirement checks and scripted page-recovery checks.
    pub(super) struct Fixture {
        deletes: AtomicUsize,
        lease: AtomicBool,
        closed: Notify,
        fail_init: bool,
        pub(super) steps: Mutex<std::collections::VecDeque<Step>>,
    }
    fn tool_reply(state: &Fixture, message: &serde_json::Value) -> serde_json::Value {
        let Some((name, args, reply)) = state.steps.lock().unwrap().pop_front() else {
            return json!({"isError":true,"content":[{"type":"text","text":"Unexpected fake Oya call"}]});
        };
        assert_eq!(message["params"]["name"], name);
        assert_eq!(message["params"]["arguments"], args);
        match reply {
            Ok(value) => json!({"content":[{"type":"text","text":value.to_string()}]}),
            Err(why) => json!({"isError":true,"content":[{"type":"text","text":why}]}),
        }
    }
    async fn rpc(
        State(state): State<Arc<Fixture>>,
        Json(message): Json<serde_json::Value>,
    ) -> ([(&'static str, &'static str); 1], Json<serde_json::Value>) {
        let result = if message["method"] == "tools/list" {
            let tools = if state.fail_init {
                vec![]
            } else {
                [
                    "open_tab",
                    "navigate",
                    "switch_tab",
                    "analyze_page",
                    "click",
                    "type",
                    "press_key",
                    "scroll",
                    "go_back",
                    "viewport",
                    "screenshot",
                ]
                .into_iter()
                .map(|name| json!({"name":name}))
                .collect()
            };
            json!({"tools":tools})
        } else if message["method"] == "tools/call" {
            tool_reply(&state, &message)
        } else {
            json!({})
        };
        (
            [("Mcp-Session-Id", "fixture-session")],
            Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})),
        )
    }
    async fn deleted(State(state): State<Arc<Fixture>>) {
        state.deletes.fetch_add(1, Ordering::SeqCst);
        state.lease.store(false, Ordering::SeqCst);
        state.closed.notify_one();
    }
    async fn fixture(fail_init: bool) -> (Client, Arc<Fixture>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let state = Arc::new(Fixture {
            deletes: AtomicUsize::new(0),
            lease: AtomicBool::new(true),
            closed: Notify::new(),
            fail_init,
            steps: Default::default(),
        });
        let app = axum::Router::new()
            .route("/mcp", post(rpc).delete(deleted))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (Client::fixture(url), state, server)
    }
    /// Queued replies make unexpected replays and cross-tab calls fail the adapter tests.
    pub(super) async fn scripted(
        steps: Vec<Step>,
    ) -> (Client, Arc<Fixture>, tokio::task::JoinHandle<()>) {
        let (client, state, server) = fixture(false).await;
        state.steps.lock().unwrap().extend(steps);
        (client, state, server)
    }
    async fn assert_closed(state: &Fixture) {
        tokio::time::timeout(std::time::Duration::from_secs(5), state.closed.notified())
            .await
            .unwrap();
        assert_eq!(state.deletes.load(Ordering::SeqCst), 1);
        assert!(!state.lease.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn dropping_a_connected_page_on_a_worker_closes_its_session_once() {
        let (client, state, server) = fixture(false).await;
        let mut page = Page::default();
        page.connect(client).await.unwrap();
        std::thread::spawn(move || drop(page)).join().unwrap();
        assert_closed(&state).await;
        server.abort();
    }
    #[tokio::test]
    async fn reset_then_drop_sends_only_one_delete() {
        let (client, state, server) = fixture(false).await;
        let mut page = Page::default();
        page.connect(client).await.unwrap();
        page.reset();
        drop(page);
        assert_closed(&state).await;
        server.abort();
    }
    #[tokio::test]
    async fn initialization_failure_releases_the_partially_allocated_session() {
        let (client, state, server) = fixture(true).await;
        let mut page = Page::default();
        assert!(page.connect(client).await.is_err());
        drop(page);
        assert_closed(&state).await;
        server.abort();
    }
}
