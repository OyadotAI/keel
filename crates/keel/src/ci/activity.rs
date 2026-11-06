//! Bounded focused-review activity never logs source contents or trusts prose as a verdict.
use super::clock;
use serde_json::Value;
use std::time::{Duration, Instant};
const FRAME_BYTES: usize = 2 * 1024 * 1024;
const RESULT_BYTES: usize = 1024 * 1024;
const ID_BYTES: usize = 128;
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const HEARTBEAT: Duration = Duration::from_secs(60);

pub(super) struct Activity {
    since: Instant,
    partial: Vec<u8>,
    oversized: bool,
    session: Option<String>,
    result: Option<Value>,
    invalid_result: bool,
    last_activity: Option<(&'static str, Duration)>,
    last_printed: Option<Duration>,
}
impl Activity {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::since(Instant::now())
    }
    pub(super) fn since(since: Instant) -> Self {
        Self {
            since,
            partial: Vec::new(),
            oversized: false,
            session: None,
            result: None,
            invalid_result: false,
            last_activity: None,
            last_printed: None,
        }
    }
    pub(super) fn push(&mut self, bytes: &[u8]) {
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            if !self.oversized && self.partial.len().saturating_add(part.len()) <= FRAME_BYTES {
                self.partial.extend_from_slice(part);
            } else {
                if self.result.is_some() {
                    self.invalid_result = true;
                }
                self.oversized = true;
                self.partial.clear();
            }
            if part.last() == Some(&b'\n') {
                self.line();
            }
        }
    }

    fn line(&mut self) {
        if !self.oversized
            && let Ok(value) = serde_json::from_slice::<Value>(&self.partial)
        {
            for line in self.event(&value, self.since.elapsed()) {
                eprintln!("{line}");
            }
        }
        self.partial.clear();
        self.oversized = false;
    }

    fn event(&mut self, value: &Value, now: Duration) -> Vec<String> {
        if value["type"] == "result" {
            self.result(value);
            return Vec::new();
        }
        if value["type"] == "system"
            && value["subtype"] == "init"
            && value["parent_tool_use_id"].is_null()
            && self.session.is_none()
        {
            self.session = id(&value["session_id"]).map(str::to_owned);
        }
        let mut label = "provider activity";
        for block in value["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" {
                label = activity_label(block["name"].as_str().unwrap_or(""));
                if matches!(block["name"].as_str(), Some("Task" | "Agent")) {
                    self.invalid_result = true;
                }
            }
        }
        if value["type"] == "system" && value["subtype"] == "task_started" {
            self.invalid_result = true;
        }
        if value["type"]
            .as_str()
            .is_some_and(|t| matches!(t, "system" | "assistant" | "user" | "stream_event"))
        {
            self.last_activity = Some((label, now));
            if self
                .last_printed
                .is_none_or(|last| now.saturating_sub(last) >= ACTIVITY_INTERVAL)
            {
                self.last_printed = Some(now);
                return vec![line(now, &format!("provider activity: {label}"))];
            }
        }
        Vec::new()
    }
    fn result(&mut self, value: &Value) {
        if !value["parent_tool_use_id"].is_null() {
            return;
        }
        if self
            .session
            .as_deref()
            .is_some_and(|s| Some(s) != value["session_id"].as_str())
        {
            return;
        }
        if self.result.is_some() || value.to_string().len() > RESULT_BYTES {
            self.invalid_result = true;
            return;
        }
        self.result = Some(value.clone());
    }

    pub(super) fn answer(&mut self) -> Value {
        if !self.partial.is_empty() {
            self.line();
        }
        if self.invalid_result {
            Value::Null
        } else {
            self.result.take().unwrap_or(Value::Null)
        }
    }

    pub(super) fn heartbeat(&self) -> String {
        let now = self.since.elapsed();
        let quiet = self
            .last_activity
            .map_or(now, |(_, at)| now.saturating_sub(at));
        line(
            now,
            &format!("review running; no provider event for {}", clock(quiet)),
        )
    }

    pub(super) fn context(&self) -> String {
        match self.last_activity {
            Some((label, at)) => format!(
                "Last provider activity: {label}, {} before stopping.",
                clock(self.since.elapsed().saturating_sub(at))
            ),
            None => "No provider activity was observed.".into(),
        }
    }
}
fn id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= ID_BYTES)
}
fn line(now: Duration, text: &str) -> String {
    format!("keel review-pr: [{}] {text}", clock(now))
}
fn activity_label(tool: &str) -> &'static str {
    match tool {
        "Read" => "reading files",
        "Grep" | "Glob" => "searching files",
        _ => "provider activity",
    }
}
#[cfg(test)]
mod tests;
