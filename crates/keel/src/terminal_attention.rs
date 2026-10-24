//! Codex's native OSC 7501 status, observed by the PTY owner even for hidden lanes. Status
//! changes resolve attention; keystrokes, terminal prose, and generic bells never do.

use crate::lock::Locked;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

const MAX_OSC_BYTES: usize = 1024;
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The native response remains in the terminal; this state supplies indicators only.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Permission,
    Question,
}

/// Stable for one unresolved request, so redraws and reconnects cannot repeat notifications.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attention {
    pub id: String,
    pub kind: Kind,
}

type Entries = HashMap<String, (u64, Option<Attention>)>;
static REVISION: AtomicU64 = AtomicU64::new(0);
fn entries() -> &'static Mutex<Entries> {
    static ENTRIES: OnceLock<Mutex<Entries>> = OnceLock::new();
    ENTRIES.get_or_init(Default::default)
}

/// Active attention for a reconnect, without replaying a notification about existing work.
pub fn snapshot() -> Snapshot {
    let all = entries().locked();
    let lanes = all
        .iter()
        .filter_map(|(lane, (_, a))| a.clone().map(|a| (lane.clone(), a)))
        .collect();
    Snapshot {
        revision: REVISION.load(Ordering::Relaxed),
        lanes,
    }
}

/// The watermark prevents queued events older than the reconnect snapshot from restoring a
/// resolved prompt or sending a historical notification.
#[derive(Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub lanes: HashMap<String, Attention>,
}

struct Lease {
    lane: String,
    token: u64,
}

/// A PTY lifetime owns its state; an old reader cannot clear a replacement terminal's request.
#[derive(Clone)]
pub struct Owner(Arc<Lease>);

impl Owner {
    /// Register this terminal before its output starts, replacing any state from its old process.
    pub fn new(lane: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let lease = Lease {
            lane: lane.into(),
            token: NEXT.fetch_add(1, Ordering::Relaxed),
        };
        let revision = {
            let mut all = entries().locked();
            all.insert(lane.into(), (lease.token, None));
            REVISION.fetch_add(1, Ordering::Relaxed) + 1
        };
        publish(lane, None, revision);
        Self(Arc::new(lease))
    }

    /// Apply only a structured native state transition, preserving the request id on repeats.
    pub fn observe(&self, kind: Option<Kind>) {
        let (next, revision) = {
            let mut all = entries().locked();
            let Some((token, current)) = all.get_mut(&self.0.lane) else {
                return;
            };
            if *token != self.0.token || current.as_ref().map(|a| a.kind) == kind {
                return;
            }
            *current = kind.map(|kind| Attention {
                id: format!(
                    "native-{}-{}",
                    self.0.token,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ),
                kind,
            });
            (
                current.clone(),
                REVISION.fetch_add(1, Ordering::Relaxed) + 1,
            )
        };
        publish(&self.0.lane, next, revision);
    }

    /// Exit removes attention even if the CLI dies before printing its normal clear sequence.
    pub fn close(&self) {
        self.0.close();
    }
}

impl Lease {
    fn close(&self) {
        let mut all = entries().locked();
        if all
            .get(&self.lane)
            .is_none_or(|(token, _)| *token != self.token)
        {
            return;
        }
        all.remove(&self.lane);
        let revision = REVISION.fetch_add(1, Ordering::Relaxed) + 1;
        drop(all);
        publish(&self.lane, None, revision);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.close();
    }
}

fn publish(lane: &str, attention: Option<Attention>, revision: u64) {
    crate::events::emit(
        "terminal.attention",
        None,
        serde_json::json!({ "lane": lane, "attention": attention, "revision": revision }),
    );
}

#[derive(Default)]
enum State {
    #[default]
    Text,
    Escape,
    Osc,
    Terminator,
}

/// A bounded incremental OSC parser; the original output bytes still pass to the terminal.
#[derive(Default)]
pub struct Parser {
    state: State,
    payload: Vec<u8>,
    overflow: bool,
}

