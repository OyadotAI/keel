//! The review a staff engineer would give, and the files that make a repository reviewable.
//!
//! Two handlers. `GET /api/review` builds the prompt for a real review of the open project —
//! not the checklist the scanner already ran, but the reading an experienced engineer does:
//! what this system is for, where it will break first, what to change and in what order. It
//! is sent as a turn in plan mode, so the agent reads and writes nothing. The scan is passed
//! in as evidence so the review starts from facts rather than from the README.
//!
//! `POST /api/adopt` writes the three reviewer subagents and the production checklist into a
//! repository that has none, creating only files that do not exist. It is the automatic fix for
//! `agent/no-reviewers`.

use crate::serve::{AppState, Checkout};
use axum::{Json, extract::State};
use keel_scanner::Report;
use serde::Serialize;
use std::sync::Arc;

pub const STAFF_ENGINEER: &str = "You are a staff engineer at a large software company, asked to review this repository before it is relied on in production. You have shipped and operated systems at scale, been paged for the ones that were not ready, and reviewed hundreds of services. You are direct, specific and kind: every point names a file or a mechanism, says what breaks and when, and gives the change — never a platitude, never a lecture.

Read the repository as that person. Start from what it is for and who calls it. Then look for the things that fail first in practice: the request path and what it does under load; where state lives and who else writes it; what happens on a deploy, a restart, a bad input, a slow dependency, a retried webhook, a second replica; what a new engineer cannot find out from the repository; what an attacker with one API key can do. Read the actual code — routes, data access, queues, config, Dockerfile, CI — not only the docs.

Deliver, in this order and with these headings:

1. **What this is** — two paragraphs: the system, its callers, its data, its current production shape. Say what it does well; a review that finds nothing good has not read carefully.
2. **The five things that will hurt first** — ranked. Each: `path:line` or file, what breaks, under what condition, the fix, and its size (hours / a day / a week). These are the review.
3. **Architecture** — is the shape right for what it does? If a component is missing (a queue, a cache, a worker, a control plane), say which template pattern fits and why; if something is over-built, say so.
4. **Security** — the concrete exposures: input trust, secrets, auth boundaries, SSRF, injection, what an API key grants. Not a checklist: what is actually reachable here.
5. **Operability** — deploy, rollback, migrations, probes, shutdown, logs, metrics, alerts, the 3am story. What the on-call engineer has and what they lack.
6. **The plan** — the same items as a sequence of small pull requests, each one reviewable in an afternoon, in the order that de-risks fastest. Name the first PR precisely enough that someone could start it now.

Rules: quote real paths and symbols; if you did not read a file, do not describe it. Prefer the change that deletes code. Do not restate the scanner's findings — they are below as evidence; go beyond them. No more than two thousand words.";

#[derive(Serialize)]
pub struct ReviewPrompt {
    pub system: &'static str,
    pub prompt: String,
}

/// The prompt for a staff-engineer review of the open project, with the scan as evidence.
pub async fn api_review(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
) -> Json<ReviewPrompt> {
    let report: Report = tokio::task::spawn_blocking(move || {
        keel_scanner::RepoContext::load(&repo)
            .map(|ctx| keel_scanner::scan(&ctx))
            .unwrap_or_else(|_| Report::new(Vec::new()))
    })
    .await
    .unwrap_or_else(|_| Report::new(Vec::new()));
    let _ = &state;
    Json(ReviewPrompt {
        system: STAFF_ENGINEER,
        prompt: prompt(&report),
    })
}

