//! Successful file reads are evidence; arbitrary references to a path are not.
use super::*;

fn observed_read(
    s: &mut Snapshot,
    repo: &Utf8Path,
    path: &str,
    id: &str,
    at: &str,
    member: &Value,
) {
    let file = Utf8Path::new(path);
    let file = if file.is_absolute() {
        file.to_owned()
    } else {
        repo.join(file)
    };
    if file.file_name() == Some("SKILL.md") {
        let name = s
            .skills
            .iter()
            .find(|v| v["path"] == file.as_str())
            .and_then(|v| v["name"].as_str())
            .or_else(|| file.parent().and_then(Utf8Path::file_name))
            .unwrap_or("unknown")
            .to_string();
        if !s.skills.iter().any(|v| v["path"] == file.as_str()) {
            s.skills.push(json!({"name":name,"path":file}));
        }
        evidence(
            s,
            json!({"key":format!("{id}:read:{name}"),"id":id,"kind":"read","name":name,"path":file,"at":at,"member":member,"detail":"Successful skill-file read observed in the transcript; selection rationale was not reported."}),
        );
    } else if s.memory.iter().any(|v| v["path"] == file.as_str())
        || file
            .file_name()
            .is_some_and(|n| ["CLAUDE.md", "AGENTS.md", "MEMORY.md"].contains(&n))
    {
        let state = if s
            .compacted_at
            .as_deref()
            .is_some_and(|compacted| !at.is_empty() && at < compacted)
        {
            "historical"
        } else {
            "read"
        };
        if let Some(source) = s.memory.iter_mut().find(|v| v["path"] == file.as_str()) {
            source["state"] = json!(state);
        } else {
            s.memory.push(json!({"path":file,"state":state}));
        }
        evidence(
            s,
            json!({"key":format!("{id}:memory_read"),"id":id,"kind":"memory_read","path":file,"at":at,"member":member,"detail":"Successful memory-file read observed."}),
        );
    }
}
pub(super) fn observe_reads(
    s: &mut Snapshot,
    r: &Value,
    repo: &Utf8Path,
    calls: &mut HashMap<String, Value>,
) {
    let at = r["timestamp"].as_str().unwrap_or("");
    for block in r["message"]["content"].as_array().into_iter().flatten() {
        if block["type"] == "tool_use"
            && block["name"] == "Read"
            && let Some(id) = block["id"].as_str()
        {
            if calls.len() >= MAX_ITEMS {
                calls.clear();
            }
            calls.insert(id.into(), block["input"].clone());
        }
        if block["type"] == "tool_use"
            && block["name"] == "Skill"
            && let Some(id) = block["id"].as_str()
        {
            if calls.len() >= MAX_ITEMS {
                calls.clear();
            }
            calls.insert(id.into(), json!({"_skill":block["input"]["skill"]}));
        }
        if block["type"] == "tool_result"
            && let Some(id) = block["tool_use_id"].as_str()
            && let Some(input) = calls.remove(id)
        {
            if input["_skill"].is_string() && block["is_error"] == true {
                for entry in &mut s.evidence {
                    if entry["id"] == id {
                        entry["kind"] = json!("failed_invocation");
                        entry["detail"] =
                            json!("Skill invocation failed; loading was not confirmed.");
                    }
                }
            } else if block["is_error"] != true
                && let Some(path) = input["file_path"].as_str().or(input["path"].as_str())
            {
                observed_read(s, repo, path, id, at, &r["subagent_type"]);
            }
        }
    }
    let payload = &r["payload"];
    if r["type"] == "response_item"
        && payload["type"] == "function_call"
        && let Some(id) = payload["call_id"].as_str()
    {
        if calls.len() >= MAX_ITEMS {
            calls.clear();
        }
        if let Some(args) = payload["arguments"]
            .as_str()
            .and_then(|v| serde_json::from_str::<Value>(v).ok())
        {
            calls.insert(id.into(), args);
        }
    }
    if r["type"] == "response_item"
        && payload["type"] == "function_call_output"
        && let Some(id) = payload["call_id"].as_str()
        && let Some(input) = calls.remove(id)
    {
        let success = payload["output"]
            .as_str()
            .is_some_and(|o| o.contains("Process exited with code 0"));
        if success
            && let Some(cmd) = input["cmd"]
                .as_str()
                .and_then(|v| v.trim().strip_prefix("cat "))
            && let Some(path) = single_path(cmd)
        {
            observed_read(s, repo, path, id, at, &Value::Null);
        }
    }
}

fn single_path(value: &str) -> Option<&str> {
    let value = value.trim();
    let quoted = value.starts_with('"') && value.ends_with('"')
        || value.starts_with('\'') && value.ends_with('\'');
    let path = if quoted {
        value.get(1..value.len().saturating_sub(1))?
    } else {
        value
    };
    if path.is_empty()
        || path.starts_with('-')
        || path.contains(['\n', ';', '&', '|', '<', '>', '`', '$', '"', '\''])
        || (!quoted && path.contains(char::is_whitespace))
    {
        return None;
    }
    Some(path)
}
