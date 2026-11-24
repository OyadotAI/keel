//! Task-format compatibility fences older writers before managed ownership can be silently evicted.
use super::{model::*, repository::Repository, tracking};
use camino::Utf8PathBuf;

fn project() -> (tempfile::TempDir, Utf8PathBuf, String) {
    use keel_workspace::Real;
    let directory = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(directory.path().to_path_buf())
        .unwrap()
        .real()
        .unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    let source = Source {
        lane: "version-source".into(),
        provider: "codex".into(),
        session: "version-source".into(),
        checkout: root.to_string(),
        wt: None,
    };
    let id = tracking::register(&root, source).unwrap();
    (directory, root, id)
}

#[test]
fn legacy_v1_tasks_load_without_requiring_a_migration() {
    let (_directory, root, id) = project();
    let state = Repository::new(&root).unwrap().read_strict().unwrap();
    assert_eq!(state.version, LEGACY_VERSION);
    assert!(!state.tasks[&id].pinned);
}
#[test]
fn first_pin_atomically_upgrades_and_later_note_writes_preserve_v2() {
    let (_directory, root, id) = project();
    super::pin(&root, &id).unwrap();
    tracking::input(&root, "codex", "version-source", "Keep task ownership").unwrap();
    let state = Repository::new(&root).unwrap().read_strict().unwrap();
    assert_eq!(state.version, PINNED_VERSION);
    assert!(state.tasks[&id].pinned);
    assert_eq!(
        std::fs::read(root.join(".keel/tasks/version")).unwrap(),
        FORMAT_MARKER
    );
}
#[test]
fn later_mutations_cannot_downgrade_the_task_format() {
    let (_directory, root, id) = project();
    super::pin(&root, &id).unwrap();
    let repository = Repository::new(&root).unwrap();
    repository
        .change(|state| {
            state.version = LEGACY_VERSION;
            Ok(())
        })
        .unwrap();
    assert_eq!(repository.read_strict().unwrap().version, PINNED_VERSION);
}
#[test]
fn the_old_v1_writer_protocol_rejects_v2_without_discarding_pins() {
    let (_directory, root, id) = project();
    super::pin(&root, &id).unwrap();
    let path = root.join(".keel/tasks/state.json");
    let before = std::fs::read(&path).unwrap();
    // This is the previous binary's version gate, exercised before its lossy Task deserializer.
    let result = crate::writes::locked(path.as_std_path(), || {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if value["version"] != LEGACY_VERSION {
            return Err("Task storage belongs to another version.");
        }
        std::fs::write(&path, b"old writer replaced metadata").unwrap();
        Ok(())
    })
    .unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[test]
fn advisory_corruption_recovery_preserves_the_pinned_format_fence() {
    let (_directory, root, id) = project();
    super::pin(&root, &id).unwrap();
    std::fs::write(root.join(".keel/tasks/state.json"), b"damaged notes").unwrap();
    let repository = Repository::new(&root).unwrap();
    repository.change(|_| Ok(())).unwrap();
    assert_eq!(repository.read_strict().unwrap().version, PINNED_VERSION);
}
#[test]
fn unknown_future_versions_remain_untouched() {
    let (_directory, root, _id) = project();
    let path = root.join(".keel/tasks/state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["version"] = 99.into();
    let bytes = serde_json::to_vec(&state).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(Repository::new(&root).unwrap().change(|_| Ok(())).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[test]
fn an_unreadable_format_marker_does_not_reset_readable_task_notes() {
    let (_directory, root, _id) = project();
    let path = root.join(".keel/tasks/state.json");
    let before = std::fs::read(&path).unwrap();
    std::fs::write(root.join(".keel/tasks/version"), b"future-format").unwrap();
    assert!(Repository::new(&root).unwrap().change(|_| Ok(())).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
