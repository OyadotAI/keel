//! How good the pipeline is, not whether one exists.
//!
//! A workflow file is read the way a reviewer reads it: what triggers it, whether the token it
//! runs with is bounded, whether the actions it pulls are pinned, whether a deploy is gated on
//! the checks, whether a second push cancels the first, whether a job can hang forever. Each
//! finding names the file. Text heuristics over YAML — no parser dependency, and GitHub's
//! syntax is regular enough for `uses:`, `permissions:`, `concurrency:` to be read by line.

use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
use camino::Utf8Path;

pub struct PipelineQuality;

struct Workflow<'a> {
    path: &'a Utf8Path,
    text: String,
    lower: String,
}

/// A comment is prose, not configuration. `has()` substring-matches the whole file, so a note
/// explaining that a build's *deployment target* comes from elsewhere used to reclassify the
/// workflow as a deploy and fail the repository's own `scan --strict`. A YAML comment starts at a
/// `#` that begins the line or follows whitespace — anything else (`sha#frag`) is a value.
fn without_comments(text: &str) -> String {
    text.lines()
        .map(|line| {
            match line
                .char_indices()
                .find(|&(i, c)| c == '#' && (i == 0 || line[..i].ends_with(char::is_whitespace)))
            {
                Some((i, _)) => &line[..i],
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Workflow<'_> {
    fn has(&self, s: &str) -> bool {
        self.lower.contains(s)
    }
    fn deploys(&self) -> bool {
        self.has("deploy")
            || self.has("kubectl")
            || self.has("docker/build-push")
            || self.has("wrangler")
            || self.has("vercel")
            || self.has("helm ")
            || self.has("kustomize")
    }
    /// A `run:` line that runs a check. Matched on whole commands, not substrings: an image
    /// tag `:latest` contains "test", and a workflow that pushes one is not testing anything.
    fn tests(&self) -> bool {
        const CHECKS: &[&str] = &[
            "npm test",
            "npm run test",
            "npm run typecheck",
            "npm run lint",
            "pnpm test",
            "pnpm run test",
            "yarn test",
            "bun test",
            "bun run test",
            "bun run typecheck",
            "cargo test",
            "cargo clippy",
            "go test",
            "pytest",
            "make check",
            "make test",
            "vitest",
            "jest",
            "tsc ",
            "tsc\n",
            "npx tsc",
            "mvn test",
            "gradle test",
        ];
        self.lower
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("- run:")
                    .or_else(|| l.trim().strip_prefix("run:"))
            })
            .any(|cmd| CHECKS.iter().any(|c| (cmd.to_string() + "\n").contains(c)))
    }
    /// `uses: owner/repo@ref` lines whose ref is a tag or branch rather than a commit SHA.
    fn unpinned(&self) -> Vec<String> {
        self.text
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("- uses:")
                    .or_else(|| l.trim().strip_prefix("uses:"))
            })
            .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\''))
            .filter(|u| !u.starts_with("./") && !u.starts_with("docker://"))
            .filter(|u| {
                // `owner/repo@<sha> # v4` — the comment is the human-readable version.
                let r = u.rsplit_once('@').map(|(_, r)| r).unwrap_or("");
                let r = r.split('#').next().unwrap_or("").trim();
                !(r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()))
            })
            .map(str::to_string)
            .collect()
    }
    fn jobs(&self) -> usize {
        // Lines two spaces in under `jobs:` that end with a colon are job names.
        let mut n = 0;
        let mut in_jobs = false;
        for l in self.text.lines() {
            if l.starts_with("jobs:") {
                in_jobs = true;
                continue;
            }
            if in_jobs && !l.starts_with(' ') && !l.trim().is_empty() {
                in_jobs = false;
            }
            if in_jobs
                && l.starts_with("  ")
                && !l.starts_with("   ")
                && l.trim_end().ends_with(':')
            {
                n += 1;
            }
        }
        n
    }
}

