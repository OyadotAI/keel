//! The review a staff engineer would give, and the files that make a repository reviewable.
//!
//! Three handlers. `GET /api/review` builds the review turn: a persona and a method as the
//! system prompt, and as evidence a **map of the repository** the daemon computes — routes,
//! entry points, environment variables, tables, tests, CI, images, the agent instructions —
//! so the model starts from facts and spends its reading on judgement. It is sent in plan
//! mode, so the agent reads and changes nothing. `POST /api/review/save` writes the finished
//! review into `docs/REVIEW.md`. `POST /api/adopt` writes the three reviewer subagents and a
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
        if p.components().any(|c| {
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
        if let Ok(rel) = p.strip_prefix(repo) {
            out.push(rel.to_owned());
        }
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
    dirs.sort_by(|a, b| b.1.1.cmp(&a.1.1));
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
        b.text.trim()
    );
    std::fs::write(&path, body).map_err(err)?;
    Ok(Json("docs/REVIEW.md".into()))
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

#[derive(Serialize)]
pub struct Adopted {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

/// Write the reviewers and a production checklist derived from this repository, never
/// overwriting.
pub async fn api_adopt(
    State(_state): State<Arc<AppState>>,
    Checkout(repo): Checkout,
) -> Result<Json<Adopted>, (axum::http::StatusCode, String)> {
    let name = repo
        .file_name()
        .map(str::to_string)
        .unwrap_or_else(|| "project".into());
    let r2 = repo.clone();
    let report = tokio::task::spawn_blocking(move || {
        keel_scanner::RepoContext::load(&r2)
            .map(|ctx| keel_scanner::scan(&ctx))
            .unwrap_or_else(|_| Report::new(Vec::new()))
    })
    .await
    .unwrap_or_else(|_| Report::new(Vec::new()));
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
        ("docs/PRODUCTION.md", production_md(&name, &report)),
    ];
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let err = |e: std::io::Error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    for (rel, body) in files {
        let path = repo.join(rel);
        if path.exists() {
            skipped.push(rel.to_string());
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::fs::write(&path, body).map_err(err)?;
        written.push(rel.to_string());
    }
    let claude = repo.join("CLAUDE.md");
    let pointer = "\n## Production\n\nThe production checklist the reviewers enforce is in `docs/PRODUCTION.md`. It applies.\n";
    match std::fs::read_to_string(&claude) {
        Ok(existing) if !existing.contains("docs/PRODUCTION.md") => {
            std::fs::write(&claude, existing + pointer).map_err(err)?;
            written.push("CLAUDE.md (appended)".into());
        }
        Err(_) => {
            std::fs::write(&claude, format!("# {name}\n{pointer}")).map_err(err)?;
            written.push("CLAUDE.md".into());
        }
        _ => skipped.push("CLAUDE.md".into()),
    }
    Ok(Json(Adopted { written, skipped }))
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
