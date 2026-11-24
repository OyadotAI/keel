//! Runtime composition and identity capture are shared by API and daemon controllers.
use super::constants::NONCE_BYTES;
use super::{
    CheckoutIdentity, Failure, Service,
    runtime::{Live, SystemClock},
};
use crate::{lock::Locked, serve::AppState};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
/// The composition root caches one service per canonical project without retaining AppState through runtime.
pub(crate) fn service(state: &Arc<AppState>) -> Result<Arc<Service>, Failure> {
    let root = state.repo();
    let mut all = state.deliveries.locked();
    if let Some(service) = all.get(root.as_str()) {
        return Ok(service.clone());
    }
    let runtime = Arc::new(Live {
        state: Arc::downgrade(state),
        root: root.clone(),
    });
    let service = Arc::new(Service::new(&root, runtime, Arc::new(SystemClock))?);
    all.insert(root.to_string(), service.clone());
    Service::start_runner(&service);
    Ok(service)
}

pub(crate) fn id() -> Result<String, Failure> {
    let mut bytes = [0; NONCE_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|_| Failure::Unavailable("Could not create a secure task identity.".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Content capture preserves staging and returns an error instead of a misleading empty fingerprint.
pub(crate) fn capture(checkout: &Utf8Path) -> Result<CheckoutIdentity, String> {
    let head = crate::git::trimmed(checkout, &["rev-parse", "HEAD"])?;
    let tree = crate::snapshot::snapshot(checkout)?;
    if !crate::github_read::valid_sha(&head) || !crate::github_read::valid_sha(&tree) {
        return Err("Could not confirm checkout identity.".into());
    }
    Ok(CheckoutIdentity { head, tree })
}

/// Launch input is frozen independently of random server identities so identical retries remain identical.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartIssue {
    pub receipt: String,
    pub preview: String,
    pub provider: String,
    pub base: String,
}
