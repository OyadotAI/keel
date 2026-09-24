//! The review a staff engineer would give, and the files that make a repository reviewable.
//!
//! Three handlers. `GET /api/review` builds the review turn: a persona and a method as the
//! system prompt, and as evidence a **map of the repository** the daemon computes — routes,
//! entry points, environment variables, tables, tests, CI, images, the agent instructions —
//! so the model starts from facts and spends its reading on judgement. It is sent in plan
//! mode, so the agent reads and changes nothing. `POST /api/review/save` writes the finished
//! review into `docs/REVIEW.md`. `POST /api/adopt` writes the agent team (`keel_generator::team`), a
//! `docs/PRODUCTION.md` **derived from this repository** — its languages, gate, hosting and
//! current findings — never the stack's boilerplate.

use crate::serve::{AppState, Checkout};
use axum::{Json, extract::State};
use camino::{Utf8Path, Utf8PathBuf};
use keel_scanner::{Profile, Report};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const STAFF_ENGINEER: &str = "You are a staff engineer at a large software company, asked to review this repository before it is relied on in production. You have shipped and operated systems at scale, been paged for the ones that were not ready, reviewed hundreds of services, and mentored the engineers who wrote them. You are direct, specific and kind: every point names a file or a mechanism, says what breaks and when, and gives the change — never a platitude, never a lecture, never a checklist for its own sake.

# Method

Work in this order, and read before you write.
1. Orient: the repository map below, then README, CLAUDE.md, AGENTS.md, the entry points, the routes. Know what this system is for and who calls it before judging anything.
2. Trace the hot path end to end: one request from the edge to the database and back. Read those files fully.
3. Threat-model it: assets, entry points, trust boundaries, who holds which credential, what one leaked key grants.
4. Failure-model it: a deploy mid-request, a restart, a slow dependency, a retried webhook, a second replica, a bad input, a full disk, a clock jump.
5. Open at least five test files. Judge what the tests prove, not how many there are.
6. Read the pipeline files. Judge what a merge actually triggers.
7. Only then write. Rank by what hurts first.

# Deliver, with these headings, in this order

1. **What this is** — two paragraphs: the system, its callers, its data, its production shape today. Say what it does well; a review that finds nothing good has not read carefully.
2. **Scorecard** — a table: Architecture · Security · Reliability · Tests · Operability · Pipeline · Agent instructions, each scored 1–5 with a one-line reason that names a file.
3. **The five things that will hurt first** — ranked. Each: `path:line` or file, what breaks, under what condition, the fix, its size (hours / a day / a week).
4. **Security posture** — the concrete exposures, from the threat model: input trust at each boundary, authn/authz and where it can be skipped, secrets (in the repo, in CI, in images, in logs), injection and SSRF paths that actually exist, dependency and supply-chain risk (unpinned actions, `eval`/exec, outdated critical packages), what an attacker with one API key or one leaked token can reach. Name the file for each. Say what is already right.
5. **Test quality** — what the suite proves and what it does not: is the hot path covered, are auth and money and migrations tested, are there integration tests against a real database or only mocks, are any tests tautological or asserting on implementation, is anything flaky by construction (time, network, order)? Name the five files you read. Then the three tests to write first, each with the file it belongs in and what it asserts.
6. **Architecture** — is the shape right for what it does? A missing queue, cache, worker, control plane; something over-built; the template pattern that fits and where this repository should not follow it.
7. **Operability** — deploy, rollback, migrations, probes, shutdown, logs, metrics, alerts: the 3am story. What the on-call engineer has and what they lack.
8. **Pipeline** — what a merge triggers, in order; what is gated on what; where a red test can still deploy; what the token can do; what is pinned. The three changes that make it trustworthy.
9. **Agent instructions** — review CLAUDE.md and AGENTS.md as documents for an engineer joining tomorrow and for an agent working unsupervised: do they name the gate, map the files, state the invariants, say how to extend and how to run? What is wrong, missing, or contradicted by the code. Write the outline they should have.
10. **The plan** — the same items as a sequence of small pull requests, each reviewable in an afternoon, in the order that de-risks fastest. Name PR 1 precisely enough that someone could start it now.

Rules: quote real paths and symbols; if you did not read a file, do not describe it. Prefer the change that deletes code. Do not restate the scanner's findings — they are evidence; go beyond them and say where they are wrong. Under three thousand words. Markdown, with the headings above.";

#[derive(Serialize)]
pub struct ReviewPrompt {
    pub system: &'static str,
    pub evidence: String,
    pub prompt: String,
}

pub async fn api_review(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
) -> Json<ReviewPrompt> {
    let (report, map) = tokio::task::spawn_blocking(move || {
        let ctx = keel_scanner::RepoContext::load(&repo).ok();
        let report = ctx
            .as_ref()
            .map(keel_scanner::scan)
            .unwrap_or_else(|| Report::new(Vec::new()));
        let map = repo_map(&repo, report.profile.as_ref());
        (report, map)
    })
    .await
    .unwrap_or_else(|_| (Report::new(Vec::new()), String::new()));
    Json(ReviewPrompt {
        system: STAFF_ENGINEER,
        evidence: evidence(&report, &map),
        prompt: "Review this repository as a staff engineer would, following the method and the headings you were given. Read the code first. Change nothing.".into(),
    })
}

/// The scan and the map, as the evidence section of the system prompt.
pub fn evidence(report: &Report, map: &str) -> String {
    let mut p = String::from("\n\n# Evidence for this review\n\n");
    if let Some(prof) = &report.profile {
        p.push_str("## What the scan saw\n\n");
        if prof.template != "blank" {
            p.push_str(&format!(
                "- Closest Keel template: **{}** (like {}), {}% from: {}. That template's practices are the target shape; say where this repository should and should not follow it.\n",
                prof.template_title, prof.like, prof.confidence, prof.signals.join(", ")
            ));
        }
        if !prof.languages.is_empty() {
            p.push_str(&format!("- Languages: {}.\n", prof.languages.join(", ")));
        }
        if !prof.stack.is_empty() {
            p.push_str(&format!("- Stack: {}.\n", prof.stack.join(", ")));
        }
        if !prof.hosting.is_empty() {
            let hosts: Vec<&str> = prof.hosting.iter().map(|h| h.name()).collect();
            p.push_str(&format!("- Runs on: {}. Build around a cloud the team already pays for; name a managed platform's ceiling; data in a backend-as-a-service is something the repository must be able to recreate.\n", hosts.join(" + ")));
        }
        if let Some(g) = &prof.gate {
            p.push_str(&format!("- Gate: `{g}`.\n"));
        }
        p.push('\n');
    }
    p.push_str(&format!("## Scanner findings ({}/100)\n\n", report.score));
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
    if !map.is_empty() {
        p.push_str("\n# Repository map\n\nComputed by Keel, not by reading. Use it to decide what to read; verify before you cite it.\n\n");
        p.push_str(map);
    }
    p.push_str("\nThe findings are what a script can see; the review is what you can see.\n");
    p
}

// ── the map ──────────────────────────────────────────────────────────────────────────────────

fn walk(repo: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(repo)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .require_git(false)
        .build();
    for e in walker.flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(p) = Utf8Path::from_path(e.path()) else {
            continue;
        };
        let Ok(rel) = p.strip_prefix(repo) else {
            continue;
        };
        // The path inside the repository, not the absolute one: a lane's checkout is itself under
        // `.keel/worktrees/`, and matching there hid every file from the review.
        if rel.components().any(|c| {
            matches!(
                c.as_str(),
                ".git"
                    | ".keel"
                    | "node_modules"
                    | "vendor"
                    | "target"
                    | "dist"
                    | ".next"
                    | "coverage"
            )
        }) {
            continue;
        }
        out.push(rel.to_owned());
        if out.len() > 20_000 {
            break;
        }
    }
    out.sort();
    out
}

fn is_test(p: &str) -> bool {
    p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("_test.go")
        || p.contains("/__tests__/")
        || p.contains("/tests/")
        || p.contains("/test/")
        || p.starts_with("tests/")
        || p.starts_with("test/")
        || p.contains("/test_")
        || p.ends_with("_test.py")
        || p.contains("cypress/")
        || p.contains("e2e/")
}

fn is_source(p: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".py"]
        .iter()
        .any(|e| p.ends_with(e))
        && !p.ends_with(".d.ts")
}

