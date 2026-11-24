//! Primary-contract tests exercise pagination, identity and failure paths without network or credentials.
use super::issues::issue_url;
use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

const SHA: &str = "1111111111111111111111111111111111111111";
#[derive(Default)]
struct Fake {
    rest: BTreeMap<String, VecDeque<Value>>,
    graphs: VecDeque<Result<Value, String>>,
    variables: Vec<Value>,
}
impl Fake {
    fn response(&mut self, path: &str, value: Value) {
        self.rest.entry(path.into()).or_default().push_back(value);
    }
}
impl Read for Fake {
    fn rest(&mut self, path: &str) -> Result<Value, String> {
        self.rest
            .get_mut(path)
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| format!("Controlled read failure: {path}"))
    }
    fn graphql(&mut self, _query: &str, variables: Value) -> Result<Value, String> {
        self.variables.push(variables);
        self.graphs
            .pop_front()
            .unwrap_or(Err("Controlled GraphQL failure".into()))
    }
}
fn project() -> Project {
    Project {
        source: "contributor/fork".into(),
        parent: Some("owner/repo".into()),
        push_identity: "push".into(),
    }
}
fn issue() -> Value {
    json!({"node_id":"issue-1","number":1,"html_url":"https://github.com/owner/repo/issues/1","title":"Fix checkout","body":"Untrusted source body","state":"open","updated_at":"2026-11-23T00:00:00Z"})
}
fn info(next: Option<&str>) -> Value {
    json!({"hasNextPage":next.is_some(),"endCursor":next})
}
fn thread(id: &str, comments: Vec<Value>, next: Option<&str>) -> Value {
    json!({"id":id,"isResolved":false,"isOutdated":true,"comments":{"nodes":comments,"pageInfo":info(next)}})
}
fn comment(id: &str) -> Value {
    json!({"id":id,"url":"https://github.com/owner/repo/pull/1#discussion_r1","body":"Retain this correction","path":"src/a.ts","line":null,"originalLine":4,"originalCommit":{"oid":SHA}})
}
fn threads(nodes: Vec<Value>, next: Option<&str>) -> Value {
    json!({"data":{"repository":{"pullRequest":{"headRefOid":SHA,"reviewThreads":{"nodes":nodes,"pageInfo":info(next)}}}}})
}
fn remote() -> Fake {
    let mut reader = Fake::default();
    for _ in 0..2 {
        reader.response("repos/owner/repo/pulls/1", json!({"head":{"sha":SHA}}));
    }
    reader.response(&format!("repos/owner/repo/commits/{SHA}/check-runs?per_page=100&page=1"), json!({"total_count":1,"check_runs":[{"id":1,"name":"unit","head_sha":SHA,"status":"completed","conclusion":"failure","details_url":"https://ci.example/unit"}]}));
    reader.response(&format!("repos/owner/repo/commits/{SHA}/status?per_page=100&page=1"), json!({"sha":SHA,"total_count":1,"statuses":[{"id":2,"context":"legacy","state":"error","target_url":"https://ci.example/legacy"}]}));
    reader
}

