//! Current activity and reported usage, independent of the settled transcript cursor: a pending
//! timer must not hide later responses or children that restart after their first answer.

use std::collections::BTreeMap;

use camino::Utf8Path;
use serde::Serialize;
use serde_json::Value;
mod children;

use super::{MAX_REPLAY_STEPS, RESPONSE, SETTLED, TOOL, USER_INPUT, count, ends, line, open};

/// Display-only state; it never opens or finishes a turn or authorizes queued work.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Observation {
    pub running: bool,
    pub at: Option<String>,
    pub background: BTreeMap<String, Option<String>>,
    pub usage: Option<ReportedUsage>,
    pub truncated: bool,
}

/// Disjoint input categories and total output for the observed parent history, assigned as a
/// snapshot so polling a response while it changes cannot count it twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ReportedUsage {
    pub context: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Normalize both the direct 1.4.6 arguments and the earlier array without inventing a member.
pub(crate) fn subagent(args: &Value, index: usize) -> &Value {
    if args["Subagents"].is_array() {
        &args["Subagents"][index]
    } else if index == 0 {
        args
    } else {
        &Value::Null
    }
}

/// Read the latest bounded history without advancing the transcript's settled cursor.
#[derive(Default, Debug)]
pub(crate) struct ObservationReader {
    children: children::Reader,
}

impl ObservationReader {
    /// Keep identity discovery separate from the bounded display history, including on reconnect.
    pub(crate) fn observe(&mut self, path: &Utf8Path) -> Option<Observation> {
        let c = open(path)?;
        let total = count(path)?;
        let from = total.saturating_sub(MAX_REPLAY_STEPS);
        let mut q = c.prepare("SELECT idx, step_type, status, step_payload FROM steps WHERE idx >= ?1 ORDER BY idx LIMIT ?2").ok()?;
        let rows = q
            .query_map([from as i64, MAX_REPLAY_STEPS as i64], |r| {
                Ok(line(
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    &r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
                ))
            })
            .ok()?;
        let mut observed = Observation {
            running: false,
            at: None,
            background: self.children.background(&c, path, total)?,
            usage: None,
            truncated: from > 0,
        };
        for row in rows {
            observe_step(&mut observed, &row.ok()?);
        }
        Some(observed)
    }
}