impl Check for PipelineQuality {
    fn id(&self) -> &'static str {
        "ci/*"
    }

    fn dimension(&self) -> Dimension {
        Dimension::Verifiability
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let workflows: Vec<Workflow> = ctx
            .files()
            .filter(|p| {
                p.as_str().starts_with(".github/workflows/")
                    && p.extension().is_some_and(|e| e == "yml" || e == "yaml")
            })
            .filter_map(|p| {
                ctx.read(p.as_str()).map(|text| Workflow {
                    path: p,
                    lower: without_comments(&text).to_lowercase(),
                    text,
                })
            })
            .collect();
        if workflows.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let any_pr = workflows.iter().any(|w| w.has("pull_request"));
        let any_tests = workflows.iter().any(|w| w.tests());

        if !any_tests {
            out.push(Finding::new(
                "ci/no-checks-run",
                Dimension::Verifiability,
                Severity::High,
                "CI runs, but runs no checks",
                "No workflow runs tests or a typecheck. Whatever CI is doing — building, deploying, \
                 labelling — it is doing to code nothing has verified.",
                Fix::Assisted { description: "Add a `check` job that runs the gate (`make check`, or typecheck + test) and make every build and deploy job `needs: check`.".into() },
            ));
        }
        if !any_pr {
            out.push(Finding::new(
                "ci/no-pull-request-trigger",
                Dimension::Verifiability,
                Severity::Medium,
                "Checks do not run on pull requests",
                "Nothing runs before a merge, so the first time a change is checked is after it is \
                 on main. Reviewers see a diff with no verdict beside it.",
                Fix::Automatic { description: "Add `pull_request:` to the check workflow's `on:` alongside `push: branches: [main]`.".into() },
            ));
        }

        for w in &workflows {
            let at = |f: Finding| f.at(w.path.to_owned());

            if !w.has("permissions:") {
                out.push(at(Finding::new(
                    "ci/token-not-scoped",
                    Dimension::Security,
                    Severity::Medium,
                    "Workflow runs with the default token permissions",
                    "No `permissions:` block, so the job's GITHUB_TOKEN can write to contents, \
                     packages and pull requests by default in older repositories. A compromised \
                     action in this workflow can push to main.",
                    Fix::Automatic { description: "Add `permissions: { contents: read }` at the top of the workflow and grant `packages: write` or `id-token: write` only to the job that needs it.".into() },
                )));
            }
            let unpinned = w.unpinned();
            if !unpinned.is_empty() {
                let sample = unpinned
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(at(Finding::new(
                    "ci/actions-not-pinned",
                    Dimension::Security,
                    Severity::Low,
                    format!("{} action{} pinned to a tag, not a commit", unpinned.len(), if unpinned.len() == 1 { "" } else { "s" }),
                    format!(
                        "`{sample}` resolve a moving tag at run time. A tag can be re-pointed by whoever \
                         controls the action — that is how the tj-actions compromise reached thousands \
                         of pipelines. A 40-character SHA cannot move."
                    ),
                    Fix::Automatic { description: "Pin each `uses:` to a commit SHA with the version in a comment (`actions/checkout@<sha> # v4`), and let Dependabot's `github-actions` ecosystem keep them current.".into() },
                )));
            }
            if !w.has("concurrency:") {
                out.push(at(Finding::new(
                    "ci/no-concurrency-group",
                    Dimension::Verifiability,
                    Severity::Low,
                    "Pushes queue instead of cancelling",
                    "Without a `concurrency:` group, five quick pushes run five full pipelines and \
                     the deploy job can run twice at once, out of order.",
                    Fix::Automatic { description: "Add `concurrency: { group: ${{ github.workflow }}-${{ github.ref }}, cancel-in-progress: true }` (without cancel-in-progress on the deploy workflow, so a rollout is never cut in half).".into() },
                )));
            }
            if !w.has("timeout-minutes") {
                out.push(at(Finding::new(
                    "ci/no-job-timeout",
                    Dimension::Verifiability,
                    Severity::Low,
                    "Jobs can hang for six hours",
                    "GitHub's default job timeout is 360 minutes. A stuck test or a waiting deploy \
                     holds a runner and a merge queue for the afternoon.",
                    Fix::Automatic { description: "Set `timeout-minutes:` on every job (15 for checks, 30 for a build and deploy).".into() },
                )));
            }
            if w.has("pull_request_target")
                && (w.has("ref: ${{ github.event.pull_request.head") || w.has("head.sha"))
            {
                out.push(at(Finding::new(
                    "ci/pull-request-target-checks-out-fork",
                    Dimension::Security,
                    Severity::Critical,
                    "pull_request_target checks out the pull request's code",
                    "`pull_request_target` runs with the base repository's secrets; checking out the \
                     PR's head then runs a stranger's code with them. This is the canonical \
                     GitHub Actions takeover.",
                    Fix::Manual { description: "Use `pull_request` for anything that runs PR code; keep `pull_request_target` only for jobs that never check out the PR (labelling, comments).".into() },
                )));
            }
            if w.deploys() {
                let gated = w.has("needs:") && w.tests() || any_tests && w.has("workflow_run");
                if !gated {
                    out.push(at(Finding::new(
                        "ci/deploy-not-gated",
                        Dimension::Deployability,
                        Severity::High,
                        "Deploys without waiting for the checks",
                        "The deploy job has no `needs:` on a job that runs the tests, so a red \
                         test and a green deploy can be the same commit.",
                        Fix::Assisted { description: "Run the gate in a `check` job in the same workflow and give the build and deploy jobs `needs: check`.".into() },
                    )));
                }
                let prod = w.has("prod") || w.has("production");
                let protected = w.has("environment:")
                    || w.has("tags:")
                    || w.has("workflow_dispatch")
                    || w.has("release:");
                if prod && !protected && w.has("push:") {
                    out.push(at(Finding::new(
                        "ci/prod-deploys-on-push",
                        Dimension::Deployability,
                        Severity::Medium,
                        "Production deploys on every push",
                        "Production goes out on `push` with no tag, release, environment approval \
                         or manual trigger between a merge and a rollout. A merge at 6pm is a \
                         production change at 6:03.",
                        Fix::Assisted { description: "Deploy dev on push to main; deploy prod on a tag (`v*`) or a `workflow_dispatch`, into a GitHub `environment: production` with a required reviewer.".into() },
                    )));
                }
                if (w.has("docker build") || w.has("docker/build-push"))
                    && w.has(":latest")
                    && !w.has("github.sha")
                    && !w.has("sha-")
                {
                    out.push(at(Finding::new(
                        "ci/image-only-latest",
                        Dimension::Deployability,
                        Severity::Medium,
                        "Images are tagged only :latest",
                        "A `:latest` tag cannot be rolled back to — the previous image has no name — \
                         and a cluster pulling `latest` can run two versions at once during a rollout.",
                        Fix::Automatic { description: "Tag every image with `${{ github.sha }}` (and the git tag on a release); deploy that tag; keep `latest` only as a convenience alias.".into() },
                    )));
                }
                if w.has("ssh ") && w.has("docker") && !w.has("kubectl") {
                    out.push(at(Finding::new(
                        "ci/deploy-by-ssh",
                        Dimension::Deployability,
                        Severity::Low,
                        "Deploys by SSH-ing into a server",
                        "The pipeline logs into a machine and runs docker there. That machine is \
                         now the deployment system: one box, hand-configured, with the SSH key in CI.",
                        Fix::Manual { description: "Move the runtime to a cluster or a platform that pulls a tagged image; the templates' kustomize overlays and deploy workflows are the shape.".into() },
                    )));
                }
            }
            if (w.has("npm install")
                || w.has("npm i ")
                || w.has("yarn install")
                || w.has("bun install")
                || w.has("pnpm install"))
                && !w.has("cache")
                && !w.has("frozen-lockfile")
                && !w.has("npm ci")
            {
                out.push(at(Finding::new(
                    "ci/install-unpinned-uncached",
                    Dimension::Verifiability,
                    Severity::Low,
                    "Installs dependencies without the lockfile or a cache",
                    "`npm install` may resolve different versions than the lockfile and does it \
                     from scratch every run.",
                    Fix::Automatic { description: "Use `npm ci` / `bun install --frozen-lockfile` / `pnpm install --frozen-lockfile`, and `cache:` on the setup action.".into() },
                )));
            }
            if w.jobs() == 0 {
                out.push(at(Finding::new(
                    "ci/no-jobs",
                    Dimension::Verifiability,
                    Severity::Low,
                    "A workflow with no jobs",
                    "The file exists and does nothing.",
                    Fix::Manual {
                        description: "Delete it, or give it a job.".into(),
                    },
                )));
            }
        }
        // One finding per issue, not one per workflow: four files with the same omission is
        // one thing to fix, said once, with the count. The first file is the anchor.
        let mut merged: Vec<Finding> = Vec::new();
        for f in out {
            if let Some(m) = merged.iter_mut().find(|m| m.id == f.id) {
                m.detail.push_str(&format!(
                    "\nAlso in `{}`.",
                    f.path.as_deref().map(|p| p.as_str()).unwrap_or("?")
                ));
                if !m.title.contains(" workflows") {
                    m.title = format!("{} — in 2 workflows", m.title);
                } else if let Some((head, n)) = m.title.rsplit_once(" — in ") {
                    let n: usize = n.trim_end_matches(" workflows").parse().unwrap_or(2);
                    m.title = format!("{head} — in {} workflows", n + 1);
                }
            } else {
                merged.push(f);
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    fn ids(ctx: &RepoContext) -> Vec<&'static str> {
        let mut v: Vec<_> = PipelineQuality.run(ctx).into_iter().map(|f| f.id).collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn a_careless_deploy_workflow_is_read_for_what_it_is() {
        let (_d, ctx) = fixture(&[(
            ".github/workflows/deploy.yml",
            "name: deploy\non:\n  push:\n    branches: [main]\njobs:\n  deploy:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: npm install\n      - run: docker build -t ghcr.io/x/app:latest . && docker push ghcr.io/x/app:latest\n      - run: ssh deploy@prod 'docker pull ghcr.io/x/app:latest && docker compose up -d'\n",
        )]);
        let got = ids(&ctx);
        for want in [
            "ci/no-checks-run",
            "ci/no-pull-request-trigger",
            "ci/token-not-scoped",
            "ci/actions-not-pinned",
            "ci/no-concurrency-group",
            "ci/no-job-timeout",
            "ci/deploy-not-gated",
            "ci/prod-deploys-on-push",
            "ci/image-only-latest",
            "ci/deploy-by-ssh",
            "ci/install-unpinned-uncached",
        ] {
            assert!(got.contains(&want), "missing {want} in {got:?}");
        }
    }

    #[test]
    fn the_templates_pipeline_is_silent() {
        let (_d, ctx) = fixture(&[
            (
                ".github/workflows/ci.yml",
                "name: ci\non:\n  pull_request:\n  push:\n    branches: [main]\npermissions:\n  contents: read\nconcurrency:\n  group: ci-${{ github.ref }}\n  cancel-in-progress: true\njobs:\n  check:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n    steps:\n      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4\n      - run: bun install --frozen-lockfile\n      - run: make check\n",
            ),
            (
                ".github/workflows/deploy-prod.yaml",
                "name: deploy-prod\non:\n  push:\n    tags: ['v*']\npermissions:\n  contents: read\n  packages: write\nconcurrency:\n  group: prod\njobs:\n  check:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n    steps:\n      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4\n      - run: make check\n  deploy:\n    needs: check\n    environment: production\n    runs-on: ubuntu-latest\n    timeout-minutes: 30\n    steps:\n      - uses: docker/build-push-action@5cd11c3a4ced054e52742c5fd54dca954e0edd85 # v6\n        with:\n          tags: ghcr.io/x/app:${{ github.sha }}\n      - run: kubectl apply -k k8s/overlays/prod\n",
            ),
        ]);
        assert!(ids(&ctx).is_empty(), "{:?}", ids(&ctx));
    }

    /// This repository's own release workflow builds a Mac app and publishes it; a comment saying
    /// the *deployment target* comes from Package.swift is prose about Swift, not a deploy step.
    /// Matching it dropped the self-scan from 92 to 77 and failed `scan --strict` on a comment.
    #[test]
    fn a_comment_is_not_a_deploy() {
        let workflow = "name: release\non:\n  push:\n    tags: ['v*']\npermissions:\n  contents: read\nconcurrency:\n  group: release\njobs:\n  publish:\n    # The app still runs on macOS 15: the deployment target comes from `platforms:`\n    # in Package.swift, not from the SDK it was built against.\n    runs-on: macos-26\n    timeout-minutes: 90\n    steps:\n      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4\n      - run: make dmg\n";
        let (_d, ctx) = fixture(&[(".github/workflows/release.yml", workflow)]);
        assert!(
            !ids(&ctx).contains(&"ci/deploy-not-gated"),
            "a comment reclassified the workflow: {:?}",
            ids(&ctx)
        );

        // The same word on a real step still counts.
        let (_d, ctx) = fixture(&[(
            ".github/workflows/release.yml",
            &workflow.replace("      - run: make dmg\n", "      - run: ./deploy.sh\n"),
        )]);
        assert!(
            ids(&ctx).contains(&"ci/deploy-not-gated"),
            "{:?}",
            ids(&ctx)
        );
    }

    #[test]
    fn the_takeover_shape_is_critical() {
        let (_d, ctx) = fixture(&[(
            ".github/workflows/pr.yml",
            "on: pull_request_target\npermissions: write-all\njobs:\n  build:\n    runs-on: ubuntu-latest\n    timeout-minutes: 10\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}\n      - run: npm ci && npm test\n",
        )]);
        let f = PipelineQuality.run(&ctx);
        assert!(
            f.iter()
                .any(|f| f.id == "ci/pull-request-target-checks-out-fork"
                    && f.severity == Severity::Critical)
        );
    }

    #[test]
    fn no_workflows_means_nothing_to_say_here() {
        let (_d, ctx) = fixture(&[("package.json", "{}")]);
        assert!(PipelineQuality.run(&ctx).is_empty());
    }
}
