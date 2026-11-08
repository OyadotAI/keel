//! Opaque per-process browser bindings: which session a lane's browser calls belong to. Which
//! browser runs a call is the lane's choice, resolved per call (`preference::resolve`).
use super::{
    Op, connection, detection,
    local::Page,
    preference::{self, Backend},
};
use crate::lock::Locked;
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
const BINDING_LIMIT: usize = 256;
const NONCE_BYTES: usize = 24;
pub(super) struct Binding {
    pub id: String,
    pub lane: String,
    pub session: Mutex<String>,
    pub project: Utf8PathBuf,
    pub provider: String,
    pub page: tokio::sync::Mutex<Page>,
}
#[derive(Default)]
struct Registry {
    by_lane: HashMap<String, Arc<Binding>>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
/// On launch/resume: a fresh unguessable id ties this lane's browser calls to this session.
pub(crate) fn prepare(
    project: &Utf8Path,
    lane: &str,
    provider: &str,
    session: &str,
    resume: bool,
) -> Result<(), String> {
    let project = project.real().map_err(|e| e.to_string())?;
    let mut registry = registry().locked();
    if let Some(old) = registry.by_lane.get(lane)
        && old.project == project
        && old.provider == provider
        && (crate::term::owned_pid(lane).is_some()
            || (resume && old.session.locked().as_str() == session))
    {
        return Ok(());
    }
    if registry.by_lane.len() >= BINDING_LIMIT && !registry.by_lane.contains_key(lane) {
        return Err(
            "Browser session capacity reached. Close an inactive lane before starting another."
                .into(),
        );
    }
    let mut random = [0; NONCE_BYTES];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let id = random.iter().map(|b| format!("{b:02x}")).collect();
    registry.by_lane.insert(
        lane.into(),
        Arc::new(Binding {
            id,
            lane: lane.into(),
            session: Mutex::new(session.into()),
            project,
            provider: provider.into(),
            page: Default::default(),
        }),
    );
    drop(registry);
    changed(lane);
    Ok(())
}
pub(super) fn changed(lane: &str) {
    crate::events::emit("browser.binding", None, serde_json::json!({"lane":lane}));
}
pub(crate) fn id(lane: &str) -> Option<String> {
    registry().locked().by_lane.get(lane).map(|b| b.id.clone())
}
pub(super) fn get(lane: &str) -> Option<Arc<Binding>> {
    registry().locked().by_lane.get(lane).cloned()
}
/// A missing marker is accepted only for legacy embedded sessions that have no new binding.
pub(super) fn authorize(
    project: &Utf8Path,
    lane: &str,
    id: Option<&str>,
) -> Result<Option<Arc<Binding>>, String> {
    let binding = get(lane);
    let project = project.real().map_err(|e| e.to_string())?;
    match binding {
        Some(binding) if id==Some(binding.id.as_str())&&binding.project==project=>Ok(Some(binding)),
        None if id.is_none()=>Ok(None),
        _=>Err("This browser request belongs to an expired or different session. Reopen the intended conversation; no browser action ran.".into()),
    }
}
pub(crate) fn alias(lane: &str, session: &str) -> Result<(), String> {
    if let Some(binding) = get(lane) {
        *binding.session.locked() = session.into();
    }
    Ok(())
}
pub(crate) fn release(lane: &str, id: Option<&str>) {
    let mut registry = registry().locked();
    if registry
        .by_lane
        .get(lane)
        .is_some_and(|binding| Some(binding.id.as_str()) == id)
    {
        registry.by_lane.remove(lane);
        drop(registry);
        changed(lane);
    }
}
/// The same words whichever browser the lane uses: the person chooses it, per lane, and can change
/// it between two calls, so text naming one would be wrong the moment they switch.
pub(crate) fn instructions(_lane: Option<&str>) -> String {
    "Browse only with Keel's browser_* tools. The person chooses, per lane, whether they run inside \
     Keel or in their own Oya Browser, and Keel may ask them the first time; never switch browsers \
     yourself, and when a call fails, report it rather than replaying an uncertain action."
        .into()
}

/// The adapter is serialized per binding; extra concurrent operations fail before producing effects.
pub(super) async fn local(binding: Arc<Binding>, op: Op) -> Result<String, String> {
    let mut page = binding
        .page
        .try_lock()
        .map_err(|_| "Another browser operation is still pending in this session.")?;
    if !page.connected() {
        let project = binding.project.clone();
        let connection = crate::serve::in_blocking(move || {
            let installation = detection::installed()?.ok_or(
                "Your selected Oya Browser is no longer available. Check Browser settings.",
            )?;
            connection::discover(&project, &installation)
        })
        .await?;
        page.connect(connection::Client::new(connection)?).await?;
    }
    let navigated = matches!(&op, Op::Open { .. });
    let result = page.run(op).await;
    drop(page);
    if navigated {
        changed(&binding.lane);
    }
    result
}
/// What the Browser tab shows for a lane: the browser its next call would use — `None` when it
/// has not chosen and will ask — and whether that is the lane's own choice or the device's.
#[derive(Serialize)]
pub(crate) struct Effective {
    pub backend: Option<Backend>,
    /// `lane`, `default` (the device's "Always …"), or `none` (asks on the next browser step).
    pub source: &'static str,
    pub binding: Option<String>,
    pub url: Option<String>,
    pub busy: bool,
}
pub(crate) fn effective(project: &Utf8Path, lane: Option<&str>) -> Effective {
    let own = lane.and_then(|l| preference::lane_only(project, l).ok().flatten());
    let backend = lane.and_then(|l| preference::resolve(project, l).ok().flatten());
    let source = match (own, backend) {
        (Some(_), _) => "lane",
        (None, Some(_)) => "default",
        _ => "none",
    };
    let binding = lane.and_then(get);
    let (url, busy) = match binding.as_ref().map(|b| b.page.try_lock()) {
        Some(Ok(page)) => (page.url.clone(), false),
        Some(Err(_)) => (None, true),
        None => (None, false),
    };
    Effective {
        backend,
        source,
        binding: binding.map(|b| b.id.clone()),
        url,
        busy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_and_foreign_bindings_cannot_control_the_current_lane() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let project = Utf8Path::from_path(root.path()).unwrap().real().unwrap();
        let lane = "browser-binding-fixture";
        let binding = Arc::new(Binding {
            id: "current".into(),
            lane: lane.into(),
            session: Mutex::new("session".into()),
            project: project.clone(),
            provider: "codex".into(),
            page: Default::default(),
        });
        registry().locked().by_lane.insert(lane.into(), binding);
        assert!(authorize(&project, lane, Some("current")).is_ok());
        assert!(authorize(&project, lane, Some("expired")).is_err());
        assert!(authorize(&project, lane, None).is_err());
        assert!(
            authorize(
                Utf8Path::from_path(other.path()).unwrap(),
                lane,
                Some("current")
            )
            .is_err()
        );
        release(lane, Some("expired"));
        assert!(get(lane).is_some());
        release(lane, Some("current"));
        assert!(get(lane).is_none());
    }
}
