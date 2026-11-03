//! Bounded deferred discovery preserves rejected child links until a reader slot becomes available.
use super::{json, observer::Entries};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Candidate {
    pub id: String,
    pub member: Option<String>,
    at: String,
    sequence: u64,
}
#[derive(Default)]
pub(super) struct Queue {
    values: BTreeMap<String, Candidate>,
    next: u64,
    pub limited: bool,
}
impl Queue {
    pub fn push(&mut self, id: String, member: Option<String>, at: String) {
        if !json::valid_id(&id) {
            return;
        }
        self.next = self.next.saturating_add(1);
        let member = member.filter(|name| name.len() <= json::LABEL_LIMIT);
        let candidate = Candidate {
            id: id.clone(),
            member,
            at,
            sequence: self.next,
        };
        if self
            .values
            .get(&id)
            .is_none_or(|old| candidate.at >= old.at)
        {
            self.values.insert(id, candidate);
        }
        if self.values.len() > json::CANDIDATE_LIMIT {
            let oldest = self
                .values
                .iter()
                .min_by(|a, b| (&a.1.at, a.1.sequence).cmp(&(&b.1.at, b.1.sequence)))
                .map(|(id, _)| id.clone())
                .unwrap();
            self.values.remove(&oldest);
            self.limited = true;
        }
    }
    pub fn take(&mut self) -> Vec<Candidate> {
        let mut values: Vec<_> = std::mem::take(&mut self.values).into_values().collect();
        values.sort_by(|a, b| (&b.at, b.sequence).cmp(&(&a.at, a.sequence)));
        values
    }
    pub fn defer(&mut self, candidate: Candidate) {
        self.limited = true;
        self.values.insert(candidate.id.clone(), candidate);
    }
}

pub(super) fn admit<T>(children: &mut BTreeMap<String, T>, entries: &Entries, id: &str) -> bool {
    if children.contains_key(id) || children.len() < json::CANDIDATE_LIMIT {
        return true;
    }
    let active: BTreeSet<_> = entries
        .values()
        .filter(|entry| entry.current && entry.invocation.status.active())
        .filter_map(|entry| entry.invocation.child.as_deref())
        .collect();
    let retired = children
        .keys()
        .find(|id| !active.contains(id.as_str()))
        .cloned();
    if let Some(retired) = retired {
        children.remove(&retired);
        true
    } else {
        false
    }
}
