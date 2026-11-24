//! Check runs and legacy statuses are read independently for one captured commit, with honest cutoffs.
use super::{
    Read,
    constants::*,
    transport::{link, text},
};
use crate::delivery::RemoteCheck;
use serde_json::Value;

pub(super) fn read(
    reader: &mut impl Read,
    repository: &str,
    sha: &str,
) -> (Vec<RemoteCheck>, Vec<String>) {
    let mut checks = Vec::new();
    let mut errors = Vec::new();
    for kind in ["check-runs", "status"] {
        if let Err(error) = pages(reader, repository, sha, kind, &mut checks) {
            errors.push(error);
        }
    }
    (checks, errors)
}

fn pages(
    reader: &mut impl Read,
    repository: &str,
    sha: &str,
    kind: &str,
    output: &mut Vec<RemoteCheck>,
) -> Result<(), String> {
    let mut page = 1;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let result = reader.rest(&format!(
            "repos/{repository}/commits/{sha}/{kind}?per_page={REST_PAGE}&page={page}"
        ))?;
        if kind == "status" && result["sha"].as_str() != Some(sha) {
            return Err("GitHub status belongs to another commit.".into());
        }
        let key = if kind == "status" {
            "statuses"
        } else {
            "check_runs"
        };
        let rows = result[key]
            .as_array()
            .ok_or("GitHub omitted its check collection.")?;
        let total = result["total_count"]
            .as_u64()
            .ok_or("GitHub omitted its check count.")?;
        for row in rows {
            if output.len() >= RECORD_LIMIT {
                return Err("Feedback refresh incomplete — check traversal limit reached.".into());
            }
            let check = decode(row, sha, kind)?;
            if !seen.insert(check.id.clone()) {
                return Err("GitHub repeated a check page. Previous feedback was retained.".into());
            }
            output.push(check);
        }
        if seen.len() as u64 >= total {
            return Ok(());
        }
        if rows.is_empty() {
            return Err("GitHub check pagination ended before the reported count.".into());
        }
        page += 1;
    }
}

fn decode(value: &Value, sha: &str, kind: &str) -> Result<RemoteCheck, String> {
    let id = value["id"]
        .as_u64()
        .ok_or("GitHub omitted a check identity.")?;
    if kind == "status" {
        Ok(RemoteCheck {
            id: format!("status:{id}"),
            source: "status".into(),
            name: text(value, "context")?,
            sha: sha.into(),
            status: text(value, "state")?,
            conclusion: None,
            url: link(&value["target_url"]),
        })
    } else {
        if value["head_sha"].as_str() != Some(sha) {
            return Err("GitHub check run belongs to another commit.".into());
        }
        Ok(RemoteCheck {
            id: format!("run:{id}"),
            source: "check_run".into(),
            name: text(value, "name")?,
            sha: sha.into(),
            status: text(value, "status")?,
            conclusion: value["conclusion"].as_str().map(str::to_string),
            url: link(&value["details_url"]).or_else(|| link(&value["html_url"])),
        })
    }
}
