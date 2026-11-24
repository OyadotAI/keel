//! Scratch failures pin restart quarantine, visible capacity halts and active-stop recovery.
use super::*;
use crate::{delivery::test_support::Fixture, lock::Locked};
use std::{collections::BTreeMap, sync::atomic::Ordering};
async fn schedule(f: &Fixture, automatic: bool) -> Schedule {
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let saved = f
        .service
        .save_schedule(ScheduleInput {
            id: None,
            revision: None,
            name: "Recovery".into(),
            prompt: "Inspect checks".into(),
            provider: "codex".into(),
            base: "main".into(),
            interval: 60,
            duration: 60,
        })
        .unwrap();
    if automatic {
        f.service
            .clone()
            .schedule_action(
                &saved.id,
                ScheduleAction {
                    action: "enable".into(),
                    revision: saved.revision,
                    receipt: None,
                },
            )
            .await
            .unwrap();
    }
    f.service.schedules().unwrap().schedules[0].clone()
}
fn execution_lease(f: &Fixture) -> crate::permissions::Lease {
    crate::permissions::try_lease(&format!(
        "execution-{}.lock",
        crate::permissions::key_of(&f.root).unwrap()
    ))
    .unwrap()
    .unwrap()
}
async fn reserve(f: &Fixture, s: &Schedule) {
    f.service
        .clone()
        .schedule_action(
            &s.id,
            ScheduleAction {
                action: "run_now".into(),
                revision: s.revision,
                receipt: Some("active".into()),
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn unreadable_startup_state_quarantines_the_first_recovered_active_slot() {
    let f = Fixture::new();
    let s = schedule(&f, false).await;
    reserve(&f, &s).await;
    let lease = execution_lease(&f);
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    assert!(restarted.initial_active.locked().is_none());
    drop(lease);
    let mut first = true;
    restarted
        .recover_tick(&mut BTreeMap::new(), &mut first)
        .await;
    let page = restarted.schedules().unwrap();
    assert_eq!(page.active.as_deref(), Some("active"));
    assert_eq!(page.active_occurrence.unwrap().phase, Phase::Unconfirmed);
    assert!(f.runtime.calls.locked().is_empty());
}
#[tokio::test]
async fn a_failed_first_tick_keeps_restart_quarantine_until_reconciliation_succeeds() {
    let f = Fixture::new();
    let s = schedule(&f, false).await;
    reserve(&f, &s).await;
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    let lease = execution_lease(&f);
    let mut first = true;
    let mut started = BTreeMap::new();
    restarted.recover_tick(&mut started, &mut first).await;
    assert!(first);
    drop(lease);
    restarted.recover_tick(&mut started, &mut first).await;
    assert!(!first);
    assert_eq!(
        restarted
            .schedules()
            .unwrap()
            .active_occurrence
            .unwrap()
            .phase,
        Phase::Unconfirmed
    );
    restarted.recover_tick(&mut started, &mut first).await;
    assert!(f.runtime.calls.locked().is_empty());
}
#[tokio::test]
async fn a_confirmed_empty_startup_still_allows_a_new_manual_reservation() {
    let f = Fixture::new();
    let s = schedule(&f, false).await;
    reserve(&f, &s).await;
    let mut first = true;
    f.service
        .recover_tick(&mut BTreeMap::new(), &mut first)
        .await;
    assert_eq!(f.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn exhausted_occurrence_receipt_and_byte_capacity_latch_visible_automation_halts() {
    for kind in ["occurrences", "receipts", "bytes"] {
        let f = Fixture::new();
        schedule(&f, true).await;
        let sample = f
            .service
            .execution
            .reserve("seed", "seed", serde_json::json!({}), 1)
            .unwrap();
        f.service
            .execution
            .change(|store| {
                if kind == "occurrences" {
                    for index in 0..crate::execution::OCCURRENCE_LIMIT {
                        let id = format!("old-{index}");
                        store.occurrences.insert(
                            id.clone(),
                            Occurrence {
                                id: id.clone(),
                                schedule: "old".into(),
                                revision: 0,
                                due_at: 1,
                                receipt: id,
                                task: None,
                                phase: Phase::Stopped,
                                started_at: None,
                                deadline: None,
                                finished_at: Some(1),
                                reason: None,
                            },
                        );
                    }
                } else if kind == "receipts" {
                    for index in 1..crate::execution::RECEIPT_LIMIT {
                        let mut receipt = sample.clone();
                        receipt.id = format!("receipt-{index}");
                        store.receipts.insert(receipt.id.clone(), receipt);
                    }
                } else {
                    let length = serde_json::to_vec(store).unwrap().len();
                    store.receipts.get_mut("seed").unwrap().input = serde_json::json!(
                        "x".repeat(crate::execution::STORE_BYTES as usize - length - 32)
                    );
                }
                Ok(())
            })
            .unwrap();
        f.clock.0.store(61_000, Ordering::SeqCst);
        let mut started = BTreeMap::new();
        assert!(f.service.tick(&mut started, false).await.is_err(), "{kind}");
        assert!(f.service.schedules().unwrap().controller_error.is_some());
        assert!(f.service.retry_dispatcher().is_err());
        let before = f.service.execution.read().unwrap();
        f.service.tick(&mut started, false).await.unwrap();
        let after = f.service.execution.read().unwrap();
        assert_eq!(before.receipts.len(), after.receipts.len());
        assert_eq!(before.occurrences.len(), after.occurrences.len());
        assert!(f.runtime.calls.locked().is_empty());
        f.service
            .execution
            .change(|store| {
                store.occurrences.clear();
                store.receipts.retain(|id, _| id == "seed");
                store.receipts.get_mut("seed").unwrap().input = serde_json::json!({});
                Ok(())
            })
            .unwrap();
        f.service.retry_dispatcher().unwrap();
        let page = f.service.schedules().unwrap();
        assert!(page.controller_error.is_none());
        assert_eq!(page.schedules[0].next_at, Some(121_000));
        f.service.tick(&mut started, false).await.unwrap();
        assert!(f.runtime.calls.locked().is_empty());
    }
}
#[tokio::test]
async fn write_failure_halts_new_runs_but_duration_stop_and_uncertain_slots_remain_owned() {
    let f = Fixture::new();
    let s = schedule(&f, false).await;
    f.runtime.running.store(true, Ordering::SeqCst);
    reserve(&f, &s).await;
    let mut started = BTreeMap::new();
    f.service.tick(&mut started, false).await.unwrap();
    f.service.execution.block_writes(true);
    f.clock.0.store(61_001, Ordering::SeqCst);
    assert!(f.service.tick(&mut started, false).await.is_err());
    assert!(!f.runtime.running.load(Ordering::SeqCst));
    assert!(f.service.schedules().unwrap().controller_error.is_some());
    assert!(f.service.schedules().unwrap().active.is_some());
    assert!(f.service.retry_dispatcher().is_err());
    f.service.execution.block_writes(false);
    f.service.retry_dispatcher().unwrap();
    assert!(f.service.schedules().unwrap().active.is_some());
    f.service.tick(&mut started, false).await.unwrap();
    f.service.tick(&mut started, false).await.unwrap();
    assert!(f.service.schedules().unwrap().active.is_none());
}

#[tokio::test]
async fn a_non_owner_daemon_reads_and_recovers_the_shared_dispatcher_halt() {
    let f = Fixture::new();
    let s = schedule(&f, true).await;
    let other = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    f.service.execution.block_writes(true);
    f.clock.0.store(61_000, Ordering::SeqCst);
    assert!(f.service.tick(&mut BTreeMap::new(), false).await.is_err());
    assert!(other.schedules().unwrap().controller_error.is_some());
    f.service.execution.block_writes(false);
    other
        .clone()
        .schedule_action(
            "dispatcher",
            ScheduleAction {
                action: "retry".into(),
                revision: s.revision,
                receipt: None,
            },
        )
        .await
        .unwrap();
    assert!(f.service.schedules().unwrap().controller_error.is_none());
    assert!(other.schedules().unwrap().controller_error.is_none());
    f.service.tick(&mut BTreeMap::new(), false).await.unwrap();
    assert!(f.runtime.calls.locked().is_empty());
    f.clock.0.store(121_000, Ordering::SeqCst);
    f.service.tick(&mut BTreeMap::new(), false).await.unwrap();
    assert_eq!(f.runtime.calls.locked().len(), 1);
}

#[test]
fn a_delayed_old_status_cannot_clear_a_newer_local_halt() {
    let f = Fixture::new();
    let first = f
        .service
        .execution
        .halt_dispatcher("First halt".into())
        .unwrap();
    let old_clear = f.service.execution.retry_dispatcher(|| Ok(())).unwrap();
    let latest = f
        .service
        .execution
        .halt_dispatcher("New halt".into())
        .unwrap();
    f.service.apply_controller_status(first);
    f.service.apply_controller_status(latest.clone());
    f.service.apply_controller_status(old_clear);
    assert_eq!(
        f.service.controller_error.locked().as_deref(),
        Some("New halt")
    );
    assert_eq!(
        *f.service.controller_revision.locked(),
        Some(latest.revision)
    );
}
