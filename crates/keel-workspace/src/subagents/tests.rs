//! Scratch provider records pin resumed lifecycle, output ownership, and missing-file recovery.
use super::*;
use crate::{Follows, Transcript};
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Value, json};
use std::io::Write;

fn append(path: &Utf8Path, value: Value) {
    writeln!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{value}"
    )
    .unwrap();
}
fn event(kind: &str, turn: &str, time: &str) -> Value {
    json!({"type":"event_msg","timestamp":time,"payload":{"type":kind,"turn_id":turn}})
}
struct Fixture {
    _temp: tempfile::TempDir,
    parent: Utf8PathBuf,
    child: Utf8PathBuf,
    observer: Observer,
}
fn codex_fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let sessions = root.join("sessions/2020/01/01");
    std::fs::create_dir_all(&sessions).unwrap();
    let parent = sessions.join("rollout-parent.jsonl");
    let child = sessions.join("rollout-child.jsonl");
    append(
        &parent,
        json!({"type":"session_meta","payload":{"id":"parent","cwd":root}}),
    );
    append(
        &child,
        json!({"type":"session_meta","payload":{"id":"child","cwd":root,"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}),
    );
    append(
        &parent,
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"SubAgentActivity","kind":"interacted","agent_thread_id":"child","agent_path":"/root/pm"}}}),
    );
    let observer = Observer::with_roots(
        Transcript::at(Follows::Codex, parent.clone()),
        root.to_owned(),
        "parent".into(),
        Roots {
            codex_sessions: root.join("sessions"),
        },
    );
    Fixture {
        _temp: temp,
        parent,
        child,
        observer,
    }
}
#[test]
fn a_reused_codex_child_animates_for_a_new_task_without_a_second_parent_started_event() {
    let mut f = codex_fixture();
    append(
        &f.child,
        event("task_started", "old", "2020-01-01T00:00:00Z"),
    );
    append(
        &f.child,
        event("task_complete", "old", "2020-01-01T00:00:01Z"),
    );
    assert_eq!(f.observer.snapshot().invocations[0].status, Status::Done);
    append(
        &f.child,
        event("task_started", "resumed", "2020-01-01T00:00:02Z"),
    );
    let s = f.observer.snapshot();
    assert_eq!(s.invocations.len(), 2);
    assert_eq!(s.invocations[1].member.as_deref(), Some("pm"));
    assert_eq!(s.invocations[1].status, Status::Working);
    append(
        &f.child,
        event("task_complete", "resumed", "2020-01-01T00:00:03Z"),
    );
    assert!(
        f.observer
            .snapshot()
            .invocations
            .iter()
            .all(|i| i.status == Status::Done)
    );
}
#[test]
fn an_ordinary_message_to_an_idle_child_does_not_invent_a_run() {
    let mut f = codex_fixture();
    append(&f.child, event("task_started", "old", "0"));
    append(&f.child, event("task_complete", "old", "1"));
    f.observer.snapshot();
    append(
        &f.parent,
        json!({"type":"event_msg","payload":{"item":{"type":"SubAgentActivity","kind":"interacted","agent_thread_id":"child","agent_path":"/root/pm"}}}),
    );
    let s = f.observer.snapshot();
    assert_eq!(s.invocations.len(), 1);
    assert_eq!(s.invocations[0].status, Status::Done);
}
#[test]
fn repeated_messages_during_one_task_do_not_duplicate_it_or_finish_it_with_the_parent() {
    let mut f = codex_fixture();
    append(&f.child, event("task_started", "run", "0"));
    f.observer.snapshot();
    append(&f.parent, event("task_complete", "parent-turn", "1"));
    assert_eq!(f.observer.snapshot().invocations.len(), 1);
    assert_eq!(f.observer.snapshot().invocations[0].status, Status::Working);
    assert_eq!(
        f.observer.interrupted().invocations[0].status,
        Status::Interrupted
    );
}
#[test]
fn missing_child_data_becomes_unknown_and_read_recovery_restores_its_confirmed_state() {
    let mut f = codex_fixture();
    append(&f.child, event("task_started", "run", "0"));
    f.observer.snapshot();
    let backup = f.child.with_extension("backup");
    std::fs::rename(&f.child, &backup).unwrap();
    assert_eq!(f.observer.snapshot().invocations[0].status, Status::Unknown);
    std::fs::rename(backup, &f.child).unwrap();
    assert_eq!(f.observer.snapshot().invocations[0].status, Status::Working);
}
#[test]
fn unrelated_children_and_traversal_never_authorize_output() {
    let mut f = codex_fixture();
    let wrong = json!({"type":"session_meta","payload":{"id":"child","cwd":Utf8Path::from_path(f._temp.path()).unwrap(),"parent_thread_id":"someone-else"}});
    std::fs::write(&f.child, format!("{wrong}\n")).unwrap();
    append(&f.child, event("task_started", "run", "0"));
    assert!(f.observer.snapshot().invocations.is_empty());
    assert!(f.observer.detail("codex:child:run").is_none());
    assert!(f.observer.detail("../../other").is_none());
}
#[test]
fn selected_output_contains_only_its_execution_and_never_raw_or_thinking_records() {
    let mut f = codex_fixture();
    append(&f.child, event("task_started", "one", "0"));
    append(
        &f.child,
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"one","item":{"type":"AgentMessage","id":"answer","content":[{"type":"Text","text":"Allowed result"}]}}}),
    );
    append(&f.child, event("task_complete", "one", "1"));
    append(&f.child, event("task_started", "two", "2"));
    append(
        &f.child,
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"two","item":{"type":"AgentMessage","id":"other","content":[{"type":"Text","text":"Other execution"}]}}}),
    );
    let detail = f.observer.detail("codex:child:one").unwrap();
    let text = detail
        .items
        .iter()
        .map(|i| i.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Allowed result"), "{text}");
    assert!(!text.contains("Other execution"));
    let ids = vec![
        "codex:child:one".into(),
        "codex:child:two".into(),
        "foreign".into(),
    ];
    let details = f.observer.details(&ids);
    assert_eq!(details.len(), 2);
    assert_eq!(details["codex:child:one"], detail);
    assert!(
        details["codex:child:two"]
            .items
            .iter()
            .any(|line| line.text.contains("Other execution"))
    );
}
#[test]
fn claude_foreground_agent_calls_are_visible_and_their_results_remain_inspectable() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let path = root.join("parent.jsonl");
    append(
        &path,
        json!({"type":"assistant","timestamp":"0","message":{"content":[{"type":"tool_use","id":"call","name":"Agent","input":{"subagent_type":"qa","description":"Verify"}}]}}),
    );
    let mut observer = Observer::new(
        Transcript::at(Follows::Claude, path.clone()),
        root.to_owned(),
        "parent".into(),
    );
    assert_eq!(observer.snapshot().invocations[0].status, Status::Working);
    assert!(
        observer
            .details(&["claude:call".into(), "foreign".into()])
            .contains_key("claude:call")
    );
    append(
        &path,
        json!({"type":"user","timestamp":"1","message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"Final result"}]}}),
    );
    assert_eq!(observer.snapshot().invocations[0].status, Status::Done);
    assert_eq!(
        observer.detail("claude:call").unwrap().items[0].text,
        "Final result"
    );
}

fn var(mut n: u64) -> Vec<u8> {
    let mut out = vec![];
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
    out
}
fn bytes(n: u64, data: &[u8]) -> Vec<u8> {
    [var((n << 3) | 2), var(data.len() as u64), data.to_vec()].concat()
}
fn database(path: &Utf8Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch("CREATE TABLE steps (idx integer PRIMARY KEY,step_type integer,status integer,step_payload blob)").unwrap();
    c
}
fn row(c: &rusqlite::Connection, idx: i64, kind: i64, status: i64, payload: &[u8]) {
    c.execute(
        "INSERT INTO steps VALUES (?1,?2,?3,?4)",
        rusqlite::params![idx, kind, status, payload],
    )
    .unwrap();
}
#[test]
fn antigravity_reads_children_beyond_pending_parent_rows_and_distinguishes_missing_from_done() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let parent = root.join("parent.db");
    let c = database(&parent);
    let args = json!({"TypeName":"pm"}).to_string();
    let call = [bytes(2, b"invoke_subagent"), bytes(3, args.as_bytes())].concat();
    let payload = [
        bytes(5, &bytes(4, &call)),
        bytes(
            140,
            &bytes(2, &bytes(1, b"{\"conversationId\": \"child\"}")),
        ),
    ]
    .concat();
    row(&c, 0, 132, 2, &[]);
    row(&c, 1, 132, 3, &payload);
    let mut observer = Observer::new(
        Transcript::at(Follows::Antigravity, parent),
        root.to_owned(),
        "parent".into(),
    );
    let s = observer.snapshot();
    assert_eq!(s.invocations.len(), 1);
    assert_eq!(s.invocations[0].status, Status::Unknown);
    let child = database(&root.join("child.db"));
    row(&child, 0, 14, 3, &[]);
    row(&child, 1, 15, 8, &bytes(20, &bytes(1, b"Progress")));
    let s = observer.snapshot();
    assert_eq!(s.invocations.len(), 1);
    assert_eq!(s.invocations[0].status, Status::Working);
    assert_eq!(s.invocations[0].member.as_deref(), Some("pm"));
    assert!(
        observer
            .details(&[s.invocations[0].id.clone()])
            .contains_key(&s.invocations[0].id)
    );
    child
        .execute("UPDATE steps SET status=3 WHERE idx=1", [])
        .unwrap();
    assert_eq!(observer.snapshot().invocations[0].status, Status::Done);
    row(&child, 2, 14, 3, &[]);
    row(&child, 3, 15, 8, &[]);
    assert_eq!(observer.snapshot().invocations.len(), 2);
    assert_eq!(observer.snapshot().invocations[1].status, Status::Working);
}
#[test]
fn claude_async_launch_stays_active_after_parent_completion_until_its_notification() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let parent = root.join("parent.jsonl");
    append(
        &parent,
        json!({"type":"assistant","timestamp":"0","message":{"content":[{"type":"tool_use","id":"call","name":"Task","input":{"subagent_type":"pm"}}]}}),
    );
    append(
        &parent,
        json!({"type":"user","timestamp":"1","toolUseResult":{"status":"async_launched"},"message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"Launched"}]}}),
    );
    append(
        &parent,
        json!({"type":"system","subtype":"turn_duration","timestamp":"2"}),
    );
    let mut observer = Observer::new(
        Transcript::at(Follows::Claude, parent.clone()),
        root.to_owned(),
        "parent".into(),
    );
    assert_eq!(observer.snapshot().invocations[0].status, Status::Working);
    append(
        &parent,
        json!({"type":"user","timestamp":"3","message":{"content":"<task-notification><tool-use-id>call</tool-use-id><status>completed</status></task-notification>"}}),
    );
    assert_eq!(observer.snapshot().invocations[0].status, Status::Done);
}
#[cfg(unix)]
#[test]
fn claude_subagent_directory_symlinks_cannot_expose_unrelated_transcripts() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let parent = root.join("parent.jsonl");
    let outside = tempfile::tempdir().unwrap();
    let foreign = Utf8Path::from_path(outside.path()).unwrap();
    append(
        &foreign.join("agent-child.jsonl"),
        json!({"agentId":"child","sessionId":"parent","type":"assistant","message":{"content":[{"type":"text","text":"Never expose this"}]}}),
    );
    std::fs::create_dir(root.join("parent")).unwrap();
    std::os::unix::fs::symlink(foreign, root.join("parent/subagents")).unwrap();
    append(
        &parent,
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"call","name":"Agent","input":{"subagent_type":"pm"}}]}}),
    );
    append(
        &parent,
        json!({"type":"user","toolUseResult":{"status":"async_launched","agentId":"child"},"message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"Launched"}]}}),
    );
    let mut observer = Observer::new(
        Transcript::at(Follows::Claude, parent),
        root.to_owned(),
        "parent".into(),
    );
    let detail = observer.detail("claude:call").unwrap();
    assert!(
        detail
            .items
            .iter()
            .all(|i| !i.text.contains("Never expose"))
    );
}

#[test]
fn a_child_header_replacement_revokes_previously_authorized_output() {
    let mut f = codex_fixture();
    append(&f.child, event("task_started", "run", "0"));
    f.observer.snapshot();
    let root = Utf8Path::from_path(f._temp.path()).unwrap();
    let wrong = json!({"type":"session_meta","payload":{"id":"child","cwd":root,"parent_thread_id":"someone-else"}});
    std::fs::write(&f.child, format!("{wrong}\n")).unwrap();
    append(
        &f.child,
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"run","item":{"type":"AgentMessage","id":"other","content":[{"type":"Text","text":"Unrelated secret"}]}}}),
    );
    let detail = f.observer.detail("codex:child:run").unwrap();
    assert_eq!(detail.availability, Availability::Disconnected);
    assert!(detail.items.is_empty());
}
#[test]
fn malformed_unfinished_task_history_is_bounded_and_disclosed() {
    let mut f = codex_fixture();
    for n in 0..300 {
        append(&f.child, event("task_started", &format!("run-{n}"), "0"));
    }
    let snapshot = f.observer.snapshot();
    assert!(snapshot.truncated);
    assert!(snapshot.invocations.len() <= super::json::CANDIDATE_LIMIT);
}

