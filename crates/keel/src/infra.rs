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
    let output = tokio::time::timeout(DEADLINE, command.output())
        .await
        .ok()?
        .ok()?;
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
    /// The namespace the current context defaults to. Empty means `default`, which is what
    /// kubectl assumes and therefore what the agent's commands will hit.
    pub namespace: Option<String>,
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
    out.namespace = run(
        "kubectl",
        &[
            "config",
            "view",
            "--minify",
            "--output",
            "jsonpath={..namespace}",
        ],
        None,
    )
    .await
    .map(|n| n.trim().to_string())
    .filter(|n| !n.is_empty())
    .or_else(|| Some("default".into()));

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Json(out);
    };
    let items = json
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();

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

/// Open a URL in the real browser.
///
/// `window.open` does nothing inside a WKWebView unless the host implements the delegate that
/// creates a second web view, so every "open on GitHub" in the application silently did nothing.
/// It is also the wrong behaviour: a run page is GitHub's, and it wants a session and an extension
/// set that Keel's window does not have.
pub async fn open_url(
    Json(body): Json<OpenUrl>,
) -> Result<Json<bool>, (axum::http::StatusCode, String)> {
    let bad = |m: &str| (axum::http::StatusCode::BAD_REQUEST, m.to_string());

    // These URLs come from `gh`, so they are not hostile — but this handler hands a string to the
    // system's URL opener, which will happily launch a `file://` or a custom scheme registered by
    // some other application. Two schemes is the whole allowance.
    if !(body.url.starts_with("https://") || body.url.starts_with("http://")) {
        return Err(bad("only http and https links can be opened"));
    }
    open::that_detached(&body.url).map_err(|e| bad(&e.to_string()))?;
    Ok(Json(true))
}

#[derive(serde::Deserialize)]
pub struct OpenUrl {
    pub url: String,
}

// ═══ workload detail ═════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Default)]
pub struct WorkloadDetail {
    pub pods: Vec<Pod>,
    pub events: Vec<String>,
    pub problem: Option<String>,
}

#[derive(Serialize)]
pub struct Pod {
    pub name: String,
    pub phase: String,
    pub ready: String,
    pub restarts: i64,
    /// The container image, which is most of what "which version is this" means.
    pub image: String,
    pub reason: String,
}

#[derive(serde::Deserialize)]
pub struct WorkloadQuery {
    pub namespace: String,
    pub name: String,
}

/// The pods behind one deployment, and why they are unhappy.
///
/// "2/3 ready" is where the question starts, not where it ends — the answer is in a pod's phase,
/// its restart count and the events attached to it.
pub async fn workload(
    axum::extract::Query(q): axum::extract::Query<WorkloadQuery>,
) -> Json<WorkloadDetail> {
    let mut out = WorkloadDetail::default();

    let safe = |s: &str| {
        !s.is_empty()
            && s.len() <= 253
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    };
    if !safe(&q.namespace) || !safe(&q.name) {
        out.problem = Some("that is not a Kubernetes name".into());
        return Json(out);
    }

    let selector = format!("app={}", q.name);
    let Some(raw) = run(
        "kubectl",
        &[
            "get",
            "pods",
            "-n",
            &q.namespace,
            "-l",
            &selector,
            "-o=json",
            "--request-timeout=5s",
        ],
        None,
    )
    .await
    else {
        out.problem = Some("The cluster did not answer.".into());
        return Json(out);
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Json(out);
    };
    for item in json
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let status = item.get("status");
        let containers = status
            .and_then(|s| s.get("containerStatuses"))
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();

        let ready_count = containers
            .iter()
            .filter(|c| c.get("ready").and_then(|r| r.as_bool()).unwrap_or(false))
            .count();
        let restarts = containers
            .iter()
            .filter_map(|c| c.get("restartCount").and_then(|r| r.as_i64()))
            .sum();

        // The reason a container is not running is on the waiting state, and it is the single most
        // useful string in the whole payload: ImagePullBackOff, CrashLoopBackOff, OOMKilled.
        let reason = containers
            .iter()
            .find_map(|c| {
                c.get("state")
                    .and_then(|s| s.get("waiting").or_else(|| s.get("terminated")))
                    .and_then(|w| w.get("reason"))
                    .and_then(|r| r.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();

        out.pods.push(Pod {
            name: item
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            phase: status
                .and_then(|s| s.get("phase"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ready: format!("{ready_count}/{}", containers.len()),
            restarts,
            image: containers
                .first()
                .and_then(|c| c.get("image"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string(),
            reason,
        });
    }

    if out.pods.is_empty() {
        // `app=<name>` is a convention, not a rule. Saying so beats an empty list that reads as
        // "this deployment has no pods", which would be a different and much worse problem.
        out.problem = Some(format!(
            "No pods matched `app={}`. This deployment may label its pods differently.",
            q.name
        ));
    }

    Json(out)
}

/// Point the current context at a namespace.
///
/// Not a filter. kubectl defaults to whatever this says, so it changes what every unqualified
/// command does — including the ones the agent runs. That is the point: "work in this namespace"
/// should be one decision, not a `-n` on every command that has to be remembered.
pub async fn set_namespace(
    axum::extract::Query(q): axum::extract::Query<NamespaceQuery>,
) -> Json<serde_json::Value> {
    // A Kubernetes name, and nothing that could be read as a flag.
    let ok = !q.name.is_empty()
        && q.name.len() <= 63
        && !q.name.starts_with('-')
        && q.name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Json(serde_json::json!({ "ok": false, "error": "that is not a namespace" }));
    }

    let arg = format!("--namespace={}", q.name);
    match run(
        "kubectl",
        &["config", "set-context", "--current", &arg],
        None,
    )
    .await
    {
        Some(_) => Json(serde_json::json!({ "ok": true, "namespace": q.name })),
        None => Json(serde_json::json!({ "ok": false, "error": "kubectl refused" })),
    }
}

#[derive(serde::Deserialize)]
pub struct NamespaceQuery {
    pub name: String,
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
