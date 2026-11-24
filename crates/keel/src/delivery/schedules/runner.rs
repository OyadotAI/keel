//! One cross-daemon dispatcher skips missed ticks and retains uncertain active slots through restart.
use super::super::{Failure, Service};
use crate::execution::{Occurrence, Phase};
use crate::lock::Locked;
use std::{collections::BTreeMap, sync::Arc, time::Instant};

impl Service {
    /// A weak project controller cannot keep a closed daemon alive or dispatch into a switched project.
    pub(crate) fn start_runner(service: &Arc<Self>) {
        let weak = Arc::downgrade(service);
        tokio::spawn(async move {
            let mut lease = None;
            let mut started = BTreeMap::<String, Instant>::new();
            let mut first = true;
            let mut last = None;
            loop {
                tokio::time::sleep(super::super::constants::RUNNER_TICK).await;
                let Some(service) = weak.upgrade() else { break };
                if !service.runtime.current() {
                    continue;
                }
                let owner = service.clone();
                let _ =
                    crate::serve::in_blocking(move || Ok::<_, Failure>(owner.controller_status()))
                        .await;
                if lease.is_none() {
                    let root = service.root.clone();
                    lease = crate::serve::in_blocking(move || {
                        crate::permissions::try_lease(&format!(
                            "schedule-owner-{}.lock",
                            crate::permissions::key_of(&root)?
                        ))
                    })
                    .await
                    .ok()
                    .flatten();
                    if lease.is_none() {
                        continue;
                    }
                }
                let now = service.clock.now();
                if service.controller_error.locked().is_none()
                    && last.is_some_and(|previous| super::cadence::shifted(previous, now))
                {
                    let owner = service.clone();
                    if let Err(error) = crate::serve::in_blocking(move || owner.reanchor()).await {
                        service.pause_dispatcher(error.message());
                        tracing::debug!("Recurring cadence remains paused: {}", error.message());
                        continue;
                    }
                }
                last = Some(now);
                service.recover_tick(&mut started, &mut first).await;
            }
        });
    }

    /// A failed initial observation retains restart recovery until the original active slot is reconciled.
    pub(super) async fn recover_tick(
        self: &Arc<Self>,
        started: &mut BTreeMap<String, Instant>,
        first: &mut bool,
    ) {
        if self.tick(started, *first).await.is_ok() {
            *first = false;
        }
    }

    pub(super) async fn tick(
        self: &Arc<Self>,
        started: &mut BTreeMap<String, Instant>,
        restarted: bool,
    ) -> Result<(), Failure> {
        let result = self.run_tick(started, restarted).await;
        if let Err(error) = &result {
            self.pause_dispatcher(error.message());
        }
        result
    }