#[test]
fn issue_inputs_reject_foreign_hosts_credentials_ports_and_ambiguous_paths() {
    for value in [
        "http://github.com/owner/repo/issues/1",
        "https://evil.test/owner/repo/issues/1",
        "https://user@github.com/owner/repo/issues/1",
        "https://github.com:444/owner/repo/issues/1",
        "https://github.com/owner/wrong/../repo/issues/1",
        "https://github.com/owner/%2e%2e/repo/issues/1",
        "https://github.com/owner/repo/pull/1",
        "https://github.com/owner/repo/issues/0",
    ] {
        assert!(issue_url(value).is_err(), "{value}");
    }
    assert_eq!(
        issue_url("https://github.com/owner/repo/issues/1?utm=x#note").unwrap(),
        ("owner/repo".into(), 1)
    );
}
#[test]
fn a_verified_fork_parent_can_supply_an_issue_snapshot() {
    let mut reader = Fake::default();
    reader.response("repos/owner/repo/issues/1", issue());
    let preview = preview(
        &mut reader,
        &project(),
        "https://github.com/owner/repo/issues/1",
        "preview-1".into(),
        1,
    )
    .unwrap();
    assert_eq!(preview.change_repository, "contributor/fork");
    assert_eq!(preview.issue.body, "Untrusted source body");
    assert_eq!(preview.issue.fetched_at, 1);
}
#[test]
fn an_unrelated_repository_is_rejected_before_an_external_read() {
    let mut reader = Fake::default();
    assert!(
        preview(
            &mut reader,
            &project(),
            "https://github.com/other/repo/issues/1",
            "preview-1".into(),
            1
        )
        .is_err()
    );
}
#[test]
fn pull_requests_from_the_issues_api_never_start_issue_tasks() {
    let mut reader = Fake::default();
    let mut value = issue();
    value["pull_request"] = json!({"url":"pull"});
    reader.response("repos/owner/repo/issues/1", value);
    assert!(
        preview(
            &mut reader,
            &project(),
            "https://github.com/owner/repo/issues/1",
            "preview-1".into(),
            1
        )
        .unwrap_err()
        .contains("pull request")
    );
}
#[test]
fn large_unicode_issue_bodies_are_explicitly_truncated_on_a_character_boundary() {
    let mut reader = Fake::default();
    let mut value = issue();
    value["body"] = "日本語".repeat(constants::BODY_BYTES).into();
    reader.response("repos/owner/repo/issues/1", value);
    let value = preview(
        &mut reader,
        &project(),
        "https://github.com/owner/repo/issues/1",
        "preview-1".into(),
        1,
    )
    .unwrap();
    assert!(value.issue.truncated);
    assert!(value.issue.body.len() <= constants::BODY_BYTES);
}
#[test]
fn thread_and_comment_cursors_are_independent_and_null_locations_stay_null() {
    let mut reader = remote();
    reader.graphs.push_back(Ok(threads(
        vec![thread("t1", vec![comment("c1")], Some("comments-1"))],
        Some("threads-1"),
    )));
    reader.graphs.push_back(Ok(
        json!({"data":{"node":{"comments":{"nodes":[comment("c2")],"pageInfo":info(None)}}}}),
    ));
    reader.graphs.push_back(Ok(threads(
        vec![thread("t2", vec![comment("c3")], None)],
        None,
    )));
    let feedback = feedback(&mut reader, "owner/repo", 1, 1).unwrap();
    assert!(feedback.complete, "{:?}", feedback.errors);
    assert_eq!(feedback.checks.len(), 2);
    assert_eq!(feedback.threads.len(), 2);
    assert_eq!(feedback.threads[0].comments[0].line, None);
    assert_eq!(feedback.threads[0].comments[0].original_line, Some(4));
    assert_eq!(reader.variables[1]["cursor"], "comments-1");
    assert_eq!(reader.variables[2]["cursor"], "threads-1");
}
#[test]
fn a_changed_pr_head_makes_the_entire_refresh_unconfirmed() {
    let mut reader = remote();
    reader.rest.get_mut("repos/owner/repo/pulls/1").unwrap()[1] =
        json!({"head":{"sha":"2222222222222222222222222222222222222222"}});
    reader.graphs.push_back(Ok(threads(vec![], None)));
    let feedback = feedback(&mut reader, "owner/repo", 1, 1).unwrap();
    assert!(!feedback.complete);
    assert_eq!(feedback.head, SHA);
    assert_eq!(feedback.checks[0].conclusion.as_deref(), Some("failure"));
}
#[test]
fn a_failed_legacy_status_read_preserves_successfully_read_check_runs() {
    let mut reader = remote();
    reader.rest.remove(&format!(
        "repos/owner/repo/commits/{SHA}/status?per_page=100&page=1"
    ));
    reader
        .graphs
        .push_back(Err("GraphQL returned errors".into()));
    let feedback = feedback(&mut reader, "owner/repo", 1, 1).unwrap();
    assert!(!feedback.complete);
    assert_eq!(feedback.checks.len(), 1);
    assert_eq!(feedback.errors.len(), 2);
}
#[test]
fn repeated_remote_cursors_end_as_partial_data_without_looping() {
    let mut reader = remote();
    reader.graphs.push_back(Ok(threads(
        vec![thread("t1", vec![], None)],
        Some("repeat"),
    )));
    reader.graphs.push_back(Ok(threads(
        vec![thread("t2", vec![], None)],
        Some("repeat"),
    )));
    let feedback = feedback(&mut reader, "owner/repo", 1, 1).unwrap();
    assert!(!feedback.complete);
    assert!(feedback.errors[0].contains("cursor"));
    assert_eq!(reader.variables.len(), 2);
}
#[test]
fn feedback_links_never_turn_untrusted_api_values_into_script_urls() {
    assert!(transport::link(&json!("javascript:alert(1)")).is_none());
    assert!(transport::link(&json!("https://user:secret@ci.example/a")).is_none());
    assert_eq!(
        transport::link(&json!("https://ci.example/a")).as_deref(),
        Some("https://ci.example/a")
    );
}