#[test]
fn a_completion_discovered_before_its_backfilled_start_never_restarts_animation() {
    let mut f = codex_fixture();
    let filler = json!({"type":"padding","data":"x".repeat(100_000)});
    for _ in 0..30 {
        append(&f.child, filler.clone());
    }
    append(&f.child, event("task_started", "run", "1"));
    for _ in 0..30 {
        append(&f.child, filler.clone());
    }
    append(&f.child, event("task_complete", "run", "2"));
    for _ in 0..5 {
        assert!(
            f.observer
                .snapshot()
                .invocations
                .iter()
                .all(|i| i.status != Status::Working)
        );
    }
    assert!(
        f.observer
            .snapshot()
            .invocations
            .iter()
            .any(|i| i.id == "codex:child:run" && i.status == Status::Done)
    );
}
#[test]
fn cold_completed_history_cannot_fill_admission_slots_ahead_of_the_newest_active_turn() {
    let mut f = codex_fixture();
    for n in 0..300 {
        append(
            &f.child,
            event("task_started", &format!("old-{n}"), &format!("{n:04}")),
        );
        append(
            &f.child,
            event("task_complete", &format!("old-{n}"), &format!("{n:04}")),
        );
    }
    append(&f.child, event("task_started", "current", "9999"));
    for _ in 0..2 {
        let s = f.observer.snapshot();
        assert!(
            s.invocations
                .iter()
                .any(|i| i.id == "codex:child:current" && i.status == Status::Working)
        );
        assert!(s.invocations.len() <= super::json::HISTORY_LIMIT + 1);
    }
}
#[test]
fn reused_claude_files_cannot_mix_output_between_agent_calls_without_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let parent = root.join("parent.jsonl");
    let dir = root.join("parent/subagents");
    std::fs::create_dir_all(&dir).unwrap();
    let child = dir.join("agent-child.jsonl");
    append(
        &child,
        json!({"type":"assistant","agentId":"child","sessionId":"parent","message":{"content":[{"type":"text","text":"First output"}]}}),
    );
    append(
        &parent,
        json!({"type":"assistant","timestamp":"0","message":{"content":[{"type":"tool_use","id":"old","name":"Agent","input":{"subagent_type":"pm"}}]}}),
    );
    append(
        &parent,
        json!({"type":"user","timestamp":"1","toolUseResult":{"agentId":"child"},"message":{"content":[{"type":"tool_result","tool_use_id":"old","content":"First final"}]}}),
    );
    let mut observer = Observer::new(
        Transcript::at(Follows::Claude, parent.clone()),
        root.to_owned(),
        "parent".into(),
    );
    observer.snapshot();
    append(
        &parent,
        json!({"type":"assistant","timestamp":"2","message":{"content":[{"type":"tool_use","id":"new","name":"Agent","input":{"subagent_type":"pm","resume":"child"}}]}}),
    );
    append(
        &parent,
        json!({"type":"user","timestamp":"3","toolUseResult":{"agentId":"child","status":"async_launched"},"message":{"content":[{"type":"tool_result","tool_use_id":"new","content":"Resumed"}]}}),
    );
    append(
        &child,
        json!({"type":"assistant","agentId":"child","sessionId":"parent","message":{"content":[{"type":"text","text":"Second execution output"}]}}),
    );
    let old = observer.detail("claude:old").unwrap();
    assert_eq!(old.items[0].text, "First final");
    assert!(old.items.iter().all(|i| !i.text.contains("Second")));
    let current = observer.detail("claude:new").unwrap();
    assert_eq!(current.availability, Availability::Unavailable);
    assert!(current.items.is_empty());
}