    async fn run_tick(
        self: &Arc<Self>,
        started: &mut BTreeMap<String, Instant>,
        restarted: bool,
    ) -> Result<(), Failure> {
        let service = self.clone();
        let (store, trust) = crate::serve::in_blocking(move || {
            service.controller_status();
            service
                .execution
                .read()
                .map(|store| (store, crate::permissions::trust_stamp(&service.root)))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let initial_active = if restarted {
            self.initial_active
                .locked()
                .get_or_insert_with(|| store.active.clone())
                .clone()
        } else {
            None
        };
        for receipt in store.receipts.values().filter(|receipt| {
            ["issue", "batch", "scheduled"].contains(&receipt.kind.as_str())
                && (receipt.phase == Phase::Accepted
                    || restarted && receipt.phase == Phase::Preparing)
        }) {
            if let Ok(updated) = self.operation_status(&receipt.id).await
                && updated.phase != receipt.phase
            {
                crate::events::emit(
                    "delivery.changed",
                    None,
                    serde_json::json!({"task":receipt.task}),
                );
            }
        }
        for receipt in store
            .receipts
            .values()
            .filter(|r| r.kind == "batch" && r.phase == Phase::Queued)
        {
            let _ = self.dispatch_batch(&receipt.id).await;
        }
        if let Some(occurrence) = store
            .active
            .as_ref()
            .and_then(|id| store.occurrences.get(id))
        {
            self.observe_occurrence(
                occurrence,
                started,
                restarted && initial_active.as_deref() == Some(&occurrence.id),
            )
            .await?;
        }
        let service = self.clone();
        if self.controller_error.locked().is_some() {
            return Ok(());
        }
        if !store.schedules.values().any(|s| {
            s.enabled
                && (trust.as_deref() != Some(&s.trust)
                    || s.next_at.is_none_or(|at| at <= self.clock.now()))
        }) {
            return Ok(());
        }
        let due = crate::serve::in_blocking(move || service.advance_definitions(restarted)).await?;
        for id in due {
            let service = self.clone();
            let receipt = super::super::service::id()?;
            let occurrence = crate::serve::in_blocking(move || {
                service.reserve_occurrence(&id, None, &receipt, true)
            })
            .await?;
            if occurrence.phase == Phase::Reserved {
                self.observe_occurrence(&occurrence, started, false).await?;
            }
        }
        Ok(())
    }

    async fn observe_occurrence(
        self: &Arc<Self>,
        occurrence: &Occurrence,
        started: &mut BTreeMap<String, Instant>,
        restarted: bool,
    ) -> Result<(), Failure> {
        if restarted && !started.contains_key(&occurrence.id) {
            return self.mark_occurrence(&occurrence.id,Phase::Unconfirmed,false,"Daemon restarted. Inspect the original task and confirm it stopped; this occurrence will not restart.").await;
        }
        if matches!(occurrence.phase, Phase::Unconfirmed | Phase::Interrupted) {
            return Ok(());
        }
        if occurrence.phase == Phase::Reserved {
            started.insert(occurrence.id.clone(), Instant::now());
            if let Err(error) = self.launch_occurrence(occurrence).await {
                return self
                    .mark_occurrence(&occurrence.id, Phase::Unconfirmed, false, error.message())
                    .await;
            }
            return Ok(());
        }
        let Some(owner) = self.occurrence_owner(occurrence).await? else {
            return self
                .mark_occurrence(
                    &occurrence.id,
                    Phase::Unconfirmed,
                    false,
                    "Original task ownership could not be confirmed.",
                )
                .await;
        };
        if occurrence.phase != Phase::Stopping
            && limit_reached(occurrence, started, self.clock.now())
        {
            let _ = self.runtime.stop(owner.clone()).await;
            return self.mark_occurrence(&occurrence.id,Phase::Stopping,false,"Run limit reached, including time waiting and running checks. Stop requested; waiting for confirmed idle.").await;
        }
        let receipt = self.launch_status(&occurrence.receipt).await?;
        let busy = self
            .runtime
            .busy(&owner)
            .await
            .map_err(Failure::Unavailable)?;
        let pending = self.runtime.pending(&owner);
        if !busy
            && !pending
            && matches!(
                receipt.phase,
                Phase::Completed
                    | Phase::Failed
                    | Phase::Interrupted
                    | Phase::FailedBeforeAcceptance
            )
        {
            started.remove(&occurrence.id);
            return self
                .mark_occurrence(
                    &occurrence.id,
                    if occurrence.phase == Phase::Stopping {
                        Phase::Stopped
                    } else {
                        receipt.phase
                    },
                    true,
                    "Execution and checks are idle; the task remains available for review.",
                )
                .await;
        }
        Ok(())
    }

    async fn mark_occurrence(
        self: &Arc<Self>,
        id: &str,
        phase: Phase,
        release: bool,
        reason: &str,
    ) -> Result<(), Failure> {
        let service = self.clone();
        let id = id.to_string();
        let reason = reason.to_string();
        crate::serve::in_blocking(move || {
            service.execution.change(|store| {
                let occurrence = store
                    .occurrences
                    .get_mut(&id)
                    .ok_or("Occurrence unavailable.")?;
                occurrence.phase = phase;
                occurrence.reason = Some(reason);
                if release {
                    occurrence.finished_at = Some(service.clock.now());
                    if store.active.as_deref() == Some(&id) {
                        store.active = None;
                    }
                }
                Ok(())
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        super::changed();
        Ok(())
    }
}

fn limit_reached(occurrence: &Occurrence, started: &BTreeMap<String, Instant>, now: u64) -> bool {
    let duration = occurrence
        .deadline
        .zip(occurrence.started_at)
        .map(|(end, start)| end.saturating_sub(start))
        .unwrap_or_default();
    let elapsed = started
        .get(&occurrence.id)
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or_default();
    elapsed >= duration || occurrence.deadline.is_some_and(|end| now >= end)
}
