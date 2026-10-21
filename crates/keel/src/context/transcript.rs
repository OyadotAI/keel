//! Incremental, bounded recovery of context observations from provider transcripts.
use super::*;
use std::io::{Seek, SeekFrom};
#[path = "evidence.rs"]
mod reads;

// Retain a cursor and incomplete line so following a live transcript reads only appended bytes.
#[derive(Default)]
struct Cursor {
    path: Utf8PathBuf,
    offset: u64,
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
    partial: Vec<u8>,
    calls: HashMap<String, Value>,
}
fn cursors() -> &'static Mutex<HashMap<Key, Cursor>> {
    static CURSORS: OnceLock<Mutex<HashMap<Key, Cursor>>> = OnceLock::new();
    CURSORS.get_or_init(Mutex::default)
}
fn follow_transcript(repo: &Utf8Path, snapshot: &mut Snapshot, file: &Utf8Path) {
    let Ok(meta) = std::fs::metadata(file) else {
        return;
    };
    let key = (
        repo.to_owned(),
        snapshot.provider.clone(),
        snapshot.session.clone(),
    );
    let mut all = cursors().locked();
    if all.len() >= MAX_SESSIONS && !all.contains_key(&key) {
        all.clear();
    }
    let cursor = all.entry(key).or_default();
    let stamp = (meta.len(), meta.modified().ok());
    if cursor.path == file && cursor.stamp.as_ref() == Some(&stamp) {
        return;
    }
    let reset = cursor.path != file
        || meta.len() < cursor.offset
        || (meta.len() == cursor.offset && cursor.stamp.as_ref() != Some(&stamp));
    if reset {
        *cursor = Cursor {
            path: file.to_owned(),
            offset: meta.len().saturating_sub(MAX_BYTES),
            ..Default::default()
        };
    }
    let mut skip_first = reset && cursor.offset > 0;
    if meta.len().saturating_sub(cursor.offset) > MAX_BYTES {
        cursor.offset = meta.len() - MAX_BYTES;
        cursor.partial.clear();
        skip_first = true;
    }
    let Ok(mut input) = std::fs::File::open(file) else {
        return;
    };
    if input.seek(SeekFrom::Start(cursor.offset)).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    if input.take(MAX_BYTES).read_to_end(&mut bytes).is_err() {
        return;
    }
    cursor.offset += bytes.len() as u64;
    let mut joined = std::mem::take(&mut cursor.partial);
    joined.extend(bytes);
    let complete = joined
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|n| n + 1)
        .unwrap_or(0);
    if joined.len() - complete < 256 * 1024 {
        cursor.partial = joined[complete..].to_vec();
    }
    for line in joined[..complete].split(|b| *b == b'\n') {
        if skip_first {
            skip_first = false;
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<Value>(line) {
            absorb(snapshot, &record, repo, &mut cursor.calls);
        }
    }
    cursor.stamp = Some(stamp);
    if cursor.offset >= MAX_BYTES {
        limitation(
            snapshot,
            "Transcript recovery starts with a bounded 2 MiB tail; earlier activity may be omitted.",
        );
    }
    if snapshot.origin == "runtime" {
        snapshot.origin = "saved_runtime".into();
    }
}
pub(super) fn recover(repo: &Utf8Path, provider: &str, session: &str) -> Snapshot {
    let mut snapshot = saved(repo, provider, session);
    if session.is_empty() {
        snapshot.limitations.push("No verified session is linked yet. Context appears once the provider reports a session ID.".into());
        return snapshot;
    }
    let transcript = if provider == "claude" {
        keel_workspace::claude_home()
            .and_then(|home| keel_workspace::transcript_path(repo, &home, session))
    } else {
        codex_transcript(repo, session)
    };
    if let Some(file) = transcript {
        follow_transcript(repo, &mut snapshot, &file);
    }
    if snapshot.used.is_none() {
        limitation(
            &mut snapshot,
            "No context usage has been reported for this session yet.",
        );
    }
    if provider == "codex" {
        limitation(
            &mut snapshot,
            "Codex does not report a category breakdown. Skill loading is shown only when there is observable evidence.",
        );
    }
    if snapshot.origin != "runtime" {
        limitation(
            &mut snapshot,
            "Showing saved observations; files available on disk are not proof they remain in context after compaction.",
        );
    }
    if snapshot.skills.is_empty()
        && provider == "claude"
        && let Some(home) = keel_workspace::claude_home()
    {
        for skill in keel_workspace::discover_skills(repo, &home)
            .into_iter()
            .take(MAX_ITEMS)
        {
            snapshot.skills.push(json!({"name":skill.name,"description":skill.description,"path":skill.path,"source":skill.scope,"available":true}));
        }
    }
    if snapshot.memory.is_empty() {
        let mut candidates = if provider == "claude" {
            vec![repo.join("CLAUDE.md"), repo.join(".claude/CLAUDE.md")]
        } else {
            vec![repo.join("AGENTS.md"), repo.join("AGENTS.override.md")]
        };
        if provider == "claude" {
            if let Some(home) = keel_workspace::claude_home() {
                candidates.push(home.join("CLAUDE.md"));
            }
        } else if let Ok(home) = std::env::var("CODEX_HOME") {
            candidates.push(Utf8PathBuf::from(home).join("AGENTS.md"));
        } else if let Ok(home) = std::env::var("HOME") {
            candidates.push(Utf8PathBuf::from(home).join(".codex/AGENTS.md"));
        }
        for file in candidates {
            if let Ok(meta) = std::fs::metadata(&file)
                && meta.is_file()
            {
                snapshot.memory.push(json!({"path":file,"state":"configured","estimate":meta.len().div_ceil(4),"accuracy":"estimated"}));
            }
        }
    }
    remember(repo, snapshot.clone());
    snapshot
}
fn number(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}
fn evidence(s: &mut Snapshot, value: Value) {
    if !s.evidence.iter().any(|e| e["key"] == value["key"]) {
        s.evidence.push(value);
    }
    if s.evidence.len() > MAX_ITEMS {
        s.evidence.drain(..s.evidence.len() - MAX_ITEMS);
    }
}
fn absorb(s: &mut Snapshot, r: &Value, repo: &Utf8Path, calls: &mut HashMap<String, Value>) {
    let at = r["timestamp"].as_str().unwrap_or("");
    let current = s.updated_at.is_empty() || !at.is_empty() && at >= s.updated_at.as_str();
    let kind = r["type"].as_str().unwrap_or("");
    // Normal Claude transcript parentUuid links messages; only explicit subagent markers exclude usage.
    if s.provider == "claude"
        && current
        && kind == "assistant"
        && r["parent_tool_use_id"].is_null()
        && r["isSidechain"] != true
    {
        let u = &r["message"]["usage"];
        if u["input_tokens"].is_number() {
            s.used = Some(
                number(u, "input_tokens")
                    + number(u, "cache_read_input_tokens")
                    + number(u, "cache_creation_input_tokens"),
            );
            s.usage_accuracy = Some("reported".into());
            s.categories_stale = true;
            s.model = r["message"]["model"]
                .as_str()
                .map(str::to_owned)
                .or(s.model.take());
        }
    }
    if current
        && (kind == "system" && r["subtype"] == "compact_boundary"
            || kind == "event_msg" && r["payload"]["type"] == "context_compacted")
    {
        s.compacted_at = Some(at.into());
        s.used = r["compact_metadata"]["post_tokens"].as_u64();
        s.categories.clear();
    }
    if current && kind == "event_msg" && r["payload"]["type"] == "token_count" {
        let info = &r["payload"]["info"];
        let u = &info["last_token_usage"];
        s.used = u["total_tokens"].as_u64().or_else(|| {
            u["input_tokens"]
                .as_u64()
                .map(|n| n + number(u, "output_tokens"))
        });
        s.capacity = info["model_context_window"].as_u64().or(s.capacity);
        s.window_kind = Some("model".into());
        s.usage_accuracy = Some("reported".into());
    }
    for b in r["message"]["content"].as_array().into_iter().flatten() {
        if b["type"] == "tool_use"
            && b["name"] == "Skill"
            && let Some(name) = b["input"]["skill"].as_str()
        {
            let id = b["id"].as_str().unwrap_or(at);
            evidence(
                s,
                json!({"key":format!("{id}:invocation:{name}"),"id":id,"kind":"invocation","name":name,"at":at,"member":r["subagent_type"],"detail":"Skill invocation observed in the transcript. Selection rationale was not reported."}),
            );
        }
    }
    if current && let Some(report) = r.get("context_usage") {
        s.used = report["total_tokens"].as_u64();
        s.capacity = report["raw_max_tokens"].as_u64();
        s.model = report["model"].as_str().map(str::to_owned);
        s.categories = report["categories"].as_array().cloned().unwrap_or_default();
        s.memory = report["memory_files"].as_array().into_iter().flatten().map(|v| json!({"path":v["path"],"source":v["type"],"tokens":v["tokens"],"state":"reported","accuracy":"provider_estimate"})).collect();
        s.categories_stale = false;
        s.usage_accuracy = Some("provider_estimate".into());
        s.window_kind = Some("compaction_window".into());
        s.breakdown_at = Some(at.into());
    }
    if kind == "system" && r["subtype"] == "memory_recall" {
        for file in r["memories"].as_array().into_iter().flatten() {
            if let Some(path) = file["path"].as_str() {
                let id = r["uuid"].as_str().unwrap_or(at);
                evidence(
                    s,
                    json!({"key":format!("{id}:memory_recall:{path}"),"id":id,"kind":"memory_recall","path":path,"at":at,"detail":"Memory recall reported by the provider."}),
                );
            }
            if let Some(path) = file["path"]
                .as_str()
                .filter(|p| !p.starts_with('<') && !p.starts_with("https:"))
                && !s.memory.iter().any(|v| v["path"] == path)
            {
                s.memory
                    .push(json!({"path":path,"source":file["scope"],"state":"recalled"}));
            }
        }
    }
    if kind == "session_meta" {
        for file in r["payload"]["instructionSources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !s.memory.iter().any(|m| m["path"] == file) {
                s.memory.push(json!({"path":file,"state":"reported"}));
            }
        }
    }
    if current && !at.is_empty() {
        s.updated_at = at.into();
    }
    s.skills.truncate(MAX_ITEMS);
    s.memory.truncate(MAX_ITEMS);
    reads::observe_reads(s, r, repo, calls);
}

fn codex_transcript(repo: &Utf8Path, session: &str) -> Option<Utf8PathBuf> {
    keel_workspace::rollout(
        &keel_workspace::codex_home()?.join("sessions"),
        repo,
        session,
    )
}
