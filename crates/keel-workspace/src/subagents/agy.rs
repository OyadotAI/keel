//! Antigravity child turns are read independently of pending parent rows and database failures.
use super::discovery;
use super::{
    Availability, Status, json,
    observer::{Adapter, Context, Entries, insert, update},
};
use crate::{Follows, antigravity};
use camino::Utf8Path;
use serde_json::Value;
use std::collections::BTreeMap;

const STEP_BATCH: i64 = 512;
const INPUT: i64 = 14;
const RESPONSE: i64 = 15;
const TOOL: i64 = 132;
const DONE: i64 = 3;
const CANCELLED: i64 = 6;
const FAILED: i64 = 7;
const WAITING: i64 = 9;
#[derive(Default)]
pub(super) struct Reader {
    next: i64,
    pending: std::collections::BTreeSet<i64>,
    children: BTreeMap<String, Option<String>>,
    deferred: discovery::Queue,
    /// Each finished child's database as it was when last read (its modification time). A child
    /// whose runs have all ended and whose database has not changed has nothing new to say, and
    /// re-reading it on every pass — each 1–2 s, up to 256 children, up to 2 MB each — was all
    /// cost. A new run writes the database, which changes the time, so it is read again.
    settled: BTreeMap<String, std::time::SystemTime>,
}
impl Reader {
    fn note(&mut self, r: Value) {
        if r["call"]["name"] != "invoke_subagent" {
            return;
        }
        let idx = r["agy"].as_i64().unwrap_or_default();
        if [DONE, CANCELLED, FAILED].contains(&r["status"].as_i64().unwrap_or_default()) {
            self.pending.remove(&idx);
        } else {
            self.pending.insert(idx);
        }
        for (i, child) in antigravity::children(r["output"].as_str().unwrap_or_default())
            .into_iter()
            .enumerate()
        {
            let member = antigravity::subagent(&r["call"]["args"], i)["TypeName"]
                .as_str()
                .map(str::to_string);
            if let Some(known) = self.children.get_mut(&child) {
                if member.is_some() {
                    *known = member;
                }
            } else {
                self.deferred.push(child, member, format!("{idx:020}"));
            }
        }
    }
}
impl Adapter for Reader {
    fn scan(&mut self, context: &Context, entries: &mut Entries) -> Option<bool> {
        let parent = antigravity::open(context.parent.path())?;
        let mut query = parent.prepare("SELECT idx,step_type,status,substr(step_payload,1,?3) FROM steps WHERE idx >= ?1 ORDER BY idx LIMIT ?2").ok()?;
        let mut rows = query
            .query([self.next, STEP_BATCH, json::READ_BYTES as i64])
            .ok()?;
        let mut count = 0;
        let mut read_bytes = 0;
        let pending: Vec<_> = self.pending.iter().copied().collect();
        while let Some(row) = rows.next().ok()? {
            let idx: i64 = row.get(0).ok()?;
            let payload: Option<Vec<u8>> = row.get(3).ok()?;
            read_bytes += payload.as_ref().map_or(0, Vec::len);
            self.next = idx + 1;
            count += 1;
            self.note(antigravity::line(
                idx,
                row.get(1).ok()?,
                row.get(2).ok()?,
                &payload.unwrap_or_default(),
            ));
            if read_bytes >= json::READ_BYTES as usize {
                break;
            }
        }
        for idx in pending {
            let value = parent
                .query_row(
                    "SELECT idx,step_type,status,substr(step_payload,1,?2) FROM steps WHERE idx=?1",
                    [idx, json::READ_BYTES as i64],
                    |r| {
                        Ok(antigravity::line(
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            &r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
                        ))
                    },
                )
                .ok()?;
            self.note(value);
        }
        for (child, member) in &self.children {
            // The later of the database and its write-ahead log: in WAL mode a new run lands in
            // `-wal` first and the `.db` itself may not change until a checkpoint.
            let modified = context.parent.path().parent().and_then(|root| {
                [format!("{child}.db"), format!("{child}.db-wal")]
                    .iter()
                    .filter_map(|f| std::fs::metadata(root.join(f)).ok()?.modified().ok())
                    .max()
            });
            if modified.is_some() && self.settled.get(child) == modified.as_ref() {
                continue;
            }
            observe(context, child, member.clone(), entries);
            let ended = entries
                .values()
                .filter(|e| e.invocation.child.as_deref() == Some(child.as_str()))
                .all(|e| !e.invocation.status.active() && !e.invocation.id.ends_with(":pending"));
            match modified {
                Some(at) if ended => self.settled.insert(child.clone(), at),
                _ => self.settled.remove(child),
            };
        }
        for candidate in self.deferred.take() {
            if !discovery::admit(&mut self.children, entries, &candidate.id) {
                self.deferred.defer(candidate);
                continue;
            }
            observe(context, &candidate.id, candidate.member.clone(), entries);
            self.children.insert(candidate.id, candidate.member);
        }
        Some(
            count == STEP_BATCH || read_bytes >= json::READ_BYTES as usize || self.deferred.limited,
        )
    }
}

