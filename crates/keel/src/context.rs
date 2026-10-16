//! Read-only session context: provider observations, bounded transcript recovery, and safe sources.
use crate::{
    lock::Locked,
    serve::{AppState, Checkout},
};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::{
    collections::HashMap,
    io::Read,
    sync::{Mutex, OnceLock},
};

mod transcript;

fn limitation(s: &mut Snapshot, text: &str) {
    if !s.limitations.iter().any(|v| v == text) {
        s.limitations.push(text.into());
    }
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const SOURCE_BYTES: u64 = 48 * 1024;
const MAX_ITEMS: usize = 200;
const MAX_SESSIONS: usize = 128;

pub(crate) fn request_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Evidence records carry paths and metadata, never memory or tool-result bodies.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Snapshot {
    pub provider: String,
    pub session: String,
    pub updated_at: String,
    pub origin: String,
    pub model: Option<String>,
    pub used: Option<u64>,
    pub capacity: Option<u64>,
    pub window_kind: Option<String>,
    pub usage_accuracy: Option<String>,
    pub categories: Vec<Value>,
    pub categories_stale: bool,
    pub breakdown_at: Option<String>,
    pub reserve: Option<u64>,
    pub memory: Vec<Value>,
    pub skills: Vec<Value>,
    pub evidence: Vec<Value>,
    pub compacted_at: Option<String>,
    pub limitations: Vec<String>,
}
type Key = (Utf8PathBuf, String, String);
fn cache() -> &'static Mutex<HashMap<Key, Snapshot>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Snapshot>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}
fn valid(provider: &str, session: &str) -> bool {
    ["claude", "codex"].contains(&provider)
        && !session.is_empty()
        && session.len() <= 128
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
fn path(repo: &Utf8Path, provider: &str, session: &str) -> Utf8PathBuf {
    repo.join(".keel/turns/context")
        .join(format!("{provider}-{session}.json"))
}
fn remember(repo: &Utf8Path, snapshot: Snapshot) {
    let mut all = cache().locked();
    let key = (
        repo.to_owned(),
        snapshot.provider.clone(),
        snapshot.session.clone(),
    );
    if all.len() >= MAX_SESSIONS
        && !all.contains_key(&key)
        && let Some(old) = all.keys().next().cloned()
    {
        all.remove(&old);
    }
    all.insert(key, snapshot);
}

/// Accept provider metadata even between turns; publish without relying on a turn's output sink.
pub fn receive(repo: &Utf8Path, lane: &str, value: Value) {
    let Ok(mut snapshot) = serde_json::from_value::<Snapshot>(value["snapshot"].clone()) else {
        return;
    };
    if !valid(&snapshot.provider, &snapshot.session) {
        return;
    }
    let previous = cache()
        .locked()
        .get(&(
            repo.to_owned(),
            snapshot.provider.clone(),
            snapshot.session.clone(),
        ))
        .cloned();
    if let Some(previous) = previous {
        let mut history = previous.evidence;
        for item in snapshot.evidence {
            if let Some(old) = history.iter_mut().find(|e| e["key"] == item["key"]) {
                *old = item;
            } else {
                history.push(item);
            }
        }
        snapshot.evidence = history;
        if snapshot.compacted_at.is_none() {
            snapshot.compacted_at = previous.compacted_at;
        }
    }
    snapshot.memory.truncate(MAX_ITEMS);
    snapshot.skills.truncate(MAX_ITEMS);
    snapshot.evidence = snapshot
        .evidence
        .into_iter()
        .rev()
        .take(MAX_ITEMS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    snapshot.categories.truncate(60);
    snapshot.limitations.truncate(20);
    remember(repo, snapshot.clone());
    crate::events::emit(
        "context.changed",
        None,
        json!({"lane":lane,"snapshot":snapshot}),
    );
    if value["persist"] == true {
        let repo = repo.to_owned();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = save(&repo, &snapshot) {
                tracing::debug!(%error, "Could not save context observations");
            }
        });
    }
}
fn save(repo: &Utf8Path, snapshot: &Snapshot) -> Result<(), String> {
    static WRITES: OnceLock<Mutex<()>> = OnceLock::new();
    let _held = WRITES.get_or_init(Mutex::default).locked();
    let latest = cache()
        .locked()
        .get(&(
            repo.to_owned(),
            snapshot.provider.clone(),
            snapshot.session.clone(),
        ))
        .cloned();
    if latest.is_some_and(|s| s.updated_at > snapshot.updated_at) {
        return Ok(());
    }
    let relative = format!(
        ".keel/turns/context/{}-{}.json",
        snapshot.provider, snapshot.session
    );
    crate::writes::no_link_under(repo, &relative)?;
    let target = path(repo, &snapshot.provider, &snapshot.session);
    let dir = target.parent().ok_or("No context directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    crate::writes::create_new(&repo.join(".keel/turns/.gitignore"), b"*\n")?;
    let tmp = dir.join(format!(".{}-{}.tmp", snapshot.session, request_id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec(snapshot).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    // Bound retention; only this module's own validated snapshot filenames can be removed.
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_type().is_ok_and(|t| t.is_file())
                && e.file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .is_some_and(|name| name.split_once('-').is_some_and(|(p, s)| valid(p, s)))
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    entries.sort_by_key(|e| e.0);
    let remove = entries.len().saturating_sub(MAX_SESSIONS);
    for (_, file) in entries.into_iter().take(remove) {
        let _ = std::fs::remove_file(file);
    }
    Ok(())
}
fn read_bounded(path: &Utf8Path, limit: u64) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Source is not a regular file".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Source is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Source exceeds the preview limit".into());
    }
    Ok(bytes)
}
fn saved(repo: &Utf8Path, provider: &str, session: &str) -> Snapshot {
    let key = (repo.to_owned(), provider.to_string(), session.to_string());
    if let Some(snapshot) = cache().locked().get(&key).cloned() {
        return snapshot;
    }
    let loaded = read_bounded(&path(repo, provider, session), MAX_BYTES)
        .ok()
        .and_then(|s| serde_json::from_slice::<Snapshot>(&s).ok())
        .filter(|s| s.provider == provider && s.session == session);
    loaded.unwrap_or_else(|| Snapshot {
        provider: provider.into(),
        session: session.into(),
        origin: "transcript".into(),
        ..Default::default()
    })
}

/// Seed a resumed runtime's evidence history before its first fresh provider event.
pub fn prime(repo: &Utf8Path, provider: &str, session: &str) {
    if valid(provider, session) {
        remember(repo, saved(repo, provider, session));
    }
}

#[derive(Deserialize)]
pub struct Request {
    pub lane: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub refresh: bool,
}

/// Inspect without spawning a provider, taking a checkout claim, or sending a user prompt.
pub async fn inspect(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Query(q): Query<Request>,
) -> Result<Json<Snapshot>, (StatusCode, String)> {
    if !["claude", "codex"].contains(&q.provider.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "Unknown provider".into()));
    }
    if !q.session.is_empty() && !valid(&q.provider, &q.session) {
        return Err((StatusCode::BAD_REQUEST, "Invalid session".into()));
    }
    let mut unavailable = None;
    if q.refresh
        && let Some(lane) = &q.lane
    {
        match crate::runtime::inspect_context(&state, lane).await {
            Ok(value)
                if value["provider"] == q.provider
                    && (q.session.is_empty() || value["session"] == q.session) =>
            {
                receive(&repo, lane, json!({"snapshot":value,"persist":true}));
                return serde_json::from_value(value)
                    .map(Json)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()));
            }
            Ok(_) => {
                unavailable = Some("The runtime is attached to a different session.".to_string())
            }
            Err(error) => unavailable = Some(error),
        }
    }
    let snapshot = crate::serve::in_blocking(move || {
        let mut snapshot = transcript::recover(&repo, &q.provider, &q.session);
        if let Some(error) = unavailable {
            limitation(
                &mut snapshot,
                &format!("Live refresh unavailable: {error}. Showing saved observations."),
            );
            if snapshot.origin == "runtime" {
                snapshot.origin = "saved_runtime".into();
            }
        }
        snapshot
    })
    .await;
    Ok(Json(snapshot))
}

