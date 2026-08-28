//! Google Cloud: what clusters exist, connecting to one, and making one.
//!
//! Connecting is the ordinary case and is free and reversible — `get-credentials` writes a context
//! into kubeconfig and nothing else. Creating is neither: a GKE cluster bills by the hour from the
//! moment it exists, takes minutes to appear and minutes more to delete, and is the single most
//! expensive thing any button in Keel can do. So the two are not the same shape. One is a click;
//! the other names the cost and asks you to type the cluster's name back.
//!
//! Keel does not manage GKE beyond this. It answers "which cluster, and point kubectl at it",
//! which is the gap between having a cluster and the Cluster panel showing anything.

use axum::response::sse::{Event, Sse};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use crate::serve::AppState;

/// gcloud talks to an API on the other side of the internet; a few seconds is normal and a minute
/// means something is wrong that waiting will not fix.
const DEADLINE: Duration = Duration::from_secs(45);

#[derive(Serialize, Default)]
pub struct GcpState {
    pub installed: bool,
    pub authenticated: bool,
    pub project: Option<String>,
    pub clusters: Vec<Cluster>,
    /// The kubeconfig context kubectl is currently pointed at, so the UI can mark the live one.
    pub current_context: Option<String>,
    pub problem: Option<String>,
}

#[derive(Serialize)]
pub struct Cluster {
    pub name: String,
    pub location: String,
    pub status: String,
    pub nodes: String,
    /// The kubeconfig context `get-credentials` would write, which is how the UI knows whether
    /// this is the cluster kubectl is already using.
    pub context: String,
}

async fn out(program: &str, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(DEADLINE, Command::new(program).args(args).output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

pub async fn state() -> Json<GcpState> {
    let mut s = GcpState::default();

    let Some(_) = out("gcloud", &["--version"]).await else {
        s.problem = Some("The Google Cloud CLI is not installed.".into());
        return Json(s);
    };
    s.installed = true;

    let account = out(
        "gcloud",
        &[
            "auth",
            "list",
            "--filter=status:ACTIVE",
            "--format=value(account)",
        ],
    )
    .await
    .unwrap_or_default();
    s.authenticated = !account.trim().is_empty();
    if !s.authenticated {
        s.problem = Some("Not signed in to Google Cloud.".into());
        return Json(s);
    }

    s.project = out("gcloud", &["config", "get-value", "project"])
        .await
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty() && p != "(unset)");
    if s.project.is_none() {
        s.problem =
            Some("No Google Cloud project is selected. `gcloud config set project <id>`.".into());
        return Json(s);
    }

    s.current_context = out("kubectl", &["config", "current-context"])
        .await
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());

    let Some(raw) = out(
        "gcloud",
        &[
            "container",
            "clusters",
            "list",
            "--format=value(name,location,status,currentNodeCount)",
        ],
    )
    .await
    else {
        // Almost always the Kubernetes Engine API being off on a new project, which is a thing to
        // say rather than an empty list that reads as "you have no clusters".
        s.problem = Some(
            "Could not list clusters. The Kubernetes Engine API may not be enabled on this \
             project."
                .into(),
        );
        return Json(s);
    };

    let project = s.project.clone().unwrap_or_default();
    for line in raw.lines() {
        let mut cols = line.split('\t');
        let (Some(name), Some(location)) = (cols.next(), cols.next()) else {
            continue;
        };
        s.clusters.push(Cluster {
            context: format!("gke_{project}_{location}_{name}"),
            name: name.to_string(),
            location: location.to_string(),
            status: cols.next().unwrap_or("").to_string(),
            nodes: cols.next().unwrap_or("").to_string(),
        });
    }

    Json(s)
}

/// A name that is safe as an argument and valid as a GKE resource name.
fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Deserialize)]
pub struct ClusterRef {
    pub name: String,
    pub location: String,
}

fn refuse(message: &str) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let message = message.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default().event("line").data(message)))
            .await;
        let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
    });
    Sse::new(ReceiverStream::new(rx))
}

async fn stream(
    mut command: Command,
    banner: String,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default().event("line").data(banner)))
            .await;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let Ok(mut child) = command.spawn() else {
            let _ = tx
                .send(Ok(Event::default()
                    .event("line")
                    .data("could not run gcloud")))
                .await;
            let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
            return;
        };
        let (o, e) = (child.stdout.take(), child.stderr.take());
        tokio::join!(
            crate::clitools::pump(o, tx.clone()),
            crate::clitools::pump(e, tx.clone())
        );
        let code = child
            .wait()
            .await
            .map(|s| s.code().unwrap_or(-1))
            .unwrap_or(-1);
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

/// Point kubectl at a cluster. Free, reversible, and the thing people actually want.
pub async fn connect(
    axum::extract::Query(q): axum::extract::Query<ClusterRef>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    if !valid(&q.name) || !valid(&q.location) {
        return refuse("that is not a cluster name");
    }
    let args = [
        "container",
        "clusters",
        "get-credentials",
        &q.name,
        "--location",
        &q.location,
    ];
    let banner = format!("$ gcloud {}", args.join(" "));
    let mut command = Command::new("gcloud");
    command.args(args);
    stream(command, banner).await
}

#[derive(Deserialize)]
pub struct NewCluster {
    pub name: String,
    pub region: String,
    /// The cluster's own name, typed back. Anything else is refused.
    pub confirm: String,
}

/// Create an Autopilot cluster.
///
/// Autopilot rather than Standard because the alternative is asking somebody to pick a machine
/// type and a node count in a dialog, and getting that wrong is how a cluster ends up costing five
/// times what it should. Autopilot bills for what the workloads request and manages the nodes.
///
/// It still bills from the moment it exists, before anything is deployed to it. That is why this
/// takes a typed confirmation and why the UI says the number rather than leaving it to be
/// discovered on an invoice.
pub async fn create(
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<NewCluster>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    if !valid(&q.name) {
        return refuse("A cluster name is lowercase letters, digits and hyphens.");
    }
    if !valid(&q.region) {
        return refuse("That is not a region.");
    }
    if q.confirm != q.name {
        return refuse("Type the cluster's name to confirm.");
    }

    let args = [
        "container",
        "clusters",
        "create-auto",
        &q.name,
        "--region",
        &q.region,
    ];
    let banner = format!(
        "$ gcloud {}\n\nThis takes several minutes and starts billing now.\n",
        args.join(" ")
    );
    let mut command = Command::new("gcloud");
    command.args(args);
    stream(command, banner).await
}

/// Regions offered for a new cluster.
///
/// A short list rather than every region Google has: the choice that matters is "near me", and
/// forty options is not a better answer to that than eight.
pub async fn regions() -> Json<Vec<&'static str>> {
    Json(vec![
        "us-central1",
        "us-east1",
        "us-west1",
        "europe-west1",
        "europe-west2",
        "asia-southeast1",
        "asia-northeast1",
        "australia-southeast1",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_name_cannot_be_a_flag_or_a_path() {
        assert!(valid("a2abaseai-gke"));
        assert!(valid("prod-1"));
        assert!(!valid("--project"));
        assert!(!valid("Prod"));
        assert!(!valid("a/b"));
        assert!(!valid(""));
    }
}
