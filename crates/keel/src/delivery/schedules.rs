//! Fixed intervals use private host authority, one durable project slot and no catch-up replay.
mod cadence;
mod controller;
mod history;
pub(crate) use history::SchedulePage;
#[cfg(test)]
mod controller_tests;
mod launch;
mod reservations;
mod runner;
#[cfg(test)]
mod tests;
use super::{Failure, Service};
use crate::execution::{
    MAX_DURATION, MAX_INTERVAL, MIN_DURATION, MIN_INTERVAL, Occurrence, Phase, Schedule,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A definition is saved paused; changes affect future occurrences only.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduleInput {
    pub id: Option<String>,
    pub revision: Option<u64>,
    pub name: String,
    pub prompt: String,
    pub provider: String,
    pub base: String,
    pub interval: u64,
    pub duration: u64,
}
/// Run-now IDs are idempotent; pause and stop have intentionally different effects.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduleAction {
    pub action: String,
    pub revision: u64,
    pub receipt: Option<String>,
}
impl Service {
    /// Validate the local base and installed provider before persisting an inert instruction.
    pub(crate) fn save_schedule(&self, input: ScheduleInput) -> Result<Schedule, Failure> {
        validate(&input)?;
        if input
            .id
            .as_ref()
            .is_some_and(|id| !crate::execution::valid_id(id))
        {
            return Err(Failure::Invalid("Invalid definition identity.".into()));
        }
        super::launch::base_sha(&self.root, &input.base).map_err(Failure::Invalid)?;
        if !self.runtime.available(&input.provider) {
            return Err(Failure::Invalid(
                "Install the selected coding agent before saving this definition.".into(),
            ));
        }
        let id = input.id.clone().unwrap_or(super::service::id()?);
        let at = self.clock.now();
        let _acceptance =
            crate::permissions::acceptance_lease(&self.root).map_err(Failure::Conflict)?;
        self.execution
            .change(|store| {
                let old = store.schedules.get(&id);
                if old.map(|s| s.revision) != input.revision {
                    return Err(
                        "Definition changed. Refresh before saving; your draft was retained."
                            .into(),
                    );
                }
                let schedule = Schedule {
                    id: id.clone(),
                    revision: old.map_or(0, |s| s.revision.saturating_add(1)),
                    name: input.name,
                    prompt: input.prompt,
                    provider: input.provider,
                    base: input.base,
                    interval: input.interval,
                    duration: input.duration,
                    enabled: false,
                    next_at: None,
                    trust: String::new(),
                    created_at: old.map_or(at, |s| s.created_at),
                    updated_at: at,
                    error: None,
                };
                store.schedules.insert(id, schedule.clone());
                Ok(schedule)
            })
            .map_err(Failure::Conflict)
    }

    /// Explicit actions never infer enablement from a project file or a finished agent response.
    pub(crate) async fn schedule_action(
        self: Arc<Self>,
        id: &str,
        input: ScheduleAction,
    ) -> Result<serde_json::Value, Failure> {
        if input.action == "retry" {
            let service = self.clone();
            crate::serve::in_blocking(move || service.retry_dispatcher()).await?;
            return Ok(serde_json::json!({"recovered":true}));
        }
        if ["enable", "pause"].contains(&input.action.as_str()) {
            let service = self.clone();
            let id = id.to_string();
            let enable = input.action == "enable";
            let schedule = crate::serve::in_blocking(move || {
                let _acceptance = crate::permissions::acceptance_lease(&service.root)?;
                service.execution.change(|store| {
                    let schedule = store
                        .schedules
                        .get_mut(&id)
                        .ok_or("Definition unavailable.")?;
                    if schedule.revision != input.revision {
                        return Err(
                            "Definition changed. Refresh before applying this action.".into()
                        );
                    }
                    let trust = crate::permissions::trust_stamp(&service.root);
                    if enable && trust.is_none() {
                        return Err(
                            "Trust this project in Settings before enabling recurring execution."
                                .into(),
                        );
                    }
                    schedule.enabled = enable;
                    schedule.trust = trust.unwrap_or_default();
                    schedule.next_at = enable.then(|| {
                        service
                            .clock
                            .now()
                            .saturating_add(milliseconds(schedule.interval))
                    });
                    schedule.revision = schedule.revision.saturating_add(1);
                    schedule.error = None;
                    schedule.updated_at = service.clock.now();
                    Ok(schedule.clone())
                })
            })
            .await
            .map_err(Failure::Conflict)?;
            changed();
            return serde_json::to_value(schedule).map_err(|e| Failure::Unavailable(e.to_string()));
        }
        if input.action == "run_now" {
            let receipt = input
                .receipt
                .ok_or_else(|| Failure::Invalid("Run now requires a saved request ID.".into()))?;
            let service = self.clone();
            let target = id.to_string();
            let occurrence = crate::serve::in_blocking(move || {
                service.reserve_occurrence(&target, Some(input.revision), &receipt, false)
            })
            .await?;
            changed();
            return serde_json::to_value(occurrence)
                .map_err(|e| Failure::Unavailable(e.to_string()));
        }
        if ["stop", "confirm_stopped"].contains(&input.action.as_str()) {
            return self
                .stop_occurrence(id, input.action == "confirm_stopped")
                .await;
        }
        Err(Failure::Invalid(
            "Choose Enable, Pause, Run now or Stop.".into(),
        ))
    }
}

fn validate(input: &ScheduleInput) -> Result<(), Failure> {
    if input.name.trim().is_empty()
        || input.name.len() > super::constants::TITLE_CHARS
        || input.prompt.trim().is_empty()
        || input.prompt.len() > super::constants::TEXT_LIMIT
        || !crate::agent::PROVIDERS.contains(&input.provider.as_str())
        || !(MIN_INTERVAL..=MAX_INTERVAL).contains(&input.interval)
        || !(MIN_DURATION..=MAX_DURATION).contains(&input.duration)
    {
        return Err(Failure::Invalid("Enter a name, instruction, installed provider, interval from 1 minute to 30 days and run limit from 1 minute to 24 hours.".into()));
    }
    Ok(())
}
pub(super) fn milliseconds(seconds: u64) -> u64 {
    seconds.saturating_mul(super::constants::MILLISECONDS_PER_SECOND)
}
pub(super) fn changed() {
    crate::events::emit(
        "delivery.changed",
        None,
        serde_json::json!({"schedules":true}),
    );
}
