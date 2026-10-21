//! Which conversation a Codex or Antigravity terminal started.
//!
//! Claude Code takes `--session-id`, so Keel picks the id before the first word is typed. Codex
//! and `agy` take none: each names its conversation itself when the person first sends it
//! something, and says so nowhere but on disk. Until Keel learns the id, the lane cannot resume
//! after a relaunch and cannot switch to the chat view — it refused, saying the terminal "does not
//! expose its session ID". So the PTY thread looks for the conversation that appeared in its
//! checkout after it started the CLI, and the lane takes that id.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How much of a file is read to recognise it. A Codex header line is a few KB; an `agy`
/// database records its workspace in its first steps.
const HEAD_BYTES: u64 = 8 * 1024 * 1024;

/// The conversation `agent` started in `cwd` at or after `since`, if one has appeared. `taken`
/// says a conversation another terminal already runs: an older Codex in the same checkout writes
/// its rollout on every turn, and a second terminal took the first one's id for its own.
pub fn started(
    agent: &str,
    cwd: &Path,
    since: SystemTime,
    taken: fn(&str) -> bool,
) -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    match agent {
        "codex" => {
            let root = keel_workspace::codex_home()?.into_std_path_buf();
            codex(&root.join("sessions"), cwd, since, taken)
        }
        "antigravity" => antigravity(
            &home.join(".gemini/antigravity-cli/conversations"),
            cwd,
            since,
        )
        .filter(|id| !taken(id)),
        _ => None,
    }
}

/// Codex writes `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, headed by a `session_meta` line
/// with the id and the working directory. A conversation that began after `since` was written on
/// the newest day there is, so only that folder is read. The interactive CLI says `codex-tui`;
/// a chat lane's `codex exec` beside it in the same checkout, and subagents (whose `source` is an
/// object), are not this terminal's.
fn codex(
    sessions: &Path,
    cwd: &Path,
    since: SystemTime,
    taken: fn(&str) -> bool,
) -> Option<String> {
    let day = newest(&newest(&newest(sessions)?)?)?;
    // Begun after the launch, not just written since: an older Codex in this checkout appends to
    // its own rollout on every turn.
    let mut files: Vec<PathBuf> = entries(&day)
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && begun_since(p, since))
        .collect();
    // Named for when they began, so the first is the earliest conversation after the launch.
    files.sort();
    files.into_iter().find_map(|path| {
        let head = read_head(&path)?;
        let line = head.split(|b| *b == b'\n').next()?;
        let first: serde_json::Value = serde_json::from_slice(line).ok()?;
        let p = &first["payload"];
        let ours = first["type"] == "session_meta"
            && p["originator"] == "codex-tui"
            && p["source"].is_string()
            && p["cwd"].as_str().is_some_and(|c| Path::new(c) == cwd);
        let id = p["id"].as_str().filter(|id| !taken(id))?;
        ours.then(|| id.to_string())
    })
}

