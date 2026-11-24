//! Project service owns dependencies and controller lifetime; operation families live beside it.
pub(crate) use super::composition::{StartIssue, capture, id, service};
pub(crate) use super::view::TaskView;
use super::{
    Failure,
    repository::Repository,
    runtime::{Clock, Runtime},
};
use crate::{lock::Locked, tasks::Source};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use std::sync::{Arc, Mutex, Weak};
/// Project-lifetime dependencies are shared by requests and the single recurring dispatcher.
pub(crate) struct Service {
    pub(super) root: Utf8PathBuf,
    pub(super) records: Repository,
    pub(super) execution: crate::execution::Repository,
    pub(super) runtime: Arc<dyn Runtime>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) generation: String,
    pub(super) active: Mutex<std::collections::BTreeSet<String>>,
    pub(super) initial_active: Mutex<Option<Option<String>>>,
    pub(super) controller_error: Mutex<Option<String>>,
    pub(super) controller_revision: Mutex<Option<u64>>,
}
impl Service {
    /// Dependency injection belongs at project ownership, where storage and runtime can both fail.
    pub(crate) fn new(
        root: &Utf8Path,
        runtime: Arc<dyn Runtime>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, Failure> {
        let root = root
            .real()
            .map_err(|e| Failure::Unavailable(e.to_string()))?;
        let execution = crate::execution::Repository::new(&root).map_err(Failure::Unavailable)?;
        let initial_active = execution.read().ok().map(|store| store.active);
        Ok(Self {
            records: Repository::new(&root).map_err(Failure::Unavailable)?,
            execution,
            root,
            runtime,
            clock,
            generation: id()?,
            active: Default::default(),
            initial_active: Mutex::new(initial_active),
            controller_error: Mutex::new(None),
            controller_revision: Mutex::new(None),
        })
    }

    pub(super) fn owner(&self, id: &str) -> Result<Source, Failure> {
        crate::tasks::recover(&self.root, id).map_err(Failure::Conflict)
    }

    pub(super) fn track(self: &Arc<Self>, id: &str) -> Active {
        self.active.locked().insert(id.into());
        Active {
            service: Arc::downgrade(self),
            id: id.into(),
        }
    }
}

pub(super) struct Active {
    service: Weak<Service>,
    id: String,
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Some(service) = self.service.upgrade() {
            service.active.locked().remove(&self.id);
        }
    }
}