#[test]
fn display_history_eviction_does_not_erase_a_recent_terminal_only_run_during_backfill() {
    let mut f = codex_fixture();
    for n in 0..100 {
        let at = format!("0{n:03}");
        append(
            &f.child,
            event("task_started", &format!("history-{n}"), &at),
        );
        append(
            &f.child,
            event("task_complete", &format!("history-{n}"), &at),
        );
    }
    let padding = json!({"type":"padding","data":"x".repeat(100_000)});
    for _ in 0..30 {
        append(&f.child, padding.clone());
    }
    append(&f.child, event("task_started", "run", "1"));
    for _ in 0..30 {
        append(&f.child, padding.clone());
    }
    append(&f.child, event("task_complete", "run", "2"));
    for _ in 0..5 {
        assert!(
            f.observer
                .snapshot()
                .invocations
                .iter()
                .all(|i| i.status != Status::Working)
        );
    }
    assert!(
        f.observer
            .snapshot()
            .invocations
            .iter()
            .any(|i| i.id == "codex:child:run" && i.status == Status::Done)
    );
}
#[test]
fn claude_async_notifications_reconcile_before_call_discovery_in_both_native_forms() {
    for structured in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let path = root.join("parent.jsonl");
        let padding = json!({"type":"padding","data":"x".repeat(100_000)});
        for _ in 0..30 {
            append(&path, padding.clone());
        }
        append(
            &path,
            json!({"type":"assistant","timestamp":"1","message":{"content":[{"type":"tool_use","id":"call","name":"Agent","input":{"subagent_type":"pm"}}]}}),
        );
        append(
            &path,
            json!({"type":"user","timestamp":"1.1","toolUseResult":{"status":"async_launched"},"message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"Launched"}]}}),
        );
        for _ in 0..30 {
            append(&path, padding.clone());
        }
        append(
            &path,
            if structured {
                json!({"type":"system","subtype":"task_notification","tool_use_id":"call","status":"completed","timestamp":"2"})
            } else {
                json!({"type":"user","timestamp":"2","message":{"content":"<task-notification><tool-use-id>call</tool-use-id><status>completed</status></task-notification>"}})
            },
        );
        let mut observer = Observer::new(
            Transcript::at(Follows::Claude, path),
            root.to_owned(),
            "parent".into(),
        );
        for _ in 0..5 {
            observer.snapshot();
        }
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.invocations[0].status, Status::Done);
    }
}
