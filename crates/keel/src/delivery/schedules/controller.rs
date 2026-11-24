//! A visible controller halt blocks new automation while retaining active observation and explicit recovery.
use super::{Failure, Service, milliseconds};
use crate::lock::Locked;
impl Service {
    /// A changed failure emits one status update instead of repeating an invisible retry every tick.
    pub(super) fn pause_dispatcher(&self, error: &str) {
        if self.controller_error.locked().as_deref() == Some(error) {
            return;
        }
        *self.controller_error.locked() = Some(error.into());
        if let Ok(status) = self.execution.halt_dispatcher(error.into()) {
            self.apply_controller_status(status);
        }
        super::changed();
    }
    pub(super) fn apply_controller_status(&self, status: crate::execution::DispatcherStatus) {
        let mut revision = self.controller_revision.locked();
        if revision.is_none_or(|old| status.revision > old) {
            *revision = Some(status.revision);
            *self.controller_error.locked() = status.error;
            super::changed();
        }
    }
    /// Non-owner daemons observe the same halt and another daemon's successful explicit Retry.
    pub(super) fn controller_status(&self) -> Option<String> {
        match self.execution.dispatcher_status() {
            Ok(Some(status)) => self.apply_controller_status(status),
            Ok(None) => {
                if self.controller_revision.locked().is_some() {
                    *self.controller_error.locked() = Some(
                        "Saved dispatcher status is missing. Retry after restoring storage.".into(),
                    );
                }
            }
            Err(error) => {
                let mut current = self.controller_error.locked();
                if current.as_ref() != Some(&error) {
                    *current = Some(error);
                    super::changed();
                }
            }
        }
        self.controller_error.locked().clone()
    }
    /// Retry proves current write capacity and moves enabled definitions to a future tick without replay.
    pub(super) fn retry_dispatcher(&self) -> Result<(), Failure> {
        let result = self.execution.retry_dispatcher(|| {
            self.execution.check_capacity().and_then(|()| {
                self.execution.change(|store| {
                    for schedule in store.schedules.values_mut().filter(|s| s.enabled) {
                        schedule.next_at = Some(
                            self.clock
                                .now()
                                .saturating_add(milliseconds(schedule.interval)),
                        );
                        schedule.error =
                            Some("Dispatcher recovered. Missed runs were skipped.".into());
                    }
                    Ok(())
                })
            })
        });
        let status = result.map_err(|error| {
            self.pause_dispatcher(&error);
            Failure::Unavailable(error)
        })?;
        self.apply_controller_status(status);
        super::changed();
        Ok(())
    }
}