pub(super) fn rows(path: &Utf8Path) -> Option<(Vec<Value>, bool)> {
    let connection = antigravity::open(path)?;
    let total = antigravity::count(path)?;
    let mut query=connection.prepare("SELECT idx,step_type,status,substr(step_payload,1,?2) FROM steps ORDER BY idx DESC LIMIT ?1").ok()?;
    let mut rows = query
        .query([
            antigravity::MAX_REPLAY_STEPS as i64,
            json::READ_BYTES as i64,
        ])
        .ok()?;
    let mut values = Vec::new();
    let mut used = 0;
    while let Some(row) = rows.next().ok()? {
        let payload: Option<Vec<u8>> = row.get(3).ok()?;
        let payload = payload.unwrap_or_default();
        if !values.is_empty() && used + payload.len() > json::READ_BYTES as usize {
            break;
        }
        used += payload.len();
        values.push(antigravity::line(
            row.get(0).ok()?,
            row.get(1).ok()?,
            row.get(2).ok()?,
            &payload,
        ));
        if used >= json::READ_BYTES as usize {
            break;
        }
    }
    values.reverse();
    let truncated = values.len() < total as usize;
    if let Some(first) = values.first()
        && first["step"] != INPUT
    {
        let start = first["agy"].as_i64()?;
        if let Ok(input)=connection.query_row("SELECT idx,step_type,status,substr(step_payload,1,?2) FROM steps WHERE step_type=14 AND idx<?1 ORDER BY idx DESC LIMIT 1",[start,json::READ_BYTES as i64],|r| Ok(antigravity::line(r.get(0)?,r.get(1)?,r.get(2)?,&r.get::<_,Option<Vec<u8>>>(3)?.unwrap_or_default()))) {values.insert(0,input);}
    }
    Some((values, truncated))
}

fn observe(context: &Context, child: &str, member: Option<String>, entries: &mut Entries) {
    let Some(root) = context.parent.path().parent() else {
        return;
    };
    let path = json::confined(&root.join(format!("{child}.db")), root)
        .filter(|p| p.file_stem() == Some(child));
    let data = path.as_ref().and_then(|p| rows(p));
    let Some((records, _)) = data else {
        if !entries
            .values()
            .any(|e| e.invocation.child.as_deref() == Some(child))
        {
            let _ = insert(
                entries,
                format!("antigravity:{child}:pending"),
                Some(child.to_string()),
                member,
                Follows::Antigravity,
                root,
                None,
            );
        }
        for e in entries.values_mut().filter(|e| {
            e.invocation.child.as_deref() == Some(child) && e.invocation.status.active()
        }) {
            e.invocation.status = Status::Unknown;
            e.path = None;
            e.invocation.output = Availability::Disconnected;
        }
        return;
    };
    let mut run = None;
    entries.remove(&format!("antigravity:{child}:pending"));
    for r in records {
        let idx = r["agy"].as_i64().unwrap_or_default();
        let kind = r["step"].as_i64().unwrap_or_default();
        let status = r["status"].as_i64().unwrap_or_default();
        if ![INPUT, RESPONSE, TOOL].contains(&kind) {
            continue;
        }
        if kind == INPUT {
            let key = format!("antigravity:{child}:{idx}");
            let Some(e) = insert(
                entries,
                key.clone(),
                Some(child.to_string()),
                member.clone(),
                Follows::Antigravity,
                root,
                r["at"].as_str().map(str::to_string),
            ) else {
                continue;
            };
            e.path = path.clone();
            e.turn = Some(idx.to_string());
            e.invocation.output = Availability::Available;
            update(e, Status::Working, r["at"].as_str());
            run = Some(key);
        }
        let Some(e) = run.as_ref().and_then(|key| entries.get_mut(key)) else {
            continue;
        };
        e.path = path.clone();
        e.invocation.output = Availability::Available;
        let at = r["at"].as_str();
        if status == CANCELLED {
            update(e, Status::Cancelled, at);
        } else if status == FAILED && kind == RESPONSE {
            update(e, Status::Failed, at);
        } else if [DONE, CANCELLED, FAILED].contains(&status) && antigravity::ends(&r) {
            update(e, Status::Done, at);
        } else if status == WAITING {
            update(e, Status::Waiting, at);
        } else {
            update(e, Status::Working, at);
        }
        if kind == TOOL {
            e.invocation.activity = r["call"]["name"].as_str().map(str::to_string);
        }
    }
}
