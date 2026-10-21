//! Codex's own transcript of an interactive session, read the way Claude Code's is.
//!
//! The interactive CLI appends every record to `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`
//! under its home, whoever started it — the same live feed Claude Code's transcript is. Two views
//! of the same work are in it: `response_item` (the model's wire format, with injected instructions
//! posing as user messages and commands as the JavaScript that ran them) and `event_msg` (typed,
//! readable, one `item_completed` per thing that happened). Only `event_msg` is followed, so
//! nothing is drawn twice and the 650 KB `compacted` lines are never decoded.
//!
//! A turn is keyed by Codex's `turn_id`, which every `item_completed` carries — so a turn whose
//! start a long replay cut off still files under the right key — and it opens on the turn's first
//! `UserMessage`. A person typing while Codex works writes more `UserMessage`s with the *same*
//! `turn_id`; they are part of that turn, not new ones.

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

use crate::{Kind, Real};

/// Codex's home, honouring `CODEX_HOME` as Codex does.
pub fn codex_home() -> Option<Utf8PathBuf> {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        return Some(Utf8PathBuf::from(dir));
    }
    Some(crate::home()?.join(".codex"))
}

/// Directory entries a search for one rollout may look at before giving up.
const MAX_ENTRIES: usize = 5_000;

/// The rollout of session `id`, if it ran in `cwd`. The id is checked as a file name before it
/// is used as one, and the header must name both the id and this working directory: the path
/// alone would let any request read any Codex conversation on the machine.
pub fn rollout(sessions: &Utf8Path, cwd: &Utf8Path, id: &str) -> Option<Utf8PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let want = cwd.real().unwrap_or_else(|_| cwd.to_owned());
    let suffix = format!("{id}.jsonl");
    let mut dirs = vec![(sessions.to_owned(), 0)];
    let mut count = 0;
    while let Some((dir, depth)) = dirs.pop() {
        // Date folders sort as their names. Pushed oldest first, so the newest is walked first and
        // the cap gives up on years-old sessions, not on the one being followed.
        let mut here: Vec<_> = std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        here.sort_by_key(|e| e.file_name());
        for entry in here {
            count += 1;
            if count > MAX_ENTRIES {
                return None;
            }
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            let Ok(ty) = entry.file_type() else { continue };
            if ty.is_dir() && depth < 4 {
                dirs.push((path, depth + 1));
            } else if ty.is_file()
                && path.file_name().is_some_and(|n| n.ends_with(&suffix))
                && header(&path)
                    .is_some_and(|(hid, hcwd)| hid == id && hcwd.real().unwrap_or(hcwd) == want)
            {
                return Some(path);
            }
        }
    }
    None
}

/// The id and working directory a rollout's first line names.
fn header(path: &Utf8Path) -> Option<(String, Utf8PathBuf)> {
    use std::io::BufRead;
    let mut first = String::new();
    std::io::BufReader::new(std::io::Read::take(
        std::fs::File::open(path).ok()?,
        256 * 1024,
    ))
    .read_line(&mut first)
    .ok()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    let p = &v["payload"];
    Some((
        p["id"].as_str()?.to_string(),
        Utf8PathBuf::from(p["cwd"].as_str()?),
    ))
}

/// Whether a rollout record is one a follower is shown: the typed events, nothing else.
pub(crate) fn followable(record: &Value) -> bool {
    record["type"] == "event_msg"
}

/// What a rollout line is, for a follower deciding what the session is doing.
pub fn kind_of(line: &str) -> Kind {
    let Ok(r) = serde_json::from_str::<Value>(line) else {
        return Kind::Other;
    };
    if let Some((uuid, prompt)) = opener(&r) {
        return Kind::Opener {
            uuid,
            prompt: prompt.chars().take(200).collect(),
            at: r["timestamp"].as_str().unwrap_or_default().to_string(),
            cwd: None,
        };
    }
    match r["payload"]["type"].as_str() {
        Some("task_complete" | "turn_aborted") => Kind::TurnEnd,
        _ => Kind::Other,
    }
}

/// The `turn_id` of a person's message, if this line is one.
pub fn opener_of(line: &str) -> Option<String> {
    opener(&serde_json::from_str(line).ok()?).map(|(id, _)| id)
}

/// A person's message, as `(turn_id, text)`. An image with no words reads as "(image)", so a
/// pasted screenshot still opens its turn.
pub(crate) fn opener(r: &Value) -> Option<(String, String)> {
    let p = &r["payload"];
    if r["type"] != "event_msg"
        || p["type"] != "item_completed"
        || p["item"]["type"] != "UserMessage"
    {
        return None;
    }
    Some((p["turn_id"].as_str()?.to_string(), said(&p["item"])))
}

/// What a `UserMessage` said, in full.
pub(crate) fn said(item: &Value) -> String {
    let blocks = item["content"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.is_empty() && !blocks.is_empty() {
        return "(image)".into();
    }
    text.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(turn: &str, text: &str) -> String {
        serde_json::json!({"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","turn_id":turn,"item":{"type":"UserMessage","id":"u","content":[{"type":"text","text":text}]}}}).to_string()
    }

    /// A turn opens on a person's message and ends on `task_complete` or `turn_aborted`; the
    /// model's own view of the same message (`response_item`) opens nothing.
    #[test]
    fn codex_kind_of_opens_on_the_user_message_and_ends_on_complete_or_abort() {
        assert!(
            matches!(kind_of(&user("T1", "fix it")), Kind::Opener { uuid, prompt, .. } if uuid == "T1" && prompt == "fix it")
        );
        assert_eq!(opener_of(&user("T1", "x")).as_deref(), Some("T1"));
        for end in ["task_complete", "turn_aborted"] {
            let line =
                serde_json::json!({"type":"event_msg","payload":{"type":end,"turn_id":"T1"}})
                    .to_string();
            assert_eq!(kind_of(&line), Kind::TurnEnd);
        }
        let echo = serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"x"}]}}).to_string();
        assert_eq!(kind_of(&echo), Kind::Other);
        let image = serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"T2","item":{"type":"UserMessage","content":[{"type":"local_image","path":"/a.png"}]}}}).to_string();
        assert!(matches!(kind_of(&image), Kind::Opener { prompt, .. } if prompt == "(image)"));
    }

    /// A rollout is found only by its own id, in its own checkout.
    #[test]
    fn a_codex_rollout_is_found_only_in_its_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        let (repo, other) = (root.join("repo"), root.join("other"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let day = root.join("sessions/2026/10/21");
        std::fs::create_dir_all(&day).unwrap();
        let meta = |id: &str, cwd: &Utf8Path| {
            serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd}}).to_string()
                + "\n"
        };
        std::fs::write(day.join("rollout-1-abc-1.jsonl"), meta("abc-1", &repo)).unwrap();
        std::fs::write(
            day.join("rollout-2-abc-2.jsonl"),
            meta("someone-else", &repo),
        )
        .unwrap();
        let sessions = root.join("sessions");
        assert_eq!(
            rollout(&sessions, &repo, "abc-1"),
            Some(day.join("rollout-1-abc-1.jsonl"))
        );
        assert_eq!(
            rollout(&sessions, &other, "abc-1"),
            None,
            "another checkout"
        );
        assert_eq!(
            rollout(&sessions, &repo, "abc-2"),
            None,
            "the header names another id"
        );
        assert_eq!(rollout(&sessions, &repo, "../abc-1"), None);
    }
}
