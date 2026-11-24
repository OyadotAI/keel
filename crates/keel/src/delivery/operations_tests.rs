//! Real task/worktree storage verifies correction, check and publication recovery through runtime seams.
use super::{
    Resolution, Service,
    batches::BatchInput,
    corrections::{CorrectionEdit, CorrectionInput},
    launch::LaunchPlan,
    publication::PublishInput,
    test_support::*,
};
use crate::{execution::Phase, lock::Locked};
use std::sync::{Arc, atomic::Ordering};

#[tokio::test]
async fn advisory_history_survives_restart_and_project_text_cannot_change_its_verdict() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let identity = f.service.task(&plan.task).unwrap().identity.unwrap();
    let review = super::ReviewEvidence {
        id: "review-1".into(),
        sha: identity.head,
        base: "main".into(),
        provider: "claude".into(),
        started_at: 1,
        completed_at: Some(2),
        verdict: "do not ship".into(),
        report: "Address the finding".into(),
        confirmed: true,
    };
    super::persist_review(&f.root, &plan.owner.lane, review).unwrap();
    f.service
        .records
        .change(|store| {
            store.records.get_mut(&plan.task).unwrap().reviews[0].verdict = "ship".into();
            Ok(())
        })
        .unwrap();
    let restarted = Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap();
    let saved = restarted.task(&plan.task).unwrap().record.reviews;
    assert_eq!(saved.len(), 1);
    assert!(saved[0].confirmed);
    assert_eq!(saved[0].verdict, "do not ship");
}
#[tokio::test]
async fn an_unfinished_advisory_review_never_becomes_a_confirmed_verdict() {
    let f = Fixture::new();
    let plan = task(&f).await;
    super::persist_review(
        &f.root,
        &plan.owner.lane,
        super::ReviewEvidence {
            id: "review-1".into(),
            sha: String::new(),
            base: String::new(),
            provider: "claude".into(),
            started_at: 1,
            completed_at: None,
            verdict: "waiting".into(),
            report: String::new(),
            confirmed: false,
        },
    )
    .unwrap();
    let restarted = Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap();
    assert!(!restarted.task(&plan.task).unwrap().record.reviews[0].confirmed);
}

#[tokio::test]
async fn queued_remote_feedback_requires_a_live_reconfirmation_of_its_saved_head() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let form = f.service.seed_form(&plan.task).unwrap();
    let view = f.service.task(&plan.task).unwrap();
    f.service
        .clone()
        .publish(
            plan.task.clone(),
            PublishInput {
                receipt: "publish".into(),
                revision: view.record.revision,
                owner: plan.owner.clone(),
                identity: view.identity.unwrap(),
                form,
            },
        )
        .await
        .unwrap();
    let feedback = super::Feedback {
        head: "a".repeat(40),
        fetched_at: 1,
        complete: true,
        checks: vec![super::RemoteCheck {
            id: "CI1".into(),
            source: "check_run".into(),
            name: "Checks".into(),
            sha: "a".repeat(40),
            status: "completed".into(),
            conclusion: Some("failure".into()),
            url: None,
        }],
        threads: vec![],
        errors: vec![],
    };
    let revision = f.service.task(&plan.task).unwrap().record.revision;
    f.service
        .merge_feedback(&plan.task, revision, feedback)
        .unwrap();
    let correction = f
        .service
        .task(&plan.task)
        .unwrap()
        .record
        .corrections
        .pop()
        .unwrap();
    let mut input = batch(&plan, &correction);
    input.remote_head = Some("a".repeat(40));
    *f.runtime.remote_head.locked() = Some("b".repeat(40));
    assert_eq!(
        f.service
            .clone()
            .send_batch(plan.task.clone(), input)
            .await
            .unwrap()
            .phase,
        Phase::FailedBeforeAcceptance
    );
    assert_eq!(f.runtime.calls.locked().len(), 1);
    assert_eq!(
        f.service.task(&plan.task).unwrap().record.corrections[0].resolution,
        Resolution::Unresolved
    );
}

async fn task(f: &Fixture) -> LaunchPlan {
    let receipt = f.service.clone().start_issue(f.issue()).await.unwrap();
    serde_json::from_value(receipt.result.unwrap()).unwrap()
}
fn add(f: &Fixture, task: &str) -> super::Correction {
    let revision = f.service.task(task).unwrap().record.revision;
    f.service
        .add_correction(
            task,
            CorrectionInput {
                revision,
                body: "Keep staged edits intact".into(),
                anchor: None,
            },
        )
        .unwrap()
}
fn batch(plan: &LaunchPlan, correction: &super::Correction) -> BatchInput {
    BatchInput {
        receipt: "batch-1".into(),
        owner: plan.owner.clone(),
        selected: [(correction.id.clone(), correction.revision)].into(),
        remote_head: None,
    }
}

