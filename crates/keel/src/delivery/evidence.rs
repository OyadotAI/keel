//! Check captures are made at execution time, never reconstructed from a later checkout or agent claim.
use super::{
    Failure,
    model::*,
    repository::Repository,
    runtime::{Clock, SystemClock},
    service::{capture, id},
};
use camino::Utf8Path;
use keel_workspace::Real;
use sha2::{Digest, Sha256};

/// A plan digest distinguishes a newly selected command from a previously checked configuration.
pub(crate) fn plan_hash(root: &Utf8Path) -> Result<String, String> {
    serde_json::to_vec(&crate::verify::detect_all(root))
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|e| e.to_string())
}

/// Initial capture runs while the caller holds the normal checkout claim; unavailable identity stays absent.
pub(crate) fn begin_check(
    root: &Utf8Path,
    source: String,
    command: Option<String>,
) -> Result<CheckEvidence, Failure> {
    Ok(CheckEvidence {
        id: id()?,
        status: "running".into(),
        source,
        command,
        plan: plan_hash(root).unwrap_or_default(),
        started_at: SystemClock.now(),
        completed_at: None,
        before: capture(root).ok(),
        after: None,
        successor: None,
        problems: vec![],
        warning: None,
    })
}

/// A reported pass remains historical unless the before/after captures both confirm the same contents.
pub(crate) fn end_check(
    root: &Utf8Path,
    check: &mut CheckEvidence,
    status: &str,
    command: Option<String>,
    problems: Vec<crate::turns::Problem>,
) {
    check.status = status.into();
    check.command = command;
    check.completed_at = Some(SystemClock.now());
    check.after = capture(root).ok();
    check.problems = problems;
    check.warning = None;
}

/// A known automatic checkpoint may bind identical tested contents to its produced commit, and only that commit.
pub(crate) fn bind_successor(root: &Utf8Path, check: &mut CheckEvidence, sha: &str) -> bool {
    let Some(before) = &check.before else {
        return false;
    };
    if check.after.as_ref() != Some(before) || check.status != "passed" {
        return false;
    }
    let Ok(current) = capture(root) else {
        return false;
    };
    if current.head != sha || current.tree != before.tree {
        return false;
    }
    check.successor = Some(sha.into());
    true
}

/// Legacy, interrupted, changed-plan and failed-capture results never become a current pass.
pub(crate) fn current(check: &CheckEvidence, identity: &CheckoutIdentity, plan: &str) -> bool {
    check.status == "passed" && matches_checkout(check, identity, plan)
}

/// Failure and cancellation remain distinct outcomes even when their recorded checkout is current.
pub(crate) fn matches_checkout(
    check: &CheckEvidence,
    identity: &CheckoutIdentity,
    plan: &str,
) -> bool {
    let Some(before) = &check.before else {
        return false;
    };
    check.warning.is_none()
        && check.completed_at.is_some()
        && !check.plan.is_empty()
        && check.plan == plan
        && check.after.as_ref() == Some(before)
        && identity.tree == before.tree
        && (identity.head == before.head || check.successor.as_ref() == Some(&identity.head))
}

/// The strict history is a second durable view of the same machine fact, not a new ownership authority.
pub(crate) fn persist_check(
    repo: &Utf8Path,
    session: &str,
    checkout: &Utf8Path,
    check: &CheckEvidence,
) -> Result<(), String> {
    let task = crate::tasks::for_session(repo, session, checkout);
    let control = crate::execution::Repository::new(repo)?;
    let proof_id = format!("evidence-{:x}", Sha256::digest(check.id.as_bytes()));
    let location = checkout.real().unwrap_or_else(|_| checkout.to_owned());
    let binding = task
        .as_ref()
        .ok()
        .and_then(|task| task.as_ref())
        .and_then(|task| {
            task.members
                .iter()
                .find(|source| source.session == session && source.checkout == location.as_str())
        })
        .cloned();
    let input = serde_json::json!({"id":check.id,"source":check.source,"owner":binding,"before":check.before,"plan":check.plan});
    control
        .reserve(&proof_id, "observed_check", input, check.started_at)
        .map_err(|error| Failure::from(error).message().to_string())?;
    control.change(|store| {
        let receipt = store
            .receipts
            .get_mut(&proof_id)
            .ok_or("Check evidence receipt unavailable.")?;
        receipt.task = task
            .as_ref()
            .ok()
            .and_then(|task| task.as_ref().map(|task| task.id.clone()));
        receipt.phase = if check.completed_at.is_some() {
            crate::execution::Phase::Completed
        } else {
            crate::execution::Phase::Running
        };
        receipt.result = Some(serde_json::json!({"check":check}));
        receipt.updated_at = check.completed_at.unwrap_or(check.started_at);
        Ok(())
    })?;
    let Some(task) = task? else { return Ok(()) };
    let repository = Repository::new(repo)?;
    if !repository.read()?.records.contains_key(&task.id) {
        return Ok(());
    }
    repository.change(|store| {
        let record = store
            .records
            .get_mut(&task.id)
            .ok_or("Task delivery record changed during check capture.")?;
        record.ownership_guard |= task.continuation.is_some();
        if let Some(previous) = record.checks.iter_mut().find(|old| old.id == check.id) {
            *previous = check.clone();
        } else {
            record.checks.push(check.clone());
        }
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or("Task revision is exhausted.")?;
        record.updated_at = SystemClock.now();
        Ok(())
    })?;
    crate::events::emit(
        "delivery.changed",
        task.owner.wt.as_deref(),
        serde_json::json!({"task":task.id}),
    );
    Ok(())
}

/// Only a completed, input-consistent private observation with a registered source can confirm a pass.
pub(super) fn trusted(
    receipt: &crate::execution::Receipt,
    task: &crate::tasks::Task,
) -> Option<(CheckEvidence, crate::tasks::Source)> {
    if receipt.task.as_deref() != Some(&task.id) {
        return None;
    }
    let (check, owner) = observation(receipt)?;
    task.members.contains(&owner).then_some((check, owner))
}

pub(super) fn observation(
    receipt: &crate::execution::Receipt,
) -> Option<(CheckEvidence, crate::tasks::Source)> {
    if receipt.kind != "observed_check" || receipt.phase != crate::execution::Phase::Completed {
        return None;
    }
    let check: CheckEvidence =
        serde_json::from_value(receipt.result.as_ref()?.get("check")?.clone()).ok()?;
    let owner: crate::tasks::Source =
        serde_json::from_value(receipt.input.get("owner")?.clone()).ok()?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&receipt.input).ok()?)
    );
    if fingerprint != receipt.fingerprint
        || receipt.input["id"] != check.id
        || receipt.input["source"] != check.source
        || receipt.input["plan"] != check.plan
        || receipt.input["before"] != serde_json::to_value(&check.before).ok()?
        || check.completed_at.is_none()
    {
        return None;
    }
    Some((check, owner))
}
