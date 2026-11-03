//! Parent activity discovers children; only verified child task lifecycle proves a resumed run.
use super::discovery;
use super::{
    Availability, Status,
    json::{self, Cursor},
    observer::{Adapter, Context, Entries, insert, update},
};
use crate::Follows;
use camino::Utf8PathBuf;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Reader {
    parent: Cursor,
    deferred: discovery::Queue,
    children: BTreeMap<String, Child>,
}
#[derive(Default)]
struct Child {
    member: Option<String>,
    path: Option<Utf8PathBuf>,
    cursor: Cursor,
    latest: Option<(String, String)>,
    recent_active: Option<String>,
}
impl Adapter for Reader {
    fn scan(&mut self, context: &Context, entries: &mut Entries) -> Option<bool> {
        let (records, mut truncated) = self.parent.read(context.parent.path())?;
        for record in records {
            let item = &record["payload"]["item"];
            if item["type"] != "SubAgentActivity" {
                continue;
            }
            let Some(id) = item["agent_thread_id"]
                .as_str()
                .filter(|id| json::valid_id(id))
            else {
                continue;
            };
            let member = item["agent_path"]
                .as_str()
                .and_then(|path| path.rsplit('/').next())
                .filter(|name| !name.is_empty() && name.len() <= json::LABEL_LIMIT)
                .map(str::to_string);
            if let Some(child) = self.children.get_mut(id) {
                if member.is_some() {
                    child.member = member;
                }
            } else {
                self.deferred.push(
                    id.to_string(),
                    member,
                    record["timestamp"].as_str().unwrap_or_default().to_string(),
                );
            }
        }
        for (id, child) in &mut self.children {
            truncated |= child.scan(context, id, entries);
        }
        for candidate in self.deferred.take() {
            if !discovery::admit(&mut self.children, entries, &candidate.id) {
                self.deferred.defer(candidate);
                continue;
            }
            let mut child = Child {
                member: candidate.member,
                ..Default::default()
            };
            truncated |= child.scan(context, &candidate.id, entries);
            self.children.insert(candidate.id, child);
        }
        Some(truncated || self.deferred.limited)
    }
}

impl Child {
    fn scan(&mut self, context: &Context, id: &str, entries: &mut Entries) -> bool {
        let located = self.path.clone().or_else(|| {
            crate::codex::rollout(&context.roots.codex_sessions, &context.directory, id)
        });
        let path = located.and_then(|p| json::confined(&p, &context.roots.codex_sessions));
        let valid = path
            .as_ref()
            .and_then(|p| json::header(p))
            .is_some_and(|h| verified(&h, &context.id, &context.directory, id));
        if !valid {
            unknown(entries, id);
            self.path = None;
            return false;
        }
        self.path = path;
        let Some((mut records, truncated)) = self.cursor.read(self.path.as_ref().unwrap()) else {
            unknown(entries, id);
            return false;
        };
        let recent = records.split_off(records.len().saturating_sub(self.cursor.recent_count));
        for record in recent {
            self.record(context, id, &record, entries, true);
        }
        for record in records {
            self.record(context, id, &record, entries, false);
        }
        // A recovered unchanged file still restores the last confirmed lifecycle after read loss.
        for entry in entries
            .values_mut()
            .filter(|e| e.invocation.child.as_deref() == Some(id))
        {
            entry.path = self.path.clone();
            entry.invocation.output = Availability::Available;
            entry.current = self
                .latest
                .as_ref()
                .is_none_or(|(_, turn)| Some(turn.as_str()) == entry.turn.as_deref());
            let proven = !truncated || self.recent_active.as_deref() == entry.turn.as_deref();
            entry.invocation.status = if entry.confirmed.active() && (!entry.current || !proven) {
                Status::Unknown
            } else {
                entry.confirmed
            };
            if self.member.is_some() {
                entry.invocation.member = self.member.clone();
            }
        }
        truncated
    }
    fn record(
        &mut self,
        context: &Context,
        child: &str,
        record: &Value,
        entries: &mut Entries,
        forward: bool,
    ) {
        let p = &record["payload"];
        if record["type"] != "event_msg" {
            return;
        }
        let Some(turn) = p["turn_id"].as_str().filter(|id| json::valid_id(id)) else {
            return;
        };
        let key = format!("codex:{child}:{turn}");
        let at = record["timestamp"].as_str();
        let terminal = matches!(p["type"].as_str(), Some("task_complete" | "turn_aborted"));
        if p["type"] == "task_started" || terminal {
            let stamp = at.unwrap_or_default().to_string();
            if self
                .latest
                .as_ref()
                .is_none_or(|(version, _)| stamp > *version)
                || (forward
                    && self
                        .latest
                        .as_ref()
                        .is_none_or(|(version, _)| stamp == *version))
            {
                self.latest = Some((stamp, turn.to_string()));
                for entry in entries.values_mut().filter(|e| {
                    e.invocation.child.as_deref() == Some(child) && e.turn.as_deref() != Some(turn)
                }) {
                    entry.current = false;
                    if entry.invocation.status.active() {
                        entry.invocation.status = Status::Unknown;
                    }
                }
            }
            if forward
                && self.latest.as_ref().is_some_and(|(version, id)| {
                    id == turn && at.unwrap_or_default() >= version.as_str()
                })
            {
                if p["type"] == "task_started" {
                    self.recent_active = Some(turn.to_string());
                } else if self.recent_active.as_deref() == Some(turn) {
                    self.recent_active = None;
                }
            }
            let Some(e) = insert(
                entries,
                key.clone(),
                Some(child.to_string()),
                self.member.clone(),
                Follows::Codex,
                &context.roots.codex_sessions,
                (!terminal).then(|| at.map(str::to_string)).flatten(),
            ) else {
                return;
            };
            if p["type"] == "task_started" && e.invocation.started.is_none() {
                e.invocation.started = at.map(str::to_string);
            }
            e.owner = Some(context.id.clone());
            e.directory = Some(context.directory.clone());
            e.turn = Some(turn.to_string());
            e.path = self.path.clone();
            e.invocation.output = Availability::Available;
            if !terminal {
                update(e, Status::Working, at);
            }
        }
        let Some(e) = entries.get_mut(&key) else {
            return;
        };
        match p["type"].as_str() {
            Some("task_complete") => update(e, Status::Done, at),
            Some("turn_aborted") => update(e, Status::Cancelled, at),
            Some("item_completed") => {
                let item = &p["item"];
                if item["type"] == "CommandExecution" {
                    e.invocation.activity = Some("Running a command".into());
                } else if item["type"] == "AgentMessage" {
                    e.invocation.activity = Some("Writing a response".into());
                }
            }
            _ => {}
        }
    }
}
pub(super) fn verified(
    header: &Value,
    parent: &str,
    directory: &camino::Utf8Path,
    id: &str,
) -> bool {
    let p = &header["payload"];
    p["id"] == id
        && (p["parent_thread_id"] == parent
            || p["source"]["subagent"]["thread_spawn"]["parent_thread_id"] == parent)
        && p["cwd"].as_str().is_some_and(|recorded| {
            crate::moved::same_place(camino::Utf8Path::new(recorded), directory)
        })
}
fn unknown(entries: &mut Entries, child: &str) {
    for e in entries
        .values_mut()
        .filter(|e| e.invocation.child.as_deref() == Some(child))
    {
        if e.invocation.status.active() {
            e.invocation.status = Status::Unknown;
        }
        e.path = None;
        e.invocation.output = Availability::Disconnected;
    }
}
