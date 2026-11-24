//! Read-only task projection reconciles saved history with privately observed execution facts.
use super::service::capture;
use super::{CheckEvidence, CheckoutIdentity, Failure, Record, Service};
use crate::tasks::Source;
use camino::Utf8Path;
use serde::Serialize;
impl Service {
    /// Status reads never dispatch work; source records remain usable through a temporary ownership failure.
    pub(crate) fn task(&self, id: &str) -> Result<TaskView, Failure> {
        let mut record = self.record(id)?;
        record.publication = None;
        for review in &mut record.reviews {
            review.confirmed = false;
        }
        let task = crate::tasks::get(&self.root, id);
        let owner = crate::tasks::recover(&self.root, id);
        let identity = owner
            .as_ref()
            .ok()
            .and_then(|s| capture(Utf8Path::new(&s.checkout)).ok());
        let mut evidence_error = None;
        let mut legacy_gate = None;
        if let Ok(task) = &task {
            for source in &task.members {
                if let Some(gate) = crate::turns::read(&self.root, &source.session)
                    .last()
                    .and_then(|turn| turn.gate.clone())
                {
                    if let Some(check) = gate.evidence {
                        merge_check(&mut record.checks, check);
                    } else {
                        legacy_gate = Some(gate);
                    }
                }
            }
        }
        let control = self.execution.read();
        let mut trusted = std::collections::BTreeMap::new();
        if let Ok(control) = &control {
            if let Ok(task) = &task {
                super::reviews::reconcile(&mut record, task, control);
            }
            for receipt in control.receipts.values().filter(|receipt| {
                ["check", "observed_check"].contains(&receipt.kind.as_str())
                    && receipt.task.as_deref() == Some(id)
            }) {
                if let Some(check) = receipt
                    .result
                    .as_ref()
                    .and_then(|result| result.get("check"))
                    .and_then(|value| serde_json::from_value::<CheckEvidence>(value.clone()).ok())
                {
                    merge_check(&mut record.checks, check);
                }
                if let Some((check, binding)) = task
                    .as_ref()
                    .ok()
                    .and_then(|task| super::evidence::trusted(receipt, task))
                {
                    if owner
                        .as_ref()
                        .is_ok_and(|owner| owner.checkout == binding.checkout)
                    {
                        trusted.insert(check.id.clone(), check.clone());
                    }
                    merge_check(&mut record.checks, check);
                }
            }
            for check in trusted.values() {
                merge_check(&mut record.checks, check.clone());
            }
            record.checks.sort_by_key(|check| check.started_at);
            if control
                .receipts
                .values()
                .filter(|r| {
                    ["check", "observed_check"].contains(&r.kind.as_str())
                        && r.task.as_deref() == Some(id)
                })
                .any(|r| {
                    record
                        .checks
                        .last()
                        .is_none_or(|check| r.id == check.id || r.created_at >= check.started_at)
                        && matches!(
                            r.phase,
                            crate::execution::Phase::Running
                                | crate::execution::Phase::Unconfirmed
                                | crate::execution::Phase::Interrupted
                        )
                })
            {
                evidence_error=Some("A check operation is running or its completion is unconfirmed. Inspect its saved request.".into());
            }
            record.publication = super::publication_target::saved(control, id);
        }
        if record
            .checks
            .last()
            .is_some_and(|check| trusted.get(&check.id) != Some(check))
        {
            evidence_error=Some("Check evidence could not be confirmed on this computer. Run checks to establish a current result.".into());
        }
        let mut operations = control
            .as_ref()
            .ok()
            .map(|control| {
                control
                    .receipts
                    .values()
                    .filter(|r| r.task.as_deref() == Some(id) && !r.kind.starts_with("observed_"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        operations.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let operations = operations
            .into_iter()
            .take(super::constants::PAGE_LIMIT)
            .map(|r| Operation {
                id: r.id.clone(),
                kind: r.kind.clone(),
                phase: r.phase.clone(),
                error: r.error.clone(),
                target:r.result.as_ref().and_then(|result|result.get("intent")).map(|intent|serde_json::json!({"head":intent["identity"]["head"],"branch":intent["branch"],"base":intent["base"],"repository":intent["repository"]})),
            })
            .collect();
        let control_error = control.err();
        let plan = owner
            .as_ref()
            .ok()
            .and_then(|owner| super::evidence::plan_hash(Utf8Path::new(&owner.checkout)).ok());
        let checks_current = evidence_error.is_none()
            && control_error.is_none()
            && record
                .checks
                .last()
                .zip(identity.as_ref())
                .zip(plan.as_ref())
                .is_some_and(|((check, identity), plan)| {
                    super::evidence::matches_checkout(check, identity, plan)
                });
        let checks_passed = checks_current
            && record
                .checks
                .last()
                .zip(identity.as_ref())
                .zip(plan.as_ref())
                .is_some_and(|((check, identity), plan)| {
                    super::evidence::current(check, identity, plan)
                });
        Ok(TaskView {
            record,
            owner: owner.as_ref().ok().cloned(),
            owner_error: owner.err(),
            identity,
            checks_current,
            checks_passed,
            evidence_error,
            control_error,
            legacy_gate,
            operations,
        })
    }
}

/// One task view keeps readable history when authoritative ownership or checkout capture is unavailable.
#[derive(Serialize)]
pub(crate) struct TaskView {
    pub record: Record,
    pub owner: Option<Source>,
    pub owner_error: Option<String>,
    pub identity: Option<CheckoutIdentity>,
    pub checks_current: bool,
    pub checks_passed: bool,
    pub evidence_error: Option<String>,
    pub control_error: Option<String>,
    pub legacy_gate: Option<crate::turns::Gate>,
    pub operations: Vec<Operation>,
}
/// Bounded operation summaries expose recovery without copying private execution inputs into the window.
#[derive(Serialize)]
pub(crate) struct Operation {
    id: String,
    kind: String,
    phase: crate::execution::Phase,
    error: Option<String>,
    target: Option<serde_json::Value>,
}

fn merge_check(history: &mut Vec<CheckEvidence>, check: CheckEvidence) -> bool {
    if let Some(old) = history.iter_mut().find(|old| old.id == check.id) {
        if old != &check {
            *old = check;
            return true;
        }
        return false;
    }
    if history.last().is_none_or(|old| {
        check.completed_at.unwrap_or(check.started_at) >= old.completed_at.unwrap_or(old.started_at)
    }) {
        history.push(check);
        return true;
    }
    false
}
