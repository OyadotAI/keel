//! Bounded acknowledgements make a lost question-answer HTTP response safe to retry.
use crate::lock::Locked;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_RECEIPTS: usize = 128;
const RETENTION: Duration = Duration::from_secs(600);
struct Receipt {
    id: String,
    fingerprint: String,
    at: Instant,
}

fn receipts() -> &'static Mutex<VecDeque<Receipt>> {
    static RECEIPTS: OnceLock<Mutex<VecDeque<Receipt>>> = OnceLock::new();
    RECEIPTS.get_or_init(Mutex::default)
}

/// Deliver once while accepting identical retries; only actual receipt can create a fact.
pub fn deliver(
    id: &str,
    answer: &str,
    send: impl FnOnce() -> Option<String>,
) -> (Value, Option<String>) {
    let fingerprint = format!("{:x}", Sha256::digest(answer.as_bytes()));
    let mut cache = receipts().locked();
    cache.retain(|r| r.at.elapsed() < RETENTION);
    if let Some(old) = cache.iter().find(|r| r.id == id) {
        return (
            json!({"ok":old.fingerprint == fingerprint, "closed":true}),
            None,
        );
    }
    let Some(lane) = send() else {
        return (
            json!({"ok":false,"closed":true,"reason":"no longer waiting"}),
            None,
        );
    };
    cache.push_back(Receipt {
        id: id.into(),
        fingerprint,
        at: Instant::now(),
    });
    while cache.len() > MAX_RECEIPTS {
        cache.pop_front();
    }
    (json!({"ok":true}), Some(lane))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_lost_http_response_can_be_retried_without_delivering_twice() {
        let (first, lane) = deliver("receipt-test", "custom answer", || Some("lane".into()));
        assert_eq!(first["ok"], true);
        assert_eq!(lane.as_deref(), Some("lane"));
        let (retry, lane) = deliver("receipt-test", "custom answer", || {
            panic!("delivered twice")
        });
        assert_eq!(retry["ok"], true);
        assert!(lane.is_none());
        let (changed, _) = deliver("receipt-test", "different answer", || {
            panic!("delivered twice")
        });
        assert_eq!(changed["ok"], false);
    }
}
