//! Discovery pressure and process epochs must preserve current work without reviving stale runs.
use super::*;
use crate::{Follows, Transcript};
use camino::Utf8Path;
use serde_json::json;
use std::io::Write;

fn add(path: &Utf8Path, value: serde_json::Value) {
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
fn child(root: &Utf8Path, parent: &Utf8Path, n: usize, finished: bool) {
    let id = format!("child-{n:03}");
    add(
        parent,
        json!({"type":"event_msg","timestamp":format!("{n:06}"),"payload":{"item":{"type":"SubAgentActivity","agent_thread_id":id}}}),
    );
    let path = root.join(format!("rollout-{id}.jsonl"));
    add(
        &path,
        json!({"type":"session_meta","payload":{"id":id,"cwd":root,"parent_thread_id":"parent"}}),
    );
    add(
        &path,
        json!({"type":"event_msg","timestamp":"2026-01-01T00:00:00Z","payload":{"type":"task_started","turn_id":"run"}}),
    );
    if finished {
        complete(&path);
    }
}
fn complete(path: &Utf8Path) {
    add(
        path,
        json!({"type":"event_msg","timestamp":"2026-01-01T00:01:00Z","payload":{"type":"task_complete","turn_id":"run"}}),
    );
}
#[test]
fn codex_discovers_new_children_after_completed_or_occupied_reader_limits() {
    for cold in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let parent = root.join("rollout-parent.jsonl");
        add(
            &parent,
            json!({"type":"session_meta","payload":{"id":"parent","cwd":root}}),
        );
        for n in 0..256 {
            child(root, &parent, n, cold);
        }
        let mut observer = Observer::with_roots(
            Transcript::at(Follows::Codex, parent.clone()),
            root.to_owned(),
            "parent".into(),
            Roots {
                codex_sessions: root.to_owned(),
            },
        );
        if !cold {
            observer.snapshot();
        }
        child(root, &parent, 256, false);
        observer.snapshot();
        if !cold {
            for n in 0..256 {
                complete(&root.join(format!("rollout-child-{n:03}.jsonl")));
            }
        }
        for _ in 0..3 {
            observer.snapshot();
        }
        assert!(
            observer
                .snapshot()
                .invocations
                .iter()
                .any(|i| i.child.as_deref() == Some("child-256") && i.status == Status::Working)
        );
    }
}
#[test]
fn a_new_terminal_lifetime_interrupts_old_unfinished_work_but_accepts_new_runs() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let parent = root.join("rollout-parent.jsonl");
    add(
        &parent,
        json!({"type":"session_meta","payload":{"id":"parent","cwd":root}}),
    );
    child(root, &parent, 0, false);
    let mut observer = Observer::with_roots(
        Transcript::at(Follows::Codex, parent),
        root.to_owned(),
        "parent".into(),
        Roots {
            codex_sessions: root.to_owned(),
        },
    );
    assert_eq!(observer.snapshot().invocations[0].status, Status::Working);
    observer.lifetime(Some("2026-01-02T00:00:00Z"));
    assert_eq!(
        observer.snapshot().invocations[0].status,
        Status::Interrupted
    );
    add(
        &root.join("rollout-child-000.jsonl"),
        json!({"type":"event_msg","timestamp":"2026-01-02T00:00:01Z","payload":{"type":"task_started","turn_id":"new"}}),
    );
    let snapshot = observer.snapshot();
    assert!(
        snapshot
            .invocations
            .iter()
            .any(|i| i.id.ends_with(":new") && i.status == Status::Working)
    );
    assert!(
        snapshot
            .invocations
            .iter()
            .any(|i| i.id.ends_with(":run") && i.status == Status::Interrupted)
    );
}
fn var(mut n: u64) -> Vec<u8> {
    let mut v = vec![];
    while n >= 128 {
        v.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    v.push(n as u8);
    v
}
fn bytes(n: u64, b: &[u8]) -> Vec<u8> {
    [var(n * 8 + 2), var(b.len() as u64), b.to_vec()].concat()
}
fn database(path: &Utf8Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch("CREATE TABLE steps(idx INTEGER PRIMARY KEY,step_type INTEGER,status INTEGER,step_payload BLOB)").unwrap();
    c
}
#[test]
fn antigravity_discovers_new_children_when_completed_reader_slots_are_reused() {
    for cold in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let parent = root.join("parent.db");
        let c = database(&parent);
        let mut observer = Observer::new(
            Transcript::at(Follows::Antigravity, parent),
            root.to_owned(),
            "parent".into(),
        );
        for n in 0..257 {
            if !cold && n == 256 {
                observer.snapshot();
            }
            let id = format!("child-{n:03}");
            let call = [bytes(2, b"invoke_subagent"), bytes(3, b"{}")].concat();
            let text = format!("{{\"conversationId\": \"{id}\"}}");
            let payload = [
                bytes(5, &bytes(4, &call)),
                bytes(140, &bytes(2, &bytes(1, text.as_bytes()))),
            ]
            .concat();
            c.execute(
                "INSERT INTO steps VALUES (?1,132,3,?2)",
                rusqlite::params![n, &payload],
            )
            .unwrap();
            let child = database(&root.join(format!("{id}.db")));
            child
                .execute("INSERT INTO steps VALUES (0,14,3,x'')", [])
                .unwrap();
            child
                .execute(
                    "INSERT INTO steps VALUES (1,15,?1,x'')",
                    [if n < 256 && cold { 3 } else { 8 }],
                )
                .unwrap();
        }
        observer.snapshot();
        if !cold {
            for n in 0..256 {
                let child =
                    rusqlite::Connection::open(root.join(format!("child-{n:03}.db"))).unwrap();
                child
                    .execute("UPDATE steps SET status=3 WHERE idx=1", [])
                    .unwrap();
            }
        }
        for _ in 0..3 {
            observer.snapshot();
        }
        assert!(
            observer
                .snapshot()
                .invocations
                .iter()
                .any(|i| i.child.as_deref() == Some("child-256") && i.status == Status::Working)
        );
    }
}