/// `agy` keeps one SQLite database per conversation, `conversations/<id>.db`, and records the
/// workspace in it as a protobuf string — the URI with its length in front. Matching that exact
/// field means a conversation in `/repo/.keel/worktrees/x` is not taken for one in `/repo`. The
/// first pages may still be in the write-ahead log, so that is read too.
fn antigravity(dir: &Path, cwd: &Path, since: SystemTime) -> Option<String> {
    let needle = field(&format!("file://{}", cwd.display()));
    let mut found: Vec<(SystemTime, String)> = entries(dir)
        .filter(|p| p.extension().is_some_and(|e| e == "db") && begun_since(p, since))
        .filter(|p| {
            let wal = p.with_extension("db-wal");
            [p.as_path(), wal.as_path()]
                .iter()
                .filter_map(|f| read_head(f))
                .any(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
        })
        .filter_map(|p| Some((created(&p)?, p.file_stem()?.to_str()?.to_string())))
        .collect();
    // ponytail: two `agy` started in one checkout in the same seconds could swap ids; take the
    // earliest, and look at the launch order if that ever turns up.
    found.sort();
    found.into_iter().next().map(|(_, id)| id)
}

/// A protobuf length-delimited string's bytes: the varint length, then the text.
fn field(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut n = text.len();
    while n >= 0x80 {
        out.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
    out.extend_from_slice(text.as_bytes());
    out
}

fn entries(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
}

/// The last-named folder in `dir`: Codex's date folders sort as their names.
fn newest(dir: &Path) -> Option<PathBuf> {
    entries(dir).filter(|p| p.is_dir()).max()
}

fn begun_since(path: &Path, since: SystemTime) -> bool {
    created(path).is_some_and(|t| t >= since)
}

fn created(path: &Path) -> Option<SystemTime> {
    let m = std::fs::metadata(path).ok()?;
    m.created().or_else(|_| m.modified()).ok()
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(path).ok()?, HEAD_BYTES),
        &mut out,
    )
    .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, cwd: &str, originator: &str, source: serde_json::Value) -> String {
        serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"originator":originator,"source":source}}).to_string() + "\n"
    }

    /// The interactive Codex in this checkout is found; an exec turn, a subagent, another
    /// checkout and a conversation from before the launch are not.
    #[test]
    fn codex_finds_the_terminal_conversation_in_its_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("2026/09/30");
        let day = tmp.path().join("2026/10/21");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        let write =
            |dir: &Path, name: &str, body: String| std::fs::write(dir.join(name), body).unwrap();
        write(
            &old,
            "rollout-a-old.jsonl",
            meta("old", "/repo", "codex-tui", "cli".into()),
        );
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        write(
            &day,
            "rollout-1-exec.jsonl",
            meta("exec", "/repo", "codex_exec", "exec".into()),
        );
        write(
            &day,
            "rollout-2-sub.jsonl",
            meta(
                "sub",
                "/repo",
                "codex-tui",
                serde_json::json!({"subagent":{}}),
            ),
        );
        write(
            &day,
            "rollout-3-other.jsonl",
            meta("other", "/elsewhere", "codex-tui", "cli".into()),
        );
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), since, |_| false),
            None
        );
        write(
            &day,
            "rollout-4-mine.jsonl",
            meta("mine", "/repo", "codex-tui", "vscode".into()),
        );
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), since, |_| false).as_deref(),
            Some("mine")
        );
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), later, |_| false),
            None,
            "nothing new since"
        );
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), since, |id| id == "mine"),
            None,
            "another terminal already runs it"
        );
        // An older Codex in this checkout, still writing its own rollout, is not this terminal's.
        let after = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut body = std::fs::read_to_string(day.join("rollout-4-mine.jsonl")).unwrap();
        body.push_str("{}\n");
        std::fs::write(day.join("rollout-4-mine.jsonl"), body).unwrap();
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), after, |_| false),
            None
        );
    }

    /// An `agy` conversation is matched on its exact workspace field, in the database or its log,
    /// so a worktree under the project is not taken for the project.
    #[test]
    fn antigravity_matches_the_exact_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        let with =
            |path: &str| [b"junk".as_slice(), &field(&format!("file://{path}")), b"j"].concat();
        std::fs::write(dir.join("wt.db"), with("/repo/.keel/worktrees/x")).unwrap();
        assert_eq!(antigravity(dir, Path::new("/repo"), since), None);
        std::fs::write(dir.join("conv-1.db"), b"SQLite format 3").unwrap();
        std::fs::write(dir.join("conv-1.db-wal"), with("/repo")).unwrap();
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since).as_deref(),
            Some("conv-1")
        );
        assert_eq!(
            antigravity(dir, Path::new("/repo/.keel/worktrees/x"), since).as_deref(),
            Some("wt")
        );
    }

    /// The length goes in front as a varint, so a path past 127 bytes still matches.
    #[test]
    fn a_long_workspace_takes_a_two_byte_length() {
        let long = "a".repeat(200);
        assert_eq!(field(&long)[..2], [0xc8, 0x01]);
        assert_eq!(field("ab"), b"\x02ab");
    }
}
