//! Bounded provider frames expose stage timing without logging prompts or trusting prose as verdicts.
use super::{clock, team};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const FRAME_BYTES: usize = 2 * 1024 * 1024;
const RESULT_BYTES: usize = 1024 * 1024;
const CORRELATIONS: usize = 64;
const ID_BYTES: usize = 128;
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const HEARTBEAT: Duration = Duration::from_secs(60);

struct Stage {
    name: &'static str,
    since: Duration,
    background: bool,
}

pub(super) struct Activity {
    since: Instant,
    partial: Vec<u8>,
    oversized: bool,
    session: Option<String>,
    result: Option<Value>,
    invalid_result: bool,
    stages: HashMap<String, Stage>,
    seen: HashSet<String>,
    aliases: HashMap<String, String>,
    last_stage: Option<&'static str>,
    last_activity: Option<(&'static str, Duration)>,
    last_printed: Option<Duration>,
}

impl Activity {
    pub(super) fn new() -> Self {
        Self {
            since: Instant::now(),
            partial: Vec::new(),
            oversized: false,
            session: None,
            result: None,
            invalid_result: false,
            stages: HashMap::new(),
            seen: HashSet::new(),
            aliases: HashMap::new(),
            last_stage: None,
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
        let mut lines = Vec::new();
        if value["type"] == "result" {
            self.result(value);
            return lines;
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
                if matches!(block["name"].as_str(), Some("Task" | "Agent"))
                    && value["parent_tool_use_id"].is_null()
                {
                    self.start(
                        &block["id"],
                        &block["input"]["subagent_type"],
                        block["input"]["run_in_background"]
                            .as_bool()
                            .unwrap_or(true),
                        now,
                        &mut lines,
                    );
                }
            } else if block["type"] == "tool_result"
                && value["tool_use_result"]["status"] != "async_launched"
                && value["tool_use_result"]["isAsync"] != true
            {
                self.finish(
                    &block["tool_use_id"],
                    value["tool_use_result"]["status"] == "completed",
                    now,
                    &mut lines,
                );
            }
        }
        if value["type"] == "system" && value["subtype"] == "task_started" {
            let key = id(&value["tool_use_id"]).or_else(|| id(&value["task_id"]));
            if let Some(key) = key {
                if let Some(stage) = self.stages.get_mut(key)
                    && let Some(background) = value["is_backgrounded"].as_bool()
                {
                    stage.background = background;
                }
                self.start(
                    &Value::String(key.into()),
                    &value["subagent_type"],
                    value["is_backgrounded"].as_bool().unwrap_or(false),
                    now,
                    &mut lines,
                );
                if let Some(task) = id(&value["task_id"])
                    && self.aliases.len() < CORRELATIONS
                {
                    self.aliases.insert(task.into(), key.into());
                }
            }
        }
        if value["type"] == "system"
            && value["subtype"] == "task_notification"
            && matches!(
                value["status"].as_str(),
                Some("completed" | "failed" | "stopped")
            )
        {
            let key = if id(&value["tool_use_id"]).is_some() {
                &value["tool_use_id"]
            } else {
                &value["task_id"]
            };
            self.finish(key, true, now, &mut lines);
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
                lines.push(line(now, &format!("provider activity: {label}")));
                self.last_printed = Some(now);
            }
        }
        lines
    }

    fn start(
        &mut self,
        key: &Value,
        name: &Value,
        background: bool,
        now: Duration,
        lines: &mut Vec<String>,
    ) {
        let Some(name) = team::PR_STAGES
            .iter()
            .copied()
            .find(|s| Some(*s) == name.as_str())
        else {
            return;
        };
        let Some(key) = id(key) else {
            return;
        };
        if self.seen.contains(key) || self.seen.len() >= CORRELATIONS {
            return;
        }
        self.seen.insert(key.into());
        self.stages.insert(
            key.into(),
            Stage {
                name,
                since: now,
                background,
            },
        );
        self.last_stage = Some(name);
        lines.push(line(now, &format!("stage {name} started")));
    }

    fn finish(&mut self, key: &Value, lifecycle: bool, now: Duration, lines: &mut Vec<String>) {
        let Some(key) = id(key) else {
            return;
        };
        let key = self.aliases.get(key).map(String::as_str).unwrap_or(key);
        if self
            .stages
            .get(key)
            .is_some_and(|s| s.background && !lifecycle)
        {
            return;
        }
        let Some(stage) = self.stages.remove(key) else {
            return;
        };
        lines.push(line(
            now,
            &format!(
                "stage {} finished; took {}",
                stage.name,
                clock(now.saturating_sub(stage.since))
            ),
        ));
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
        let stage = self
            .last_stage
            .map_or("No stage activity was observed.".into(), |s| {
                format!("Last observed stage: {s}.")
            });
        match self.last_activity {
            Some((label, at)) => format!(
                "{stage}\nLast provider activity: {label}, {} before stopping.",
                clock(self.since.elapsed().saturating_sub(at))
            ),
            None => stage,
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
        "Task" | "Agent" => "running a stage",
        _ => "provider activity",
    }
}

#[cfg(test)]
mod tests;
