//! Scripted Oya replies pin owned-tab recovery without touching personal tabs or live sessions.
use super::{retirement::*, *};

const OLD_TAB: u64 = 22;
const NEW_TAB: u64 = 23;
const ELEMENT: u32 = 1;
const VIEWPORT_WIDTH: u32 = 800;
const VIEWPORT_HEIGHT: u32 = 600;
const OLD_URL: &str = "https://example.test/old";
const NEW_URL: &str = "https://example.test/new";
const CLOSED: &str = "This lane's tab was closed. Open a page with browser_open to continue.";

fn switch_error(why: &str) -> Step {
    ("switch_tab", json!({"tab_id":OLD_TAB}), Err(why.into()))
}
fn open_reply(tab: u64, url: &str) -> Step {
    ("open_tab", json!({"url":url}), Ok(json!({"tab_id":tab})))
}
async fn owned_page(
    steps: Vec<Step>,
) -> (Page, std::sync::Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let (client, state, server) = scripted(steps).await;
    let mut page = Page::default();
    page.connect(client).await.unwrap();
    page.tab = Some(OLD_TAB);
    page.url = Some(OLD_URL.into());
    page.analyzed = true;
    (page, state, server)
}
fn assert_cleared(page: &Page) {
    assert_eq!(page.tab, None);
    assert_eq!(page.url, None);
    assert!(!page.analyzed);
}
fn assert_consumed(state: &Fixture) {
    assert!(state.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn open_replaces_a_closed_owned_tab_at_the_requested_url() {
    let steps = vec![
        switch_error("Tab 22 not found"),
        open_reply(NEW_TAB, NEW_URL),
    ];
    let (mut page, state, server) = owned_page(steps).await;
    page.run(Op::Open {
        url: NEW_URL.into(),
    })
    .await
    .unwrap();
    assert_eq!(page.tab, Some(NEW_TAB));
    assert_eq!(page.url.as_deref(), Some(NEW_URL));
    assert!(!page.analyzed);
    assert!(page.connected());
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn read_reports_a_closed_tab_and_the_next_open_succeeds() {
    let steps = vec![
        switch_error("Tab 22 not found"),
        open_reply(NEW_TAB, NEW_URL),
    ];
    let (mut page, state, server) = owned_page(steps).await;
    assert_eq!(page.run(Op::Read).await.unwrap_err(), CLOSED);
    assert_cleared(&page);
    assert!(page.connected());
    page.run(Op::Open {
        url: NEW_URL.into(),
    })
    .await
    .unwrap();
    assert_eq!(page.tab, Some(NEW_TAB));
    assert_eq!(page.url.as_deref(), Some(NEW_URL));
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn every_page_operation_clears_a_closed_tab_without_replaying_its_action() {
    let ops = [
        Op::Read,
        Op::Click { target: ELEMENT },
        Op::Type {
            target: ELEMENT,
            text: "fixture".into(),
            submit: true,
        },
        Op::Key {
            key: "Enter".into(),
        },
        Op::Scroll {
            direction: "down".into(),
        },
        Op::Back,
        Op::Viewport {
            settings: super::super::super::Viewport::parse(
                &json!({"width":VIEWPORT_WIDTH,"height":VIEWPORT_HEIGHT}),
            )
            .unwrap(),
        },
        Op::Screenshot,
    ];
    for op in ops {
        let (mut page, state, server) = owned_page(vec![switch_error("Tab 22 not found")]).await;
        assert_eq!(page.run(op.clone()).await.unwrap_err(), CLOSED, "{op:?}");
        assert_cleared(&page);
        assert!(page.connected());
        assert_consumed(&state);
        server.abort();
    }
}

#[tokio::test]
async fn show_clears_a_closed_tab_and_preserves_successful_switch_text() {
    let (mut page, state, server) = owned_page(vec![switch_error("Tab 22 not found")]).await;
    assert_eq!(page.show().await.unwrap_err(), CLOSED);
    assert_cleared(&page);
    assert!(page.connected());
    assert_consumed(&state);
    server.abort();

    let result = json!({"tab_id":OLD_TAB});
    let steps = vec![("switch_tab", json!({"tab_id":OLD_TAB}), Ok(result.clone()))];
    let (mut page, state, server) = owned_page(steps).await;
    assert_eq!(page.show().await.unwrap(), result.to_string());
    assert_eq!(page.tab, Some(OLD_TAB));
    assert!(page.analyzed);
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn unrelated_switch_errors_preserve_the_owned_tab_and_url() {
    for op in [
        Op::Open {
            url: NEW_URL.into(),
        },
        Op::Read,
        Op::Screenshot,
    ] {
        let (mut page, state, server) = owned_page(vec![switch_error("switch timed out")]).await;
        assert_eq!(page.run(op).await.unwrap_err(), "switch timed out");
        assert_eq!(page.tab, Some(OLD_TAB));
        assert_eq!(page.url.as_deref(), Some(OLD_URL));
        assert!(!page.analyzed);
        assert!(page.connected());
        assert_consumed(&state);
        server.abort();
    }
    let (mut page, state, server) = owned_page(vec![switch_error("switch timed out")]).await;
    assert_eq!(page.show().await.unwrap_err(), "switch timed out");
    assert_eq!(page.tab, Some(OLD_TAB));
    assert_eq!(page.url.as_deref(), Some(OLD_URL));
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn restarted_oya_still_retires_the_connection_and_owned_state() {
    let restarted = super::super::super::connection::RESTARTED;
    let (mut page, state, server) = owned_page(vec![switch_error(restarted)]).await;
    assert_eq!(page.run(Op::Read).await.unwrap_err(), restarted);
    assert_cleared(&page);
    assert!(!page.connected());
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn replacement_failure_leaves_the_closed_tab_forgotten() {
    let steps = vec![
        switch_error("Tab 22 not found"),
        (
            "open_tab",
            json!({"url":NEW_URL}),
            Err("open timed out".into()),
        ),
    ];
    let (mut page, state, server) = owned_page(steps).await;
    assert_eq!(
        page.run(Op::Open {
            url: NEW_URL.into()
        })
        .await
        .unwrap_err(),
        "open timed out"
    );
    assert_cleared(&page);
    assert!(page.connected());
    assert_consumed(&state);
    server.abort();
}

#[tokio::test]
async fn first_open_creates_a_tab_and_repeat_open_navigates_the_same_owned_tab() {
    let steps = vec![
        open_reply(OLD_TAB, OLD_URL),
        (
            "switch_tab",
            json!({"tab_id":OLD_TAB}),
            Ok(json!({"tab_id":OLD_TAB})),
        ),
        (
            "navigate",
            json!({"url":NEW_URL}),
            Ok(json!({"url":NEW_URL})),
        ),
    ];
    let (client, state, server) = scripted(steps).await;
    let mut page = Page::default();
    page.connect(client).await.unwrap();
    page.run(Op::Open {
        url: OLD_URL.into(),
    })
    .await
    .unwrap();
    assert_eq!(page.tab, Some(OLD_TAB));
    page.run(Op::Open {
        url: NEW_URL.into(),
    })
    .await
    .unwrap();
    assert_eq!(page.tab, Some(OLD_TAB));
    assert_eq!(page.url.as_deref(), Some(NEW_URL));
    assert_consumed(&state);
    server.abort();
}