fn observe_step(observed: &mut Observation, v: &Value) {
    if matches!(v["step"].as_i64(), Some(USER_INPUT | RESPONSE | TOOL)) {
        // A scheduled timer is independent work, not a model actively generating at its prompt.
        if v["call"]["name"] != "schedule" {
            observed.running =
                !SETTLED.contains(&v["status"].as_i64().unwrap_or_default()) || !ends(v);
            observed.at = v["at"].as_str().map(str::to_string);
        }
    }
    if v["step"] == RESPONSE && v["usage"].is_object() {
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or_default();
        let usage = observed.usage.get_or_insert_default();
        usage.input += n("input");
        usage.output += n("output");
        usage.cache_read += n("cache_read");
        let context = n("input") + n("cache_read");
        if context > 0 {
            usage.context = Some(context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Follows, Transcript};

    fn var(mut n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while n >= 128 {
            out.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        out.push(n as u8);
        out
    }

    fn bytes(n: u64, b: &[u8]) -> Vec<u8> {
        [var((n << 3) | 2), var(b.len() as u64), b.to_vec()].concat()
    }

    fn integer(n: u64, v: u64) -> Vec<u8> {
        [var(n << 3), var(v)].concat()
    }

    fn response() -> Vec<u8> {
        let usage = [
            integer(2, 100),
            integer(3, 1616),
            integer(5, 50),
            integer(9, 1400),
        ]
        .concat();
        bytes(5, &bytes(9, &usage))
    }

    fn database(path: &Utf8Path) -> rusqlite::Connection {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE steps (idx integer PRIMARY KEY, step_type integer, status integer, step_payload blob);").unwrap();
        c
    }

    fn insert(c: &rusqlite::Connection, idx: i64, kind: i64, status: i64, payload: Vec<u8>) {
        c.execute(
            "INSERT INTO steps VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![idx, kind, status, payload],
        )
        .unwrap();
    }

    /// A timer still holds the ordered cursor, while later reported response state is visible
    /// and assigned once, including output whose thinking is already included.
    #[test]
    fn observations_cross_pending_steps_without_advancing_or_duplicating_the_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("parent.db");
        let c = database(&path);
        insert(&c, 0, USER_INPUT, 3, vec![]);
        insert(&c, 1, TOOL, 2, vec![]);
        insert(&c, 2, RESPONSE, 8, response());
        let t = Transcript::at(Follows::Antigravity, path);
        assert_eq!(t.tail(0).unwrap().1, 1);
        let observed = t.observation().unwrap();
        assert!(observed.running);
        let u = observed.usage.as_ref().unwrap();
        assert_eq!(
            (u.input, u.cache_read, u.output, u.context),
            (100, 50, 1616, Some(150))
        );
        assert_eq!(t.observation(), Some(observed));
        c.execute("UPDATE steps SET status=3 WHERE idx=2", [])
            .unwrap();
        let done = t.observation().unwrap();
        assert!(!done.running);
        assert_eq!(done.usage.as_ref().unwrap().output, 1616);
        assert_eq!(t.tail(1).unwrap().1, 1);
        c.execute("UPDATE steps SET status=6 WHERE idx=2", [])
            .unwrap();
        assert!(!t.observation().unwrap().running);
    }

    fn invocation(args: Value, ids: &[&str]) -> Vec<u8> {
        let call = [
            bytes(2, b"invoke_subagent"),
            bytes(3, args.to_string().as_bytes()),
        ]
        .concat();
        let output = ids
            .iter()
            .map(|id| format!("{{\"conversationId\": \"{id}\"}}"))
            .collect::<Vec<_>>()
            .join("\n");
        [
            bytes(5, &bytes(4, &call)),
            bytes(140, &bytes(2, &bytes(1, output.as_bytes()))),
        ]
        .concat()
    }

    /// Both measured argument formats retain member names, and every observation rechecks a
    /// child after completion or a missing database rather than permanently forgetting it.
    #[test]
    fn children_recover_and_restart_with_their_member_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let parent = dir.join("parent.db");
        let c = database(&parent);
        insert(
            &c,
            0,
            TOOL,
            3,
            invocation(serde_json::json!({"TypeName":"pm"}), &["pm-child"]),
        );
        insert(
            &c,
            1,
            TOOL,
            3,
            invocation(
                serde_json::json!({"Subagents":[{"TypeName":"qa"},{}]}),
                &["qa-child", "unknown-child"],
            ),
        );
        insert(&c, 2, RESPONSE, 3, vec![]);
        let t = Transcript::at(Follows::Antigravity, parent);
        assert!(t.observation().unwrap().background.is_empty());
        let child = database(&dir.join("pm-child.db"));
        insert(&child, 0, RESPONSE, 8, vec![]);
        let qa = database(&dir.join("qa-child.db"));
        insert(&qa, 0, RESPONSE, 8, vec![]);
        let unknown = database(&dir.join("unknown-child.db"));
        insert(&unknown, 0, RESPONSE, 8, vec![]);
        let observed = t.observation().unwrap();
        assert!(!observed.running);
        assert_eq!(observed.background["pm-child"].as_deref(), Some("pm"));
        assert_eq!(observed.background["qa-child"].as_deref(), Some("qa"));
        assert_eq!(observed.background["unknown-child"], None);
        child.execute("UPDATE steps SET status=3", []).unwrap();
        assert!(!t.observation().unwrap().background.contains_key("pm-child"));
        insert(&child, 1, RESPONSE, 8, vec![]);
        assert!(t.observation().unwrap().background.contains_key("pm-child"));
        child
            .execute("UPDATE steps SET status=7 WHERE idx=1", [])
            .unwrap();
        assert!(!t.observation().unwrap().background.contains_key("pm-child"));
        assert_eq!(
            Transcript::at(Follows::Antigravity, dir.join("parent.db")).observation(),
            t.observation()
        );
    }

    /// No provider usage stays absent; a bounded replay reports its incomplete history.
    #[test]
    fn missing_usage_and_truncated_history_are_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("parent.db");
        let c = database(&path);
        let t = Transcript::at(Follows::Antigravity, path);
        assert!(t.observation().unwrap().usage.is_none());
        for i in 0..=MAX_REPLAY_STEPS {
            insert(&c, i as i64, RESPONSE, 3, response());
        }
        let observed = t.observation().unwrap();
        assert!(observed.truncated);
        assert_eq!(observed.usage.unwrap().input, MAX_REPLAY_STEPS * 100);
    }

    /// Older child launches and pending invocation results survive both the usage window and
    /// reconnect discovery; only usage replay is bounded by recent parent steps.
    #[test]
    fn active_and_pending_child_discovery_outlives_recent_history() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let parent = dir.join("parent.db");
        let c = database(&parent);
        insert(
            &c,
            0,
            TOOL,
            3,
            invocation(serde_json::json!({"TypeName":"pm"}), &["old-child"]),
        );
        insert(&c, 1, TOOL, 2, vec![]);
        let child = database(&dir.join("old-child.db"));
        insert(&child, 0, RESPONSE, 8, vec![]);
        let t = Transcript::at(Follows::Antigravity, parent.clone());
        assert!(
            t.observation()
                .unwrap()
                .background
                .contains_key("old-child")
        );
        for idx in 2..MAX_REPLAY_STEPS + 2 {
            insert(&c, idx as i64, RESPONSE, 3, vec![]);
        }
        assert!(
            t.observation()
                .unwrap()
                .background
                .contains_key("old-child")
        );
        let pending = database(&dir.join("late-child.db"));
        insert(&pending, 0, RESPONSE, 8, vec![]);
        c.execute(
            "UPDATE steps SET status=3,step_payload=?1 WHERE idx=1",
            [invocation(
                serde_json::json!({"TypeName":"qa"}),
                &["late-child"],
            )],
        )
        .unwrap();
        let observed = t.observation().unwrap();
        assert_eq!(observed.background.len(), 2);
        assert_eq!(
            Transcript::at(Follows::Antigravity, parent).observation(),
            Some(observed)
        );
        child.execute("UPDATE steps SET status=3", []).unwrap();
        assert!(
            !t.observation()
                .unwrap()
                .background
                .contains_key("old-child")
        );
    }
}
