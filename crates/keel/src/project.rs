//! Creating a new project.
//!
//! A new project is scaffolded from the golden path rather than left empty: the whole point of Keel
//! is that a repository arrives already shippable, so a project it creates itself should start at a
//! passing readiness score rather than at the same findings every empty directory produces.

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

#[derive(Deserialize)]
pub struct NewProject {
    /// Directory to create the project in.
    pub parent: String,
    pub name: String,
    /// `worker` for a Cloudflare Worker, `empty` for just the agent scaffolding.
    pub template: Option<String>,
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

/// Names that are safe as a directory and as a Worker name.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewProject>,
) -> Result<Json<Created>, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());

    if !valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens — the name becomes a directory and a Worker name.",
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

    let worker = req.template.as_deref() != Some("empty");
    for (rel, body) in scaffold(&req.name, worker) {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(&e.to_string()))?;
        }
        std::fs::write(&path, body).map_err(|e| bad(&e.to_string()))?;
    }

    // A repository from the start, so the diff view and the readiness scan both have a baseline.
    let _ = std::process::Command::new("git")
        .current_dir(&root)
        .arg("init")
        .arg("--quiet")
        .output();

    state.set_repo(root.clone());
    Ok(Json(Created {
        path: root.to_string(),
    }))
}

/// The files a new project starts with.
fn scaffold(name: &str, worker: bool) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&'static str, String)> = vec![
        (
            "CLAUDE.md",
            format!(
                "# {name}\n\n\
                 ## Conventions\n\n\
                 - Explain *why* in comments, not what. The code already says what.\n\
                 - Every change keeps `make check` green.\n\n\
                 ## Verification\n\n\
                 `make check` runs the typecheck, the linter and the tests. It is the gate.\n"
            ),
        ),
        (
            ".gitignore",
            "node_modules/\ndist/\n.wrangler/\n.DS_Store\n\n\
             # Never commit real values.\n.env\n.env.*\n!.env.example\n\n\
             # Repository-supplied agent config, quarantined by `keel trust`.\n.keel/quarantine/\n"
                .to_string(),
        ),
        (
            "README.md",
            format!(
                "# {name}\n\n\
                 ## Run it\n\n```\nbun install\nbun run dev\n```\n\n\
                 ## Check it\n\n```\nbun run typecheck && bun test\n```\n"
            ),
        ),
    ];

    if worker {
        files.push((
            "wrangler.jsonc",
            format!(
                r#"{{
  // Two environments from the first deploy. A project with one environment teaches you to test in
  // production, and no approval gate downstream recovers from that.
  //
  // Every stateful binding below must stay DISTINCT per environment. Sharing a database_id between
  // dev and prod is the most common way a safe-looking action destroys real data.
  "name": "{name}",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-01",
  "compatibility_flags": ["nodejs_compat"],

  // Traces come from Cloudflare's native Workers tracing, not an SDK in the bundle.
  "observability": {{ "enabled": true }},

  "env": {{
    "dev":  {{ "name": "{name}-dev",  "vars": {{ "ENVIRONMENT": "dev" }} }},
    "prod": {{ "name": "{name}-prod", "vars": {{ "ENVIRONMENT": "prod" }} }}
  }}
}}
"#
            ),
        ));
        files.push((
            "src/index.ts",
            "export default {\n  async fetch(request: Request, env: Env): Promise<Response> {\n    \
             const url = new URL(request.url);\n\n    if (url.pathname === \"/health\") {\n      \
             // A readiness endpoint the platform can poll, separate from any business route.\n      \
             return Response.json({ ok: true, environment: env.ENVIRONMENT });\n    }\n\n    \
             return new Response(\"Not found\", { status: 404 });\n  },\n} satisfies ExportedHandler<Env>;\n\n\
             interface Env {\n  ENVIRONMENT: string;\n}\n"
                .to_string(),
        ));
        files.push((
            "src/index.test.ts",
            "import { expect, test } from \"bun:test\";\nimport worker from \"./index\";\n\n\
             const env = { ENVIRONMENT: \"test\" };\n\n\
             test(\"health reports the environment it is running in\", async () => {\n  \
             const res = await worker.fetch(new Request(\"https://x/health\"), env);\n  \
             expect(res.status).toBe(200);\n  \
             // Typed, because an untyped json() widens to unknown and the matcher narrows wrong.\n  \
             const body = (await res.json()) as { ok: boolean; environment: string };\n  \
             expect(body).toEqual({ ok: true, environment: \"test\" });\n});\n\n\
             test(\"unknown routes are a 404, not a 500\", async () => {\n  \
             const res = await worker.fetch(new Request(\"https://x/nope\"), env);\n  \
             expect(res.status).toBe(404);\n});\n"
                .to_string(),
        ));
        files.push((
            "package.json",
            format!(
                r#"{{
  "name": "{name}",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "wrangler dev",
    "test": "bun test",
    "typecheck": "tsc --noEmit",
    "deploy:dev": "wrangler deploy --env dev",
    "deploy:prod": "wrangler versions upload --env prod"
  }},
  "devDependencies": {{
    "@cloudflare/workers-types": "^4",
    "@types/bun": "^1",
    "typescript": "^5",
    "wrangler": "^4"
  }}
}}
"#
            ),
        ));
        files.push((
            "tsconfig.json",
            "{\n  \"compilerOptions\": {\n    \"target\": \"ES2022\",\n    \"module\": \"ESNext\",\n    \
             \"moduleResolution\": \"Bundler\",\n    \"lib\": [\"ES2022\"],\n    \
             \"types\": [\"@cloudflare/workers-types\", \"bun\"],\n    \"strict\": true,\n    \
             \"noEmit\": true,\n    \"skipLibCheck\": true\n  },\n  \"include\": [\"src\"]\n}\n"
                .to_string(),
        ));
        files.push((
            ".github/workflows/deploy.yml",
            "name: deploy\n\non:\n  pull_request:\n  push:\n    branches: [main]\n\njobs:\n  \
             verify:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v5\n      \
             - uses: oven-sh/setup-bun@v2\n      - run: bun install --frozen-lockfile\n      \
             - run: bun run typecheck\n      - run: bun test\n\n  \
             deploy-dev:\n    if: github.ref == 'refs/heads/main'\n    needs: verify\n    \
             runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v5\n      \
             - uses: cloudflare/wrangler-action@v3\n        with:\n          \
             apiToken: ${{ secrets.CLOUDFLARE_API_TOKEN }}\n          \
             command: deploy --env dev\n\n  \
             # Production is deliberately absent. Promotion happens through Keel, which shows the\n  \
             # commit diff, the binding diff and any pending migrations before a human approves it.\n"
                .to_string(),
        ));
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_names_that_are_not_safe_as_a_directory() {
        assert!(valid_name("my-worker"));
        assert!(valid_name("api2"));
        assert!(!valid_name("../escape"));
        assert!(!valid_name("My Worker"));
        assert!(!valid_name(""));
        assert!(!valid_name("-leading"));
    }

    /// The scaffold shipped a `bun:test` import with no Bun types, so `bun run typecheck` failed on
    /// a freshly created project — caught only by actually running it, not by the scanner.
    #[test]
    fn the_scaffold_typechecks_its_own_test_file() {
        let files = scaffold("demo", true);
        let get = |name: &str| {
            files
                .iter()
                .find(|(p, _)| *p == name)
                .map(|(_, b)| b.clone())
                .unwrap_or_default()
        };
        assert!(get("src/index.test.ts").contains("bun:test"));
        assert!(get("tsconfig.json").contains("\"bun\""), "tsconfig must include Bun types");
        assert!(get("package.json").contains("@types/bun"));
    }

    #[test]
    fn the_worker_scaffold_ships_two_isolated_environments() {
        let files = scaffold("demo", true);
        let wrangler = files
            .iter()
            .find(|(p, _)| *p == "wrangler.jsonc")
            .expect("wrangler config");
        assert!(wrangler.1.contains("\"dev\""));
        assert!(wrangler.1.contains("\"prod\""));
        // A new project should start with something that can fail.
        assert!(files.iter().any(|(p, _)| p.ends_with(".test.ts")));
        // And with instructions for the agent that will work in it.
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
    }

    #[test]
    fn an_empty_project_still_gets_agent_scaffolding() {
        let files = scaffold("demo", false);
        assert!(files.iter().any(|(p, _)| *p == "CLAUDE.md"));
        assert!(!files.iter().any(|(p, _)| *p == "wrangler.jsonc"));
    }
}
