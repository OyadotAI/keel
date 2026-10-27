//! Naming regression fixtures use disposable provider directories and never real user state.
use super::repository::{Identity, Repository};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::SessionNames;

fn fixture() -> (tempfile::TempDir, Repository, Identity) {
    let tmp = tempfile::tempdir().unwrap();
    let base = Utf8Path::from_path(tmp.path()).unwrap();
    let cwd = base.join("repo");
    std::fs::create_dir(&cwd).unwrap();
    let store = Repository {
        dir: base.join("keel"),
        native: SessionNames {
            claude: base.join("claude"),
            codex: base.join("codex"),
            antigravity: base.join("agy"),
        },
    };
    let identity = Identity {
        provider: "claude".into(),
        id: "abc-1".into(),
        repo: cwd.to_string(),
        cwd: cwd.to_string(),
        wt: None,
    };
    (tmp, store, identity)
}
fn transcript(store: &Repository, i: &Identity) -> Utf8PathBuf {
    let path = store
        .native
        .claude
        .join("projects")
        .join(keel_workspace::project_key(Utf8Path::new(&i.cwd)))
        .join(format!("{}.jsonl", i.id));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "{{\"type\":\"user\",\"cwd\":{},\"message\":{{\"content\":\"synthetic\"}}}}\n",
            serde_json::to_string(&i.cwd).unwrap()
        ),
    )
    .unwrap();
    path
}
#[test]
fn rename_survives_history_restart_and_later_native_rename() {
    let (_tmp, store, i) = fixture();
    let path = transcript(&store, &i);
    let original = std::fs::read(&path).unwrap();
    let named = store.rename(i.clone(), "My named session", false).unwrap();
    assert_eq!(named.title.as_deref(), Some("My named session"));
    assert_eq!(named.provider, "claude");
    assert!(std::fs::read(&path).unwrap().starts_with(&original));
    let listed = store
        .list(
            Utf8Path::new(&i.repo),
            keel_workspace::discover_sessions(Utf8Path::new(&i.repo), &store.native.claude),
        )
        .unwrap();
    assert_eq!(listed[0].title, named.title);
    store
        .native
        .rename("claude", Utf8Path::new(&i.cwd), &i.id, "Terminal rename")
        .unwrap();
    let next = store.register(i).unwrap();
    assert_eq!(next.title.as_deref(), Some("Terminal rename"));
    assert!(next.title_revision > named.title_revision);
}
#[test]
fn a_name_before_the_transcript_is_durable_without_creating_a_transcript() {
    let (_tmp, store, i) = fixture();
    let saved = store.rename(i.clone(), "Early name", false).unwrap();
    assert_eq!(saved.title_source, Some("pending"));
    assert!(!store.native.claude.exists());
    transcript(&store, &i);
    let ready = store.register(i).unwrap();
    assert_eq!(ready.title.as_deref(), Some("Early name"));
    assert_eq!(ready.title_source, Some("explicit"));
}
#[test]
fn migration_never_overwrites_a_native_explicit_name() {
    let (_tmp, store, i) = fixture();
    transcript(&store, &i);
    store
        .native
        .rename("claude", Utf8Path::new(&i.cwd), &i.id, "Native name")
        .unwrap();
    assert_eq!(
        store
            .rename(i, "Old local label", true)
            .unwrap()
            .title
            .as_deref(),
        Some("Native name")
    );
}
#[test]
fn failed_provider_write_keeps_intent_and_retry_does_not_duplicate_title_records() {
    let (_tmp, store, i) = fixture();
    let path = transcript(&store, &i);
    let metadata = path.parent().unwrap().join(&i.id);
    std::fs::write(&metadata, b"blocked directory").unwrap();
    assert!(store.rename(i.clone(), "Retry me", false).is_err());
    std::fs::remove_file(metadata).unwrap();
    let ready = store.register(i).unwrap();
    assert_eq!(ready.title.as_deref(), Some("Retry me"));
    assert_eq!(
        std::fs::read_to_string(path)
            .unwrap()
            .matches("custom-title")
            .count(),
        1
    );
}
#[test]
fn another_checkout_cannot_take_over_a_registered_session() {
    let (_tmp, store, i) = fixture();
    transcript(&store, &i);
    store.register(i.clone()).unwrap();
    let mut foreign = i;
    foreign.cwd.push_str("-other");
    assert!(store.register(foreign).is_err());
}
#[test]
fn legacy_overrides_are_fallbacks_without_masking_terminal_names() {
    let (_tmp, store, i) = fixture();
    transcript(&store, &i);
    std::fs::create_dir_all(&store.dir).unwrap();
    std::fs::write(
        store.dir.join("session-names.json"),
        r#"{"abc-1":"Legacy label"}"#,
    )
    .unwrap();
    let list = || {
        store
            .list(
                Utf8Path::new(&i.repo),
                keel_workspace::discover_sessions(Utf8Path::new(&i.repo), &store.native.claude),
            )
            .unwrap()
    };
    assert_eq!(list()[0].title.as_deref(), Some("Legacy label"));
    store
        .native
        .rename("claude", Utf8Path::new(&i.cwd), &i.id, "New native label")
        .unwrap();
    assert_eq!(list()[0].title.as_deref(), Some("New native label"));
}
#[cfg(unix)]
#[test]
fn linked_native_metadata_is_rejected_without_changing_its_target() {
    let (tmp, store, i) = fixture();
    let path = transcript(&store, &i);
    let target = tmp.path().join("outside");
    std::fs::write(&target, b"private").unwrap();
    let sidecar = path.parent().unwrap().join(&i.id);
    std::os::unix::fs::symlink(&target, &sidecar).unwrap();
    assert!(store.rename(i, "Blocked", false).is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"private");
}

