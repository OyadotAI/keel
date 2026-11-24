//! Claimed manual checks run real scratch commands and reject late cancellation of newer work.
use super::owned::check_owned;
use crate::{
    delivery::DeliveryFixture as Fixture,
    serve::{AppState, Held},
};
use std::sync::Arc;

#[tokio::test]
async fn claimed_manual_checks_persist_real_results_and_release_their_writer() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let task = receipt.task.unwrap();
    let source = fixture.service.task(&task).unwrap().owner.unwrap();
    let state = Arc::new(AppState::new(fixture.root.clone()));
    let result = check_owned(
        state.clone(),
        source.checkout.clone().into(),
        Some(source.lane.clone()),
        Some("owned-check".into()),
        |_| {},
        || false,
    )
    .await
    .unwrap();
    assert_eq!(result.status, "passed");
    assert!(result.completed_at.is_some());
    assert!(!state.has_turn(&source.lane));
    assert!(fixture.service.task(&task).unwrap().checks_passed);
    let token = state
        .claim(&source.lane, camino::Utf8Path::new(&source.checkout), true)
        .unwrap();
    let _held = Held::new(state.clone(), source.lane.clone(), token);
    assert!(!state.stop_check("owned-check"));
    assert!(!state.was_interrupted(&source.lane, token));
    assert!(!state.checks_stopped(&source.lane, token));
}

#[tokio::test]
async fn a_check_that_changes_its_own_inputs_does_not_certify_the_resulting_checkout() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let task = receipt.task.unwrap();
    let source = fixture.service.task(&task).unwrap().owner.unwrap();
    std::fs::write(
        camino::Utf8Path::new(&source.checkout).join("Makefile"),
        "check:\n\tprintf changed > README.md\n",
    )
    .unwrap();
    let state = Arc::new(AppState::new(fixture.root.clone()));
    let result = check_owned(
        state,
        source.checkout.clone().into(),
        Some(source.lane),
        Some("changing-check".into()),
        |_| {},
        || false,
    )
    .await
    .unwrap();
    assert_eq!(result.status, "passed");
    assert_ne!(result.before, result.after);
    assert!(!fixture.service.task(&task).unwrap().checks_passed);
}

#[tokio::test]
async fn a_busy_checkout_is_refused_without_replacing_the_existing_claim() {
    let fixture = Fixture::new();
    let receipt = fixture
        .service
        .clone()
        .start_issue(fixture.issue())
        .await
        .unwrap();
    let source = fixture
        .service
        .task(&receipt.task.unwrap())
        .unwrap()
        .owner
        .unwrap();
    let state = Arc::new(AppState::new(fixture.root.clone()));
    let token = state
        .claim(&source.lane, camino::Utf8Path::new(&source.checkout), true)
        .unwrap();
    let _held = Held::new(state.clone(), source.lane.clone(), token);
    assert!(
        check_owned(
            state.clone(),
            source.checkout.into(),
            Some(source.lane.clone()),
            None,
            |_| {},
            || false
        )
        .await
        .is_err()
    );
    assert!(state.has_turn(&source.lane));
    assert!(!state.was_interrupted(&source.lane, token));
}
