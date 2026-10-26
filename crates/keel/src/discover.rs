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

/// A day's margin covers local-date folders on either side of the UTC launch window.
const DATE_MARGIN: std::time::Duration = std::time::Duration::from_secs(86_400);
/// Length of the date portion shared by ISO timestamps and slash-separated folder paths.
const DATE_LENGTH: usize = 10;
/// Prefix length, component width and maximum for the YYYY/MM/DD folder hierarchy.
const DATE_PARTS: [(usize, usize, u32); 3] = [(4, 4, 9999), (7, 2, 12), (DATE_LENGTH, 2, 31)];

/// How long `lsof` may take before the earliest-conversation rule decides instead.
const LSOF_CEILING: std::time::Duration = std::time::Duration::from_secs(1);

/// The conversation `agent` started in `cwd` at or after `since`, if one has appeared. `taken`
/// says a conversation another terminal already runs: an older Codex in the same checkout writes
/// its rollout on every turn, and a second terminal took the first one's id for its own.
/// `holder` is the terminal's process group: an `agy` keeps its own conversation open.
pub fn started(
    agent: &str,
    cwd: &Path,
    since: SystemTime,
    taken: fn(&str) -> bool,
    holder: Option<u32>,
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
            taken,
            || holder.and_then(open_by),
        ),
        _ => None,
    }
}

/// Codex writes `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, headed by a `session_meta` line
/// with the id and the working directory. Search the launch-to-now date window so a newer folder
/// cannot hide a conversation started before midnight. The interactive CLI says `codex-tui`;
/// a chat lane's `codex exec` beside it in the same checkout, and subagents (whose `source` is an
/// object), are not this terminal's.
fn codex(
    sessions: &Path,
    cwd: &Path,
    since: SystemTime,
    taken: fn(&str) -> bool,
) -> Option<String> {
    // Begun after the launch, not just written since: an older Codex in this checkout appends to
    // its own rollout on every turn.
    let mut files: Vec<PathBuf> = codex_days(sessions, since, SystemTime::now())
        .into_iter()
        .flat_map(|day| entries(&day))
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
/// Measured on 1.4.4, a terminal `agy`'s first prompt is not always the first conversation in
/// its workspace after it started: an `agy -p` run beside it (another lane's, or a command an
/// agent ran) began one five seconds after launch, and the lane followed that one — two steps,
/// never written again — and read as idle for the whole session. Its subagents' conversations
/// begin in the same workspace too. So when the files the terminal's own process group holds
/// open are known (`open`), only those are candidates; when they cannot be read, the rule falls
/// back to the earliest. `open` is asked only once a candidate exists: it runs `lsof` on the
/// terminal's input loop, and before the first prompt there is nothing for it to decide.
fn antigravity(
    dir: &Path,
    cwd: &Path,
    since: SystemTime,
    taken: fn(&str) -> bool,
    open: impl FnOnce() -> Option<Vec<PathBuf>>,
) -> Option<String> {
    let needle = field(&format!("file://{}", cwd.display()));
    let candidates: Vec<PathBuf> = entries(dir)
        .filter(|p| p.extension().is_some_and(|e| e == "db") && begun_since(p, since))
        .filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|id| !taken(id))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let open = open();
    let mut found: Vec<(SystemTime, String)> = candidates
        .into_iter()
        .filter(|p| {
            open.as_ref()
                .is_none_or(|held| held.iter().any(|h| h.file_name() == p.file_name()))
        })
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

/// The files process group `pgid` has open, from `lsof`; `None` when that cannot be asked (no
/// `lsof`, as on Windows, or one that did not answer in time), which is not the same answer as
/// "none". Bounded: it runs on the terminal's input loop, and an `lsof` stuck on a stale mount
/// would hold every keystroke behind it.
fn open_by(pgid: u32) -> Option<Vec<PathBuf>> {
    let mut lsof = std::process::Command::new("lsof");
    lsof.args(["-w", "-b", "-Fn", "-g", &pgid.to_string()]);
    let out = crate::git::output_within(lsof, LSOF_CEILING).ok()?;
    // `lsof` exits 1 when the group holds nothing it can list, which is still an answer.
    Some(held(&String::from_utf8_lossy(&out.stdout)))
}

/// The paths in `lsof -Fn` output: one field per line, a path's line starting with `n`.
fn held(fields: &str) -> Vec<PathBuf> {
    fields
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(PathBuf::from)
        .collect()
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

/// Own the directory stream so callers can chain scans without retaining the parent path.
fn entries(dir: &Path) -> impl Iterator<Item = PathBuf> + use<> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
}

/// Prune each calendar level before reading children so old rollout histories are never scanned.
fn codex_days(root: &Path, since: SystemTime, now: SystemTime) -> Vec<PathBuf> {
    let lower = calendar_date(
        since
            .checked_sub(DATE_MARGIN)
            .unwrap_or(SystemTime::UNIX_EPOCH),
    );
    let upper = calendar_date(now.checked_add(DATE_MARGIN).unwrap_or(now));
    let mut dirs = vec![root.to_path_buf()];
    for (prefix, width, max) in DATE_PARTS {
        let first = root.join(&lower[..prefix]);
        let last = root.join(&upper[..prefix]);
        dirs = dirs
            .into_iter()
            .flat_map(|dir| entries(&dir))
            .filter(|p| calendar_part(p, width, max) && *p >= first && *p <= last)
            .collect();
    }
    dirs
}

/// Use the shared UTC formatter rather than introducing a second calendar implementation.
fn calendar_date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let iso = keel_workspace::iso(seconds as i64, 0);
    iso[..DATE_LENGTH].replace('-', "/")
}

