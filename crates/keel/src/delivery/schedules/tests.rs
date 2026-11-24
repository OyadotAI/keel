//! Fake clocks and real repositories pin recurring ownership, no-replay and revocation rules.
use super::*;
use crate::delivery::test_support::Fixture;
use crate::lock::Locked;
use std::{collections::BTreeMap, sync::atomic::Ordering};

fn definition(f: &Fixture, name: &str) -> Schedule {
    f.service
        .save_schedule(ScheduleInput {
            id: None,
            revision: None,
            name: name.into(),
            prompt: "Inspect the repository and report actual checks.".into(),
            provider: "codex".into(),
            base: "main".into(),
            interval: 60,
            duration: 60,
        })
        .unwrap()
}

#[test]
fn clock_rollback_and_host_gaps_are_distinct_from_an_ordinary_tick() {
    assert!(super::cadence::shifted(10_000, 9_000));
    assert!(super::cadence::shifted(1_000, 40_000));
    assert!(!super::cadence::shifted(1_000, 3_000));
}
#[tokio::test]
async fn cadence_reanchoring_records_one_skip_and_preserves_an_active_slot() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let schedule = definition(&f, "Clock check");
    action(&f, &schedule, "enable", None).await;
    let enabled = f.service.schedules().unwrap().schedules[0].clone();
    action(&f, &enabled, "run_now", Some("now-1")).await;
    f.clock.0.store(10, Ordering::SeqCst);
    f.service.reanchor().unwrap();
    let page = f.service.schedules().unwrap();
    assert_eq!(page.active.as_deref(), Some("now-1"));
    assert_eq!(page.occurrences.len(), 2);
    assert_eq!(page.schedules[0].next_at, Some(60_010));
    assert!(f.runtime.calls.locked().is_empty());
}
async fn action(f: &Fixture, schedule: &Schedule, name: &str, receipt: Option<&str>) {
    f.service
        .clone()
        .schedule_action(
            &schedule.id,
            ScheduleAction {
                action: name.into(),
                revision: schedule.revision,
                receipt: receipt.map(str::to_string),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn save_is_paused_and_enable_sets_a_future_tick() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let schedule = definition(&f, "Daily check");
    assert!(!schedule.enabled);
    assert!(schedule.next_at.is_none());
    action(&f, &schedule, "enable", None).await;
    let enabled = f.service.schedules().unwrap().schedules.pop().unwrap();
    assert!(enabled.enabled);
    assert_eq!(enabled.next_at, Some(61_000));
    crate::permissions::remember(&f.root, "Bash(sh *)", "project", None).unwrap();
    let mut started = BTreeMap::new();
    f.service.tick(&mut started, false).await.unwrap();
    assert!(f.runtime.calls.locked().is_empty());
    assert!(f.service.schedules().unwrap().schedules[0].enabled);
    f.clock.0.store(61_000, Ordering::SeqCst);
    f.service.tick(&mut started, false).await.unwrap();
    assert_eq!(f.runtime.calls.locked().len(), 1);
    assert!(f.service.schedules().unwrap().active.is_some());
}
#[tokio::test]
async fn run_now_on_a_paused_definition_is_idempotent_and_isolated() {
    let f = Fixture::new();
    let schedule = definition(&f, "Manual check");
    action(&f, &schedule, "run_now", Some("now-1")).await;
    let mut started = BTreeMap::new();
    f.service.tick(&mut started, false).await.unwrap();
    action(&f, &schedule, "run_now", Some("now-1")).await;
    let page = f.service.schedules().unwrap();
    assert_eq!(page.occurrences.len(), 1);
    assert_eq!(f.runtime.calls.locked().len(), 1);
    assert!(!page.schedules[0].enabled);
    let task = page.occurrences[0].task.as_ref().unwrap();
    let view = f.service.task(task).unwrap();
    assert!(view.owner.unwrap().wt.is_some());
    assert!(matches!(
        view.record.source,
        crate::delivery::Provenance::Recurring { .. }
    ));
}
#[tokio::test]
async fn two_definitions_cannot_own_the_project_slot_at_once() {
    let f = Fixture::new();
    let first = definition(&f, "First");
    let second = definition(&f, "Second");
    action(&f, &first, "run_now", Some("now-1")).await;
    action(&f, &second, "run_now", Some("now-2")).await;
    let page = f.service.schedules().unwrap();
    assert_eq!(page.active.as_deref(), Some("now-1"));
    assert_eq!(
        page.occurrences
            .iter()
            .find(|o| o.id == "now-2")
            .unwrap()
            .phase,
        Phase::Stopped
    );
    assert!(f.runtime.calls.locked().is_empty());
}
#[tokio::test]
async fn restart_never_replays_an_active_occurrence_and_retains_its_slot() {
    let f = Fixture::new();
    let schedule = definition(&f, "Restart check");
    action(&f, &schedule, "run_now", Some("now-1")).await;
    let mut started = BTreeMap::new();
    f.service.tick(&mut started, false).await.unwrap();
    let restarted = Arc::new(Service::new(&f.root, f.runtime.clone(), f.clock.clone()).unwrap());
    restarted.tick(&mut BTreeMap::new(), true).await.unwrap();
    let page = restarted.schedules().unwrap();
    assert_eq!(page.active.as_deref(), Some("now-1"));
    assert_eq!(page.occurrences[0].phase, Phase::Unconfirmed);
    assert_eq!(f.runtime.calls.locked().len(), 1);
}
#[tokio::test]
async fn sleep_and_restart_skip_missed_intervals_without_a_burst() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let schedule = definition(&f, "Cadence");
    action(&f, &schedule, "enable", None).await;
    f.clock.0.store(1_000_000, Ordering::SeqCst);
    f.service.tick(&mut BTreeMap::new(), true).await.unwrap();
    let page = f.service.schedules().unwrap();
    assert_eq!(page.schedules[0].next_at, Some(1_060_000));
    assert!(f.runtime.calls.locked().is_empty());
    assert_eq!(page.occurrences.len(), 1);
    assert!(
        page.occurrences[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("Missed interval skipped")
    );
}
#[tokio::test]
async fn revocation_requires_explicit_enable_even_after_retrust() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let schedule = definition(&f, "Trust check");
    action(&f, &schedule, "enable", None).await;
    crate::permissions::set_trusted(&f.root, false).unwrap();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    f.service.tick(&mut BTreeMap::new(), false).await.unwrap();
    let page = f.service.schedules().unwrap();
    assert!(!page.schedules[0].enabled);
    assert!(page.schedules[0].error.is_some());
    assert!(f.runtime.calls.locked().is_empty());
}
#[tokio::test]
async fn run_limit_requests_stop_and_keeps_slot_until_confirmed_idle() {
    let f = Fixture::new();
    let schedule = definition(&f, "Bounded");
    f.runtime.running.store(true, Ordering::SeqCst);
    action(&f, &schedule, "run_now", Some("now-1")).await;
    let mut started = BTreeMap::new();
    f.service.tick(&mut started, false).await.unwrap();
    f.clock.0.store(61_001, Ordering::SeqCst);
    f.service.tick(&mut started, false).await.unwrap();
    assert_eq!(
        f.service.schedules().unwrap().occurrences[0].phase,
        Phase::Stopping
    );
    assert!(f.service.schedules().unwrap().active.is_some());
    f.service.tick(&mut started, false).await.unwrap();
    let page = f.service.schedules().unwrap();
    assert!(page.active.is_none());
    assert_eq!(page.occurrences[0].phase, Phase::Stopped);
}
#[tokio::test]
async fn stop_before_preparation_starts_no_agent_and_releases_the_slot() {
    let f = Fixture::new();
    let schedule = definition(&f, "Stop reserved");
    action(&f, &schedule, "run_now", Some("now-1")).await;
    action(&f, &schedule, "stop", None).await;
    f.service.tick(&mut BTreeMap::new(), false).await.unwrap();
    assert!(f.runtime.calls.locked().is_empty());
    assert!(f.service.schedules().unwrap().active.is_none());
}

#[tokio::test]
async fn successful_pause_edit_or_revoke_during_preparation_prevents_agent_acceptance() {
    for control in ["pause", "edit", "revoke"] {
        let f = Fixture::new();
        crate::permissions::set_trusted(&f.root, true).unwrap();
        let schedule = definition(&f, "Preparation race");
        action(&f, &schedule, "enable", None).await;
        let enabled = f.service.schedules().unwrap().schedules[0].clone();
        let weak = Arc::downgrade(&f.service);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runtime = tokio::runtime::Handle::current();
        *f.runtime.available_hook.locked() = Some(Arc::new(move || {
            if calls.fetch_add(1, Ordering::SeqCst) != 1 {
                return;
            }
            let service = weak.upgrade().unwrap();
            match control {
                "pause" => {
                    runtime
                        .block_on(service.clone().schedule_action(
                            &enabled.id,
                            ScheduleAction {
                                action: "pause".into(),
                                revision: enabled.revision,
                                receipt: None,
                            },
                        ))
                        .unwrap();
                }
                "edit" => {
                    service
                        .save_schedule(ScheduleInput {
                            id: Some(enabled.id.clone()),
                            revision: Some(enabled.revision),
                            name: "Edited".into(),
                            prompt: "New instruction".into(),
                            provider: "codex".into(),
                            base: "main".into(),
                            interval: 60,
                            duration: 60,
                        })
                        .unwrap();
                }
                _ => crate::permissions::set_trusted(&service.root, false).unwrap(),
            }
        }));
        f.clock.0.store(61_000, Ordering::SeqCst);
        let mut started = BTreeMap::new();
        f.service.tick(&mut started, false).await.unwrap();
        *f.runtime.available_hook.locked() = None;
        assert!(f.runtime.calls.locked().is_empty(), "{control}");
        let store = f.service.execution.read().unwrap();
        let receipt = store
            .receipts
            .values()
            .find(|r| r.kind == "scheduled")
            .unwrap();
        assert_eq!(receipt.phase, Phase::FailedBeforeAcceptance, "{control}");
        f.service.tick(&mut started, false).await.unwrap();
        assert!(f.service.schedules().unwrap().active.is_none());
    }
}

#[tokio::test]
async fn control_actions_cannot_report_success_across_an_agent_acceptance_boundary() {
    let f = Fixture::new();
    crate::permissions::set_trusted(&f.root, true).unwrap();
    let schedule = definition(&f, "Acceptance fence");
    let lease = crate::permissions::acceptance_lease(&f.root).unwrap();
    assert!(
        f.service
            .clone()
            .schedule_action(
                &schedule.id,
                ScheduleAction {
                    action: "pause".into(),
                    revision: schedule.revision,
                    receipt: None
                }
            )
            .await
            .is_err()
    );
    assert!(crate::permissions::set_trusted(&f.root, false).is_err());
    assert!(
        f.service
            .save_schedule(ScheduleInput {
                id: Some(schedule.id.clone()),
                revision: Some(schedule.revision),
                name: "Edited".into(),
                prompt: "New instruction".into(),
                provider: "codex".into(),
                base: "main".into(),
                interval: 60,
                duration: 60
            })
            .is_err()
    );
    drop(lease);
    action(&f, &schedule, "pause", None).await;
}

#[test]
fn chronological_history_pages_always_retain_active_recovery() {
    let f = Fixture::new();
    f.service
        .execution
        .change(|store| {
            for at in 0..205 {
                let id = format!("run-{:03}", 204 - at);
                store.occurrences.insert(
                    id.clone(),
                    Occurrence {
                        id: id.clone(),
                        schedule: "definition".into(),
                        revision: 0,
                        due_at: at,
                        receipt: id,
                        task: None,
                        phase: Phase::Unconfirmed,
                        started_at: None,
                        deadline: None,
                        finished_at: None,
                        reason: None,
                    },
                );
            }
            store.active = Some("run-204".into());
            Ok(())
        })
        .unwrap();
    let first = f.service.schedules_page(None).unwrap();
    assert_eq!(first.occurrences.len(), 100);
    assert_eq!(first.occurrences[0].due_at, 204);
    assert_eq!(first.active_occurrence.unwrap().due_at, 0);
    let second = f.service.schedules_page(first.next.as_deref()).unwrap();
    assert_eq!(second.occurrences[0].due_at, 104);
    assert_eq!(second.active_occurrence.unwrap().id, "run-204");
    let third = f.service.schedules_page(second.next.as_deref()).unwrap();
    assert_eq!(third.occurrences.len(), 5);
    assert!(third.next.is_none());
    assert!(f.service.schedules_page(Some("bad-cursor")).is_err());
}