/// What the daemon can say about the repository without a model: enough that the review
/// starts from a map. Every list is capped; the model is told to verify before citing.
pub fn repo_map(repo: &Utf8Path, profile: Option<&Profile>) -> String {
    let files = walk(repo);
    let read = |p: &Utf8Path| std::fs::read_to_string(repo.join(p)).ok();
    let mut out = String::new();

    // ── shape ───────────────────────────────────────────────────────────────────────────
    let mut by_dir: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut loc_total = 0usize;
    let mut test_files = 0usize;
    let mut source_files = 0usize;
    let mut test_loc = 0usize;
    for p in &files {
        let s = p.as_str();
        let top = s.split('/').next().unwrap_or("").to_string();
        let top = if s.contains('/') {
            top + "/"
        } else {
            "(root)".into()
        };
        let e = by_dir.entry(top).or_default();
        e.0 += 1;
        if is_source(s) {
            let loc = read(p).map(|t| t.lines().count()).unwrap_or(0);
            e.1 += loc;
            loc_total += loc;
            if is_test(s) {
                test_files += 1;
                test_loc += loc;
            } else {
                source_files += 1;
            }
        }
    }
    out.push_str(&format!(
        "## Shape\n\n{} files, {} lines of source. By top-level directory (files, lines):\n\n",
        files.len(),
        loc_total
    ));
    let mut dirs: Vec<_> = by_dir.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.1.1));
    for (d, (n, loc)) in dirs.iter().take(18) {
        out.push_str(&format!("- `{d}` — {n} files, {loc} lines\n"));
    }

    // ── entry points and scripts ────────────────────────────────────────────────────────
    let entries: Vec<&Utf8PathBuf> = files
        .iter()
        .filter(|p| {
            let n = p.file_name().unwrap_or("");
            matches!(
                n,
                "main.go"
                    | "main.rs"
                    | "main.py"
                    | "app.py"
                    | "server.ts"
                    | "server.js"
                    | "index.ts"
                    | "index.js"
                    | "app.ts"
                    | "worker.ts"
                    | "manage.py"
            ) && p.components().count() <= 4
        })
        .take(12)
        .collect();
    if !entries.is_empty() {
        out.push_str("\n## Entry points\n\n");
        for e in entries {
            out.push_str(&format!("- `{e}`\n"));
        }
    }
    let mut scripts = Vec::new();
    for p in files
        .iter()
        .filter(|p| p.file_name() == Some("package.json"))
        .take(6)
    {
        if let Some(v) = read(p).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            && let Some(sc) = v.get("scripts").and_then(|s| s.as_object())
        {
            let keys: Vec<&str> = sc.keys().map(String::as_str).collect();
            scripts.push(format!("- `{p}`: {}", keys.join(", ")));
        }
    }
    if let Some(m) = read(Utf8Path::new("Makefile")) {
        let targets: Vec<&str> = m
            .lines()
            .filter(|l| {
                !l.starts_with(['\t', ' ', '#', '.']) && l.contains(':') && !l.contains('=')
            })
            .map(|l| l.split(':').next().unwrap_or(""))
            .filter(|t| !t.is_empty())
            .take(20)
            .collect();
        if !targets.is_empty() {
            scripts.push(format!("- `Makefile`: {}", targets.join(", ")));
        }
    }
    if !scripts.is_empty() {
        out.push_str("\n## Scripts and targets\n\n");
        out.push_str(&scripts.join("\n"));
        out.push('\n');
    }

    // ── routes ──────────────────────────────────────────────────────────────────────────
    let route_pats = [
        "app.get(",
        "app.post(",
        "app.put(",
        "app.patch(",
        "app.delete(",
        "router.get(",
        "router.post(",
        "router.put(",
        "router.delete(",
        ".route(",
        "r.Get(",
        "r.Post(",
        "r.Put(",
        "r.Patch(",
        "r.Delete(",
        "r.Route(",
        "r.Handle(",
        "r.HandleFunc(",
        "mux.HandleFunc(",
        "http.HandleFunc(",
        ".GET(",
        ".POST(",
        ".PUT(",
        ".DELETE(",
        "@app.get(",
        "@app.post(",
        "@router.get(",
        "@router.post(",
        "@app.route(",
        "path(",
        ".route(\"",
        "Router::new()",
        "get(",
        "post(",
    ];
    let mut routes: Vec<String> = Vec::new();
    let mut next_routes: Vec<String> = Vec::new();
    for p in &files {
        let s = p.as_str();
        if !is_source(s) || is_test(s) {
            continue;
        }
        if (s.starts_with("app/") || s.contains("/app/"))
            && (s.ends_with("/route.ts") || s.ends_with("/route.js"))
        {
            next_routes.push(
                s.trim_end_matches("/route.ts")
                    .trim_end_matches("/route.js")
                    .trim_start_matches("app")
                    .to_string(),
            );
            continue;
        }
        if s.starts_with("pages/api/") || s.contains("/pages/api/") {
            next_routes.push(s.to_string());
            continue;
        }
        let Some(t) = read(p) else { continue };
        if !route_pats.iter().any(|r| t.contains(r)) {
            continue;
        }
        for (i, l) in t.lines().enumerate() {
            let lt = l.trim();
            let looks = (route_pats.iter().any(|r| lt.contains(r))
                && (lt.contains("\"/") || lt.contains("'/") || lt.contains("`/")))
                || lt.starts_with("@app.")
                || lt.starts_with("@router.");
            if looks {
                routes.push(format!(
                    "`{}:{}` {}",
                    s,
                    i + 1,
                    lt.chars().take(110).collect::<String>()
                ));
                if routes.len() >= 80 {
                    break;
                }
            }
        }
        if routes.len() >= 80 {
            break;
        }
    }
    if !routes.is_empty() || !next_routes.is_empty() {
        out.push_str("\n## Routes\n\n");
        for r in next_routes.iter().take(60) {
            out.push_str(&format!("- `{r}`\n"));
        }
        for r in &routes {
            out.push_str(&format!("- {r}\n"));
        }
        if routes.len() >= 80 {
            out.push_str("- … (capped at 80)\n");
        }
    }

    // ── environment, tables, external hosts, risky calls ────────────────────────────────
    let mut env: BTreeSet<String> = BTreeSet::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    let mut risky: BTreeMap<&str, usize> = BTreeMap::new();
    let mut todos = 0usize;
    let risky_pats = [
        "eval(",
        "new Function(",
        "child_process",
        "exec(",
        "execSync(",
        "os/exec",
        "subprocess.",
        "dangerouslySetInnerHTML",
        "innerHTML",
        "Math.random()",
        "md5",
        "sha1(",
        "http://",
    ];
    for p in &files {
        let s = p.as_str();
        if !is_source(s) {
            continue;
        }
        let Some(t) = read(p) else { continue };
        for pat in [
            "process.env.",
            "os.Getenv(\"",
            "os.environ[\"",
            "os.environ.get(\"",
            "std::env::var(\"",
            "env::var(\"",
            "os.getenv(\"",
        ] {
            for (i, _) in t.match_indices(pat) {
                let rest = &t[i + pat.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if name.len() > 1
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
                {
                    env.insert(name);
                }
            }
        }
        for (i, _) in t.match_indices("https://") {
            let rest = &t[i + 8..];
            let host: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            if host.contains('.')
                && !host.ends_with(".")
                && !host.contains("example")
                && !host.contains("localhost")
                && !host.contains("schema")
                && !host.contains("w3.org")
                && !host.contains("github.com")
                && !host.contains("npmjs")
            {
                hosts.insert(host);
            }
        }
        if !is_test(s) {
            for pat in risky_pats {
                let n = t.matches(pat).count();
                if n > 0 {
                    *risky.entry(pat).or_default() += n;
                }
            }
            todos +=
                t.matches("TODO").count() + t.matches("FIXME").count() + t.matches("HACK").count();
        }
    }
    if !env.is_empty() {
        out.push_str(&format!(
            "\n## Environment variables read ({})\n\n{}\n",
            env.len(),
            env.iter()
                .take(60)
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let example = files.iter().find(|p| {
            p.file_name()
                .is_some_and(|n| n == ".env.example" || n == ".env.sample" || n == ".env.template")
        });
        match example {
            Some(ex) => {
                let documented: BTreeSet<String> = read(ex)
                    .map(|t| {
                        t.lines()
                            .filter_map(|l| l.split('=').next())
                            .map(|k| k.trim().to_string())
                            .filter(|k| !k.is_empty() && !k.starts_with('#'))
                            .collect()
                    })
                    .unwrap_or_default();
                let undocumented: Vec<&String> =
                    env.iter().filter(|e| !documented.contains(*e)).collect();
                if !undocumented.is_empty() {
                    out.push_str(&format!(
                        "\nRead by code but absent from `{ex}` ({}): {}\n",
                        undocumented.len(),
                        undocumented
                            .iter()
                            .take(30)
                            .map(|e| format!("`{e}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            None => out.push_str("\nNo `.env.example` documents them.\n"),
        }
    }
    let mut tables: Vec<String> = Vec::new();
    for p in files
        .iter()
        .filter(|p| p.as_str().contains("migrations/") && p.extension() == Some("sql"))
        .take(200)
    {
        if let Some(t) = read(p) {
            for l in t.lines() {
                let l = l.trim().to_lowercase();
                if let Some(rest) = l.strip_prefix("create table") {
                    let name = rest
                        .trim()
                        .trim_start_matches("if not exists")
                        .trim()
                        .split(|c: char| c == '(' || c.is_whitespace())
                        .next()
                        .unwrap_or("")
                        .trim_matches('"')
                        .to_string();
                    if !name.is_empty() {
                        tables.push(name);
                    }
                }
            }
        }
    }
    tables.sort();
    tables.dedup();
    if !tables.is_empty() {
        out.push_str(&format!(
            "\n## Tables from migrations ({})\n\n{}\n",
            tables.len(),
            tables
                .iter()
                .take(60)
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !hosts.is_empty() {
        out.push_str(&format!(
            "\n## External hosts called from code\n\n{}\n",
            hosts
                .iter()
                .take(30)
                .map(|h| format!("`{h}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !risky.is_empty() || todos > 0 {
        out.push_str("\n## Worth a look\n\n");
        for (k, n) in &risky {
            out.push_str(&format!("- `{k}` × {n}\n"));
        }
        if todos > 0 {
            out.push_str(&format!("- TODO/FIXME/HACK × {todos}\n"));
        }
    }

    // ── tests ───────────────────────────────────────────────────────────────────────────
    out.push_str(&format!("\n## Tests\n\n{test_files} test files ({test_loc} lines) against {source_files} source files"));
    if source_files > 0 {
        out.push_str(&format!(
            " — ratio {:.2}",
            test_files as f64 / source_files as f64
        ));
    }
    out.push_str(".\n");
    let frameworks: Vec<&str> = profile
        .map(|p| {
            p.stack
                .iter()
                .copied()
                .filter(|s| matches!(*s, "vitest" | "jest" | "pytest" | "testify"))
                .collect()
        })
        .unwrap_or_default();
    if !frameworks.is_empty() {
        out.push_str(&format!("Frameworks: {}.\n", frameworks.join(", ")));
    }
    let tiers: Vec<&str> = [
        ("integration", "integration"),
        ("e2e", "e2e"),
        ("cypress", "cypress"),
        ("playwright", "playwright"),
        ("load", "load"),
    ]
    .iter()
    .filter(|(needle, _)| {
        files
            .iter()
            .any(|p| p.as_str().to_lowercase().contains(needle))
    })
    .map(|(_, name)| *name)
    .collect();
    out.push_str(&format!(
        "Tiers seen in paths: {}.\n",
        if tiers.is_empty() {
            "unit only".to_string()
        } else {
            tiers.join(", ")
        }
    ));
    let sample: Vec<&Utf8PathBuf> = files
        .iter()
        .filter(|p| is_source(p.as_str()) && is_test(p.as_str()))
        .take(12)
        .collect();
    if !sample.is_empty() {
        out.push_str("Test files to open first:\n");
        for p in sample {
            let (asserts, mocks) = read(p)
                .map(|t| {
                    (
                        t.matches("expect(").count() + t.matches("assert").count(),
                        t.matches("mock").count()
                            + t.matches("Mock").count()
                            + t.matches("stub").count(),
                    )
                })
                .unwrap_or((0, 0));
            out.push_str(&format!(
                "- `{p}` — {asserts} assertions, {mocks} mock references\n"
            ));
        }
    }
    let untested_dirs: Vec<String> = dirs
        .iter()
        .filter(|(d, (_, loc))| {
            *loc > 300
                && !files
                    .iter()
                    .any(|p| p.as_str().starts_with(d.as_str()) && is_test(p.as_str()))
        })
        .map(|(d, _)| d.clone())
        .take(8)
        .collect();
    if !untested_dirs.is_empty() {
        out.push_str(&format!(
            "Directories with source and no tests: {}.\n",
            untested_dirs
                .iter()
                .map(|d| format!("`{d}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // ── pipeline and image ──────────────────────────────────────────────────────────────
    let workflows: Vec<&Utf8PathBuf> = files
        .iter()
        .filter(|p| p.as_str().starts_with(".github/workflows/"))
        .collect();
    if !workflows.is_empty() {
        out.push_str("\n## Pipeline\n\n");
        for w in workflows.iter().take(10) {
            let Some(t) = read(w) else { continue };
            let on: Vec<&str> = [
                "push",
                "pull_request",
                "pull_request_target",
                "schedule",
                "workflow_dispatch",
                "release",
                "tags",
            ]
            .iter()
            .copied()
            .filter(|k| t.contains(&format!("{k}:")))
            .collect();
            let jobs: Vec<&str> = t
                .lines()
                .filter(|l| {
                    l.starts_with("  ")
                        && !l.starts_with("   ")
                        && l.trim_end().ends_with(':')
                        && !l.trim().starts_with('-')
                })
                .map(|l| l.trim().trim_end_matches(':'))
                .take(8)
                .collect();
            let runs: Vec<String> = t
                .lines()
                .filter_map(|l| {
                    l.trim()
                        .strip_prefix("- run:")
                        .or_else(|| l.trim().strip_prefix("run:"))
                })
                .map(|r| r.trim().chars().take(70).collect())
                .take(8)
                .collect();
            out.push_str(&format!(
                "- `{w}` — on: {}; jobs: {}; runs: {}\n",
                on.join(", "),
                jobs.join(", "),
                runs.iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let dockerfiles: Vec<&Utf8PathBuf> = files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n.starts_with("Dockerfile")))
        .collect();
    if !dockerfiles.is_empty() {
        out.push_str("\n## Images\n\n");
        for d in dockerfiles.iter().take(6) {
            let Some(t) = read(d) else { continue };
            let froms: Vec<&str> = t
                .lines()
                .filter(|l| l.starts_with("FROM "))
                .map(|l| {
                    l.trim_start_matches("FROM ")
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                })
                .collect();
            let user = t
                .lines()
                .find(|l| l.starts_with("USER "))
                .map(|l| l.trim_start_matches("USER ").trim())
                .unwrap_or("root (no USER)");
            let health = if t.contains("HEALTHCHECK") {
                "healthcheck"
            } else {
                "no healthcheck"
            };
            out.push_str(&format!(
                "- `{d}` — from {}; user {user}; {health}\n",
                froms.join(" → ")
            ));
        }
    }

    // ── agent instructions ──────────────────────────────────────────────────────────────
    out.push_str("\n## Agent instructions\n\n");
    for name in ["CLAUDE.md", "AGENTS.md", "README.md", "docs/PRODUCTION.md"] {
        match read(Utf8Path::new(name)) {
            Some(t) => {
                let heads: Vec<&str> = t
                    .lines()
                    .filter(|l| l.starts_with('#'))
                    .map(|l| l.trim_start_matches('#').trim())
                    .take(14)
                    .collect();
                out.push_str(&format!(
                    "- `{name}` — {} lines; headings: {}\n",
                    t.lines().count(),
                    if heads.is_empty() {
                        "none".to_string()
                    } else {
                        heads.join(" · ")
                    }
                ));
            }
            None => out.push_str(&format!("- `{name}` — absent\n")),
        }
    }
    let agents: Vec<&Utf8PathBuf> = files
        .iter()
        .filter(|p| p.as_str().starts_with(".claude/agents/"))
        .collect();
    out.push_str(&format!(
        "- `.claude/agents/` — {}\n",
        if agents.is_empty() {
            "none".to_string()
        } else {
            agents
                .iter()
                .map(|a| a.file_name().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));

    out
}

// ── saving and adopting ──────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SaveBody {
    pub text: String,
}

/// Write a finished review to `docs/REVIEW.md`, keeping the previous one as `REVIEW-<date>.md`.
pub async fn api_review_save(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Json(b): Json<SaveBody>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    crate::serve::blocking(
        move || save_review(&repo, &b.text),
        Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "the save was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

fn save_review(repo: &Utf8Path, text: &str) -> Result<String, (axum::http::StatusCode, String)> {
    let err = |e: std::io::Error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let dir = repo.join("docs");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join("REVIEW.md");
    if path.exists() {
        let stamp = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(humantime_date)
            .unwrap_or_else(|_| "previous".into());
        let _ = std::fs::rename(&path, dir.join(format!("REVIEW-{stamp}.md")));
    }
    let today = humantime_date(std::time::SystemTime::now());
    let body = format!(
        "# Staff-engineer review — {today}\n\nWritten by Keel's review turn; the scanner's findings are evidence, this is judgement. Re-run from Readiness.\n\n{}\n",
        text.trim()
    );
    std::fs::write(&path, body).map_err(err)?;
    write_contract(repo, &today, text.trim()).map_err(err)?;
    Ok("docs/REVIEW.md and .claude/agents/contract.md".into())
}

const CONTRACT_MARK: &str = "## Latest review";

/// The contract subagent: a part the team owns and edits, and a part the review refreshes.
/// The first save seeds the owned part from the review's scorecard, five things and plan;
/// later saves leave it alone and replace only the review below the mark.
pub fn write_contract(repo: &Utf8Path, today: &str, review: &str) -> std::io::Result<()> {
    let dir = repo.join(".claude/agents");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("contract.md");
    let owned = match std::fs::read_to_string(&path) {
        Ok(existing) => existing
            .split(CONTRACT_MARK)
            .next()
            .unwrap_or("")
            .trim_end()
            .to_string(),
        Err(_) => seed_contract(today, review),
    };
    let body = format!(
        "{owned}\n\n{CONTRACT_MARK}\n\n_{today}. Replaced on every save; the sections above are yours and are never touched._\n\n{review}\n"
    );
    std::fs::write(path, body)
}

/// Pull the sections that read as rules out of the review, under a frontmatter that makes the
/// file a subagent Claude Code can run.
fn seed_contract(today: &str, review: &str) -> String {
    let mut out = String::from(
        "---\nname: contract\ndescription: The engineering contract for this repository — invariants, rules and the plan, first written by the staff-engineer review and since corrected by the team. Run before a change merges; it reports what the change breaks in the contract, and nothing else.\ntools: Read, Grep, Glob, Bash\n---\n\n",
    );
    out.push_str(&format!(
        "# Contract\n\nSeeded {today} from the review below. **This part is the team's**: correct a rule here and it stays corrected — the review section under `{CONTRACT_MARK}` is replaced on every save, this is not. When you run as the `contract` agent, read the change, then report each rule below it breaks as `rule — path:line — what breaks — the fix`, and stop.\n\n"
    ));
    for (heading, take) in [
        ("Scorecard", "Scorecard"),
        ("The five things", "The five things"),
        ("Security", "Security posture"),
        ("Tests", "Test quality"),
        ("Agent instructions", "Agent instructions"),
        ("The plan", "The plan"),
    ] {
        if let Some(section) = section_of(review, take) {
            out.push_str(&format!("## {heading}\n\n{}\n\n", section.trim()));
        }
    }
    if out.matches("\n## ").count() == 0 {
        out.push_str("## Rules\n\n- (the review had no headed sections to seed from; write the rules here)\n\n");
    }
    out.trim_end().to_string()
}

/// The body under a heading whose text (after `#`, numbers and bold marks) starts with `key`,
/// up to the next heading.
fn section_of<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let needle = key.to_lowercase();
    let is_heading = |l: &str| l.starts_with('#');
    let title = |l: &str| {
        l.trim_start_matches(|c: char| {
            c == '#' || c == ' ' || c.is_ascii_digit() || c == '.' || c == '*'
        })
        .to_lowercase()
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| is_heading(l) && title(l).starts_with(&needle))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| is_heading(l))
        .map(|i| start + 1 + i)
        .unwrap_or(lines.len());
    let from: usize = lines[..=start].iter().map(|l| l.len() + 1).sum();
    let to: usize = lines[..end]
        .iter()
        .map(|l| l.len() + 1)
        .sum::<usize>()
        .min(text.len());
    text.get(from..to)
}

fn humantime_date(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date from days since epoch (Howard Hinnant's algorithm), no chrono dependency.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Deserialize)]
pub struct MemoryBody {
    pub text: String,
}

/// `# something` in the composer, the way `#` works in the terminal: a line the agent reads
/// every turn, appended to the project's own CLAUDE.md under one heading. Not a hidden store —
/// it is a file in the repository, reviewed like any other line of it.
pub async fn api_memory(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    Json(b): Json<MemoryBody>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let text = b.text.trim().trim_start_matches('#').trim();
    if text.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "nothing to remember".into(),
        ));
    }
    let text = text.to_string();
    crate::serve::blocking(
        move || remember(&repo, &text),
        Err("the write was cancelled".into()),
    )
    .await
    .map(Json)
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))
}

const MEMORY_HEADING: &str = "## Project memory";

pub fn remember(repo: &Utf8Path, text: &str) -> Result<String, String> {
    let path = repo.join("CLAUDE.md");
    let line = format!("- {}\n", text.replace('\n', " ").trim());
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let next = match existing.find(MEMORY_HEADING) {
        // Under the heading, at the end of its section, so the newest note is the last line
        // of it rather than the first line of whatever follows.
        Some(at) => {
            let after = at + MEMORY_HEADING.len();
            let end = existing[after..]
                .find("\n## ")
                .map(|i| after + i + 1)
                .unwrap_or(existing.len());
            let mut out = String::with_capacity(existing.len() + line.len());
            out.push_str(existing[..end].trim_end());
            out.push('\n');
            out.push_str(&line);
            if end < existing.len() {
                out.push('\n');
                out.push_str(&existing[end..]);
            }
            out
        }
        None => {
            let name = repo.file_name().unwrap_or("project");
            let head = if existing.trim().is_empty() {
                format!("# {name}\n")
            } else {
                existing
            };
            format!(
                "{}\n{MEMORY_HEADING}\n\nWhat the team told Keel to remember here. Read every turn; edit or delete a line like any other.\n\n{line}",
                head.trim_end()
            )
        }
    };
    std::fs::write(&path, next).map_err(|e| e.to_string())?;
    Ok("CLAUDE.md".into())
}

#[derive(Serialize, Debug)]
pub struct Adopted {
    /// Repository-relative paths written or appended to.
    pub written: Vec<String>,
    /// What was left alone, each with why: already there, or a `CLAUDE.md` Keel will not edit.
    pub skipped: Vec<String>,
    /// The short sha of the commit holding `written`, when one was asked for and made.
    pub committed: Option<String>,
    /// Why it was not committed, when a commit was asked for.
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct AdoptQuery {
    /// The person's "commit after every turn" setting. Adopting is a write into the tree like a
    /// turn's, so it is checkpointed like one — on its own, under its own message.
    #[serde(default)]
    pub commit: bool,
}

/// Write the agent team, a production checklist derived from this repository, and the lifecycle
/// that runs the team — never overwriting, and under the tree's writer claim: written while a
/// turn runs, all of it was committed under that turn's prompt and reported as its work.
pub async fn api_adopt(
    State(state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
    axum::extract::Query(query): axum::extract::Query<AdoptQuery>,
) -> Result<Json<Adopted>, crate::writes::Failure> {
    let failed = |e: String| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e);
    crate::serve::blocking(
        move || adopt_and_commit(&state, &repo, query.commit),
        Err(failed("the write was cancelled".into())),
    )
    .await
    .map(Json)
}

/// Adopt under the tree's writer claim, then commit what Keel wrote — never the person's work.
fn adopt_and_commit(
    state: &Arc<AppState>,
    repo: &Utf8Path,
    commit: bool,
) -> Result<Adopted, crate::writes::Failure> {
    let _held = crate::writes::hold(state, repo, "adopt")?;
    // Read before writing: an instructions file holding anything of the person's that is not
    // committed is appended to but not committed, or Keel's commit would carry their work.
    let theirs: Vec<String> = instructions_file(repo)
        .ok()
        .map(|(_, rel)| rel)
        .filter(|rel| !crate::writes::clean_before(repo, rel))
        .into_iter()
        .collect();
    let mut done = adopt(repo).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if commit {
        let (ours, kept): (Vec<String>, Vec<String>) = done
            .written
            .iter()
            .cloned()
            .partition(|p| !theirs.contains(p));
        let mut notes = Vec::new();
        if !ours.is_empty() {
            let c = crate::writes::commit_only(repo, &ours, "Add the agent team");
            done.committed = c.sha;
            notes.extend(c.note);
        }
        for p in kept {
            notes.push(format!(
                "{p} has your own uncommitted edits, so Keel's sections are in it but not committed"
            ));
        }
        done.note = (!notes.is_empty()).then(|| notes.join("; "));
    }
    Ok(done)
}

/// What `api_adopt` writes, synchronously. A path that is taken — a file, a link, a folder that is
/// a file — is left alone and said so, one path at a time, so one odd path never stops the rest;
/// `CLAUDE.md` is the one file appended to, and only with what it lacks.
pub fn adopt(repo: &Utf8Path) -> Result<Adopted, String> {
    let name = repo.file_name().unwrap_or("project").to_string();
    let ctx = keel_scanner::RepoContext::load(repo).ok();
    let report = ctx
        .as_ref()
        .map(keel_scanner::scan)
        .unwrap_or_else(|| Report::new(Vec::new()));
    let mut files: Vec<(&str, String)> = keel_generator::team::agents()
        .into_iter()
        .map(|(p, b)| (p, b.to_string()))
        .collect();
    if ctx.as_ref().is_some_and(keel_scanner::has_frontend) {
        files.push((
            keel_generator::team::FRONTEND_SKILL_PATH,
            keel_generator::team::FRONTEND_SKILL.to_string(),
        ));
    }
    files.push(("docs/PRODUCTION.md", production_md(&name, &report)));
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for (rel, body) in files {
        let wrote = crate::writes::no_link_under(repo, rel)
            .and_then(|_| crate::writes::create_new(&repo.join(rel), body.as_bytes()));
        match wrote {
            Ok(true) => written.push(rel.to_string()),
            Ok(false) => skipped.push(format!("{rel} (already there)")),
            Err(why) => skipped.push(format!("{rel} ({})", short(repo, &why))),
        }
    }
    match claude_md(repo, &name) {
        Ok(Some(rel)) => written.push(rel),
        Ok(None) => skipped.push("CLAUDE.md (already says it)".into()),
        Err(why) => skipped.push(format!("CLAUDE.md ({})", short(repo, &why))),
    }
    Ok(Adopted {
        written,
        skipped,
        committed: None,
        note: None,
    })
}

/// A reason without the absolute path in front of it: the person knows where their project is.
fn short(repo: &Utf8Path, why: &str) -> String {
    why.strip_prefix(repo.as_str())
        .map(|r| r.trim_start_matches('/'))
        .unwrap_or(why)
        .to_string()
}

/// The file `claude_md` edits, absolute and repository-relative: `CLAUDE.md` under the name it
/// actually has on disk (a case-insensitive Mac opens `claude.md` for it, and git only knows the
/// real one), or — when it is a link, as `CLAUDE.md -> AGENTS.md` conventionally is — the file
/// behind it, as long as that is a file inside the repository.
fn instructions_file(repo: &Utf8Path) -> Result<(Utf8PathBuf, String), String> {
    let name = std::fs::read_dir(repo)
        .ok()
        .and_then(|entries| {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.eq_ignore_ascii_case("CLAUDE.md"))
                .collect();
            names
                .iter()
                .find(|n| *n == "CLAUDE.md")
                .or(names.first())
                .cloned()
        })
        .unwrap_or_else(|| "CLAUDE.md".into());
    let link = repo.join(&name);
    match std::fs::symlink_metadata(&link) {
        Ok(m) if m.file_type().is_symlink() => {
            let root = repo.canonicalize_utf8().map_err(|e| e.to_string())?;
            let target = link
                .canonicalize_utf8()
                .map_err(|_| "a symbolic link to nothing — left alone".to_string())?;
            let rel = target
                .strip_prefix(&root)
                .map_err(|_| "a symbolic link to outside the repository — left alone".to_string())?
                .to_string();
            if !target.is_file() {
                return Err("a symbolic link to a folder — left alone".into());
            }
            // Inside the repository is not enough: `.git/config` is inside it, and so is a
            // submodule's file. The same rule every other write keeps — no git internals, no
            // other repository on the way.
            let git_dirs = ["--absolute-git-dir", "--git-common-dir"].map(|flag| {
                crate::git::trimmed(repo, &["rev-parse", flag])
                    .ok()
                    .map(|d| repo.join(d))
                    .and_then(|d| d.canonicalize_utf8().ok())
            });
            // By name, and by where git actually keeps it: `--separate-git-dir` can put it
            // anywhere, under any name, inside the tree.
            if rel.split('/').any(|c| c.eq_ignore_ascii_case(".git"))
                || git_dirs.iter().flatten().any(|d| target.starts_with(d))
            {
                return Err("a symbolic link into .git — left alone".into());
            }
            crate::writes::no_link_under(&root, &rel)
                .map_err(|why| format!("a symbolic link to {rel}, where {why}"))?;
            Ok((target, rel))
        }
        Ok(m) if !m.is_file() => Err("not a file — left alone".into()),
        _ => Ok((link, name)),
    }
}

/// Append what the instructions lack, and return the repository-relative path written, or `None`
/// when they already say it.
fn claude_md(repo: &Utf8Path, name: &str) -> Result<Option<String>, String> {
    let (path, rel) = instructions_file(repo)?;
    let existing = match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("could not be read: {e}")),
        Ok(bytes) => {
            Some(String::from_utf8(bytes).map_err(|_| "not UTF-8 text — left alone".to_string())?)
        }
    };
    // Worked on with `\n` and written back in the file's own line endings.
    let crlf = existing.as_deref().is_some_and(|s| s.contains("\r\n"));
    let plain = existing.as_deref().map(|s| s.replace("\r\n", "\n"));
    let mut next = plain.clone().unwrap_or_else(|| format!("# {name}\n"));
    if !next.contains("docs/PRODUCTION.md") {
        next.push_str("\n## Production\n\nThe production checklist the reviewers enforce is in `docs/PRODUCTION.md`. It applies.\n");
    }
    let next = keel_generator::team::with_guidance(&next);
    if plain.as_deref() == Some(next.as_str()) {
        return Ok(None);
    }
    let next = if crlf {
        next.replace('\n', "\r\n")
    } else {
        next
    };
    if existing.is_none() {
        // Created, not written through: a link that appeared since the check is refused.
        return match crate::writes::create_new(&path, next.as_bytes())? {
            true => Ok(Some(rel)),
            false => Err("appeared meanwhile — left alone".into()),
        };
    }
    std::fs::write(&path, next).map_err(|e| format!("{path}: {e}"))?;
    Ok(Some(rel))
}

/// The production checklist, for this repository: its languages, its gate, where it runs,
/// and where it stands today — then the rules, phrased for its stack.
pub fn production_md(name: &str, report: &Report) -> String {
    let p = report.profile.clone().unwrap_or(Profile {
        template: "blank",
        template_title: "Blank project",
        like: "",
        confidence: 0,
        signals: vec![],
        hosting: vec![],
        stack: vec![],
        is_js: false,
        languages: vec![],
        gate: None,
    });
    let langs = if p.languages.is_empty() {
        "unknown".to_string()
    } else {
        p.languages.join(", ")
    };
    let stack = if p.stack.is_empty() {
        "—".to_string()
    } else {
        p.stack.join(", ")
    };
    let hosts = if p.hosting.is_empty() {
        "not said in the repository".to_string()
    } else {
        p.hosting
            .iter()
            .map(|h| h.name())
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let gate = p
        .gate
        .clone()
        .unwrap_or_else(|| "none yet — the first thing to add".into());
    let js = p.is_js;
    let go = p.languages.contains(&"go");
    let py = p.languages.contains(&"python");
    let rs = p.languages.contains(&"rust");
    let has_db = p.stack.iter().any(|s| {
        matches!(
            *s,
            "postgres"
                | "prisma"
                | "drizzle"
                | "gorm"
                | "sqlx"
                | "sqlalchemy"
                | "mongodb"
                | "alembic"
                | "golang-migrate"
                | "goose"
        )
    });

    let mut s = format!("# {name} — production checklist\n\n");
    s.push_str(&format!("Derived by Keel from this repository on adoption; edit it to fit, and keep it true. The reviewers under `.claude/agents/` enforce it.\n\n| | |\n|---|---|\n| Languages | {langs} |\n| Stack | {stack} |\n| Runs on | {hosts} |\n| Gate | `{gate}` |\n"));
    if p.template != "blank" {
        s.push_str(&format!(
            "| Closest template | {} (like {}) |\n",
            p.template_title, p.like
        ));
    }
    s.push_str("\n## The gate\n\n");
    s.push_str(&format!("`{gate}` is the one command that says whether the project is good. Keel runs it after every turn; CI runs it before every build; nobody says \"done\" without it. "));
    s.push_str(match (js, go, py, rs) {
        (_, true, _, _) => "For Go that is `go vet ./... && go test ./...` at minimum, with `-race` in CI.\n",
        (_, _, true, _) => "For Python that is `ruff check && pytest`, with type checking (`mypy` or `pyright`) once the annotations are there.\n",
        (_, _, _, true) => "For Rust that is `cargo clippy --all-targets -- -D warnings && cargo test`.\n",
        (true, _, _, _) => "For TypeScript that is the typecheck (`tsc --noEmit`) and the tests; lint is a bonus, types are the point.\n",
        _ => "\n",
    });

    s.push_str("\n## Where it stands\n\n");
    if report.plan.is_empty() {
        s.push_str("The scan found nothing outstanding. Keep it that way: the checks re-run every time Keel reads the project.\n");
    } else {
        s.push_str(&format!(
            "Readiness {}/100 when this file was written. In order:\n\n",
            report.score
        ));
        for (i, ph) in report.plan.iter().enumerate() {
            s.push_str(&format!("{}. **{}** — {}\n", i + 1, ph.title, ph.why));
            for id in &ph.findings {
                if let Some(f) = report.findings.iter().find(|f| f.id == *id) {
                    s.push_str(&format!("   - {} — {}\n", f.title, f.fix.description()));
                }
            }
        }
    }

    s.push_str("\n## Change discipline\n\n- Small commits, one thing each, conventional messages. A change appends to `CHANGELOG.md`: past tense, what and why.\n- Prefer a dependency the project already has over a new one.\n- Config comes from the environment and every variable is in `.env.example` with a comment. Secrets are never in the repository");
    s.push_str(if p.hosting.iter().any(|h| h.is_cloud()) {
        " — they live in the cloud's secret manager and are rendered at deploy.\n"
    } else {
        ".\n"
    });
    if has_db {
        s.push_str("- Schema changes are migration files in the repository, applied before the new version serves — expand/contract, N−1 compatible, never a rename in place.\n");
    }
    s.push_str(match (js, go, py, rs) {
        (_, true, _, _) => "- Logs are structured (`slog` or `zerolog`), one line per event with `request_id`; never `log.Printf` in a handler.\n",
        (_, _, true, _) => "- Logs are structured (`structlog` or JSON logging), one line per event with `request_id`; never bare `print`.\n",
        (_, _, _, true) => "- Logs go through `tracing` with a JSON subscriber in production and `request_id` on every span.\n",
        _ => "- Logs are one JSON object per line with `request_id`; no bare `console.log` in the backend.\n",
    });

    s.push_str("\n## Production rules\n\nChecked by the `reliability` and `security` reviewers; argue with the rule here, not in a PR.\n\n");
    s.push_str("- Handlers are stateless; session and rate-limit state live in Redis or a signed cookie, never in process memory once there are replicas.\n");
    s.push_str("- Every outbound call has a timeout shorter than its caller's, and the deadline travels in a header. Retries: bounded, exponential backoff with jitter, idempotent operations only.\n");
    s.push_str("- A mutating route a client may retry takes `Idempotency-Key`, stored with the request hash and the response. Exactly-once is at-least-once plus an idempotent consumer.\n");
    s.push_str("- \"Write the row and publish the event\" is one transaction through an outbox; every consumer has a dead-letter path and a replay tool.\n");
    s.push_str("- Pagination is keyset, capped at 100. IDs are UUIDs (v7 where the database sorts on them).\n");
    s.push_str("- Inputs are validated once at the boundary with a schema; unknown fields are rejected; bodies are bounded. Between services: short-lived signed tokens with an audience.\n");
    s.push_str(
        "- Rate limits per principal (`429` + `RateLimit-*`); auth endpoints also lock out.\n",
    );
    s.push_str("- Three probes with three meanings: startup, readiness (checks dependencies), liveness (never does). Drain on SIGTERM; a `preStop` sleep so endpoints are removed first.\n");
    s.push_str("- Circuit breakers and per-dependency concurrency limits on every external call; degrade with a typed \"unavailable\", never a 500.\n");
    s.push_str("- Images run as a non-root user, pinned base, `.dockerignore`, tagged by commit SHA — `latest` is an alias, never what a cluster pulls.\n");
    s.push_str("- CI: `permissions: contents: read` by default, actions pinned to SHAs, a concurrency group, a timeout on every job, and no deploy that does not `needs:` the gate. Production deploys from a tag or a dispatch into a protected environment.\n");
    s.push_str("- Alerts fire on SLO burn rate and link to a runbook. Backups are only real once restored.\n");

    s.push_str("\n## Deploying\n\n");
    s.push_str(&match p.hosting.first() {
        Some(h) if h.is_cloud() => format!("Already on {}: build around it — the image registry, managed Postgres, the secret manager and the cluster or runtime it offers. Dev deploys on push to `main`; prod on a tag, into a protected environment.\n", h.name()),
        Some(h) if h.is_managed_platform() => format!("On {} today, which is the right place to launch from. Its ceiling is no long-lived processes and a bill that scales with the wrong number; when it is close, the same image runs on a cluster — the migration is a build step, not a rewrite.\n", h.name()),
        Some(h) if h.is_baas() => format!("Data and auth in {}: make sure the repository can recreate the schema (migrations here, not in the dashboard), and run dev against a local database, never production data.\n", h.name()),
        Some(keel_scanner::Hosting::Kubernetes) => "kustomize `base` + `dev`/`prod` overlays; `git push` to `main` deploys dev, a tag deploys prod; only what changed rolls.\n".to_string(),
        _ => "Not yet described in the repository. Target: an image, a compose file for the laptop, manifests for the runtime, dev and prod apart, a pipeline that does it.\n".to_string(),
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go_repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("cmd/api")).unwrap();
        std::fs::create_dir_all(root.join("migrations")).unwrap();
        std::fs::write(
            root.join("go.mod"),
            "module x\n\nrequire (\n\tgithub.com/go-chi/chi/v5 v5.1.0\n\tgorm.io/gorm v1.2.3\n)\n",
        )
        .unwrap();
        std::fs::write(root.join("cmd/api/main.go"), "package main\nfunc main() {\n\tr.Get(\"/api/health\", health)\n\tos.Getenv(\"DATABASE_URL\")\n}\n").unwrap();
        std::fs::write(
            root.join("cmd/api/main_test.go"),
            "package main\nfunc TestX(t *testing.T) { assert.Equal(t, 1, 1) }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("migrations/0001.sql"),
            "create table users (id uuid);\n",
        )
        .unwrap();
        std::fs::write(root.join("Makefile"), "test:\n\tgo test ./...\n").unwrap();
        (dir, root)
    }

    #[test]
    fn the_map_reads_routes_env_tables_and_tests() {
        let (_d, root) = go_repo();
        let m = repo_map(&root, None);
        assert!(
            m.contains("`cmd/api/main.go:3` r.Get(\"/api/health\""),
            "{m}"
        );
        assert!(m.contains("`DATABASE_URL`"));
        assert!(m.contains("No `.env.example`"));
        assert!(m.contains("`users`"));
        assert!(m.contains("1 test files"));
        assert!(m.contains("`CLAUDE.md` — absent"));
    }

    fn team_finding(root: &Utf8Path) -> Option<keel_scanner::Finding> {
        let ctx = keel_scanner::RepoContext::load(root).unwrap();
        keel_scanner::scan(&ctx)
            .findings
            .into_iter()
            .find(|f| f.id == "agent/no-reviewers")
    }

    /// The finding's Fix is this function, so the function must clear it — which it can only do
    /// if the scanner's list of the team and the generator's are the same list. And Claude Code
    /// must actually load what was written, under the names the lifecycle calls.
    #[test]
    fn adopting_clears_the_team_finding() {
        let (_d, root) = go_repo();
        let before = team_finding(&root).expect("a bare repo lacks the team");
        assert_eq!(before.severity, keel_scanner::Severity::Medium);

        let first = adopt(&root).unwrap();
        for (path, _) in keel_generator::team::agents() {
            assert!(
                first.written.iter().any(|w| w == path),
                "{path} not written"
            );
        }
        assert!(first.written.iter().any(|w| w == "CLAUDE.md"));
        assert_eq!(team_finding(&root), None, "adopting must clear it");

        let loaded: Vec<String> = keel_workspace::discover_agents(&root, &root.join("no-home"))
            .into_iter()
            .filter(|a| a.description.is_some())
            .map(|a| a.name)
            .collect();
        for name in [
            "pm",
            "designer",
            "principal",
            "qa",
            "reviewer",
            "security",
            "reliability",
        ] {
            assert!(
                loaded.iter().any(|n| n == name),
                "{name} does not load: {loaded:?}"
            );
        }

        let again = adopt(&root).unwrap();
        assert!(
            again.written.is_empty(),
            "second adopt wrote {:?}",
            again.written
        );
    }

    /// A team the project already wrote is the project's. Adopting fills the gaps around it and
    /// appends to CLAUDE.md without touching a line that was there.
    #[test]
    fn adopting_fills_gaps_and_never_overwrites() {
        let (_d, root) = go_repo();
        std::fs::create_dir_all(root.join(".claude/agents")).unwrap();
        let ours = "---\nname: reviewer\ndescription: \"ours\"\n---\nkeep me\n";
        std::fs::write(root.join(".claude/agents/reviewer.md"), ours).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# ours\n\nhouse rules\n").unwrap();

        let partial = team_finding(&root).expect("six are missing");
        assert_eq!(partial.severity, keel_scanner::Severity::Low);
        assert!(partial.title.contains("`pm`") && !partial.title.contains("`reviewer`"));

        let r = adopt(&root).unwrap();
        assert!(
            r.skipped
                .iter()
                .any(|s| s.starts_with(".claude/agents/reviewer.md"))
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/agents/reviewer.md")).unwrap(),
            ours
        );
        let claude = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(claude.starts_with("# ours\n\nhouse rules\n"), "{claude}");
        assert_eq!(
            claude.matches("## How a request becomes a change").count(),
            1
        );
        assert_eq!(claude.matches("docs/PRODUCTION.md").count(), 1);
        assert_eq!(team_finding(&root), None);
    }

    /// The frontend skill goes where there is a frontend and nowhere else, and the scanner asks
    /// for it on exactly the same condition — otherwise the fix could not clear the finding.
    #[test]
    fn the_frontend_skill_goes_only_where_there_is_a_frontend() {
        let (_d, go) = go_repo();
        adopt(&go).unwrap();
        assert!(!go.join(keel_generator::team::FRONTEND_SKILL_PATH).exists());
        assert!(
            std::fs::read_to_string(go.join("CLAUDE.md"))
                .unwrap()
                .contains(keel_generator::team::RULES_HEADING)
        );

        let (_d2, web) = go_repo();
        std::fs::write(
            web.join("package.json"),
            r#"{"dependencies":{"react":"19","next":"16"}}"#,
        )
        .unwrap();
        let before = team_finding(&web).expect("no team, no skill");
        assert!(
            before.title.contains("the frontend skill"),
            "{}",
            before.title
        );
        adopt(&web).unwrap();
        let skills: Vec<String> = keel_workspace::discover_skills(&web, &web.join("no-home"))
            .into_iter()
            .filter(|s| s.description.is_some())
            .map(|s| s.name)
            .collect();
        assert_eq!(skills, ["frontend"]);
        assert_eq!(team_finding(&web), None);
    }

    /// A repository that gitignores `.claude/` hides the team from the scan's walk; the fix must
    /// still clear the finding it was offered for.
    #[test]
    fn adopting_clears_the_finding_when_claude_is_gitignored() {
        let (_d, root) = go_repo();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"react":"19"}}"#,
        )
        .unwrap();
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
        adopt(&root).unwrap();
        assert_eq!(team_finding(&root), None);
    }

    /// A repository cannot point Keel's write somewhere else with a link, even a dangling one.
    #[test]
    fn adopting_never_writes_through_a_link() {
        let (_d, root) = go_repo();
        let outside = tempfile::tempdir().unwrap();
        let target = Utf8Path::from_path(outside.path())
            .unwrap()
            .join("pwned.md");
        std::fs::create_dir_all(root.join(".claude/agents")).unwrap();
        std::os::unix::fs::symlink(&target, root.join(".claude/agents/pm.md")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("c.md"), root.join("CLAUDE.md")).unwrap();
        let r = adopt(&root).unwrap();
        assert!(!target.exists() && !outside.path().join("c.md").exists());
        assert!(
            r.skipped
                .iter()
                .any(|s| s.starts_with(".claude/agents/pm.md"))
        );
        assert!(
            r.skipped
                .iter()
                .any(|s| s.starts_with("CLAUDE.md (a symbolic link")),
            "{r:?}"
        );
    }

    /// A CLAUDE.md Keel cannot append to is left alone and said so; the rest is still written,
    /// and a retry does not fail forever.
    #[test]
    fn a_claude_md_that_is_not_text_is_left_alone() {
        let (_d, root) = go_repo();
        std::fs::write(root.join("CLAUDE.md"), b"# x\n\xff\xfe bad\n").unwrap();
        let r = adopt(&root).unwrap();
        assert!(
            r.skipped
                .iter()
                .any(|s| s == "CLAUDE.md (not UTF-8 text — left alone)"),
            "{r:?}"
        );
        assert!(r.written.iter().any(|w| w == ".claude/agents/pm.md"));
        assert_eq!(
            std::fs::read(root.join("CLAUDE.md")).unwrap(),
            b"# x\n\xff\xfe bad\n"
        );
        assert!(adopt(&root).is_ok());
    }

    fn git_repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let (d, root) = go_repo();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "-A"],
            &["commit", "-qm", "base"],
        ] {
            crate::git::run(&root, args).unwrap();
        }
        (d, root)
    }

    /// The person's half-written CLAUDE.md is theirs: Keel appends to it and does not commit it.
    #[test]
    fn a_commit_never_takes_the_persons_claude_md_edits() {
        let (_d, root) = git_repo();
        std::fs::write(root.join("CLAUDE.md"), "# x\n").unwrap();
        crate::git::run(&root, &["add", "CLAUDE.md"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "claude"]).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# x\nMY WIP\n").unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let done = adopt_and_commit(&state, &root, true).unwrap();
        assert!(done.committed.is_some(), "{done:?}");
        assert!(
            done.note
                .as_deref()
                .unwrap_or("")
                .contains("CLAUDE.md has your own uncommitted edits")
        );
        let shown = crate::git::run(&root, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        assert!(
            !shown.contains("CLAUDE.md") && shown.contains(".claude/agents/pm.md"),
            "{shown}"
        );
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert_eq!(status.trim(), "M CLAUDE.md");
    }

    /// A clean CLAUDE.md is Keel's to commit along with the team, and adopting is then silent.
    #[test]
    fn a_clean_tree_is_committed_whole() {
        let (_d, root) = git_repo();
        let state = Arc::new(AppState::new(root.clone()));
        let done = adopt_and_commit(&state, &root, true).unwrap();
        assert!(done.committed.is_some() && done.note.is_none(), "{done:?}");
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    /// One odd path — a file where a folder should be — is skipped; the rest is written.
    #[test]
    fn a_file_named_docs_does_not_stop_the_rest() {
        let (_d, root) = go_repo();
        std::fs::write(root.join("docs"), "a file").unwrap();
        let r = adopt(&root).unwrap();
        assert!(
            r.skipped
                .iter()
                .any(|s| s.starts_with("docs/PRODUCTION.md (docs is a file")),
            "{r:?}"
        );
        assert!(r.written.iter().any(|w| w == "CLAUDE.md"));
        assert_eq!(team_finding(&root), None);
    }

    /// `CLAUDE.md -> AGENTS.md` is a convention; the file behind it is the one appended to.
    #[test]
    fn a_claude_md_linked_to_agents_md_is_followed() {
        let (_d, root) = go_repo();
        std::fs::write(root.join("AGENTS.md"), "# rules\n").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();
        let r = adopt(&root).unwrap();
        assert!(r.written.iter().any(|w| w == "AGENTS.md"), "{r:?}");
        assert!(
            std::fs::symlink_metadata(root.join("CLAUDE.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(root.join("AGENTS.md"))
                .unwrap()
                .contains(keel_generator::team::LIFECYCLE_HEADING)
        );
        assert_eq!(team_finding(&root), None);
    }

    /// A case-insensitive disk opens `claude.md` for `CLAUDE.md`; git only knows the real name,
    /// and committing the other one failed the whole commit.
    #[test]
    fn a_lowercase_claude_md_is_committed_under_its_own_name() {
        let (_d, root) = git_repo();
        std::fs::write(root.join("claude.md"), "# lower\n").unwrap();
        crate::git::run(&root, &["add", "claude.md"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "c"]).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let done = adopt_and_commit(&state, &root, true).unwrap();
        assert!(done.committed.is_some() && done.note.is_none(), "{done:?}");
        assert!(done.written.iter().any(|w| w == "claude.md"), "{done:?}");
        let status = crate::git::run(&root, &["status", "--porcelain"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    /// A link to some other file than AGENTS.md is still the person's file when they have edited it.
    #[test]
    fn a_linked_instructions_file_with_edits_is_not_committed() {
        let (_d, root) = git_repo();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/INSTR.md"), "# i\n").unwrap();
        std::os::unix::fs::symlink("notes/INSTR.md", root.join("CLAUDE.md")).unwrap();
        crate::git::run(&root, &["add", "-A"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "l"]).unwrap();
        std::fs::write(root.join("notes/INSTR.md"), "# i\nPRIVATE\n").unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let done = adopt_and_commit(&state, &root, true).unwrap();
        let shown = crate::git::run(&root, &["show", "HEAD:notes/INSTR.md"]).unwrap();
        assert!(!shown.contains("PRIVATE"), "{done:?}");
        assert!(
            done.note
                .as_deref()
                .unwrap_or("")
                .contains("notes/INSTR.md has your own")
        );
    }

    /// Inside the repository is not enough: `.git/config` is inside it, and so is a submodule.
    #[test]
    fn a_link_into_git_internals_or_another_repository_is_left_alone() {
        let (_d, root) = git_repo();
        std::os::unix::fs::symlink(".git/config", root.join("CLAUDE.md")).unwrap();
        let config = std::fs::read(root.join(".git/config")).unwrap();
        let r = adopt(&root).unwrap();
        assert_eq!(std::fs::read(root.join(".git/config")).unwrap(), config);
        assert!(
            r.skipped
                .iter()
                .any(|s| s == "CLAUDE.md (a symbolic link into .git — left alone)"),
            "{r:?}"
        );

        let (_d2, root) = git_repo();
        std::fs::create_dir_all(root.join("sub/.git")).unwrap();
        std::fs::write(root.join("sub/INSTR.md"), "# sub\n").unwrap();
        std::os::unix::fs::symlink("sub/INSTR.md", root.join("CLAUDE.md")).unwrap();
        let r = adopt(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("sub/INSTR.md")).unwrap(),
            "# sub\n"
        );
        assert!(
            r.skipped
                .iter()
                .any(|s| s.contains("another git repository")),
            "{r:?}"
        );
    }

    /// `--separate-git-dir` puts the git dir anywhere, under any name, inside the tree.
    #[test]
    fn a_link_into_a_git_dir_by_another_name_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path())
            .unwrap()
            .canonicalize_utf8()
            .unwrap();
        let meta = root.join("meta");
        crate::git::run(&root, &["init", "-q", "--separate-git-dir", meta.as_str()]).unwrap();
        std::os::unix::fs::symlink("meta/config", root.join("CLAUDE.md")).unwrap();
        let before = std::fs::read(meta.join("config")).unwrap();
        let r = adopt(&root).unwrap();
        assert_eq!(std::fs::read(meta.join("config")).unwrap(), before);
        assert!(r.skipped.iter().any(|s| s.contains("into .git")), "{r:?}");
    }

    #[test]
    fn a_crlf_claude_md_stays_crlf() {
        let (_d, root) = go_repo();
        std::fs::write(root.join("CLAUDE.md"), "# x\r\nrules\r\n").unwrap();
        adopt(&root).unwrap();
        let body = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(body.starts_with("# x\r\nrules\r\n"));
        assert!(
            !body.replace("\r\n", "").contains('\n'),
            "a bare newline was written"
        );
    }

    /// Every agent present but a CLAUDE.md that never says when to call them: files nobody runs.
    #[test]
    fn a_team_with_no_lifecycle_is_still_a_finding() {
        let (_d, root) = go_repo();
        adopt(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# plain\n").unwrap();
        let f = team_finding(&root).expect("no lifecycle");
        assert_eq!(
            f.title,
            "Agent team incomplete: missing the lifecycle in CLAUDE.md"
        );
    }

    #[test]
    fn production_md_is_about_this_repository_not_the_stack() {
        let (_d, root) = go_repo();
        let ctx = keel_scanner::RepoContext::load(&root).unwrap();
        let report = keel_scanner::scan(&ctx);
        let md = production_md("x", &report);
        assert!(md.contains("| Languages | go |"), "{md}");
        assert!(md.contains("`make test`"));
        assert!(md.contains("slog"));
        assert!(!md.contains("Hono"));
        assert!(!md.contains("bun"));
        assert!(md.contains("## Where it stands"));
    }

    #[test]
    fn the_contract_keeps_the_teams_part_and_refreshes_the_review() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let review = "## Scorecard\n\n| a | 3 |\n\n## 3. **The five things that will hurt first**\n\n1. x\n\n## The plan\n\n1. PR one\n";
        write_contract(&root, "2026-08-29", review).unwrap();
        let path = root.join(".claude/agents/contract.md");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.starts_with("---\nname: contract"), "{first}");
        assert!(first.contains("## The five things\n\n1. x"), "{first}");
        assert!(first.contains("## Latest review"));
        let edited = first.replace("1. x", "1. x — corrected by the team");
        std::fs::write(&path, edited).unwrap();
        write_contract(&root, "2026-09-01", "## Scorecard\n\n| a | 4 |\n").unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert!(second.contains("corrected by the team"));
        assert!(second.contains("| a | 4 |"));
        assert_eq!(second.matches("## Latest review").count(), 1);
        assert!(second.contains("_2026-09-01."));
    }

    #[test]
    fn a_note_lands_under_one_heading_and_later_ones_follow_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("CLAUDE.md"),
            "# proj\n\n## Rules\n\n- never push\n",
        )
        .unwrap();
        remember(&root, "the staging database is read-only").unwrap();
        remember(&root, "deploys go out on Thursdays").unwrap();
        let md = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert_eq!(md.matches(MEMORY_HEADING).count(), 1, "{md}");
        assert!(md.contains("- never push"), "the rest of the file survives");
        let first = md.find("read-only").unwrap();
        let second = md.find("Thursdays").unwrap();
        assert!(first < second, "newest last: {md}");
    }

    #[test]
    fn the_evidence_carries_the_scan_and_the_map() {
        let (_d, root) = go_repo();
        let ctx = keel_scanner::RepoContext::load(&root).unwrap();
        let report = keel_scanner::scan(&ctx);
        let e = evidence(&report, &repo_map(&root, report.profile.as_ref()));
        assert!(e.contains("Closest Keel template: **REST API service**"));
        assert!(e.contains("# Repository map"));
        assert!(
            STAFF_ENGINEER.contains("Security posture")
                && STAFF_ENGINEER.contains("Test quality")
                && STAFF_ENGINEER.contains("Agent instructions")
        );
        assert_eq!(
            humantime_date(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_756_425_600)),
            "2025-08-29"
        );
    }
}