/// Only padded numeric directories belong to the calendar hierarchy; side folders are skipped.
fn calendar_part(path: &Path, width: usize, max: u32) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.len() == width
                    && name.bytes().all(|b| b.is_ascii_digit())
                    && name.parse::<u32>().is_ok_and(|n| n > 0 && n <= max)
            })
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

    /// A later date folder must not hide a terminal started on the previous date.
    #[test]
    fn codex_keeps_searching_when_a_newer_day_is_empty_or_unrelated() {
        let tmp = tempfile::tempdir().unwrap();
        let since = SystemTime::UNIX_EPOCH;
        let previous = tmp.path().join("2025/12/31");
        let newer = tmp.path().join("2026/01/01");
        std::fs::create_dir_all(&previous).unwrap();
        std::fs::create_dir_all(&newer).unwrap();
        std::fs::write(
            previous.join("rollout-1.jsonl"),
            meta("mine", "/repo", "codex-tui", "cli".into()),
        )
        .unwrap();
        for unrelated in [false, true] {
            if unrelated {
                std::fs::write(
                    newer.join("rollout-2.jsonl"),
                    meta("other", "/elsewhere", "codex-tui", "cli".into()),
                )
                .unwrap();
            }
            assert_eq!(
                codex(tmp.path(), Path::new("/repo"), since, |_| false).as_deref(),
                Some("mine")
            );
        }
    }

    /// Selection is earliest across all eligible folders, then advances past claimed IDs.
    #[test]
    fn codex_selects_the_earliest_unclaimed_session_across_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let since = SystemTime::UNIX_EPOCH;
        for (date, id) in [("2025/12/31", "first"), ("2026/01/01", "second")] {
            let dir = tmp.path().join(date);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("rollout.jsonl"),
                meta(id, "/repo", "codex-tui", "cli".into()),
            )
            .unwrap();
        }
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), since, |_| false).as_deref(),
            Some("first")
        );
        assert_eq!(
            codex(tmp.path(), Path::new("/repo"), since, |id| id == "first").as_deref(),
            Some("second")
        );
        assert_eq!(codex(tmp.path(), Path::new("/repo"), since, |_| true), None);
    }

    /// Calendar pruning crosses year/month boundaries and keeps one day for timezone offsets.
    #[test]
    fn codex_date_window_prunes_history_future_and_noncalendar_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let dates = [
            "2024/12/31",
            "2025/12/29",
            "2025/12/30",
            "2025/12/31",
            "2026/01/01",
            "2026/01/02",
            "2026/01/03",
            "2027/01/01",
            "2026/1/01",
            "2026/00/01",
            "2026/01/00",
            "2026/01/32",
            "misc/01/01",
        ];
        for date in dates {
            std::fs::create_dir_all(tmp.path().join(date)).unwrap();
        }
        // Noon UTC on December 31, 2025 through noon UTC on January 1, 2026.
        let since = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_767_182_400);
        let now = since + DATE_MARGIN;
        let mut found = codex_days(tmp.path(), since, now);
        found.sort();
        let expected: Vec<_> = ["2025/12/30", "2025/12/31", "2026/01/01", "2026/01/02"]
            .into_iter()
            .map(|day| tmp.path().join(day))
            .collect();
        assert_eq!(found, expected);
    }

    /// The interactive Codex in this checkout is found; an exec turn, a subagent, another
    /// checkout and a conversation from before the launch are not.
    #[test]
    fn codex_finds_the_terminal_conversation_in_its_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("1969/12/31");
        let day = tmp.path().join(calendar_date(SystemTime::now()));
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
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since, |_| false, || None),
            None
        );
        std::fs::write(dir.join("conv-1.db"), b"SQLite format 3").unwrap();
        std::fs::write(dir.join("conv-1.db-wal"), with("/repo")).unwrap();
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since, |_| false, || None).as_deref(),
            Some("conv-1")
        );
        assert_eq!(
            antigravity(
                dir,
                Path::new("/repo/.keel/worktrees/x"),
                since,
                |_| false,
                || None
            )
            .as_deref(),
            Some("wt")
        );
    }

    /// Measured on agy 1.4.4: an `agy -p` beside the terminal began a conversation in the same
    /// workspace before the person's first prompt, and the lane took it. What the terminal's own
    /// process holds open decides; when that cannot be read, the earliest still does.
    #[test]
    fn antigravity_takes_the_conversation_its_terminal_holds_open() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        let here = [b"j".as_slice(), &field("file:///repo")].concat();
        std::fs::write(dir.join("beside.db"), &here).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("mine.db"), &here).unwrap();
        let open = |name: &str| Some(vec![dir.join(name), dir.join(format!("{name}-wal"))]);
        assert_eq!(
            antigravity(
                dir,
                Path::new("/repo"),
                since,
                |_| false,
                || open("mine.db")
            )
            .as_deref(),
            Some("mine")
        );
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since, |_| false, || Some(vec![])),
            None,
            "not yet open"
        );
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since, |_| false, || None).as_deref(),
            Some("beside"),
            "no lsof: the earliest, as before"
        );
        assert_eq!(
            antigravity(dir, Path::new("/repo"), since, |id| id == "beside", || None).as_deref(),
            Some("mine"),
            "another terminal's conversation is passed over, not the end of the search"
        );
        assert_eq!(
            antigravity(
                dir,
                Path::new("/repo"),
                since,
                |_| true,
                || { panic!("claimed sessions need no ownership lookup") }
            ),
            None,
            "no conversation can be claimed twice"
        );
        assert_eq!(
            antigravity(
                tempfile::tempdir().unwrap().path(),
                Path::new("/repo"),
                since,
                |_| false,
                || panic!("lsof is asked only once a candidate exists")
            ),
            None
        );
        assert_eq!(
            held("p123\nfcwd\nn/Users/x/conversations/mine.db\nf9\nn/dev/ttys001\n"),
            [
                PathBuf::from("/Users/x/conversations/mine.db"),
                PathBuf::from("/dev/ttys001")
            ]
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
