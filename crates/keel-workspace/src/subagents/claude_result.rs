//! Bounded result tombstones reconcile parent replies discovered before their Agent call.
use super::{
    Availability, Status, json,
    observer::{Entries, Entry, update},
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct Reply {
    child: Option<String>,
    asynchronous: bool,
    failed: bool,
    at: Option<String>,
    output: Option<String>,
}
impl Reply {
    pub fn apply(self, entry: &mut Entry) {
        if self.child.is_some() {
            entry.invocation.child = self.child;
        }
        if !self.asynchronous || self.failed {
            update(
                entry,
                if self.failed {
                    Status::Failed
                } else {
                    Status::Done
                },
                self.at.as_deref(),
            );
            entry.result = self.output;
        }
        if entry.result.as_ref().is_some_and(|s| !s.is_empty()) {
            entry.invocation.output = Availability::Available;
        }
    }
}

pub(super) fn receive(entries: &mut Entries, pending: &mut BTreeMap<String, Reply>, r: &Value) {
    for block in r["message"]["content"].as_array().into_iter().flatten() {
        if block["type"] != "tool_result" {
            continue;
        }
        let Some(call) = block["tool_use_id"]
            .as_str()
            .filter(|id| json::valid_id(id))
        else {
            continue;
        };
        let metadata = if r["toolUseResult"].is_object() {
            &r["toolUseResult"]
        } else {
            &r["tool_use_result"]
        };
        let reply = Reply {
            child: metadata["agentId"]
                .as_str()
                .or(metadata["agent_id"].as_str())
                .filter(|id| json::valid_id(id))
                .map(str::to_string),
            asynchronous: metadata["status"] == "async_launched",
            failed: block["is_error"] == true,
            at: r["timestamp"].as_str().map(str::to_string),
            output: text(&block["content"]),
        };
        if let Some(entry) = entries.get_mut(&format!("claude:{call}")) {
            reply.apply(entry);
        } else {
            if pending.len() >= json::HISTORY_LIMIT {
                let oldest = pending
                    .iter()
                    .min_by(|a, b| a.1.at.cmp(&b.1.at))
                    .map(|(id, _)| id.clone())
                    .unwrap();
                pending.remove(&oldest);
            }
            pending.insert(call.to_string(), reply);
        }
    }
}
pub(super) fn text(value: &Value) -> Option<String> {
    let value = value.as_str().map(str::to_string).or_else(|| {
        value.as_array().map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
    })?;
    let mut end = value.len().min(json::OUTPUT_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_string())
}