#[tokio::test]
async fn sending_and_finishing_never_resolve_a_correction() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let correction = add(&f, &plan.task);
    let input = batch(&plan, &correction);
    f.service
        .clone()
        .send_batch(plan.task.clone(), input.clone())
        .await
        .unwrap();
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    assert_eq!(
        restarted
            .clone()
            .send_batch(plan.task.clone(), input)
            .await
            .unwrap()
            .phase,
        Phase::Completed
    );
    let saved = restarted.task(&plan.task).unwrap();
    assert_eq!(
        saved.record.corrections[0].resolution,
        Resolution::Unresolved
    );
    assert_eq!(saved.record.corrections[0].deliveries, vec!["batch-1"]);
    assert_eq!(f.runtime.calls.locked().len(), 2);
    let prompt = &f.runtime.calls.locked()[1].1.prompt;
    assert!(prompt.contains("Keep staged edits intact"));
}
#[tokio::test]
async fn queued_batches_wait_for_current_work_and_pending_requests() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let correction = add(&f, &plan.task);
    f.runtime.running.store(true, Ordering::SeqCst);
    assert_eq!(
        f.service
            .clone()
            .send_batch(plan.task.clone(), batch(&plan, &correction))
            .await
            .unwrap()
            .phase,
        Phase::Queued
    );
    f.runtime.running.store(false, Ordering::SeqCst);
    f.runtime.waiting.store(true, Ordering::SeqCst);
    assert_eq!(
        f.service.dispatch_batch("batch-1").await.unwrap().phase,
        Phase::Queued
    );
    assert_eq!(f.runtime.calls.locked().len(), 1);
    f.runtime.waiting.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service.dispatch_batch("batch-1").await.unwrap().phase,
        Phase::Accepted
    );
    assert_eq!(f.runtime.calls.locked().len(), 2);
}
#[tokio::test]
async fn editing_a_queued_version_prevents_its_old_dispatch() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let correction = add(&f, &plan.task);
    f.runtime.running.store(true, Ordering::SeqCst);
    f.service
        .clone()
        .send_batch(plan.task.clone(), batch(&plan, &correction))
        .await
        .unwrap();
    let revision = f.service.task(&plan.task).unwrap().record.revision;
    f.service
        .edit_correction(
            &plan.task,
            &correction.id,
            CorrectionEdit {
                revision,
                body: Some("New correction".into()),
                resolved: None,
            },
        )
        .unwrap();
    f.runtime.running.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service.dispatch_batch("batch-1").await.unwrap().phase,
        Phase::FailedBeforeAcceptance
    );
    assert_eq!(f.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn revoke_then_retrust_does_not_authorize_a_queued_batch() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let plan = task(&f).await;
    let correction = add(&f, &plan.task);
    f.runtime.running.store(true, Ordering::SeqCst);
    f.service
        .clone()
        .send_batch(plan.task.clone(), batch(&plan, &correction))
        .await
        .unwrap();
    crate::permissions::set_trusted(&f.root, false).unwrap();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    f.runtime.running.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service.dispatch_batch("batch-1").await.unwrap().phase,
        Phase::FailedBeforeAcceptance
    );
    assert_eq!(f.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn failed_remote_refresh_does_not_resolve_or_duplicate_prior_threads() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let thread = super::RemoteThread {
        id: "T1".into(),
        resolved: false,
        outdated: false,
        comments: vec![super::RemoteComment {
            id: "C1".into(),
            url: "https://github.com/owner/repo/pull/1#discussion_r1".into(),
            body: "Fix this line".into(),
            path: "README.md".into(),
            line: Some(1),
            original_line: Some(1),
            original_commit: None,
        }],
    };
    let mut feedback = super::Feedback {
        head: "a".repeat(40),
        fetched_at: 1000,
        complete: true,
        checks: vec![],
        threads: vec![thread],
        errors: vec![],
    };
    for _ in 0..2 {
        let revision = f.service.task(&plan.task).unwrap().record.revision;
        f.service
            .merge_feedback(&plan.task, revision, feedback.clone())
            .unwrap();
    }
    feedback.complete = false;
    feedback.errors = vec!["Partial read".into()];
    feedback.threads[0].resolved = true;
    let revision = f.service.task(&plan.task).unwrap().record.revision;
    f.service
        .merge_feedback(&plan.task, revision, feedback.clone())
        .unwrap();
    let saved = f.service.task(&plan.task).unwrap();
    assert_eq!(saved.record.corrections.len(), 1);
    assert_eq!(
        saved.record.corrections[0].resolution,
        Resolution::Unresolved
    );
    feedback.complete = true;
    feedback.errors.clear();
    f.service
        .merge_feedback(&plan.task, saved.record.revision, feedback)
        .unwrap();
    assert_eq!(
        f.service.task(&plan.task).unwrap().record.corrections[0].resolution,
        Resolution::RemoteResolved
    );
}
#[tokio::test]
async fn manual_check_receipts_recover_real_saved_evidence() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let input = super::CheckInput {
        receipt: "manual-1".into(),
        owner: plan.owner.clone(),
    };
    f.service
        .clone()
        .run_checks(plan.task.clone(), input.clone())
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let status = f.service.check_status("manual-1").await.unwrap();
            if status.phase == Phase::Completed {
                break status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result.phase, Phase::Completed);
    assert!(f.service.task(&plan.task).unwrap().checks_passed);
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    assert_eq!(
        restarted
            .clone()
            .run_checks(plan.task.clone(), input)
            .await
            .unwrap()
            .phase,
        Phase::Completed
    );
    assert!(restarted.task(&plan.task).unwrap().checks_passed);
    std::fs::write(
        camino::Utf8Path::new(&plan.owner.checkout).join("README.md"),
        "later edit",
    )
    .unwrap();
    assert!(!restarted.task(&plan.task).unwrap().checks_passed);
}
#[tokio::test]
async fn form_seeding_preserves_edits_and_publication_reuses_one_captured_intent() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let mut form = f.service.seed_form(&plan.task).unwrap();
    assert!(form.body.contains("https://github.com/owner/repo/issues/1"));
    assert!(form.body.contains("not recorded"));
    form.body = "Person's exact body".into();
    form.edited = true;
    let revision = f.service.task(&plan.task).unwrap().record.revision;
    f.service
        .save_form(
            &plan.task,
            super::FormInput {
                revision,
                form: form.clone(),
            },
        )
        .unwrap();
    assert_eq!(f.service.seed_form(&plan.task).unwrap().body, form.body);
    let view = f.service.task(&plan.task).unwrap();
    let input = PublishInput {
        receipt: "publication-1".into(),
        revision: view.record.revision,
        owner: plan.owner.clone(),
        identity: view.identity.unwrap(),
        form,
    };
    let first = f
        .service
        .clone()
        .publish(plan.task.clone(), input.clone())
        .await
        .unwrap();
    assert_eq!(first.phase, Phase::Completed);
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    let again = restarted
        .clone()
        .publish(plan.task.clone(), input)
        .await
        .unwrap();
    assert_eq!(first.result, again.result);
    assert_eq!(
        restarted
            .task(&plan.task)
            .unwrap()
            .record
            .publication
            .unwrap()
            .url
            .as_deref(),
        Some("https://github.com/owner/repo/pull/1")
    );
}
#[tokio::test]
async fn changed_checkout_cannot_retarget_a_new_publication() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let form = f.service.seed_form(&plan.task).unwrap();
    let view = f.service.task(&plan.task).unwrap();
    let input = PublishInput {
        receipt: "publication-1".into(),
        revision: view.record.revision,
        owner: plan.owner.clone(),
        identity: view.identity.unwrap(),
        form,
    };
    std::fs::write(
        camino::Utf8Path::new(&plan.owner.checkout).join("README.md"),
        "later edit",
    )
    .unwrap();
    assert!(f.service.clone().publish(plan.task, input).await.is_err());
}

