//! Working code for the templates: packs laid over the containers scaffold.
//!
//! A pack replaces `backend/src/app.ts`, adds a store with a Postgres and an in-memory
//! implementation (so the gate needs no database), migrations, a page, and tests. Each is
//! modelled on the open-source project the template says it is like, so the code behaves
//! like the thing people already know. One module per pack; `files` is the only entry.

mod agent;
mod aigateway;
mod api;
mod auth;
mod bubble;
mod builder;
mod controlplane;
mod dag;
mod dataplane;
mod evals;
mod flags;
mod fullstack;
mod gateway;
mod graph;
mod harness;
mod internal;
mod jobs;
mod llmproxy;
mod loop_;
mod mcpgateway;
mod n8n;
mod orchestrator;
mod pipeline;
mod realtime;
mod sandbox;
mod tenant;

pub fn files(id: &str, name: &str) -> Vec<(&'static str, String)> {
    match id {
        "api" => api::files(name),
        "jobs" => jobs::files(name),
        "llmproxy" => llmproxy::files(name),
        "flags" => flags::files(name),
        "loop" => loop_::files(name),
        "graph" => graph::files(name),
        "pipeline" => pipeline::files(name),
        "realtime" => realtime::files(name),
        "fullstack" => fullstack::files(name),
        "auth" => auth::files(name),
        "tenant" => tenant::files(name),
        "internal" => internal::files(name),
        "agent" => agent::files(name),
        "orchestrator" => orchestrator::files(name),
        "evals" => evals::files(name),
        "dag" => dag::files(name),
        "controlplane" => controlplane::files(name),
        "dataplane" => dataplane::files(name),
        "gateway" => gateway::files(name),
        "aigateway" => aigateway::files(name),
        "mcpgateway" => mcpgateway::files(name),
        "harness" => harness::files(name),
        "sandbox" => sandbox::files(name),
        "builder" => builder::files(name),
        "bubble" => bubble::files(name),
        "n8n" => n8n::files(name),
        _ => Vec::new(),
    }
}

/// Ids that have a pack.
#[cfg(test)]
const WITH_PACK: &[&str] = &[
    "api",
    "jobs",
    "llmproxy",
    "flags",
    "loop",
    "graph",
    "pipeline",
    "realtime",
    "fullstack",
    "auth",
    "tenant",
    "internal",
    "agent",
    "orchestrator",
    "evals",
    "dag",
    "controlplane",
    "dataplane",
    "gateway",
    "aigateway",
    "mcpgateway",
    "harness",
    "sandbox",
    "builder",
    "bubble",
    "n8n",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_replaces_the_app_and_brings_its_own_tests() {
        for id in WITH_PACK {
            let files = files(id, "demo");
            let paths: Vec<_> = files.iter().map(|(p, _)| *p).collect();
            assert!(paths.contains(&"backend/src/app.ts"), "{id}");
            assert!(paths.contains(&"backend/src/app.test.ts"), "{id}");
            assert!(paths.contains(&"frontend/app/page.tsx"), "{id}");
            assert!(
                paths
                    .iter()
                    .any(|p| p.starts_with("backend/migrations/0002_")),
                "{id}"
            );
            let page = &files
                .iter()
                .find(|(p, _)| *p == "frontend/app/page.tsx")
                .unwrap()
                .1;
            assert!(page.contains("demo") && !page.contains("{{NAME}}"), "{id}");
            // Tests must run without a database: they build the app on the memory store.
            let test = &files
                .iter()
                .find(|(p, _)| *p == "backend/src/app.test.ts")
                .unwrap()
                .1;
            assert!(test.contains("memoryStore()"), "{id}");
        }
        assert!(files("nope", "x").is_empty());
    }
}
