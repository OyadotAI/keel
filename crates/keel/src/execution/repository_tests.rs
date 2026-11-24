//! Receipt persistence tests use the existing isolated permission directory, never the user's HOME.
use super::{Repository, constants::*};
use camino::Utf8PathBuf;
use serde_json::json;

fn root() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    std::fs::create_dir(path.join(".git")).unwrap();
    (dir, path)
}

fn file(path: &camino::Utf8Path, suffix: &str) -> String {
    format!("{}.{suffix}", crate::permissions::key_of(path).unwrap())
}

#[test]
fn lost_response_retries_and_restarts_recover_one_frozen_receipt() {
    let (_dir, path) = root();
    let first = Repository::new(&path)
        .unwrap()
        .reserve("launch-1", "issue", json!({"provider":"codex"}), 1)
        .unwrap();
    let repeated = Repository::new(&path)
        .unwrap()
        .reserve("launch-1", "issue", json!({"provider":"codex"}), 2)
        .unwrap();
    assert_eq!(first.fingerprint, repeated.fingerprint);
    assert_eq!(repeated.created_at, 1);
    assert_eq!(
        Repository::new(&path)
            .unwrap()
            .read()
            .unwrap()
            .receipts
            .len(),
        1
    );
}

#[test]
fn reusing_a_receipt_with_changed_intent_preserves_the_original() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository
        .reserve("launch-1", "issue", json!({"provider":"codex"}), 1)
        .unwrap();
    let before = crate::permissions::read_local(AREA, &file(&path, "json"), STORE_BYTES).unwrap();
    assert!(
        repository
            .reserve("launch-1", "issue", json!({"provider":"claude"}), 2)
            .is_err()
    );
    assert_eq!(
        crate::permissions::read_local(AREA, &file(&path, "json"), STORE_BYTES).unwrap(),
        before
    );
}

#[test]
fn damaged_execution_state_never_authorizes_a_fresh_dispatch() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository
        .reserve("launch-1", "issue", json!({}), 1)
        .unwrap();
    crate::permissions::replace_local(AREA, &file(&path, "json"), b"damaged").unwrap();
    assert!(
        Repository::new(&path)
            .unwrap()
            .reserve("launch-2", "issue", json!({}), 2)
            .is_err()
    );
    assert_eq!(
        crate::permissions::read_local(AREA, &file(&path, "json"), STORE_BYTES)
            .unwrap()
            .unwrap(),
        b"damaged"
    );
}

#[test]
fn a_missing_previously_bound_store_cannot_be_recreated_as_runnable() {
    let (_dir, path) = root();
    Repository::new(&path)
        .unwrap()
        .reserve("launch-1", "issue", json!({}), 1)
        .unwrap();
    std::fs::remove_file(crate::permissions::local_path(AREA, &file(&path, "json")).unwrap())
        .unwrap();
    assert!(
        Repository::new(&path)
            .unwrap()
            .reserve("launch-2", "issue", json!({}), 2)
            .is_err()
    );
}

#[test]
fn an_invalid_active_slot_does_not_replace_confirmed_history() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository
        .reserve("launch-1", "issue", json!({}), 1)
        .unwrap();
    assert!(
        repository
            .change(|store| {
                store.active = Some("absent".into());
                Ok(())
            })
            .is_err()
    );
    assert!(repository.read().unwrap().active.is_none());
}

#[cfg(unix)]
#[test]
fn linked_control_files_are_refused_without_reading_or_replacing_the_target() {
    let (_dir, path) = root();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "private outside").unwrap();
    let filename = file(&path, "json");
    std::os::unix::fs::symlink(
        outside.path(),
        crate::permissions::local_path(AREA, &filename).unwrap(),
    )
    .unwrap();
    assert!(Repository::new(&path).unwrap().read().is_err());
    assert_eq!(
        std::fs::read_to_string(outside.path()).unwrap(),
        "private outside"
    );
}
