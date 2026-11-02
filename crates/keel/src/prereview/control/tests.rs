//! In-memory workers in scratch checkouts prove cancellation is a request and dismissal is conditional.
use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    root: Utf8PathBuf,
    id: String,
    lane: String,
}
impl Fixture {
    fn new(lane: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let id = crate::context::request_id();
        reviews().locked().push(Entry {
            review: Review {
                id: id.clone(),
                revision: 0,
                lane: lane.into(),
                wt: None,
                sha: "fixture".into(),
                base: "main".into(),
                started: now(),
                phase: "reviewing",
                opening: Opening::default(),
                by_agent: false,
                outcome: None,
            },
            dir: root.clone(),
            stop: Arc::new(AtomicBool::new(false)),
        });
        Self {
            _dir: dir,
            root,
            id,
            lane: lane.into(),
        }
    }
    fn control(&self) -> Control {
        Control {
            lane: self.lane.clone(),
            id: Some(self.id.clone()),
        }
    }
    fn finish(&self) {
        set(&self.id, &self.root, |r| {
            r.phase = "done";
            r.outcome = Some(Done::None {
                cause: "cancelled",
                line: "Stopped".into(),
                took: 0,
                cost: None,
            });
        });
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        reviews().locked().retain(|e| e.review.id != self.id);
    }
}

#[test]
fn cancellation_acknowledges_request_then_dismissal_waits_for_the_worker() {
    let f = Fixture::new("control-request");
    let response = request_stop(&f.root, &f.control()).unwrap();
    assert_eq!(response["cancel_requested"], true);
    assert_eq!(response["finished"], false);
    assert!(dismiss(&f.root, &f.control()).is_err());
    f.finish();
    assert_eq!(dismiss(&f.root, &f.control()).unwrap()["dismissed"], true);
    assert_eq!(dismiss(&f.root, &f.control()).unwrap()["dismissed"], true);
}

#[test]
fn a_stale_mutation_cannot_cancel_or_remove_the_replacement_review() {
    let f = Fixture::new("control-stale");
    let stale = Control {
        lane: f.lane.clone(),
        id: Some("old-review".into()),
    };
    assert!(request_stop(&f.root, &stale).is_err());
    assert!(dismiss(&f.root, &stale).is_err());
    let mut all = reviews().locked();
    let entry = find(&mut all, &f.lane, &f.root).unwrap();
    assert!(!entry.stop.load(Ordering::Relaxed));
    assert_eq!(entry.review.revision, 0);
}

#[test]
fn checkout_identity_and_run_id_protect_other_records_from_late_workers() {
    let first = Fixture::new("control-shared-lane");
    let second = Fixture::new("control-shared-lane");
    first.finish();
    set("old-run", &second.root, |r| r.phase = "done");
    let mut all = reviews().locked();
    assert!(
        find(&mut all, &second.lane, &second.root)
            .unwrap()
            .review
            .outcome
            .is_none()
    );
    assert!(
        find(&mut all, &first.lane, &first.root)
            .unwrap()
            .review
            .outcome
            .is_some()
    );
}

#[tokio::test(start_paused = true)]
async fn opening_waits_for_real_completion_after_request_acknowledgement() {
    let f = Fixture::new("control-confirm");
    let root = f.root.clone();
    let waiting = tokio::spawn(async move { cancel_for_open(&root).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    f.finish();
    tokio::time::advance(STOP_POLL).await;
    waiting.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn an_unfinished_worker_times_out_without_claiming_cancellation_or_dismissal() {
    let f = Fixture::new("control-timeout");
    let root = f.root.clone();
    let waiting = tokio::spawn(async move { cancel_for_open(&root).await });
    tokio::task::yield_now().await;
    tokio::time::advance(STOP_WAIT).await;
    assert!(
        waiting
            .await
            .unwrap()
            .unwrap_err()
            .contains("still stopping")
    );
    assert!(dismiss(&f.root, &f.control()).is_err());
}
