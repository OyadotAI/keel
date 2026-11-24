//! Runtime and clock seams reuse real chat receipts while making launch/restart failures hermetic.
use crate::{
    agent::ChatQuery,
    chat::{ReceiptStatus, Submission},
    lock::Locked,
    serve::AppState,
    tasks::Source,
};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Weak};

/// Unix milliseconds are injected so persisted due times and operation timestamps are deterministic.
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> u64;
}
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// A rejected check did not run; an interrupted controller cannot make that assurance.
#[derive(Debug)]
pub(crate) enum VerifyError {
    Before(String),
    Unknown(String),
}

/// Durable task work uses one submission/observation path, never terminal typing or hidden HTTP calls.
pub(crate) trait Runtime: Send + Sync {
    fn current(&self) -> bool;
    fn available(&self, provider: &str) -> bool;
    fn pending(&self, owner: &Source) -> bool;
    fn submit(&self, id: String, query: ChatQuery) -> BoxFuture<'_, Result<Submission, String>>;
    fn receipt(&self, lane: String, id: String) -> BoxFuture<'_, Result<ReceiptStatus, String>>;
    fn busy(&self, owner: &Source) -> BoxFuture<'_, Result<bool, String>>;
    fn stop(&self, owner: Source) -> BoxFuture<'_, Result<bool, String>>;
    fn verify(
        &self,
        owner: Source,
        id: String,
    ) -> BoxFuture<'_, Result<super::CheckEvidence, VerifyError>>;
    fn stop_check(&self, id: String) -> BoxFuture<'_, Result<bool, String>>;
    fn feedback_head(
        &self,
        repository: String,
        number: u64,
    ) -> BoxFuture<'_, Result<String, String>>;
    fn capture_publication(
        &self,
        task: String,
        owner: Source,
        identity: super::CheckoutIdentity,
        form: super::PrForm,
    ) -> BoxFuture<'_, Result<super::PublicationIntent, String>>;
    fn publish(
        &self,
        intent: super::PublicationIntent,
    ) -> BoxFuture<'_, Result<(String, String), String>>;
}

pub(super) struct Live {
    pub state: Weak<AppState>,
    pub root: camino::Utf8PathBuf,
}
impl Live {
    fn state(&self) -> Result<Arc<AppState>, String> {
        let state = self
            .state
            .upgrade()
            .ok_or("The project service has closed.")?;
        if state.repo() != self.root {
            return Err("The project changed. Reopen this task in its original project.".into());
        }
        Ok(state)
    }
}
impl Runtime for Live {
    fn feedback_head(
        &self,
        repository: String,
        number: u64,
    ) -> BoxFuture<'_, Result<String, String>> {
        let root = self.root.clone();
        Box::pin(async move {
            crate::serve::in_blocking(move || {
                use crate::github_read::Read;
                let mut reader = crate::github_read::Gh::new(&root);
                let value = reader.rest(&format!("repos/{repository}/pulls/{number}"))?;
                value["head"]["sha"]
                    .as_str()
                    .filter(|sha| crate::github_read::valid_sha(sha))
                    .map(str::to_string)
                    .ok_or("Pull request head could not be confirmed.".into())
            })
            .await
        })
    }
    fn current(&self) -> bool {
        self.state().is_ok()
    }
    fn available(&self, provider: &str) -> bool {
        crate::agent::PROVIDERS.contains(&provider)
            && crate::permissions::find_on_path(crate::agent::program_of(provider)).is_some()
    }
    fn pending(&self, owner: &Source) -> bool {
        crate::approve::queue().locked().iter().any(|p| {
            p.lane == owner.lane || (!owner.session.is_empty() && p.session_id == owner.session)
        })
    }
    fn submit(&self, id: String, query: ChatQuery) -> BoxFuture<'_, Result<Submission, String>> {
        Box::pin(async move {
            crate::chat::submit(self.state()?, id, query)
                .await
                .map_err(|e| e.1)
        })
    }
    fn receipt(&self, lane: String, id: String) -> BoxFuture<'_, Result<ReceiptStatus, String>> {
        Box::pin(async move {
            let state = self.state()?;
            crate::serve::in_blocking(move || {
                crate::chat::receipt_status(&state, &lane, &id).map_err(|e| e.1)
            })
            .await
        })
    }
    fn busy(&self, owner: &Source) -> BoxFuture<'_, Result<bool, String>> {
        let owner = owner.clone();
        Box::pin(async move {
            let state = self.state()?;
            crate::serve::in_blocking(move || {
                Ok(state.has_turn(&owner.lane)
                    || state
                        .writer_of(camino::Utf8Path::new(&owner.checkout))
                        .is_some()
                    || crate::term::live(&owner.session))
            })
            .await
        })
    }
    fn stop(&self, owner: Source) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move { Ok(crate::runtime::stop(&self.state()?, &owner.lane).await) })
    }
    fn verify(
        &self,
        owner: Source,
        id: String,
    ) -> BoxFuture<'_, Result<super::CheckEvidence, VerifyError>> {
        Box::pin(async move {
            let state = self.state().map_err(VerifyError::Before)?;
            tokio::spawn(async move {
                crate::verify::check_owned(
                    state,
                    owner.checkout.into(),
                    Some(owner.lane),
                    Some(id),
                    |_| {},
                    || false,
                )
                .await
            })
            .await
            .map_err(|_| {
                VerifyError::Unknown(
                    "The check controller ended before completion was confirmed.".into(),
                )
            })?
            .map_err(VerifyError::Before)
        })
    }
    fn stop_check(&self, id: String) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move { Ok(self.state()?.stop_check(&id)) })
    }
    fn capture_publication(
        &self,
        task: String,
        owner: Source,
        identity: super::CheckoutIdentity,
        form: super::PrForm,
    ) -> BoxFuture<'_, Result<super::PublicationIntent, String>> {
        Box::pin(async move {
            crate::serve::in_blocking(move || {
                crate::pr::capture_delivery(task, owner, identity, form)
            })
            .await
        })
    }
    fn publish(
        &self,
        intent: super::PublicationIntent,
    ) -> BoxFuture<'_, Result<(String, String), String>> {
        Box::pin(async move { crate::pr::publish_delivery(self.state()?, intent).await })
    }
}
