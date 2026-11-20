//! OpenCode subagents are sessions of their own whose `parent_id` names the session that asked.
use super::{
    Availability, Status, json,
    observer::{Adapter, Context, Entries, insert, update},
};
use crate::{Follows, opencode};
use serde_json::Value;

/// The link is a column OpenCode writes itself, so there is nothing to discover or defer: each
/// scan asks the database which sessions the parent started and how each one's last reply ended.
#[derive(Default)]
pub(super) struct Reader;

impl Adapter for Reader {
    fn scan(&mut self, context: &Context, entries: &mut Entries) -> Option<bool> {
        let db = context.parent.path();
        let children = opencode::children(db, &context.id, json::CANDIDATE_LIMIT)?;
        let truncated = children.len() >= json::CANDIDATE_LIMIT;
        for child in children {
            let at = crate::iso(child.created.div_euclid(1000), 0);
            let root = db.parent().unwrap_or(db);
            let Some(entry) = insert(
                entries,
                child.id.clone(),
                Some(child.id.clone()),
                child.member.filter(|m| m.len() <= json::LABEL_LIMIT),
                Follows::OpenCode,
                root,
                Some(at),
            ) else {
                continue;
            };
            entry.path = Some(db.to_owned());
            entry.owner = Some(context.id.clone());
            entry.invocation.output = Availability::Available;
            let moved = crate::iso(child.updated.div_euclid(1000), 0);
            update(entry, status(&child.last), Some(&moved));
        }
        Some(truncated)
    }
}

/// How a child's newest message says it stands. A reply that asked for no tool is the end; an
/// abort is the person or the parent stopping it; anything still being written is work.
fn status(last: &Value) -> Status {
    if last["error"]["name"] == "MessageAbortedError" {
        Status::Cancelled
    } else if last["error"].is_object() {
        Status::Failed
    } else if last["role"] == "assistant"
        && last["time"]["completed"].is_number()
        && last["finish"] != "tool-calls"
    {
        Status::Done
    } else {
        Status::Working
    }
}
