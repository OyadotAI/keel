//! Native metadata fixtures prove provider precedence, guarded writes, and unrelated-field retention.
use super::*;
use rusqlite::Connection;

fn fixture() -> (tempfile::TempDir, SessionNames, Utf8PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(tmp.path()).unwrap();
    let cwd = root.join("repo");
    std::fs::create_dir(&cwd).unwrap();
    let homes = SessionNames {
        claude: root.join("claude"),
        codex: root.join("codex"),
        antigravity: root.join("agy"),
    };
    (tmp, homes, cwd)
}
fn codex(homes: &SessionNames, cwd: &Utf8Path) -> Connection {
    std::fs::create_dir(&homes.codex).unwrap();
    let c = Connection::open(homes.codex.join("state_5.sqlite")).unwrap();
    c.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,name TEXT,title TEXT,cwd TEXT,updated_at INTEGER,archived INTEGER)").unwrap();
    c.execute(
        "INSERT INTO threads VALUES('s1',NULL,'Generated',?1,100,0)",
        [cwd.as_str()],
    )
    .unwrap();
    c
}
fn agy(homes: &SessionNames, cwd: &Utf8Path) -> Connection {
    std::fs::create_dir(&homes.antigravity).unwrap();
    let c = Connection::open(homes.antigravity.join("conversation_summaries.db")).unwrap();
    c.execute_batch("CREATE TABLE conversation_summaries(conversation_id TEXT PRIMARY KEY,title TEXT,workspace_uris TEXT,last_modified_time TEXT,step_count INTEGER,raw_summary BLOB)").unwrap();
    let raw = b"\x0a\x03Old\x10\x07\x4a\x04keep\x7a\x0c\x0a\x03Old\x12\x03tag\x20\x01";
    c.execute(
        "INSERT INTO conversation_summaries VALUES('s1','Old',?1,'2026-10-27T00:00:00Z',2,?2)",
        rusqlite::params![
            serde_json::to_string(&vec![format!("file://{cwd}")]).unwrap(),
            raw.as_slice()
        ],
    )
    .unwrap();
    c
}
#[test]
fn codex_prefers_native_explicit_name_and_reflects_terminal_changes() {
    let (_tmp, homes, cwd) = fixture();
    let c = codex(&homes, &cwd);
    let generated = homes.read("codex", &cwd, "s1").unwrap().unwrap();
    assert_eq!(generated.title.as_deref(), Some("Generated"));
    assert!(!generated.explicit);
    c.execute("UPDATE threads SET name='Terminal name'", [])
        .unwrap();
    let explicit = homes.read("codex", &cwd, "s1").unwrap().unwrap();
    assert_eq!(explicit.title.as_deref(), Some("Terminal name"));
    assert!(explicit.explicit);
    assert!(homes.read("codex", Utf8Path::new("/other"), "s1").is_err());
}
#[test]
fn antigravity_renames_authoritative_annotations_and_the_native_resume_summary() {
    let (_tmp, homes, cwd) = fixture();
    let c = agy(&homes, &cwd);
    let dir = homes.antigravity.join("annotations");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("s1.pbtxt"),
        "title:\"Old\"  tags:\"keep\" pinned:true\n",
    )
    .unwrap();
    homes
        .rename("antigravity", &cwd, "s1", "My \"quoted\" name 日本")
        .unwrap();
    let saved = homes.read("antigravity", &cwd, "s1").unwrap().unwrap();
    assert_eq!(saved.title.as_deref(), Some("My \"quoted\" name 日本"));
    assert!(saved.explicit);
    let text = std::fs::read_to_string(dir.join("s1.pbtxt")).unwrap();
    assert!(text.contains("tags:\"keep\" pinned:true"));
    let (title, raw): (String, Vec<u8>) = c
        .query_row(
            "SELECT title,raw_summary FROM conversation_summaries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(title, saved.title.clone().unwrap());
    assert!(raw.windows(8).any(|w| w == b"\x10\x07\x4a\x04keep"));
    assert_eq!(
        raw.windows(title.len())
            .filter(|w| *w == title.as_bytes())
            .count(),
        2
    );
    assert!(raw.windows(7).any(|w| w == b"\x12\x03tag\x20\x01"));
    drop(c);
    assert_eq!(
        homes
            .read("antigravity", &cwd, "s1")
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("My \"quoted\" name 日本")
    );
}
#[test]
fn antigravity_cannot_rename_a_different_workspace_or_erase_malformed_annotations() {
    let (_tmp, homes, cwd) = fixture();
    agy(&homes, &cwd);
    assert!(
        homes
            .rename("antigravity", Utf8Path::new("/other"), "s1", "No")
            .is_err()
    );
    let dir = homes.antigravity.join("annotations");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("s1.pbtxt");
    std::fs::write(&path, "title: not-a-string\n").unwrap();
    assert!(homes.rename("antigravity", &cwd, "s1", "No").is_err());
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "title: not-a-string\n"
    );
}
#[test]
fn identifiers_are_validated_before_provider_paths_are_used() {
    let (_tmp, homes, cwd) = fixture();
    for provider in ["claude", "codex", "antigravity"] {
        assert!(homes.read(provider, &cwd, "../foreign").is_err());
        assert!(homes.rename(provider, &cwd, "../foreign", "No").is_err());
    }
}
