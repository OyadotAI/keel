//! Controlled runtime/clock seams exercise the real task repository and scratch Git worktrees.
use super::{
    Service, StartIssue,
    runtime::{Clock, Runtime},
};
use crate::{
    agent::ChatQuery,
    chat::{ReceiptStatus, Submission},
    lock::Locked,
    tasks::Source,
};
use camino::Utf8PathBuf;
use futures_util::future::BoxFuture;
use keel_workspace::Real;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(crate) struct TestClock(pub AtomicU64);
impl Clock for TestClock {
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
#[derive(Default)]
pub(crate) struct FakeRuntime {
    pub calls: Mutex<Vec<(String, ChatQuery)>>,
    pub unavailable: AtomicBool,
    pub error_after_accept: AtomicBool,
    pub unknown: AtomicBool,
    pub running: AtomicBool,
    pub waiting: AtomicBool,
    pub root: Mutex<Option<Utf8PathBuf>>,
    pub remote_head: Mutex<Option<String>>,
    pub feedback_requests: Mutex<Vec<(String, u64)>>,
    pub capture_fails: AtomicBool,
    pub publish_fails: AtomicBool,
    pub published_head: Mutex<Option<String>>,
    pub available_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Runtime for FakeRuntime {
    fn feedback_head(
        &self,
        repository: String,
        number: u64,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.feedback_requests.locked().push((repository, number));
            self.remote_head
                .locked()
                .clone()
                .ok_or("Controlled remote read failure".into())
        })
    }
    fn current(&self) -> bool {
        true
    }
    fn available(&self, _provider: &str) -> bool {
        let hook = self.available_hook.locked().clone();
        if let Some(hook) = hook {
            hook();
        }
        !self.unavailable.load(Ordering::SeqCst)
    }
    fn pending(&self, _owner: &Source) -> bool {
        self.waiting.load(Ordering::SeqCst)
    }
    fn submit(&self, id: String, query: ChatQuery) -> BoxFuture<'_, Result<Submission, String>> {
        Box::pin(async move {
            let mut calls = self.calls.locked();
            if !calls.iter().any(|(old, _)| old == &id) {
                calls.push((id.clone(), query));
            }
            if self.error_after_accept.load(Ordering::SeqCst) {
                return Err("Controlled lost acceptance response".into());
            }
            Ok(Submission {
                id,
                accepted: true,
                running: self.running.load(Ordering::SeqCst),
            })
        })
    }
    fn receipt(&self, _lane: String, id: String) -> BoxFuture<'_, Result<ReceiptStatus, String>> {
        Box::pin(async move {
            if self.unknown.load(Ordering::SeqCst) {
                return Err("Controlled receipt read failure".into());
            }
            let accepted = self.calls.locked().iter().any(|(old, _)| old == &id);
            let running = self.running.load(Ordering::SeqCst);
            Ok(ReceiptStatus {
                accepted,
                running,
                finished: accepted && !running,
                interrupted: false,
                error: None,
            })
        })
    }
    fn busy(&self, _owner: &Source) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move { Ok(self.running.load(Ordering::SeqCst)) })
    }
    fn stop(&self, _owner: Source) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move {
            self.running.store(false, Ordering::SeqCst);
            Ok(true)
        })
    }
    fn verify(
        &self,
        owner: Source,
        id: String,
    ) -> BoxFuture<'_, Result<super::CheckEvidence, super::runtime::VerifyError>> {
        Box::pin(async move {
            let checkout = camino::Utf8Path::new(&owner.checkout);
            let mut check = super::begin_check(
                checkout,
                format!("manual:{}", owner.lane),
                Some("controlled check".into()),
            )
            .map_err(|e| super::runtime::VerifyError::Before(e.message().into()))?;
            check.id = id;
            super::end_check(
                checkout,
                &mut check,
                "passed",
                Some("controlled check".into()),
                vec![],
            );
            let root = self.root.locked().clone().unwrap();
            super::persist_check(&root, &owner.session, checkout, &check)
                .map_err(super::runtime::VerifyError::Unknown)?;
            Ok(check)
        })
    }
    fn stop_check(&self, _id: String) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }
    fn capture_publication(
        &self,
        task: String,
        owner: Source,
        identity: super::CheckoutIdentity,
        form: super::PrForm,
    ) -> BoxFuture<'_, Result<super::PublicationIntent, String>> {
        Box::pin(async move {
            if self.capture_fails.load(Ordering::SeqCst) {
                return Err("Controlled hosting capture failure".into());
            }
            Ok(super::PublicationIntent {
                id: "intent".into(),
                task,
                owner,
                repository: "owner/repo".into(),
                source_repository: "owner/repo".into(),
                push_identity: "push".into(),
                branch: "test".into(),
                base: form.base.clone(),
                identity,
                form,
            })
        })
    }
    fn publish(
        &self,
        intent: super::PublicationIntent,
    ) -> BoxFuture<'_, Result<(String, String), String>> {
        Box::pin(async move {
            if self.publish_fails.load(Ordering::SeqCst) {
                return Err("Controlled publication failure".into());
            }
            Ok((
                "https://github.com/owner/repo/pull/1".into(),
                self.published_head
                    .locked()
                    .clone()
                    .unwrap_or(intent.identity.head),
            ))
        })
    }
}
pub(crate) struct Fixture {
    pub _dir: tempfile::TempDir,
    pub root: Utf8PathBuf,
    pub service: Arc<Service>,
    pub runtime: Arc<FakeRuntime>,
    pub clock: Arc<TestClock>,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .real()
            .unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("README.md"), "original\n").unwrap();
        std::fs::write(root.join("Makefile"), "check:\n\ttrue\n").unwrap();
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Delivery tests",
                "-c",
                "user.email=delivery@example.invalid",
                "commit",
                "-qm",
                "initial",
            ],
        );
        let runtime = Arc::new(FakeRuntime::default());
        let clock = Arc::new(TestClock(AtomicU64::new(1000)));
        *runtime.root.locked() = Some(root.clone());
        let service = Arc::new(Service::new(&root, runtime.clone(), clock.clone()).unwrap());
        Self {
            _dir: dir,
            root,
            service,
            runtime,
            clock,
        }
    }
    pub fn issue(&self) -> StartIssue {
        let mut reader = IssueReader;
        let project = crate::github_read::Project {
            source: "owner/repo".into(),
            parent: None,
            push_identity: "push".into(),
        };
        let preview = self
            .service
            .preview(
                "https://github.com/owner/repo/issues/1",
                &mut reader,
                &project,
            )
            .unwrap();
        StartIssue {
            receipt: "start-1".into(),
            preview: preview.id,
            provider: "codex".into(),
            base: "main".into(),
        }
    }
}
struct IssueReader;
impl crate::github_read::Read for IssueReader {
    fn rest(&mut self, _path: &str) -> Result<Value, String> {
        Ok(
            json!({"node_id":"I1","number":1,"html_url":"https://github.com/owner/repo/issues/1","title":"Fix checkout","body":"Preserve receipt identity.","state":"open","updated_at":"2026-11-23T00:00:00Z"}),
        )
    }
    fn graphql(&mut self, _query: &str, _variables: Value) -> Result<Value, String> {
        Err("Unexpected GraphQL read".into())
    }
}
pub(super) fn git(root: &camino::Utf8Path, args: &[&str]) -> String {
    crate::git::run(root, args).unwrap().trim().to_string()
}
