//! Persisted discovery merges ordinary tasks with strict delivery records after a window or lane closes.
use super::{Service, constants::PAGE_LIMIT, error::Failure, model::*};
use serde::Serialize;
use std::collections::BTreeMap;

/// List rows carry bounded source labels, not every saved issue body or conversation transcript.
#[derive(Serialize)]
pub(crate) struct Summary {
    pub task: String,
    pub title: String,
    pub source: String,
    pub source_url: Option<String>,
    pub owner: crate::tasks::Source,
    pub updated_at: u64,
    pub unresolved: usize,
    pub remote_open: usize,
    pub check: Option<CheckEvidence>,
    pub publication: Option<Publication>,
    pub managed: bool,
    pub lanes: Vec<String>,
}
/// Missing note ownership is a coverage limit while durable delivery rows remain readable.
#[derive(Serialize)]
pub(crate) struct Page {
    pub tasks: Vec<Summary>,
    pub next: Option<String>,
    pub owner_error: Option<String>,
}
impl Service {
    /// Cursor pages are stable by task identity and include records without any mounted lane.
    pub(crate) fn list(&self, after: Option<&str>, limit: usize) -> Result<Page, Failure> {
        let store = self.records.read().map_err(Failure::Unavailable)?;
        let control = self.execution.read().map_err(Failure::Unavailable)?;
        let mut rows: BTreeMap<String, Summary> = store
            .records
            .values()
            .filter(|r| after.is_none_or(|a| r.task.as_str() > a))
            .map(|r| {
                let mut row = summary(r);
                row.publication = super::publication_target::saved(&control, &r.task);
                (r.task.clone(), row)
            })
            .collect();
        let notes = crate::tasks::list(&self.root, after, PAGE_LIMIT);
        let owner_error = notes.as_ref().err().cloned();
        if let Ok(notes) = notes {
            for task in notes {
                let lanes = task
                    .members
                    .iter()
                    .map(|source| source.lane.clone())
                    .collect::<Vec<_>>();
                rows.entry(task.id.clone())
                    .and_modify(|row| {
                        row.owner = task.owner.clone();
                        row.lanes = lanes.clone();
                    })
                    .or_insert(Summary {
                        task: task.id,
                        title: task
                            .objective
                            .lines()
                            .next()
                            .unwrap_or("Conversation task")
                            .chars()
                            .take(super::constants::TITLE_CHARS)
                            .collect(),
                        source: "Conversation task".into(),
                        source_url: None,
                        owner: task.owner,
                        updated_at: 0,
                        unresolved: 0,
                        remote_open: 0,
                        check: None,
                        publication: None,
                        managed: false,
                        lanes,
                    });
            }
        }
        let limit = limit.clamp(1, PAGE_LIMIT);
        let has_more = rows.len() >= limit;
        let tasks: Vec<_> = rows.into_values().take(limit).collect();
        let next = has_more
            .then(|| tasks.last().map(|r| r.task.clone()))
            .flatten();
        Ok(Page {
            tasks,
            next,
            owner_error,
        })
    }
}
fn summary(record: &Record) -> Summary {
    let (source, source_url) = match &record.source {
        Provenance::Conversation => ("Conversation task".into(), None),
        Provenance::Issue { snapshot } => (
            format!("{}#{}", snapshot.repository, snapshot.number),
            Some(snapshot.url.clone()),
        ),
        Provenance::Recurring { name, .. } => (format!("Recurring: {name}"), None),
    };
    Summary {
        task: record.task.clone(),
        title: record.title.clone(),
        source,
        source_url,
        owner: record.owner.clone(),
        updated_at: record.updated_at,
        unresolved: record
            .corrections
            .iter()
            .filter(|c| c.resolution == Resolution::Unresolved)
            .count(),
        remote_open: record
            .corrections
            .iter()
            .filter(|c| c.remote_open == Some(true))
            .count(),
        check: record.checks.last().cloned(),
        publication: record.publication.clone(),
        managed: true,
        lanes: vec![record.owner.lane.clone()],
    }
}
