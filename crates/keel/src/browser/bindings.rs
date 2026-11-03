//! Opaque per-process browser bindings keep saved preferences from redirecting running conversations.
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
    pub backend: Backend,
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
/// Capture on launch/resume only; status reads and preference saves do not mutate active sessions.
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
    let backend = preference::capture(&project, provider, session, resume)?;
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
            backend,
            page: Default::default(),
        }),
    );
    drop(registry);
    changed(lane);
    Ok(())
}
fn changed(lane: &str) {
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
        preference::alias(
            &binding.project,
            &binding.provider,
            session,
            binding.backend,
        )?;
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
pub(crate) fn instructions(lane: Option<&str>) -> String {
    match lane.and_then(get).map(|b|b.backend).unwrap_or_default(){
        Backend::Embedded=>"This session uses Inside Keel. Use only Keel browser_* tools; its embedded page has this project’s sign-ins, separate from local Oya. Never switch browser providers when a call fails.".into(),
        Backend::LocalOya=>"This session uses Local Oya Browser with its existing signed-in profile, through Keel browser_* tools and the project’s permission gate. Open an owned page first; personal tabs are unavailable. Read the page before using element IDs. Respect Human Mode and report connection failures without switching browsers or replaying uncertain actions.".into(),
    }
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
#[derive(Serialize)]
pub(crate) struct Effective {
    pub backend: Backend,
    pub binding: Option<String>,
    pub url: Option<String>,
    pub busy: bool,
}
pub(crate) fn effective(lane: Option<&str>) -> Effective {
    let Some(binding) = lane.and_then(get) else {
        return Effective {
            backend: Backend::Embedded,
            binding: None,
            url: None,
            busy: false,
        };
    };
    let (url, busy) = match binding.page.try_lock() {
        Ok(page) => (page.url.clone(), false),
        Err(_) => (None, true),
    };
    Effective {
        backend: binding.backend,
        binding: Some(binding.id.clone()),
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
            backend: Backend::LocalOya,
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
