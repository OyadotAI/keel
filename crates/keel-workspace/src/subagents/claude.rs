//! Claude Agent/Task calls are executions; asynchronous replies do not finish their children.
use super::claude_result::{self, Reply, text};
use super::{
    Availability, Status,
    json::{self, Cursor},
    observer::{Adapter, Context, Entries, Entry, insert, update},
};
use crate::Follows;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Reader {
    parent: Cursor,
    pending: BTreeMap<String, Reply>,
    notices: BTreeMap<String, (Status, Option<String>)>,
}
impl Adapter for Reader {
    fn scan(&mut self, context: &Context, entries: &mut Entries) -> Option<bool> {
        let (records, truncated) = self.parent.read(context.parent.path())?;
        let root = crate::Real::real(context.parent.path().parent()?)
            .ok()?
            .join(&context.id)
            .join("subagents");
        let recent_from = records.len().saturating_sub(self.parent.recent_count);
        for (index, r) in records.into_iter().enumerate() {
            let at = r["timestamp"].as_str();
            if r["type"] == "assistant" {
                for block in r["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] != "tool_use"
                        || !matches!(block["name"].as_str(), Some("Agent" | "Task"))
                    {
                        continue;
                    }
                    let Some(call) = block["id"].as_str().filter(|id| json::valid_id(id)) else {
                        continue;
                    };
                    let Some(e) = insert(
                        entries,
                        format!("claude:{call}"),
                        None,
                        block["input"]["subagent_type"].as_str().map(str::to_string),
                        Follows::Claude,
                        &root,
                        at.map(str::to_string),
                    ) else {
                        continue;
                    };
                    e.invocation.activity = block["input"]["description"]
                        .as_str()
                        .map(|s| s.chars().take(json::LABEL_LIMIT).collect());
                    e.attributed |= index >= recent_from || !truncated;
                    update(e, Status::Working, at);
                    if let Some(reply) = self.pending.remove(call) {
                        reply.apply(e);
                    }
                    if let Some((status, at)) = self.notices.remove(call) {
                        update(e, status, at.as_deref());
                    }
                }
            }
            if r["type"] == "user" {
                claude_result::receive(entries, &mut self.pending, &r);
            }
            if let Some((call, status, at)) = notification(&r) {
                if let Some(e) = entries.get_mut(&format!("claude:{call}")) {
                    update(e, status, at.as_deref());
                } else {
                    if self.notices.len() >= json::HISTORY_LIMIT {
                        let oldest = self
                            .notices
                            .iter()
                            .min_by(|a, b| a.1.1.cmp(&b.1.1))
                            .map(|(id, _)| id.clone())
                            .unwrap();
                        self.notices.remove(&oldest);
                    }
                    self.notices.insert(call, (status, at));
                }
            }
        }
        for e in entries.values_mut() {
            attach(context, e);
        }
        let boundaries: Vec<_> = entries
            .values()
            .filter_map(|e| Some((e.invocation.child.clone()?, e.invocation.started.clone()?)))
            .collect();
        for e in entries.values_mut() {
            e.attributed |= !truncated;
            e.until = e.invocation.finished.clone();
            if let (Some(child), Some(start)) = (&e.invocation.child, &e.invocation.started)
                && let Some(next) = boundaries
                    .iter()
                    .filter(|(id, at)| id == child && at > start)
                    .map(|(_, at)| at)
                    .min()
                && e.until.as_ref().is_none_or(|end| next < end)
            {
                e.until = Some(next.clone());
            }
        }
        Some(truncated)
    }
}
fn attach(context: &Context, entry: &mut Entry) {
    entry.owner = Some(context.id.clone());
    let Some(id) = entry
        .invocation
        .child
        .as_deref()
        .filter(|id| json::valid_id(id))
    else {
        return;
    };
    let candidate = entry.root.join(format!("agent-{id}.jsonl"));
    let path = json::confined(&candidate, &entry.root).filter(|p| {
        p.starts_with(&entry.root)
            && json::header(p).is_some_and(|h| h["agentId"] == id && h["sessionId"] == context.id)
    });
    entry.path = path;
    if entry.path.is_some() {
        entry.invocation.output = Availability::Available;
        if entry.invocation.status == Status::Unknown {
            entry.invocation.status = entry.confirmed;
        }
    } else if entry.invocation.status.active() {
        entry.invocation.status = Status::Unknown;
        entry.invocation.output = Availability::Disconnected;
    } else if entry.result.is_none() {
        entry.invocation.output = Availability::Unavailable;
    }
}
fn tag(text: &str, name: &str) -> Option<String> {
    let from = text.find(&format!("<{name}>"))? + name.len() + 2;
    Some(
        text[from..]
            .split_once(&format!("</{name}>"))?
            .0
            .trim()
            .to_string(),
    )
}
fn terminal(status: Option<&str>) -> Status {
    match status {
        Some("completed" | "done") => Status::Done,
        Some("failed") => Status::Failed,
        Some("cancelled" | "killed" | "stopped") => Status::Cancelled,
        _ => Status::Unknown,
    }
}
fn notification(record: &Value) -> Option<(String, Status, Option<String>)> {
    let at = record["timestamp"].as_str().map(str::to_string);
    if record["type"] == "system" && record["subtype"] == "task_notification" {
        return Some((
            record["tool_use_id"].as_str()?.to_string(),
            terminal(record["status"].as_str()),
            at,
        ));
    }
    if record["type"] != "user" {
        return None;
    }
    let text = text(&record["message"]["content"]).filter(|s| s.contains("<task-notification>"))?;
    Some((
        tag(&text, "tool-use-id")?,
        terminal(tag(&text, "status").as_deref()),
        at,
    ))
}