#[tokio::test]
async fn mismatched_publication_keeps_the_pr_link_without_confirming_the_requested_commit() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let form = f.service.seed_form(&plan.task).unwrap();
    let view = f.service.task(&plan.task).unwrap();
    let input = PublishInput {
        receipt: "publication-1".into(),
        revision: view.record.revision,
        owner: plan.owner,
        identity: view.identity.unwrap(),
        form,
    };
    *f.runtime.published_head.locked() = Some("another-commit".into());
    let first = f
        .service
        .clone()
        .publish(plan.task.clone(), input.clone())
        .await
        .unwrap();
    assert_eq!(first.phase, Phase::Unconfirmed);
    assert_eq!(
        first.result.as_ref().unwrap()["url"],
        "https://github.com/owner/repo/pull/1"
    );
    assert_eq!(
        first.result.as_ref().unwrap()["actual_head"],
        "another-commit"
    );
    f.runtime.publish_fails.store(true, Ordering::SeqCst);
    let failed = f
        .service
        .clone()
        .publish(plan.task.clone(), input.clone())
        .await
        .unwrap();
    assert_eq!(failed.phase, Phase::Unconfirmed);
    assert_eq!(
        failed.result.as_ref().unwrap()["url"],
        "https://github.com/owner/repo/pull/1"
    );
    f.runtime.publish_fails.store(false, Ordering::SeqCst);
    *f.runtime.published_head.locked() = None;
    assert_eq!(
        f.service
            .clone()
            .publish(plan.task, input)
            .await
            .unwrap()
            .phase,
        Phase::Completed
    );
}

