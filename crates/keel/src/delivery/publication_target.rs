//! Authenticated feedback reads use host-owned publication facts, never project-editable links.
use super::{Failure, Publication, Service};

/// Project history can display only publications whose intent and receipt agree in private storage.
pub(super) fn saved(store: &crate::execution::Store, task: &str) -> Option<Publication> {
    store
        .receipts
        .values()
        .filter(|r| r.kind == "publication" && r.task.as_deref() == Some(task))
        .filter_map(|r| {
            r.result
                .as_ref()
                .and_then(|v| serde_json::from_value::<Publication>(v.clone()).ok())
                .filter(|p| p.receipt == r.id && p.intent.task == task)
        })
        .max_by_key(|p| p.updated_at)
}
impl Service {
    /// A canonical, privately captured PR association is the only authority for GitHub feedback reads.
    pub(super) fn feedback_target(&self, task: &str) -> Result<String, Failure> {
        let store = self.execution.read().map_err(Failure::Unavailable)?;
        let publication = saved(&store, task).ok_or_else(|| {
            Failure::Invalid(
                "Publish this task's pull request on this computer before refreshing feedback."
                    .into(),
            )
        })?;
        let url = publication.url.ok_or_else(|| {
            Failure::Invalid("The pull request link is not confirmed yet.".into())
        })?;
        let (repository, _) = super::feedback::pr_link(&url)?;
        if !repository.eq_ignore_ascii_case(&publication.intent.repository) {
            return Err(Failure::Conflict(
                "Saved pull request repository does not match its captured target.".into(),
            ));
        }
        Ok(url)
    }
}
