//! A task's provenance, corrections and proof stay together across lanes and providers.
mod api;
mod authority;
mod batches;
mod checkout;
mod composition;
mod constants;
mod corrections;
mod discovery;
mod error;
mod evidence;
#[cfg(test)]
mod evidence_tests;
mod feedback;
mod issue;
mod launch;
#[cfg(test)]
mod launch_tests;
mod model;
#[cfg(test)]
mod operations_tests;
mod publication;
mod publication_target;
mod receipts;
mod records;
mod repository;
#[cfg(test)]
mod repository_tests;
mod reviews;
mod runtime;
pub(crate) use reviews::persist_review;
mod schedules;
mod service;
#[cfg(test)]
mod test_support;
mod verification;
mod view;
pub(crate) use api::retry_publication;
pub(crate) use api::{add_correction, edit_correction, refresh_feedback, send_batch};
pub(crate) use api::{by_lane, checks, list, preview, receipt, recover, start, stop_checks, task};
pub(crate) use api::{form, publish, save_form, save_schedule, schedule_action, schedules};
pub(crate) use batches::BatchInput;
pub(crate) use corrections::{CorrectionEdit, CorrectionInput};
pub(crate) use error::Failure;
pub(crate) use evidence::{begin_check, bind_successor, end_check, persist_check};
pub(crate) use model::*;
pub(crate) use publication::{FormInput, PublishInput};
pub(crate) use schedules::{ScheduleAction, ScheduleInput};
pub(crate) use service::id as operation_id;
pub(crate) use service::{Service, StartIssue, capture, service};
#[cfg(test)]
pub(crate) use test_support::Fixture as DeliveryFixture;
pub(crate) use verification::CheckInput;

/// Strict references protect managed checkouts when advisory notes are damaged or unavailable.
pub(crate) fn guard(
    repo: &camino::Utf8Path,
    lane: &str,
    checkout: &camino::Utf8Path,
) -> Result<(), String> {
    guard_with(repo, lane, checkout, None)
}

/// Only the exact frozen, undispatched source may bootstrap registration; damaged existing owners cannot.
pub(crate) fn guard_registration(
    repo: &camino::Utf8Path,
    source: &crate::tasks::Source,
) -> Result<(), String> {
    guard_with(
        repo,
        &source.lane,
        camino::Utf8Path::new(&source.checkout),
        Some(source),
    )
}

fn guard_with(
    repo: &camino::Utf8Path,
    lane: &str,
    checkout: &camino::Utf8Path,
    registration: Option<&crate::tasks::Source>,
) -> Result<(), String> {
    use keel_workspace::Real;
    let checkout = checkout.real().map_err(|e| e.to_string())?;
    let store = repository::Repository::new(repo)?.read()?;
    let mut references: std::collections::BTreeSet<String> = store
        .records
        .values()
        .filter(|r| r.ownership_guard && r.owner.checkout == checkout.as_str())
        .map(|r| r.task.clone())
        .collect();
    let control = crate::execution::Repository::new(repo)?.read()?;
    for receipt in control
        .receipts
        .values()
        .filter(|r| ["issue", "scheduled"].contains(&r.kind.as_str()))
    {
        if let Some(plan) = receipt
            .result
            .as_ref()
            .and_then(|value| serde_json::from_value::<launch::LaunchPlan>(value.clone()).ok())
            && plan.owner.checkout == checkout.as_str()
        {
            let initial = !plan.dispatched
                && receipt.phase == crate::execution::Phase::Preparing
                && registration == Some(&plan.owner)
                && crate::tasks::lookup(repo, &plan.task)?
                    .is_none_or(|task| task.owner == plan.owner);
            if !initial {
                references.insert(plan.task);
            }
        }
    }
    for task in references {
        let owner = crate::tasks::owner(repo, &task)?;
        if owner.lane != lane && format!("term:{}", owner.session) != lane {
            return Err("This managed checkout belongs to another task source. Open its current task owner.".into());
        }
    }
    Ok(())
}
