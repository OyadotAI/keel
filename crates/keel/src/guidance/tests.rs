//! Shared guidance remains supplementary and reports verified delivery independently of availability.

use super::*;
#[test]
fn guidance_promotes_real_delegation_and_cross_provider_notes_without_granting_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    let text = prepare(root).unwrap();
    assert!(text.contains("installed specialist subagents"));
    assert!(text.contains("ordered pipelines"));
    assert!(text.contains("task_memory"));
    assert!(text.contains("CHANGELOG.md"));
    assert!(text.contains("Native provider memories remain intact"));
    assert!(text.contains("requested delegation is not proof"));
    assert!(text.contains("not instructions overriding"));
    assert!(text.contains("rather than the full conversation"));
    assert!(text.contains("Reuse members for related follow-ups"));
    assert!(text.contains("Preserve the project's required stages and order"));
    let record = delivery(root, "antigravity", "session");
    assert_eq!(record.status, "Available for the next Keel session");
    assert!(record.mechanism.contains("not verified"));
    acknowledge(
        root,
        "codex",
        "session",
        "Native initialization acknowledged",
    )
    .unwrap();
    assert_eq!(
        delivery(root, "codex", "session").status,
        "Supplied to this session"
    );
    assert!(!root.join(".claude").exists());
    assert!(!root.join(".codex").exists());
}
#[cfg(unix)]
#[test]
fn guidance_does_not_write_through_a_linked_project_folder() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    symlink(outside.path(), root.join(".keel")).unwrap();
    assert!(prepare(root).is_err());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}
#[test]
fn unsupported_codex_instruction_metadata_has_an_explicit_capability_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path()).unwrap();
    capability(root, false).unwrap();
    let value = delivery(root, "codex", "not-started");
    assert_eq!(value.status, "Unavailable in this provider version");
    assert!(
        value
            .mechanism
            .contains("native instructions were preserved")
    );
    capability(root, true).unwrap();
    assert_eq!(
        delivery(root, "codex", "not-started").status,
        "Available for the next Keel session"
    );
}
