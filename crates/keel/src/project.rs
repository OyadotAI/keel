//! Creating a new project.
//!
//! A new project is scaffolded from the golden path rather than left empty: the whole point of Keel
//! is that a repository arrives already shippable, so a project it creates itself should start at a
//! passing readiness score rather than at the same findings every empty directory produces.
//!
//! # The three folders
//!
//! `frontend/` is Next.js on Workers, `backend/` is Go, `infra/` is how both of them ship. The
//! split is not organisational tidiness — it is the seam where the two halves have genuinely
//! different constraints, and putting them in one folder hides that.
//!
//! # Why the backend is its own Worker rather than Next.js API routes
//!
//! Two Workers cost almost nothing on Cloudflare and buy three things a single Next.js app cannot
//! have: the backend deploys on its own schedule, it is reachable by things that are not the
//! website, and the frontend talks to it over a service binding — an internal call with no public
//! route, no CORS and no second TLS hop.
//!
//! The seam is typed rather than documented. The backend exports the type of its route table, and
//! the frontend builds its client from that type, so a route that changes shape breaks the
//! frontend at compile time instead of at runtime. That is the whole argument for TypeScript on
//! both sides, and it is worth more than the language being the same.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;
use keel_generator::cloudflare::{Template, scaffold, valid_name};

#[derive(Deserialize)]
pub struct NewProject {
    /// Directory to create the project in.
    pub parent: String,
    pub name: String,
    /// `app` for the full three-folder stack, `empty` for just the agent scaffolding.
    pub template: Option<String>,
    /// Markdown appended to the generated CLAUDE.md: the architecture the person chose, so the
    /// agent reads the same plan they did.
    pub notes: Option<String>,
    /// The whole template catalogue as Markdown, for a blank project: written to
    /// `docs/PATTERNS.md` and pointed at from CLAUDE.md, so an agent starting from nothing has
    /// the same architectures to build from that the gallery shows.
    pub patterns: Option<String>,
    /// A working feature pack laid over the stack scaffold — real code for the template, so the
    /// project does something the minute it opens. See `stack::pack`.
    pub pack: Option<String>,
}

#[derive(Serialize)]
pub struct Created {
    pub path: String,
}

fn expand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => std::env::var("HOME")
            .map(|h| format!("{h}{rest}"))
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewProject>,
) -> Result<Json<Created>, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());

    if !valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens — the name becomes a directory, two Worker \
             names.",
        ));
    }

    let parent = Utf8PathBuf::from(expand(&req.parent));
    if !parent.is_dir() {
        return Err(bad(&format!("no such directory: {parent}")));
    }
    let root = parent.join(&req.name);
    if root.exists() {
        return Err(bad(&format!("{root} already exists")));
    }

    std::fs::create_dir_all(&root).map_err(|e| bad(&e.to_string()))?;

    let template = match req.template.as_deref() {
        Some("empty") => Template::Empty,
        Some("stack") => Template::Stack,
        _ => Template::App,
    };
    let mut files = scaffold(&req.name, template);
    if template == Template::Stack
        && let Some(pack) = req.pack.as_deref()
    {
        // A pack's file replaces the scaffold's at the same path; everything else is added.
        let packed = keel_generator::stack::pack(pack, &req.name);
        // A pack that brings its own CLAUDE.md is about the service; the stack's rules — the
        // gate, the seam, the production checklist — still apply, so they move to a file the
        // pack's CLAUDE.md points at rather than being lost.
        if packed.iter().any(|(p, _)| *p == "CLAUDE.md") {
            files.push((
                "docs/PRODUCTION.md",
                keel_generator::stack::claude_md(&req.name),
            ));
        }
        for (rel, body) in packed {
            files.retain(|(p, _)| *p != rel);
            files.push((rel, body));
        }
    }
    for (rel, body) in files {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(&e.to_string()))?;
        }
        std::fs::write(&path, body).map_err(|e| bad(&e.to_string()))?;
    }

    // The deploy script is the one file that is useless without the executable bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for script in ["infra/deploy.sh", "k8s/scripts/env-to-secrets.sh"] {
            let p = root.join(script);
            if p.exists() {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    if let Some(patterns) = req.patterns.as_deref().filter(|p| !p.trim().is_empty()) {
        std::fs::create_dir_all(root.join("docs")).map_err(|e| bad(&e.to_string()))?;
        std::fs::write(root.join("docs/PATTERNS.md"), patterns).map_err(|e| bad(&e.to_string()))?;
        let claude = root.join("CLAUDE.md");
        let mut body = std::fs::read_to_string(&claude).unwrap_or_default();
        body.push_str(
            "\n## Patterns\n\n`docs/PATTERNS.md` holds the architectures Keel's templates are built \
             from — APIs, job systems, agents (loop, graph, DAG), control and data planes, gateways \
             — each with components, request flow and the rules that keep it up. Before designing a \
             new component, find the closest pattern there and build to it; say which one you used.\n",
        );
        std::fs::write(&claude, body).map_err(|e| bad(&e.to_string()))?;
    }

    if let Some(notes) = req.notes.as_deref().filter(|n| !n.trim().is_empty()) {
        let claude = root.join("CLAUDE.md");
        let mut body = std::fs::read_to_string(&claude).unwrap_or_default();
        body.push_str("\n## Architecture\n\nChosen when the project was created. Build to it; change it here first if it has to change.\n\n");
        body.push_str(notes.trim());
        body.push('\n');
        std::fs::write(&claude, body).map_err(|e| bad(&e.to_string()))?;
    }

    // A repository from the start, so the diff view and the readiness scan both have a baseline.
    let mut init = crate::git::command(&root);
    init.arg("init").arg("--quiet");
    let _ = crate::git::output(init);

    state.set_repo(root.clone());
    Ok(Json(Created {
        path: root.to_string(),
    }))
}