#[tokio::test]
async fn hosting_capture_failure_can_retry_without_mutating_the_form_revision() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let form = f.service.seed_form(&plan.task).unwrap();
    let view = f.service.task(&plan.task).unwrap();
    let input = PublishInput {
        receipt: "publication-1".into(),
        revision: view.record.revision,
        owner: plan.owner,
        identity: view.identity.unwrap(),
        form,
    };
    f.runtime.capture_fails.store(true, Ordering::SeqCst);
    assert!(
        f.service
            .clone()
            .publish(plan.task.clone(), input.clone())
            .await
            .is_err()
    );
    assert_eq!(
        f.service.task(&plan.task).unwrap().record.revision,
        input.revision
    );
    f.runtime.capture_fails.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service
            .clone()
            .publish(plan.task, input)
            .await
            .unwrap()
            .phase,
        Phase::Completed
    );
}

async fn publication(f: &Fixture, plan: &super::launch::LaunchPlan) -> super::Publication {
    let form = f.service.seed_form(&plan.task).unwrap();
    let view = f.service.task(&plan.task).unwrap();
    f.service
        .clone()
        .publish(
            plan.task.clone(),
            PublishInput {
                receipt: "publication-test".into(),
                revision: view.record.revision,
                owner: plan.owner.clone(),
                identity: view.identity.unwrap(),
                form,
            },
        )
        .await
        .unwrap();
    f.service
        .task(&plan.task)
        .unwrap()
        .record
        .publication
        .unwrap()
}
fn forge_publication(f: &Fixture, task: &str, mut publication: super::Publication) {
    publication.url = Some("https://github.com/private-org/private-repo/pull/99".into());
    publication.intent.repository = "private-org/private-repo".into();
    f.service
        .records
        .change(|store| {
            store.records.get_mut(task).unwrap().publication = Some(publication);
            Ok(())
        })
        .unwrap();
}
#[derive(Default)]
struct FeedbackReader(Vec<String>);
impl crate::github_read::Read for FeedbackReader {
    fn rest(&mut self, path: &str) -> Result<serde_json::Value, String> {
        self.0.push(path.into());
        Err("Controlled read failure".into())
    }
    fn graphql(&mut self, _: &str, _: serde_json::Value) -> Result<serde_json::Value, String> {
        panic!("No graph read expected")
    }
}
#[tokio::test]
async fn project_publication_tampering_cannot_redirect_privileged_feedback_reads() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let saved = publication(&f, &plan).await;
    forge_publication(&f, &plan.task, saved.clone());
    assert_eq!(
        f.service.feedback_target(&plan.task).unwrap(),
        saved.url.clone().unwrap()
    );
    assert_eq!(
        f.service
            .task(&plan.task)
            .unwrap()
            .record
            .publication
            .unwrap()
            .url,
        saved.url
    );
    let mut reader = FeedbackReader::default();
    assert!(f.service.refresh_feedback(&plan.task, &mut reader).is_err());
    assert_eq!(reader.0, vec!["repos/owner/repo/pulls/1"]);
}
#[tokio::test]
async fn a_project_only_publication_grants_no_feedback_read_or_confirmed_display() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let saved = publication(&f, &plan).await;
    forge_publication(&f, &plan.task, saved);
    f.service
        .execution
        .change(|store| {
            store
                .receipts
                .retain(|_, receipt| receipt.kind != "publication");
            Ok(())
        })
        .unwrap();
    let mut reader = FeedbackReader::default();
    assert!(f.service.refresh_feedback(&plan.task, &mut reader).is_err());
    assert!(reader.0.is_empty());
    assert!(
        f.service
            .task(&plan.task)
            .unwrap()
            .record
            .publication
            .is_none()
    );
    assert!(
        f.service.list(None, 100).unwrap().tasks[0]
            .publication
            .is_none()
    );
}
#[tokio::test]
async fn queued_remote_feedback_freezes_its_private_pr_association() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let saved = publication(&f, &plan).await;
    let view = f.service.task(&plan.task).unwrap();
    f.service
        .merge_feedback(
            &plan.task,
            view.record.revision,
            super::Feedback {
                head: "a".repeat(40),
                fetched_at: 1,
                complete: true,
                checks: vec![super::RemoteCheck {
                    id: "failed".into(),
                    source: "status".into(),
                    name: "failed".into(),
                    sha: "a".repeat(40),
                    status: "failure".into(),
                    conclusion: None,
                    url: None,
                }],
                threads: vec![],
                errors: vec![],
            },
        )
        .unwrap();
    let correction = f.service.task(&plan.task).unwrap().record.corrections[0].clone();
    let mut input = batch(&plan, &correction);
    input.remote_head = Some("a".repeat(40));
    *f.runtime.remote_head.locked() = input.remote_head.clone();
    f.runtime.waiting.store(true, Ordering::SeqCst);
    f.service
        .clone()
        .send_batch(plan.task.clone(), input.clone())
        .await
        .unwrap();
    forge_publication(&f, &plan.task, saved);
    f.runtime.waiting.store(false, Ordering::SeqCst);
    f.service.dispatch_batch(&input.receipt).await.unwrap();
    assert_eq!(
        *f.runtime.feedback_requests.locked(),
        vec![("owner/repo".into(), 1)]
    );
}
#[tokio::test]
async fn a_changed_private_pr_association_rejects_a_previously_queued_batch() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let saved = publication(&f, &plan).await;
    let correction = add(&f, &plan.task);
    let mut input = batch(&plan, &correction);
    input.remote_head = Some("a".repeat(40));
    f.runtime.waiting.store(true, Ordering::SeqCst);
    f.service
        .clone()
        .send_batch(plan.task.clone(), input.clone())
        .await
        .unwrap();
    f.service
        .execution
        .change(|store| {
            let receipt = store.receipts.get_mut(&saved.receipt).unwrap();
            let mut changed = saved.clone();
            changed.url = Some("https://github.com/owner/repo/pull/2".into());
            receipt.result = Some(serde_json::to_value(changed).unwrap());
            Ok(())
        })
        .unwrap();
    f.runtime.waiting.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service
            .dispatch_batch(&input.receipt)
            .await
            .unwrap()
            .phase,
        Phase::FailedBeforeAcceptance
    );
    assert!(f.runtime.feedback_requests.locked().is_empty());
}

