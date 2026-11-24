//! Launch tests use real isolated Git checkouts and persistent receipts with a controlled provider seam.
use super::{Service, error::Failure, launch::LaunchPlan, test_support::*};
use crate::{execution::Phase, lock::Locked};
use std::sync::{Arc, atomic::Ordering};

#[tokio::test]
async fn one_issue_launch_keeps_the_root_checkout_and_pins_its_original_task() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    let before = git(&fixture.root, &["rev-parse", "HEAD"]);
    let receipt = fixture.service.clone().start_issue(input).await.unwrap();
    assert_eq!(receipt.phase, Phase::Accepted);
    let plan: LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    assert_eq!(git(&fixture.root, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        git(
            camino::Utf8Path::new(&plan.owner.checkout),
            &["rev-parse", "HEAD"]
        ),
        before
    );
    assert!(crate::tasks::get(&fixture.root, &plan.task).unwrap().pinned);
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
    assert!(fixture.service.task(&plan.task).unwrap().owner.is_some());
    assert!(git(&fixture.root, &["status", "--porcelain"]).is_empty());
}
#[tokio::test]
async fn retry_after_a_lost_response_and_service_restart_never_submits_twice() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    fixture
        .runtime
        .error_after_accept
        .store(true, Ordering::SeqCst);
    let first = fixture
        .service
        .clone()
        .start_issue(input.clone())
        .await
        .unwrap();
    assert_eq!(first.phase, Phase::Accepted);
    let restarted = Arc::new(
        Service::new(
            &fixture.root,
            fixture.runtime.clone(),
            fixture.clock.clone(),
        )
        .unwrap(),
    );
    let repeated = restarted.start_issue(input).await.unwrap();
    assert_eq!(first.task, repeated.task);
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn uncertain_acceptance_is_reconciled_without_redispatch() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    fixture
        .runtime
        .error_after_accept
        .store(true, Ordering::SeqCst);
    fixture.runtime.unknown.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .clone()
            .start_issue(input.clone())
            .await
            .unwrap()
            .phase,
        Phase::Unconfirmed
    );
    assert_eq!(
        fixture
            .service
            .clone()
            .start_issue(input.clone())
            .await
            .unwrap()
            .phase,
        Phase::Unconfirmed
    );
    fixture.runtime.unknown.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .launch_status(&input.receipt)
            .await
            .unwrap()
            .phase,
        Phase::Completed
    );
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn changed_inputs_on_the_same_launch_receipt_are_a_conflict() {
    let fixture = Fixture::new();
    let mut input = fixture.issue();
    fixture
        .service
        .clone()
        .start_issue(input.clone())
        .await
        .unwrap();
    input.provider = "claude".into();
    assert!(matches!(
        fixture.service.clone().start_issue(input).await,
        Err(Failure::Conflict(_))
    ));
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn an_unavailable_provider_starts_no_worktree_or_runtime() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    fixture.runtime.unavailable.store(true, Ordering::SeqCst);
    let receipt = fixture.service.clone().start_issue(input).await.unwrap();
    assert_eq!(receipt.phase, Phase::FailedBeforeAcceptance);
    assert!(!fixture.root.join(crate::worktree::DIR).exists());
    assert!(fixture.runtime.calls.locked().is_empty());
}
#[tokio::test]
async fn concurrent_repeated_starts_register_only_one_writer() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    let (first, second) = tokio::join!(
        fixture.service.clone().start_issue(input.clone()),
        fixture.service.clone().start_issue(input.clone())
    );
    assert!(first.is_ok() || second.is_ok());
    fixture.service.clone().start_issue(input).await.unwrap();
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
    assert_eq!(fixture.service.records.read().unwrap().records.len(), 1);
}
#[tokio::test]
async fn a_prepared_launch_retains_its_captured_base_after_the_branch_moves() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    let original = git(&fixture.root, &["rev-parse", "HEAD"]);
    let lane = "prepared-source";
    let wt = "task-prepared";
    let source = crate::tasks::Source {
        lane: lane.into(),
        session: lane.into(),
        provider: input.provider.clone(),
        checkout: crate::worktree::path_of(&fixture.service.root, wt)
            .unwrap()
            .to_string(),
        wt: Some(wt.into()),
    };
    let preview = fixture.service.records.read().unwrap().previews[&input.preview].clone();
    let plan = LaunchPlan {
        task: crate::tasks::key_for(&source),
        owner: source,
        base: "main".into(),
        sha: original.clone(),
        title: preview.issue.title.clone(),
        source: super::Provenance::Issue {
            snapshot: preview.issue,
        },
        prompt: "saved prompt".into(),
        dispatched: false,
        preview: Some(input.preview.clone()),
        warnings: vec![],
    };
    fixture
        .service
        .execution
        .reserve(
            &input.receipt,
            "issue",
            serde_json::to_value(&input).unwrap(),
            1000,
        )
        .unwrap();
    fixture
        .service
        .execution
        .change(|store| {
            let receipt = store.receipts.get_mut(&input.receipt).unwrap();
            receipt.phase = Phase::Preparing;
            receipt.result = Some(serde_json::to_value(&plan).unwrap());
            Ok(())
        })
        .unwrap();
    std::fs::write(fixture.root.join("README.md"), "moved base\n").unwrap();
    git(&fixture.root, &["add", "README.md"]);
    git(
        &fixture.root,
        &[
            "-c",
            "user.name=Delivery tests",
            "-c",
            "user.email=delivery@example.invalid",
            "commit",
            "-qm",
            "move base",
        ],
    );
    let moved = git(&fixture.root, &["rev-parse", "HEAD"]);
    let receipt = fixture.service.clone().start_issue(input).await.unwrap();
    assert_eq!(receipt.phase, Phase::Accepted, "{:?}", receipt.error);
    assert_ne!(original, moved);
    assert_eq!(
        git(
            camino::Utf8Path::new(&plan.owner.checkout),
            &["rev-parse", "HEAD"]
        ),
        original
    );
    assert_eq!(git(&fixture.root, &["rev-parse", "HEAD"]), moved);
}
#[tokio::test]
async fn a_saved_task_survives_markdown_projection_failure_without_duplicate_registration() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    std::fs::create_dir(fixture.root.join(".keel/CHANGELOG.md")).unwrap();
    let receipt = fixture.service.clone().start_issue(input).await.unwrap();
    assert_eq!(receipt.phase, Phase::Accepted);
    let plan: LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    assert!(!plan.warnings.is_empty());
    assert!(crate::tasks::get(&fixture.root, &plan.task).unwrap().pinned);
    assert_eq!(fixture.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn damaged_advisory_notes_cannot_make_a_managed_checkout_unowned() {
    let fixture = Fixture::new();
    let input = fixture.issue();
    let receipt = fixture.service.clone().start_issue(input).await.unwrap();
    let plan: LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    std::fs::write(fixture.root.join(".keel/tasks/state.json"), "damaged notes").unwrap();
    let mut other = plan.owner;
    other.lane = "unrelated".into();
    other.session = "unrelated".into();
    assert!(crate::tasks::register(&fixture.root, other).is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.root.join(".keel/tasks/state.json")).unwrap(),
        "damaged notes"
    );
    assert!(
        fixture
            .service
            .task(&plan.task)
            .unwrap()
            .owner_error
            .is_some()
    );
}
