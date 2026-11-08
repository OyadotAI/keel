//! Scratch stores exercise durable provenance, immutable revisions and transferred checkout isolation.
use super::{handoff, model::*, progress, repository::Repository, tracking};
use crate::serve::AppState;
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use serde_json::json;
use std::sync::Arc;
fn fixture() -> (tempfile::TempDir, Utf8PathBuf, Source) {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap().real().unwrap();
    let source = Source {
        lane: "source-lane".into(),
        provider: "claude".into(),
        session: "source-session".into(),
        checkout: root.to_string(),
        wt: None,
    };
    tracking::register(&root, source.clone()).unwrap();
    (temp, root, source)
}
#[test]
fn checkpoints_and_ordered_shared_changelog_survive_reopening_without_native_memory_copying() {
    let (_temp, root, source) = fixture();
    tracking::input(
        &root,
        &source.provider,
        &source.session,
        "Implement checkout recovery",
    )
    .unwrap();
    tracking::input(
        &root,
        &source.provider,
        &source.session,
        "Preserve the original files",
    )
    .unwrap();
    tracking::observed(
        &root,
        &source.session,
        &crate::turns::Fact::Gate {
            status: "passed".into(),
            command: Some("make check".into()),
            ms: None,
            problems: vec![],
        },
    )
    .unwrap();
    let store = Repository::new(&root).unwrap().read().unwrap();
    let task = store.tasks.values().next().unwrap();
    assert_eq!(task.objective, "Implement checkout recovery");
    assert_eq!(task.steering, vec!["Preserve the original files"]);
    assert!(
        task.evidence
            .iter()
            .any(|s| s.contains("Observed check: passed"))
    );
    assert!(
        store
            .events
            .windows(2)
            .all(|e| e[0].sequence < e[1].sequence)
    );
    let changelog = std::fs::read_to_string(root.join(".keel/CHANGELOG.md")).unwrap();
    assert!(
        changelog.contains("source-session") && changelog.contains("Preserve the original files")
    );
    assert!(!root.join(".claude").exists());
    assert!(!root.join(".codex").exists());
}
#[test]
fn later_checkpoint_updates_never_change_the_inspected_handoff_revision() {
    let (_temp, root, source) = fixture();
    tracking::input(&root, "claude", &source.session, "First objective").unwrap();
    let saved = handoff::capture(&root, &source.lane)
        .unwrap()
        .saved
        .pop()
        .unwrap();
    tracking::input(&root, "claude", &source.session, "Latest steering").unwrap();
    handoff::capture(&root, &source.lane).unwrap();
    let store = Repository::new(&root).unwrap().read().unwrap();
    let retained = &store.tasks.values().next().unwrap().saved;
    assert_eq!(retained[0].text, saved.text);
    assert!(!retained[0].text.contains("Latest steering"));
    assert!(retained[1].text.contains("Latest steering"));
}
#[tokio::test]
async fn progress_claims_require_the_current_generation_and_revision() {
    let (_temp, root, source) = fixture();
    let state = Arc::new(AppState::new(root.clone()));
    let read: serde_json::Value = serde_json::from_str(
        &progress::call(state.clone(), Some(source.lane.clone()), json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    let args = json!({"revision":read["revision"],"generation":read["generation"],"notes":{"completed":"Implemented recovery","next_action":"Run failure-path checks"}});
    progress::call(state.clone(), Some(source.lane.clone()), args.clone())
        .await
        .unwrap();
    assert!(
        progress::call(state, Some(source.lane), args)
            .await
            .unwrap_err()
            .contains("changed")
    );
    let store = Repository::new(&root).unwrap().read().unwrap();
    let task = store.tasks.values().next().unwrap();
    assert_eq!(task.notes.completed, "Implemented recovery");
    assert!(
        !task
            .evidence
            .iter()
            .any(|s| s.contains("Implemented recovery"))
    );
    assert!(
        std::fs::read_to_string(root.join(".keel/CHANGELOG.md"))
            .unwrap()
            .contains("Agent-reported progress")
    );
}
#[test]
fn transferred_sources_cannot_start_or_destroy_the_checkout_or_replace_its_latest_notes() {
    let (_temp, root, source) = fixture();
    Repository::new(&root)
        .unwrap()
        .change(|store| {
            let task = store.tasks.values_mut().next().unwrap();
            let destination = Source {
                lane: "destination-lane".into(),
                provider: "codex".into(),
                session: "destination-session".into(),
                ..source.clone()
            };
            task.owner = destination.clone();
            task.members.push(destination.clone());
            task.continuation = Some(Continuation {
                id: "receipt".into(),
                revision: 1,
                source: source.clone(),
                destination,
                prompt: "Continue".into(),
            });
            Ok(())
        })
        .unwrap();
    assert!(tracking::guard(&root, &source.lane, &root).is_err());
    assert!(tracking::destructive_guard(&root, Some(&source.lane), &root).is_err());
    assert!(tracking::guard(&root, "destination-lane", &root).is_ok());
    assert!(handoff::capture(&root, &source.lane).is_err());
    tracking::input(&root, "claude", &source.session, "Late old source").unwrap();
    assert!(
        Repository::new(&root)
            .unwrap()
            .read()
            .unwrap()
            .tasks
            .values()
            .all(|t| !t.objective.contains("Late old source"))
    );
}
#[test]
fn credential_like_text_and_control_sequences_are_excluded_from_shared_memory() {
    let (_temp, root, source) = fixture();
    tracking::input(
        &root,
        "claude",
        &source.session,
        "Implement\nAuthorization: Bearer sensitive\n4/0AUn6fakecode\n\u{1b}[31mprogress",
    )
    .unwrap();
    let body = std::fs::read_to_string(root.join(".keel/tasks/state.json")).unwrap();
    assert!(!body.contains("sensitive"));
    assert!(!body.contains("4/0AUn6"));
    assert!(!body.contains("\\u001b"));
    assert!(body.contains("Credential-like content omitted"));
}
#[cfg(unix)]
#[test]
fn task_storage_and_checkout_symlinks_never_escape_the_authorized_project() {
    use std::os::unix::fs::symlink;
    let (_temp, root, _source) = fixture();
    let other = tempfile::tempdir().unwrap();
    std::fs::remove_file(root.join(".keel/CHANGELOG.md")).unwrap();
    symlink(
        other.path().join("outside"),
        root.join(".keel/CHANGELOG.md"),
    )
    .unwrap();
    assert!(Repository::new(&root).unwrap().change(|_| Ok(())).is_err());
    assert!(!other.path().join("outside").exists());
}
#[test]
fn saving_a_missing_checkout_preserves_previous_handoff_and_reports_recovery() {
    let (_temp, root, source) = fixture();
    handoff::capture(&root, &source.lane).unwrap();
    Repository::new(&root)
        .unwrap()
        .change(|store| {
            let task = store.tasks.values_mut().next().unwrap();
            task.owner.checkout = root.join("missing").to_string();
            Ok(())
        })
        .unwrap();
    assert!(
        handoff::capture(&root, &source.lane)
            .unwrap_err()
            .contains("unavailable")
    );
    assert_eq!(
        Repository::new(&root)
            .unwrap()
            .read()
            .unwrap()
            .tasks
            .values()
            .next()
            .unwrap()
            .saved
            .len(),
        1
    );
}

#[test]
fn continuation_keeps_the_exact_checkout_and_retries_the_same_receipt_without_a_second_lease() {
    let (_temp, root, source) = fixture();
    tracking::input(&root, "claude", &source.session, "Continue exact work").unwrap();
    let saved = handoff::capture(&root, &source.lane)
        .unwrap()
        .saved
        .pop()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let result = handoff::transfer(
        &root,
        &source.lane,
        saved.revision,
        "codex",
        "same-receipt",
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    let again = handoff::transfer(
        &root,
        &source.lane,
        saved.revision,
        "codex",
        "same-receipt",
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(result.destination.checkout, source.checkout);
    assert_ne!(result.destination.session, source.session);
    assert_eq!(again.destination.lane, result.destination.lane);
    assert!(result.prompt.contains(&saved.text));
    assert!(
        handoff::transfer(
            &root,
            &source.lane,
            saved.revision,
            "antigravity",
            "other-receipt",
            |_| Ok(())
        )
        .is_err()
    );
}
#[test]
fn checkout_reservation_outlives_persisting_ownership_and_acquisition_failure_preserves_source() {
    struct Lease {
        root: Utf8PathBuf,
        observed: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            let store = Repository::new(&self.root).unwrap().read().unwrap();
            self.observed.store(
                store.tasks.values().any(|t| t.owner.provider == "codex"),
                std::sync::atomic::Ordering::SeqCst,
            );
        }
    }
    let (_temp, root, source) = fixture();
    let saved = handoff::capture(&root, &source.lane)
        .unwrap()
        .saved
        .pop()
        .unwrap();
    let failure = handoff::transfer::<()>(
        &root,
        &source.lane,
        saved.revision,
        "codex",
        "failed",
        |_| Err("Source children still running".into()),
    );
    assert!(failure.is_err());
    assert_eq!(
        Repository::new(&root)
            .unwrap()
            .read()
            .unwrap()
            .tasks
            .values()
            .next()
            .unwrap()
            .owner,
        source
    );
    let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    handoff::transfer(
        &root,
        &source.lane,
        saved.revision,
        "codex",
        "success",
        |_| {
            Ok(Lease {
                root: root.clone(),
                observed: observed.clone(),
            })
        },
    )
    .unwrap();
    assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
}
#[test]
fn concurrent_continuations_create_one_checkout_owner_and_preserve_the_source_revision() {
    let (_temp, root, source) = fixture();
    let saved = handoff::capture(&root, &source.lane)
        .unwrap()
        .saved
        .pop()
        .unwrap();
    let accepted = std::thread::scope(|scope| {
        let requests = (0..4)
            .map(|n| {
                let root = &root;
                let source = &source;
                let revision = saved.revision;
                scope.spawn(move || {
                    handoff::transfer(
                        root,
                        &source.lane,
                        revision,
                        "codex",
                        &format!("request-{n}"),
                        |_| Ok(()),
                    )
                    .is_ok()
                })
            })
            .collect::<Vec<_>>();
        requests
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|accepted| *accepted)
            .count()
    });
    assert_eq!(accepted, 1);
    assert_eq!(
        Repository::new(&root)
            .unwrap()
            .read()
            .unwrap()
            .tasks
            .values()
            .next()
            .unwrap()
            .saved[0]
            .text,
        saved.text
    );
}

/// Every lane registers a task and none is removed by hand: at the limit the oldest goes, so the
/// 129th lane still starts — the store being full once refused every turn in the project.
#[test]
fn a_full_task_store_evicts_the_oldest_instead_of_refusing_new_lanes() {
    let (_temp, root, source) = fixture();
    for i in 0..TASK_LIMIT + 3 {
        let lane = format!("lane-{i}");
        let next = Source {
            lane: lane.clone(),
            session: format!("session-{i}"),
            ..source.clone()
        };
        tracking::register(&root, next).unwrap_or_else(|e| panic!("lane {i}: {e}"));
    }
    let store = Repository::new(&root).unwrap().read().unwrap();
    assert_eq!(store.tasks.len(), TASK_LIMIT);
}

/// A project moved on disk keeps its `.keel` folder: its tasks are re-rooted, not refused.
#[test]
fn a_moved_project_keeps_its_task_store() {
    let (_temp, root, source) = fixture();
    let moved = tempfile::tempdir().unwrap();
    let to = Utf8Path::from_path(moved.path())
        .unwrap()
        .real()
        .unwrap()
        .join("moved");
    std::fs::rename(&root, &to).unwrap();
    let next = Source {
        checkout: to.to_string(),
        ..source.clone()
    };
    tracking::register(&to, next).expect("the moved project still registers");
    let store = Repository::new(&to).unwrap().read().unwrap();
    assert_eq!(store.project, to.as_str());
    assert!(
        store
            .tasks
            .values()
            .all(|t| t.owner.checkout.starts_with(to.as_str()))
    );
    std::fs::rename(&to, &root).unwrap();
}
