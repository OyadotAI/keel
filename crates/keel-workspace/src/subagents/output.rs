//! Selected child output reuses provider decoders and exposes only bounded prose and tool rows.
use super::{Availability, Detail, OutputLine, agy, json, observer::Entry};
use crate::conversation::{Op, TextKind};
use crate::{Follows, Transcript};
use std::collections::BTreeMap;

pub(super) fn read(entry: &Entry) -> Detail {
    let mut detail = Detail {
        items: Vec::new(),
        availability: entry.invocation.output,
        truncated: false,
    };
    if entry.provider == Follows::Claude
        && !entry.confirmed.active()
        && let Some(result) = &entry.result
    {
        detail.items.push(OutputLine {
            kind: "result".into(),
            text: result.clone(),
        });
        detail.availability = Availability::Available;
        trim(&mut detail);
        return detail;
    }
    if entry.provider == Follows::Claude && !entry.attributed && entry.until.is_none() {
        detail.availability = Availability::Unavailable;
        return detail;
    }
    let Some(path) = entry
        .path
        .as_ref()
        .and_then(|p| json::confined(p, &entry.root))
    else {
        if let Some(result) = &entry.result {
            detail.items.push(OutputLine {
                kind: "result".into(),
                text: result.clone(),
            });
            detail.availability = Availability::Available;
        } else if entry.path.is_some() {
            detail.availability = Availability::Disconnected;
        }
        return detail;
    };
    let data = if entry.provider == Follows::Antigravity {
        agy::rows(&path)
    } else {
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .unwrap_or_default();
        json::records(
            &path,
            size.saturating_sub(json::READ_BYTES),
            json::READ_BYTES,
        )
        .map(|(lines, _)| (lines, size > json::READ_BYTES))
    };
    let Some((lines, truncated)) = data else {
        detail.availability = Availability::Disconnected;
        return detail;
    };
    detail.availability = Availability::Available;
    detail.truncated = truncated;
    let transcript = Transcript::at(entry.provider, path);
    let mut decoder = transcript.agent().decoder();
    let mut chunks = BTreeMap::<String, usize>::new();
    let mut inside = entry.provider != Follows::Antigravity;
    let mut attributed = entry.provider != Follows::Claude;
    for value in lines {
        if entry.provider == Follows::Claude {
            let Some(start) = entry.invocation.started.as_deref() else {
                continue;
            };
            let Some(at) = value["timestamp"].as_str() else {
                continue;
            };
            if at < start || entry.until.as_deref().is_some_and(|end| at >= end) {
                continue;
            }
            attributed = true;
        }
        if entry.provider == Follows::Codex
            && value["payload"]["turn_id"].as_str() != entry.turn.as_deref()
        {
            continue;
        }
        if entry.provider == Follows::Antigravity && value["step"] == 14 {
            inside = value["agy"].to_string().as_str() == entry.turn.as_deref().unwrap_or_default();
        }
        if !inside {
            continue;
        }
        for frame in decoder.feed(&value.to_string(), None, std::time::Instant::now()) {
            fold(&mut detail.items, &mut chunks, frame.op);
        }
    }
    for frame in decoder.flush_all() {
        fold(&mut detail.items, &mut chunks, frame.op);
    }
    if detail.items.is_empty()
        && let Some(result) = &entry.result
    {
        detail.items.push(OutputLine {
            kind: "result".into(),
            text: result.clone(),
        });
    }
    if !attributed && detail.items.is_empty() {
        detail.availability = Availability::Unavailable;
    }
    trim(&mut detail);
    detail
}
fn fold(items: &mut Vec<OutputLine>, chunks: &mut BTreeMap<String, usize>, op: Op) {
    match op {
        Op::Text {
            step,
            kind: TextKind::Say,
            append,
        } => {
            let index = *chunks.entry(step).or_insert_with(|| {
                items.push(OutputLine {
                    kind: "text".into(),
                    text: String::new(),
                });
                items.len() - 1
            });
            items[index].text.push_str(&append);
        }
        Op::Call { tool, subject, .. } => items.push(OutputLine {
            kind: "tool".into(),
            text: format!("{tool} {subject}").trim().to_string(),
        }),
        Op::Result { output, .. } if !output.is_empty() => items.push(OutputLine {
            kind: "result".into(),
            text: output,
        }),
        _ => {}
    }
}
fn trim(detail: &mut Detail) {
    if detail.items.len() > json::OUTPUT_LINES {
        detail
            .items
            .drain(..detail.items.len() - json::OUTPUT_LINES);
        detail.truncated = true;
    }
    let mut left = json::OUTPUT_BYTES;
    for item in detail.items.iter_mut().rev() {
        if item.text.len() > left {
            let mut from = item.text.len() - left;
            while !item.text.is_char_boundary(from) {
                from += 1;
            }
            item.text = item.text[from..].to_string();
            detail.truncated = true;
        }
        left = left.saturating_sub(item.text.len());
    }
    detail.items.retain(|i| !i.text.is_empty());
}

// Cache only one selected execution per parent, invalidating WAL writes and read availability.
pub(super) fn signature(entry: &Entry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    entry.path.hash(&mut hash);
    entry.result.hash(&mut hash);
    entry.until.hash(&mut hash);
    entry.attributed.hash(&mut hash);
    format!("{:?}", entry.invocation.output).hash(&mut hash);
    if let Some(path) = &entry.path {
        for file in [path.to_string(), format!("{path}-wal")] {
            if let Ok(metadata) = std::fs::metadata(file) {
                metadata.len().hash(&mut hash);
                metadata.modified().ok().hash(&mut hash);
            }
        }
    }
    hash.finish()
}

// Revalidate proof before serving cached output as well as after filesystem changes.
pub(super) fn authorized(entry: &Entry) -> bool {
    let Some(path) = &entry.path else {
        return true;
    };
    let Some(path) = json::confined(path, &entry.root) else {
        return false;
    };
    match entry.provider {
        Follows::Codex => entry
            .owner
            .as_ref()
            .zip(entry.directory.as_ref())
            .zip(entry.invocation.child.as_ref())
            .is_some_and(|((parent, directory), id)| {
                json::header(&path)
                    .is_some_and(|h| super::codex::verified(&h, parent, directory, id))
            }),
        Follows::Claude => {
            path.starts_with(&entry.root)
                && entry
                    .owner
                    .as_ref()
                    .zip(entry.invocation.child.as_ref())
                    .is_some_and(|(parent, id)| {
                        json::header(&path)
                            .is_some_and(|h| h["sessionId"] == *parent && h["agentId"] == *id)
                    })
        }
        Follows::Antigravity => path.file_stem() == entry.invocation.child.as_deref(),
    }
}