pub fn prompt(report: &Report) -> String {
    let mut p = String::from(
        "Review this repository as a staff engineer would, following the structure you were given. Read the code first: the routes, the data access, the queues and workers, the Dockerfile and CI, the tests. Do not edit anything.\n\n",
    );
    if let Some(prof) = &report.profile {
        p.push_str("## What the scan saw\n\n");
        if prof.template != "blank" {
            p.push_str(&format!(
                "- Closest Keel template: **{}** (like {}), {}% from: {}. Read that template's practices as the target shape, and say where this repository should and should not follow it.\n",
                prof.template_title,
                prof.like,
                prof.confidence,
                prof.signals.join(", ")
            ));
        }
        if !prof.hosting.is_empty() {
            let hosts: Vec<&str> = prof.hosting.iter().map(|h| h.name()).collect();
            p.push_str(&format!(
                "- Runs on: {}. Build around a cloud the team already pays for; name the ceiling of a managed platform; treat data in a backend-as-a-service as something the repository must be able to recreate.\n",
                hosts.join(" + ")
            ));
        }
        if !prof.stack.is_empty() {
            p.push_str(&format!("- Stack: {}.\n", prof.stack.join(", ")));
        }
        p.push('\n');
    }
    p.push_str(&format!(
        "## Scanner findings (evidence, {}/100)\n\n",
        report.score
    ));
    if report.findings.is_empty() {
        p.push_str("None.\n");
    }
    for f in &report.findings {
        p.push_str(&format!(
            "- [{:?}] {} — {}\n",
            f.severity,
            f.title,
            f.fix.description()
        ));
    }
    if !report.plan.is_empty() {
        p.push_str("\n## The scanner's plan\n\n");
        for (i, ph) in report.plan.iter().enumerate() {
            p.push_str(&format!(
                "{}. {} ({} items)\n",
                i + 1,
                ph.title,
                ph.findings.len()
            ));
        }
    }
    p.push_str("\nGo beyond this list. The findings are what a script can see; the review is what you can see.\n");
    p
}

#[derive(Serialize)]
pub struct Adopted {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

/// Write the reviewers and the production checklist into the project, never overwriting.
pub async fn api_adopt(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
) -> Result<Json<Adopted>, (axum::http::StatusCode, String)> {
    let name = repo
        .file_name()
        .map(str::to_string)
        .unwrap_or_else(|| "project".into());
    let files: Vec<(&str, String)> = vec![
        (
            ".claude/agents/reviewer.md",
            crate::project::REVIEWER_AGENT.to_string(),
        ),
        (
            ".claude/agents/security.md",
            crate::stack::SECURITY_AGENT.to_string(),
        ),
        (
            ".claude/agents/reliability.md",
            crate::stack::RELIABILITY_AGENT.to_string(),
        ),
        ("docs/PRODUCTION.md", crate::stack::claude_md(&name)),
    ];
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for (rel, body) in files {
        let path = repo.join(rel);
        if path.exists() {
            skipped.push(rel.to_string());
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        std::fs::write(&path, body)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        written.push(rel.to_string());
    }
    // Point the agent at the checklist from the file it already reads, once.
    let claude = repo.join("CLAUDE.md");
    let pointer = "\n## Production\n\nThe production checklist the reviewers enforce is in `docs/PRODUCTION.md`. It applies.\n";
    match std::fs::read_to_string(&claude) {
        Ok(existing) if !existing.contains("docs/PRODUCTION.md") => {
            std::fs::write(&claude, existing + pointer)
                .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            written.push("CLAUDE.md (appended)".into());
        }
        Err(_) => {
            std::fs::write(&claude, format!("# {name}\n{pointer}"))
                .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            written.push("CLAUDE.md".into());
        }
        _ => skipped.push("CLAUDE.md".into()),
    }
    Ok(Json(Adopted { written, skipped }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_the_scan_and_asks_for_more() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"express":"4","pg":"8"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("server.js"), "app.get('/api/x')").unwrap();
        let ctx = keel_scanner::RepoContext::load(&root).unwrap();
        let p = prompt(&keel_scanner::scan(&ctx));
        assert!(p.contains("Closest Keel template: **REST API service**"));
        assert!(p.contains("Request bodies are not validated"));
        assert!(p.contains("Go beyond this list"));
        assert!(STAFF_ENGINEER.contains("The five things that will hurt first"));
    }
}