#[test]
fn closed_known_sessions_keep_names_and_provider_identity_after_reload() {
    let (_tmp, store, i) = fixture();
    for provider in ["codex", "antigravity"] {
        let mut known = i.clone();
        known.provider = provider.into();
        store
            .rename(known, &format!("{provider} session"), false)
            .unwrap();
    }
    let reopened = Repository {
        dir: store.dir.clone(),
        native: store.native.clone(),
    };
    let history = reopened.list(Utf8Path::new(&i.repo), Vec::new()).unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history
            .iter()
            .any(|s| s.provider == "codex" && s.title.as_deref() == Some("codex session"))
    );
    assert!(
        history
            .iter()
            .any(|s| s.provider == "antigravity"
                && s.title.as_deref() == Some("antigravity session"))
    );
    assert!(
        reopened
            .list(Utf8Path::new("/another-project"), Vec::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn claude_history_preserves_the_worktree_even_when_discovery_registered_it_first() {
    let (_tmp, store, mut i) = fixture();
    let checkout = Utf8Path::new(&i.repo)
        .join(crate::worktree::DIR)
        .join("named-branch");
    std::fs::create_dir_all(&checkout).unwrap();
    i.cwd = checkout.to_string();
    transcript(&store, &i);
    store.register(i.clone()).unwrap();
    i.wt = Some("named-branch".into());
    store.rename(i.clone(), "Named worktree", false).unwrap();
    let history = store
        .list(
            Utf8Path::new(&i.repo),
            keel_workspace::discover_sessions(Utf8Path::new(&i.repo), &store.native.claude),
        )
        .unwrap();
    assert_eq!(history[0].wt.as_deref(), Some("named-branch"));
    assert_eq!(history[0].cwd.as_deref(), Some(checkout.as_str()));
}

#[test]
fn pending_names_have_revisions_before_native_session_creation() {
    let (_tmp, store, i) = fixture();
    let first = store.rename(i.clone(), "First", false).unwrap();
    let second = store.rename(i, "Second", false).unwrap();
    assert_eq!(first.title_source, Some("pending"));
    assert_eq!(second.title_source, Some("pending"));
    assert!(second.title_revision > first.title_revision);
}