#[derive(Deserialize)]
pub struct SourceRequest {
    provider: String,
    session: String,
    path: String,
}

/// Preview only a discovered source under the checkout or a provider's configuration roots.
pub async fn source(
    Checkout(repo): Checkout,
    Query(q): Query<SourceRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    crate::serve::in_blocking(move || preview(&repo, q))
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}
fn preview(repo: &Utf8Path, q: SourceRequest) -> Result<Value, String> {
    if !valid(&q.provider, &q.session) {
        return Err("Invalid session".into());
    }
    let snapshot = saved(repo, &q.provider, &q.session);
    if !snapshot
        .memory
        .iter()
        .chain(&snapshot.skills)
        .any(|v| v["path"] == q.path)
    {
        return Err("This file is not a discovered session source".into());
    }
    let file = Utf8PathBuf::from(q.path);
    let canonical = file.canonicalize_utf8().map_err(|e| e.to_string())?;
    let mut roots = vec![repo.to_owned()];
    if let Some(home) = keel_workspace::claude_home() {
        roots.push(home);
    }
    if let Ok(home) = std::env::var("HOME") {
        roots.push(Utf8PathBuf::from(&home).join(".codex"));
        roots.push(Utf8PathBuf::from(home).join(".agents"));
    }
    if let Ok(home) = std::env::var("CODEX_HOME") {
        roots.push(home.into());
    }
    let ancestor_instruction = canonical == file
        && file
            .file_name()
            .is_some_and(|n| ["AGENTS.md", "CLAUDE.md"].contains(&n))
        && repo.ancestors().any(|parent| Some(parent) == file.parent());
    if !ancestor_instruction
        && !roots
            .into_iter()
            .filter_map(|r| r.canonicalize_utf8().ok())
            .any(|r| canonical.starts_with(r))
    {
        return Err("Source is outside the permitted roots".into());
    }
    if canonical.extension() != Some("md") {
        return Err("Only Markdown context sources can be previewed".into());
    }
    let bytes = read_bounded(&canonical, SOURCE_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| "Source is not UTF-8")?;
    Ok(json!({"path":canonical,"text":text,"estimate":text.len().div_ceil(4)}))
}