#[tokio::test]
async fn failed_legacy_statuses_become_unresolved_corrections_without_a_check_run_conclusion() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let view = f.service.task(&plan.task).unwrap();
    let feedback = super::Feedback {
        head: "a".repeat(40),
        fetched_at: 1,
        complete: true,
        checks: ["failure", "error"]
            .into_iter()
            .map(|status| super::RemoteCheck {
                id: status.into(),
                source: "status".into(),
                name: status.into(),
                sha: "a".repeat(40),
                status: status.into(),
                conclusion: None,
                url: None,
            })
            .collect(),
        threads: vec![],
        errors: vec![],
    };
    f.service
        .merge_feedback(&plan.task, view.record.revision, feedback)
        .unwrap();
    let corrections = f.service.task(&plan.task).unwrap().record.corrections;
    assert_eq!(corrections.len(), 2);
    assert!(
        corrections
            .iter()
            .all(|c| c.resolution == Resolution::Unresolved)
    );
}
#[tokio::test]
async fn session_rebinding_preserves_private_proof_source_aliases() {
    let f = Fixture::new();
    let plan = task(&f).await;
    let checkout = camino::Utf8Path::new(&plan.owner.checkout);
    let mut check = super::begin_check(checkout, "turn".into(), None).unwrap();
    super::end_check(checkout, &mut check, "passed", None, vec![]);
    super::persist_check(&f.root, &plan.owner.session, checkout, &check).unwrap();
    crate::tasks::session(&f.root, &plan.owner.lane, "provider-session").unwrap();
    let saved = crate::tasks::get(&f.root, &plan.task).unwrap();
    assert!(saved.members.contains(&plan.owner));
    assert_eq!(saved.owner.session, "provider-session");
    assert_eq!(
        crate::tasks::register(&f.root, plan.owner.clone()).unwrap(),
        plan.task
    );
    assert_eq!(
        crate::tasks::get(&f.root, &plan.task)
            .unwrap()
            .owner
            .session,
        "provider-session"
    );
    assert!(f.service.task(&plan.task).unwrap().checks_passed);
}
