//! Chronological history pages never hide the separate active occurrence or its recovery controls.
use super::{Failure, Occurrence, Schedule, Service};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct SchedulePage {
    pub schedules: Vec<Schedule>,
    pub occurrences: Vec<Occurrence>,
    pub active: Option<String>,
    pub active_occurrence: Option<Occurrence>,
    pub next: Option<String>,
    pub controller_error: Option<String>,
    pub defaults: serde_json::Value,
}
impl Service {
    #[cfg(test)]
    pub(crate) fn schedules(&self) -> Result<SchedulePage, Failure> {
        self.schedules_page(None)
    }
    /// Read-only keyset pagination retains active recovery independently of the selected history page.
    pub(crate) fn schedules_page(&self, after: Option<&str>) -> Result<SchedulePage, Failure> {
        let cursor = after.map(parse).transpose()?;
        let store = self.execution.read().map_err(Failure::Unavailable)?;
        let active_occurrence = store
            .active
            .as_ref()
            .and_then(|id| store.occurrences.get(id))
            .cloned();
        let mut occurrences = store
            .occurrences
            .into_values()
            .filter(|occurrence| {
                cursor.as_ref().is_none_or(|(at, id)| {
                    (occurrence.due_at, occurrence.id.as_str()) < (*at, id.as_str())
                })
            })
            .collect::<Vec<_>>();
        occurrences.sort_by(|a, b| (b.due_at, &b.id).cmp(&(a.due_at, &a.id)));
        let more = occurrences.len() > super::super::constants::PAGE_LIMIT;
        occurrences.truncate(super::super::constants::PAGE_LIMIT);
        let next = more
            .then(|| {
                occurrences
                    .last()
                    .map(|occurrence| format!("{}:{}", occurrence.due_at, occurrence.id))
            })
            .flatten();
        Ok(SchedulePage {
            schedules: store.schedules.into_values().collect(),
            occurrences,
            active: store.active,
            active_occurrence,
            next,
            controller_error: self.controller_status(),
            defaults: serde_json::json!({"interval":crate::execution::DEFAULT_INTERVAL,"duration":crate::execution::DEFAULT_DURATION}),
        })
    }
}
fn parse(cursor: &str) -> Result<(u64, String), Failure> {
    let (at, id) = cursor
        .split_once(':')
        .ok_or_else(|| Failure::Invalid("History cursor is invalid. Refresh history.".into()))?;
    if !crate::execution::valid_id(id) {
        return Err(Failure::Invalid(
            "History cursor is invalid. Refresh history.".into(),
        ));
    }
    Ok((
        at.parse()
            .map_err(|_| Failure::Invalid("History cursor is invalid. Refresh history.".into()))?,
        id.into(),
    ))
}
