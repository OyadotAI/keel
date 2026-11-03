//! New projects start Empty and reserve their actual destination before scaffolding any files.
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;
use keel_generator::cloudflare::{Template, scaffold};
mod location;
mod operations;
pub(crate) use location::parent as project_parent;

#[derive(Clone, Deserialize, Serialize)]
pub struct NewProject {
    /// Directory to create the project in.
    #[serde(default)]
    pub parent: String,
    /// Replaying this operation returns the same created project after a lost response.
    pub operation: Option<String>,
    #[serde(default)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Created {
    pub name: String,
    pub path: String,
}

fn expand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => keel_workspace::home()
            .map(String::from)
            .ok_or(std::env::VarError::NotPresent)
            .map(|h| format!("{h}{rest}"))
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewProject>,
) -> Result<Json<Created>, (StatusCode, String)> {
    // A scaffold is a hundred file writes and a scan: off the executor.
    crate::serve::blocking(
        move || {
            if let Some(id) = req.operation.clone() {
                let parent =
                    project_parent(Some(&req.parent)).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
                operations::run(&parent, &id, &req, || scaffold_project(&state, req.clone()))
            } else {
                scaffold_project(&state, req)
            }
        },
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the scaffold was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn scaffold_project(state: &AppState, req: NewProject) -> Result<Created, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());

    match req.template.as_deref() {
        None | Some("empty") => Template::Empty,
        Some("app") => Template::App,
        Some("stack") => Template::Stack,
        Some(_) => return Err(bad("Unknown project template")),
    };
    let parent = project_parent(Some(&req.parent)).map_err(|e| bad(&e))?;
    let (name, root) = location::reserve(&parent, &req.name).map_err(|e| bad(&e))?;
    let req = NewProject { name, ..req };
    let failure_path = root.to_string();
    scaffold_reserved(state, req, root).map_err(|(status, error)| {
        (
            status,
            format!(
                "Project creation stopped at {failure_path}; partial files were preserved. {error}"
            ),
        )
    })
}

fn scaffold_reserved(
    state: &AppState,
    req: NewProject,
    root: camino::Utf8PathBuf,
) -> Result<Created, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    let template = match req.template.as_deref() {
        Some("app") => Template::App,
        Some("stack") => Template::Stack,
        _ => Template::Empty,
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
        // The pack's CLAUDE.md replaced the one `scaffold` gave the lifecycle to.
        for (rel, body) in files.iter_mut() {
            if *rel == "CLAUDE.md" {
                *body = keel_generator::team::with_guidance(body);
            }
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

    // Finished by the same function the Fix button runs, so a new project already has everything
    // adopting would add — the production checklist derived from what was just written — and the
    // two cannot drift into disagreeing about what "the team" is.
    crate::review::adopt(&root).map_err(|e| bad(&e))?;

    // A repository from the start, so the diff view and the readiness scan both have a baseline.
    let mut init = crate::git::command(&root);
    init.arg("init").arg("--quiet");
    let initialized = crate::git::output(init).map_err(|e| bad(&format!("Project files were created at {root}, but Git initialization failed: {e}. Open that folder to recover.")))?;
    if !initialized.status.success() {
        return Err(bad(&format!(
            "Project files were created at {root}, but Git initialization failed. Open that folder to recover."
        )));
    }

    state.set_repo(root.clone());
    Ok(Created {
        name: req.name,
        path: root.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;

    /// A scaffold ends where the Fix button would: adopting a fresh project changes nothing, for
    /// every template, so the two can never disagree about what "the team" is.
    #[test]
    fn a_fresh_scaffold_is_already_adopted() {
        for template in ["app", "empty", "stack"] {
            let dir = tempfile::tempdir().unwrap();
            let parent = Utf8Path::from_path(dir.path()).unwrap().to_owned();
            let state = AppState::new(parent.clone());
            let req = NewProject {
                parent: parent.to_string(),
                operation: None,
                name: "demo".into(),
                template: Some(template.into()),
                notes: None,
                patterns: None,
                pack: None,
            };
            let made = scaffold_project(&state, req).unwrap();
            let again = crate::review::adopt(Utf8Path::new(&made.path)).unwrap();
            assert!(
                again.written.is_empty(),
                "{template}: adopt wrote {:?}",
                again.written
            );
        }
    }
}

#[cfg(test)]
mod default_tests {
    use super::*;
    use camino::Utf8Path;
    #[test]
    fn omitted_names_and_templates_create_a_uniquely_named_empty_git_project() {
        let temp = tempfile::tempdir().unwrap();
        let parent = Utf8Path::from_path(temp.path()).unwrap();
        let state = AppState::new(parent.to_owned());
        let req: NewProject = serde_json::from_value(serde_json::json!({"parent":parent})).unwrap();
        let created = scaffold_project(&state, req).unwrap();
        let root = Utf8Path::new(&created.path);
        assert!(created.name.starts_with("project-"));
        assert!(root.join(".git").exists());
        assert!(root.join("CLAUDE.md").exists());
        assert!(!root.join("frontend").exists());
        assert!(!root.join("backend").exists());
    }
    #[test]
    fn unknown_templates_are_rejected_before_reserving_a_folder() {
        let temp = tempfile::tempdir().unwrap();
        let parent = Utf8Path::from_path(temp.path()).unwrap();
        let state = AppState::new(parent.to_owned());
        let req: NewProject = serde_json::from_value(
            serde_json::json!({"parent":parent,"name":"demo","template":"unknown"}),
        )
        .unwrap();
        assert!(scaffold_project(&state, req).is_err());
        assert!(!parent.join("demo").exists());
    }
}
