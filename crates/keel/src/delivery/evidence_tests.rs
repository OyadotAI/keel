//! Evidence tests distinguish exact checked contents, successor commits and untrusted project claims.
use super::{capture, evidence::*, test_support::*};
use crate::lock::Locked;

fn checked(root: &camino::Utf8Path) -> super::CheckEvidence {
    let mut check =
        begin_check(root, "controlled-check".into(), Some("make check".into())).unwrap();
    end_check(
        root,
        &mut check,
        "passed",
        Some("make check".into()),
        vec![],
    );
    check
}
#[test]
fn editing_at_the_same_head_revokes_current_pass_currency() {
    let fixture = Fixture::new();
    let check = checked(&fixture.root);
    let before = capture(&fixture.root).unwrap();
    let plan = plan_hash(&fixture.root).unwrap();
    assert!(current(&check, &before, &plan));
    std::fs::write(fixture.root.join("README.md"), "new contents\n").unwrap();
    let after = capture(&fixture.root).unwrap();
    assert_eq!(before.head, after.head);
    assert!(!current(&check, &after, &plan));
}
#[test]
fn content_changes_during_checks_are_not_a_current_pass() {
    let fixture = Fixture::new();
    let mut check = begin_check(&fixture.root, "controlled-check".into(), None).unwrap();
    std::fs::write(fixture.root.join("README.md"), "changed during check\n").unwrap();
    end_check(&fixture.root, &mut check, "passed", None, vec![]);
    assert!(!current(
        &check,
        &capture(&fixture.root).unwrap(),
        &plan_hash(&fixture.root).unwrap()
    ));
}
#[test]
fn failed_identity_capture_and_changed_check_plans_never_become_current_passes() {
    let fixture = Fixture::new();
    let mut check = checked(&fixture.root);
    let identity = capture(&fixture.root).unwrap();
    assert!(!current(&check, &identity, "another-plan"));
    check.before = None;
    assert!(!current(
        &check,
        &identity,
        &plan_hash(&fixture.root).unwrap()
    ));
}
#[test]
fn capture_preserves_the_persons_staged_version() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("README.md"), "staged\n").unwrap();
    git(&fixture.root, &["add", "README.md"]);
    std::fs::write(fixture.root.join("README.md"), "working\n").unwrap();
    let before = git(&fixture.root, &["diff", "--cached"]);
    capture(&fixture.root).unwrap();
    assert_eq!(git(&fixture.root, &["diff", "--cached"]), before);
}
#[test]
fn only_a_known_unchanged_checkpoint_is_a_valid_successor_binding() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("README.md"), "tested change\n").unwrap();
    let mut check = checked(&fixture.root);
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
            "checkpoint",
        ],
    );
    let identity = capture(&fixture.root).unwrap();
    let plan = plan_hash(&fixture.root).unwrap();
    assert!(!current(&check, &identity, &plan));
    assert!(bind_successor(&fixture.root, &mut check, &identity.head));
    assert!(current(&check, &identity, &plan));
    git(
        &fixture.root,
        &[
            "-c",
            "user.name=Delivery tests",
            "-c",
            "user.email=delivery@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "another commit",
        ],
    );
    assert!(!current(&check, &capture(&fixture.root).unwrap(), &plan));
}
#[tokio::test]
async fn project_records_cannot_manufacture_current_machine_evidence() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let plan: super::launch::LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    let checkout = camino::Utf8Path::new(&plan.owner.checkout);
    let check = checked(checkout);
    fixture
        .service
        .records
        .change(|store| {
            store
                .records
                .get_mut(&plan.task)
                .unwrap()
                .checks
                .push(check.clone());
            Ok(())
        })
        .unwrap();
    let view = fixture.service.task(&plan.task).unwrap();
    assert!(!view.checks_passed);
    assert!(view.evidence_error.is_some());
    persist_check(&fixture.service.root, &plan.owner.session, checkout, &check).unwrap();
    assert!(fixture.service.task(&plan.task).unwrap().checks_passed);
}
#[tokio::test]
async fn private_observations_override_a_forged_project_pass() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let plan: super::launch::LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    let checkout = camino::Utf8Path::new(&plan.owner.checkout);
    let mut check = checked(checkout);
    check.status = "failed".into();
    persist_check(&fixture.service.root, &plan.owner.session, checkout, &check).unwrap();
    fixture
        .service
        .records
        .change(|store| {
            store
                .records
                .get_mut(&plan.task)
                .unwrap()
                .checks
                .last_mut()
                .unwrap()
                .status = "passed".into();
            Ok(())
        })
        .unwrap();
    let view = fixture.service.task(&plan.task).unwrap();
    assert_eq!(view.record.checks.last().unwrap().status, "failed");
    assert!(view.checks_current);
    assert!(!view.checks_passed);
}
#[tokio::test]
async fn legacy_gates_without_execution_time_identity_remain_unconfirmed() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let plan: super::launch::LaunchPlan = serde_json::from_value(receipt.result.unwrap()).unwrap();
    crate::turns::emit(
        &fixture.service.root,
        &plan.owner.session,
        "legacy-turn",
        crate::turns::Fact::Gate {
            status: "passed".into(),
            command: Some("make check".into()),
            ms: Some(1),
            problems: vec![],
            evidence: None,
        },
    );
    let view = fixture.service.task(&plan.task).unwrap();
    assert!(!view.checks_current);
    assert_eq!(view.legacy_gate.unwrap().status, "passed");
}
#[test]
fn recording_checks_does_not_monopolize_an_ordinary_shared_root_checkout() {
    let fixture = Fixture::new();
    let source = crate::tasks::Source {
        lane: "root-one".into(),
        provider: "codex".into(),
        session: "root-one".into(),
        checkout: fixture.service.root.to_string(),
        wt: None,
    };
    crate::tasks::register(&fixture.service.root, source.clone()).unwrap();
    persist_check(
        &fixture.service.root,
        &source.session,
        &fixture.service.root,
        &checked(&fixture.root),
    )
    .unwrap();
    let mut next = source;
    next.lane = "root-two".into();
    next.session = "root-two".into();
    assert!(crate::tasks::register(&fixture.service.root, next).is_ok());
    assert!(fixture.runtime.calls.locked().is_empty());
}
