//! Repository behavior tests preserve damaged records and reject unsafe paths without launching work.
use super::repository::Repository;
use camino::Utf8PathBuf;

fn root() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    std::fs::create_dir(path.join(".git")).unwrap();
    (dir, path)
}

#[test]
fn corrupt_delivery_records_are_preserved_instead_of_reset() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository.change(|_| Ok(())).unwrap();
    let state = path.join(".keel/delivery/state.json");
    std::fs::write(&state, "damaged").unwrap();
    assert!(repository.read().is_err());
    assert!(repository.change(|_| Ok(())).is_err());
    assert_eq!(std::fs::read_to_string(state).unwrap(), "damaged");
}

#[test]
fn failed_revision_mutation_preserves_the_last_saved_document() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository.change(|_| Ok(())).unwrap();
    let state = path.join(".keel/delivery/state.json");
    let before = std::fs::read(&state).unwrap();
    assert!(
        repository
            .change::<()>(|store| {
                store.revision = 99;
                Err("Revision conflict".into())
            })
            .is_err()
    );
    assert_eq!(std::fs::read(state).unwrap(), before);
}

#[test]
fn unknown_delivery_versions_cannot_be_overwritten() {
    let (_dir, path) = root();
    let repository = Repository::new(&path).unwrap();
    repository.change(|_| Ok(())).unwrap();
    let state = path.join(".keel/delivery/state.json");
    let mut body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    body["version"] = 999.into();
    std::fs::write(&state, serde_json::to_vec(&body).unwrap()).unwrap();
    assert!(repository.change(|_| Ok(())).is_err());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(state).unwrap()).unwrap()["version"],
        999
    );
}

#[test]
fn delivery_metadata_is_excluded_from_checkpoint_commits() {
    let (_dir, path) = root();
    Repository::new(&path).unwrap().change(|_| Ok(())).unwrap();
    assert!(
        std::fs::read_to_string(path.join(".keel/.gitignore"))
            .unwrap()
            .lines()
            .any(|line| line == "delivery/")
    );
}

#[cfg(unix)]
#[test]
fn a_linked_delivery_directory_never_receives_project_metadata() {
    let (_dir, path) = root();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(path.join(".keel")).unwrap();
    std::os::unix::fs::symlink(outside.path(), path.join(".keel/delivery")).unwrap();
    assert!(Repository::new(&path).unwrap().change(|_| Ok(())).is_err());
    assert!(!outside.path().join("state.json").exists());
}