impl Parser {
    /// Decode complete supported status sequences across arbitrary PTY read boundaries.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Option<Kind>> {
        let mut changes = Vec::new();
        for &byte in bytes {
            match self.state {
                State::Text if byte == ESC => self.state = State::Escape,
                State::Escape if byte == b']' => {
                    self.payload.clear();
                    self.overflow = false;
                    self.state = State::Osc;
                }
                State::Escape if byte != ESC => self.state = State::Text,
                State::Osc if byte == BEL => self.finish(&mut changes),
                State::Osc if byte == ESC => self.state = State::Terminator,
                State::Osc => self.push(byte),
                State::Terminator if byte == b'\\' || byte == BEL => self.finish(&mut changes),
                State::Terminator => {
                    self.push(ESC);
                    self.push(byte);
                    self.state = State::Osc;
                }
                _ => {}
            }
        }
        changes
    }

    fn push(&mut self, byte: u8) {
        if self.payload.len() < MAX_OSC_BYTES {
            self.payload.push(byte);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self, changes: &mut Vec<Option<Kind>>) {
        if !self.overflow
            && let Some(change) = status(&self.payload)
        {
            changes.push(change);
        }
        self.state = State::Text;
        self.payload.clear();
    }
}

fn status(payload: &[u8]) -> Option<Option<Kind>> {
    // Captured on native Codex 0.164: blocked permission -> working after an actual answer.
    match payload {
        b"7501;state=blocked:app=codex:kind=permission" => Some(Some(Kind::Permission)),
        b"7501;state=blocked:app=codex:kind=question" => Some(Some(Kind::Question)),
        b"7501;state=idle:app=codex" | b"7501;state=working:app=codex" | b"7501;state=clear" => {
            Some(None)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These are captured status payloads, not guessed terminal copy or generic OSC 9 messages.
    #[test]
    fn fragmented_native_status_opens_and_resolves_attention() {
        let fixture = b"\x1b]7501;state=blocked:app=codex:kind=permission\x1b\\\x1b]7501;state=working:app=codex\x07";
        let mut parser = Parser::default();
        let changes: Vec<_> = fixture.iter().flat_map(|b| parser.feed(&[*b])).collect();
        assert_eq!(changes, [Some(Kind::Permission), None]);
    }

    #[test]
    fn prose_bells_unknown_states_and_oversized_payloads_are_not_attention() {
        let mut parser = Parser::default();
        assert!(parser.feed(b"Would you like to run?\x07\x1b]9;Approval requested: example\x07\x1b]7501;state=blocked:app=other:kind=permission\x07").is_empty());
        let mut oversized = b"\x1b]7501;state=blocked:app=codex:kind=permission".to_vec();
        oversized.extend(vec![b'x'; MAX_OSC_BYTES]);
        oversized.push(BEL);
        assert!(parser.feed(&oversized).is_empty());
        assert_eq!(parser.feed(b"\x1b]7501;state=clear\x1b\\"), [None]);
    }

    #[test]
    fn repeated_requests_reconnect_and_old_process_exit_preserve_current_identity() {
        let owner = Owner::new("attention-fixture");
        owner.observe(Some(Kind::Permission));
        let first = snapshot().lanes["attention-fixture"].clone();
        owner.observe(Some(Kind::Permission));
        assert_eq!(snapshot().lanes["attention-fixture"], first);
        owner.observe(None);
        owner.observe(Some(Kind::Permission));
        assert_ne!(snapshot().lanes["attention-fixture"].id, first.id);
        let replacement = Owner::new("attention-fixture");
        replacement.observe(Some(Kind::Question));
        owner.close();
        owner.observe(None);
        assert_eq!(snapshot().lanes["attention-fixture"].kind, Kind::Question);
        replacement.close();
        assert!(!snapshot().lanes.contains_key("attention-fixture"));
    }
}
