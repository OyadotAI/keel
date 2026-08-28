//! What is actually running: the cluster, and the pipelines.
//!
//! Both are read-only and both are the same shape — shell out to a CLI the user already has, parse
//! its JSON, and answer one question. Keel does not manage a cluster or dispatch a workflow; it
//! tells you whether the thing you deployed is up and whether the build that deployed it passed,
//! which are the two questions you otherwise leave the editor to answer.
//!
//! Everything here is bounded. `kubectl` against an unreachable cluster blocks until its own
//! default timeout, which is long enough that a panel using it would read as broken, so every call
//! carries a deadline and a missing answer is reported as one.

use axum::{Json, extract::State};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

use crate::serve::AppState;

/// Long enough for a cluster on the other side of a VPN, short enough that a wrong context does
/// not look like a hang.
const DEADLINE: Duration = Duration::from_secs(6);

async fn run(program: &str, args: &[&str], cwd: Option<&camino::Utf8Path>) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = tokio::time::timeout(DEADLINE, command.output()).await.ok()?.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

// ═══ kubernetes ══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Default)]
pub struct Cluster {
    pub installed: bool,
    /// The kubeconfig context in use. Present even when the cluster cannot be reached.
    pub context: Option<String>,
    pub reachable: bool,
    /// Why not, when it is not.
    pub problem: Option<String>,
    pub workloads: Vec<Workload>,
    pub namespaces: usize,
}

#[derive(Serialize)]
pub struct Workload {
    pub namespace: String,
    pub name: String,
    pub ready: i64,
    pub wanted: i64,
    /// `up`, `partial`, or `down` — the question anybody opens this to answer.
    pub state: &'static str,
}

pub async fn cluster() -> Json<Cluster> {
    let mut out = Cluster::default();

    let Some(context) = run("kubectl", &["config", "current-context"], None).await else {
        // No binary, or no context selected. Either way there is nothing to show and the UI says
        // which, so the two are kept apart.
        out.installed = std::process::Command::new("kubectl")
            .arg("version")
            .arg("--client=true")
            .output()
            .is_ok_and(|o| o.status.success());
        out.problem = Some(if out.installed {
            "No kubectl context is selected.".into()
        } else {
            "kubectl is not installed.".into()
        });
        return Json(out);
    };
    out.installed = true;
    out.context = Some(context.trim().to_string());

    let Some(raw) = run(
        "kubectl",
        &[
            "get",
            "deployments",
            "--all-namespaces",
            "-o=json",
            "--request-timeout=5s",
        ],
        None,
    )
    .await
    else {
        out.problem = Some(
            "The cluster did not answer. It may be unreachable, asleep, or behind a VPN.".into(),
        );
        return Json(out);
    };
    out.reachable = true;

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Json(out);
    };
    let items = json.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();

    let mut namespaces = std::collections::BTreeSet::new();
    for item in items {
        let meta = item.get("metadata");
        let namespace = meta
            .and_then(|m| m.get("namespace"))
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let name = meta
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let wanted = item
            .get("spec")
            .and_then(|s| s.get("replicas"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let ready = item
            .get("status")
            .and_then(|s| s.get("readyReplicas"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        namespaces.insert(namespace.clone());
        out.workloads.push(Workload {
            namespace,
            name,
            ready,
            wanted,
            // A deployment scaled to zero is not broken, it is off. Reporting it as down would
            // teach people to ignore the colour.
            state: if wanted == 0 {
                "down"
            } else if ready >= wanted {
                "up"
            } else if ready > 0 {
                "partial"
            } else {
                "down"
            },
        });
    }

    // Broken first: a list sorted by name buries the one row worth looking at.
    let rank = |s: &str| match s {
        "down" => 0,
        "partial" => 1,
        _ => 2,
    };
    out.workloads.sort_by(|a, b| {
        rank(a.state)
            .cmp(&rank(b.state))
            .then_with(|| a.namespace.cmp(&b.namespace))
            .then_with(|| a.name.cmp(&b.name))
    });
    out.namespaces = namespaces.len();
    Json(out)
}

// ═══ pipelines ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Default)]
pub struct Pipelines {
    pub available: bool,
    pub problem: Option<String>,
    pub runs: Vec<Run>,
}

#[derive(Serialize)]
pub struct Run {
    pub name: String,
    pub branch: String,
    pub event: String,
    /// `success`, `failure`, `cancelled`, or empty while it is still going.
    pub conclusion: String,
    pub status: String,
    pub started: String,
    pub url: String,
}

pub async fn pipelines(State(state): State<Arc<AppState>>) -> Json<Pipelines> {
    let mut out = Pipelines::default();
    let repo = state.repo();

    let Some(raw) = run(
        "gh",
        &[
            "run",
            "list",
            "--limit",
            "20",
            "--json",
            "displayTitle,headBranch,event,conclusion,status,startedAt,url,workflowName",
        ],
        Some(&repo),
    )
    .await
    else {
        // `gh` distinguishes "not installed", "not signed in" and "not a GitHub repository", but
        // from here they collapse into one non-zero exit. The message covers all three rather than
        // guessing at which.
        out.problem = Some(
            "No workflow runs. This needs the GitHub CLI, signed in, in a repository with a \
             GitHub remote."
                .into(),
        );
        return Json(out);
    };

    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return Json(out);
    };
    out.available = true;

    let text = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    out.runs = items
        .iter()
        .map(|r| Run {
            name: {
                let workflow = text(r, "workflowName");
                if workflow.is_empty() {
                    text(r, "displayTitle")
                } else {
                    workflow
                }
            },
            branch: text(r, "headBranch"),
            event: text(r, "event"),
            conclusion: text(r, "conclusion"),
            status: text(r, "status"),
            started: text(r, "startedAt"),
            url: text(r, "url"),
        })
        .collect();

    Json(out)
}
