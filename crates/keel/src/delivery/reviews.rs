//! Advisory history is captured at its real lifecycle and confirmed through private host receipts.
use super::{
    Provenance, Record, ReviewEvidence,
    records::blank_record,
    repository::Repository,
    runtime::{Clock, SystemClock},
};
use camino::Utf8Path;
use sha2::{Digest, Sha256};

/// A review completing without a window still saves its commit, base and advisory report.
pub(crate) fn persist_review(
    root: &Utf8Path,
    lane: &str,
    review: ReviewEvidence,
) -> Result<(), String> {
    let Some(id) = crate::tasks::for_lane(root, lane)? else {
        return Ok(());
    };
    let task = crate::tasks::get(root, &id)?;
    let control = crate::execution::Repository::new(root)?;
    let proof = format!("review-{:x}", Sha256::digest(review.id.as_bytes()));
    let input = control
        .read()?
        .receipts
        .get(&proof)
        .map(|receipt| receipt.input.clone())
        .unwrap_or_else(|| serde_json::json!({"id":review.id,"owner":task.owner}));
    control
        .reserve(&proof, "observed_review", input, review.started_at)
        .map_err(|_| "Review proof could not be reserved.")?;
    control.change(|store| {
        let proof = store
            .receipts
            .get_mut(&proof)
            .ok_or("Review proof unavailable.")?;
        proof.task = Some(id.clone());
        proof.result = Some(serde_json::to_value(&review).map_err(|e| e.to_string())?);
        proof.phase = if review.completed_at.is_some() {
            crate::execution::Phase::Completed
        } else {
            crate::execution::Phase::Running
        };
        proof.updated_at = SystemClock.now();
        Ok(())
    })?;
    crate::tasks::pin(root, &id)?;
    let repository = Repository::new(root)?;
    repository.change(|store| {
        let record = store.records.entry(id.clone()).or_insert_with(|| {
            blank_record(
                &id,
                task.objective.clone(),
                task.owner.clone(),
                Provenance::Conversation,
                (
                    crate::pr::base_for(root, Utf8Path::new(&task.owner.checkout)),
                    String::new(),
                    String::new(),
                ),
                SystemClock.now(),
            )
        });
        merge(record, review);
        record.revision = record.revision.saturating_add(1);
        record.updated_at = SystemClock.now();
        Ok(())
    })?;
    crate::events::emit(
        "delivery.changed",
        task.owner.wt.as_deref(),
        serde_json::json!({"task":id}),
    );
    Ok(())
}
pub(super) fn merge(record: &mut Record, review: ReviewEvidence) {
    if let Some(old) = record.reviews.iter_mut().find(|old| old.id == review.id) {
        *old = review;
    } else {
        record.reviews.push(review);
    }
    record.reviews.sort_by_key(|review| review.started_at);
}
/// Project text can retain a report, but cannot manufacture a confirmed advisory verdict.
pub(super) fn reconcile(
    record: &mut Record,
    task: &crate::tasks::Task,
    store: &crate::execution::Store,
) {
    for review in &mut record.reviews {
        review.confirmed = false;
    }
    for receipt in store
        .receipts
        .values()
        .filter(|r| r.kind == "observed_review" && r.task.as_deref() == Some(&task.id))
    {
        let Some(value) = receipt.result.clone() else {
            continue;
        };
        let Ok(mut review) = serde_json::from_value::<ReviewEvidence>(value) else {
            continue;
        };
        let fingerprint = serde_json::to_vec(&receipt.input)
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
            .unwrap_or_default();
        let owner =
            serde_json::from_value::<crate::tasks::Source>(receipt.input["owner"].clone()).ok();
        let trusted = fingerprint == receipt.fingerprint
            && receipt.input["id"] == review.id
            && owner.is_some_and(|owner| task.members.contains(&owner));
        review.confirmed = trusted
            && receipt.phase == crate::execution::Phase::Completed
            && review.completed_at.is_some();
        merge(record, review);
    }
}
