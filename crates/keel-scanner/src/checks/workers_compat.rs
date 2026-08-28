use super::wrangler;
use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Whether the code can run on the Workers runtime at all.
///
/// `env_hygiene` asks whether the environments are separated; this asks the prior question. Workers
/// is not Node: there is no filesystem, no process spawning, no thread pool, and Node's own module
/// namespace only exists behind a compatibility flag that itself has a minimum date. Every failure
/// here surfaces at deploy time or — worse — on the first request in production.
pub struct WorkersCompat;

/// The compatibility date at which `nodejs_compat` began providing the full Node module namespace.
///
/// Before it, the flag enabled a much smaller v1 shim, so a config can carry the flag and still not
/// have the builtins its code imports. Encoded as a constant rather than read from a clock: a scan
/// of an unchanged repo must produce a byte-identical report tomorrow.
const NODEJS_COMPAT_MIN_DATE: &str = "2024-09-23";

/// Node builtins the Workers runtime does not provide even with `nodejs_compat`, and why.
///
/// This is the load-bearing list. Import one of these and the Worker deploys cleanly, then throws on
/// the first request that reaches the code path — the most expensive shape of failure Keel exists to
/// prevent. Kept deliberately short: only modules with no Workers implementation at all, so a
/// finding here is never a judgement call.
const UNSUPPORTED: &[(&str, &str)] = &[
    ("node:fs", "the Workers runtime has no filesystem"),
    ("node:fs/promises", "the Workers runtime has no filesystem"),
    ("node:child_process", "Workers cannot spawn processes"),
    ("node:cluster", "Workers cannot fork worker processes"),
    ("node:worker_threads", "Workers has no thread pool"),
    ("node:dgram", "Workers exposes no UDP socket API"),
    ("node:v8", "the V8 introspection API is not exposed"),
    ("node:vm", "Workers disallows dynamic code evaluation"),
];

const SOURCE_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"];

/// Directories whose contents never reach the deployed bundle.
const NON_BUNDLE_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".wrangler",
    "tests",
    "test",
    "__tests__",
    "__mocks__",
    "scripts",
];

/// Filename fragments that mark a file as tooling or types rather than shipped code.
///
/// A test importing `node:fs` runs under Vitest on a real machine, not on Workers. Flagging it is
/// the kind of false positive that teaches people to stop reading the report.
const NON_BUNDLE_INFIXES: &[&str] = &[".test.", ".spec.", ".config.", ".d."];

impl Check for WorkersCompat {
    fn id(&self) -> &'static str {
        "workers/unsupported-node-builtin"
    }

    fn dimension(&self) -> Dimension {
        Dimension::WorkersCompat
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        wrangler::find_configs(ctx)
            .into_iter()
            .filter(|p| !wrangler::is_toml(p))
            .flat_map(|p| self.analyse(ctx, &p))
            .collect()
    }
}

impl WorkersCompat {
    fn analyse(&self, ctx: &RepoContext, path: &Utf8Path) -> Vec<Finding> {
        // A config that does not parse is already reported by `env_hygiene`. Repeating it here
        // would charge the repo twice for one syntax error.
        let Some(config) = wrangler::parse(ctx, path) else {
            return Vec::new();
        };

        let envs = environments(&config);
        let mut findings = self.compatibility_date_findings(path, &config, &envs);
        findings.extend(self.node_findings(ctx, path, &config, &envs));
        findings
    }

    fn compatibility_date_findings(
        &self,
        path: &Utf8Path,
        config: &Value,
        envs: &[Environment],
    ) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Only a config with `main` describes Worker code. An assets-only project deploys without a
        // compatibility date, and telling it otherwise is noise.
        if config.get("main").and_then(Value::as_str).is_some() {
            let undated: Vec<_> = envs
                .iter()
                .filter(|e| e.date.is_none())
                .map(|e| e.name.clone())
                .collect();

            if !undated.is_empty() {
                findings.push(
                    Finding::new(
                        "workers/missing-compatibility-date",
                        self.dimension(),
                        Severity::High,
                        format!(
                            "`{path}` sets no `compatibility_date`{}",
                            scope_suffix(&undated, envs)
                        ),
                        "Wrangler refuses to deploy a Worker without one. The date pins which \
                         runtime semantics your code gets, so leaving it unset is not a default — \
                         it is a deploy that never happens.",
                        // Nothing can be running yet, so there is no behaviour to preserve and
                        // writing the date is safe without review.
                        Fix::Automatic {
                            description: format!(
                                "Add `\"compatibility_date\": \"{NODEJS_COMPAT_MIN_DATE}\"` or \
                                 later to the Wrangler config."
                            ),
                        },
                    )
                    .at(path.to_owned()),
                );
            }
        }

        let too_early: Vec<_> = envs
            .iter()
            .filter(|e| {
                e.has_flag("nodejs_compat")
                    && e.date.as_deref().is_some_and(|d| d < NODEJS_COMPAT_MIN_DATE)
            })
            .map(|e| e.name.clone())
            .collect();

        if !too_early.is_empty() {
            findings.push(
                Finding::new(
                    "workers/nodejs-compat-date-too-early",
                    self.dimension(),
                    Severity::Medium,
                    format!(
                        "`{path}` enables `nodejs_compat` before {NODEJS_COMPAT_MIN_DATE}{}",
                        scope_suffix(&too_early, envs)
                    ),
                    format!(
                        "Below a compatibility date of {NODEJS_COMPAT_MIN_DATE} the flag enables \
                         the older, much smaller shim. The config looks like it has Node support \
                         and the imports still fail at runtime, which is the hardest version of \
                         this bug to diagnose."
                    ),
                    Fix::Assisted {
                        description: format!(
                            "Raise `compatibility_date` to {NODEJS_COMPAT_MIN_DATE} or later and \
                             re-run the test suite — the date also governs unrelated runtime \
                             semantics."
                        ),
                    },
                )
                .at(path.to_owned()),
            );
        }

        findings
    }

    fn node_findings(
        &self,
        ctx: &RepoContext,
        path: &Utf8Path,
        config: &Value,
        envs: &[Environment],
    ) -> Vec<Finding> {
        // specifier -> the files importing it, first one wins for the reported path
        let mut imports: BTreeMap<String, Utf8PathBuf> = BTreeMap::new();
        for source in worker_sources(ctx, path, config) {
            let Some(contents) = ctx.read(source.as_str()) else {
                continue;
            };
            for specifier in node_specifiers(&contents) {
                imports.entry(specifier).or_insert_with(|| source.clone());
            }
        }

        if imports.is_empty() {
            return Vec::new();
        }

        let mut findings = Vec::new();

        for (specifier, reason) in UNSUPPORTED {
            let Some(source) = imports.get(*specifier) else {
                continue;
            };
            findings.push(
                Finding::new(
                    "workers/unsupported-node-builtin",
                    self.dimension(),
                    Severity::High,
                    format!("`{source}` imports `{specifier}`, which Workers does not provide"),
                    format!(
                        "{reason}, with or without `nodejs_compat`. This deploys cleanly and then \
                         throws on the first request that reaches the import."
                    ),
                    // Which way out to take — a Cloudflare primitive, or moving the workload into
                    // a Container — is an architectural decision Keel is not entitled to make.
                    Fix::Manual {
                        description: format!(
                            "Replace `{specifier}` with the Cloudflare primitive that covers it \
                             (R2 or D1 for storage, Queues for background work), or move this \
                             workload into a Container — remembering that Container disk is \
                             ephemeral and resets on every restart."
                        ),
                    },
                )
                .at(source.clone()),
            );
        }

        // The flag is what makes `node:*` resolve at all, so report it once for the whole config
        // rather than once per import.
        let unflagged: Vec<_> = envs
            .iter()
            .filter(|e| !e.has_flag("nodejs_compat"))
            .map(|e| e.name.clone())
            .collect();

        if !unflagged.is_empty() {
            let example = imports.keys().next().expect("imports is non-empty");
            findings.push(
                Finding::new(
                    "workers/node-builtins-without-compat-flag",
                    self.dimension(),
                    Severity::High,
                    format!(
                        "`{path}` imports `{example}` without `nodejs_compat`{}",
                        scope_suffix(&unflagged, envs)
                    ),
                    "Workers only exposes Node's module namespace behind the `nodejs_compat` \
                     compatibility flag. Without it the import fails when the bundle is built or \
                     the moment it is evaluated.",
                    Fix::Automatic {
                        description: format!(
                            "Add `\"nodejs_compat\"` to `compatibility_flags`, with a \
                             `compatibility_date` of {NODEJS_COMPAT_MIN_DATE} or later."
                        ),
                    },
                )
                .at(path.to_owned()),
            );
        }

        findings
    }
}

/// One environment's effective compatibility settings.
struct Environment {
    name: String,
    date: Option<String>,
    flags: BTreeSet<String>,
}

impl Environment {
    fn has_flag(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }
}

/// The environments to judge, with Wrangler's inheritance already applied.
///
/// A config with no `env` block is judged as a single unnamed environment, so the same code path
/// covers both shapes.
fn environments(config: &Value) -> Vec<Environment> {
    let top_date = config
        .get("compatibility_date")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let top_flags = flags(config);

    match config.get("env").and_then(Value::as_object) {
        Some(envs) if !envs.is_empty() => envs
            .iter()
            .map(|(name, env)| Environment {
                name: name.clone(),
                date: env
                    .get("compatibility_date")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| top_date.clone()),
                // Wrangler inherits `compatibility_date` into an environment, but an environment
                // that declares `compatibility_flags` *replaces* the top-level list wholesale
                // rather than merging into it. Merging here would report a flag the deploy will
                // not actually have.
                flags: if env.get("compatibility_flags").is_some() {
                    flags(env)
                } else {
                    top_flags.clone()
                },
            })
            .collect(),
        _ => vec![Environment {
            name: String::new(),
            date: top_date,
            flags: top_flags,
        }],
    }
}

fn flags(node: &Value) -> BTreeSet<String> {
    node.get("compatibility_flags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Name the affected environments, or say nothing when the config has none to name.
fn scope_suffix(affected: &[String], all: &[Environment]) -> String {
    if all.len() == 1 && all[0].name.is_empty() {
        return String::new();
    }
    let names: Vec<_> = affected.iter().map(|n| format!("`{n}`")).collect();
    match names.split_last() {
        Some((last, [])) => format!(" in {last}"),
        Some((last, rest)) => format!(" in {} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The source files that plausibly end up in this Worker's bundle.
///
/// Scoped by the config's `main` entrypoint rather than scanning the whole repository. In a monorepo
/// the tree also holds a Node server, build tooling and tests, all of which legitimately use Node
/// builtins; judging them against the Workers runtime would be wrong, not merely noisy.
fn worker_sources(ctx: &RepoContext, config_path: &Utf8Path, config: &Value) -> Vec<Utf8PathBuf> {
    let dir = config_path.parent().unwrap_or_else(|| Utf8Path::new(""));
    let root = match config.get("main").and_then(Value::as_str) {
        Some(main) => dir
            .join(main)
            .parent()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| dir.to_owned()),
        None => dir.join("src"),
    };

    ctx.files()
        .filter(|p| p.starts_with(&root) && is_bundled_source(p))
        .map(ToOwned::to_owned)
        .collect()
}

fn is_bundled_source(path: &Utf8Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    SOURCE_EXTENSIONS.iter().any(|e| name.ends_with(e))
        && !NON_BUNDLE_INFIXES.iter().any(|i| name.contains(i))
        && !path
            .components()
            .any(|c| NON_BUNDLE_DIRS.contains(&c.as_str()))
}

/// Every `node:`-prefixed module specifier the file references.
///
/// Matching the quoted specifier rather than the import syntax around it covers `import`,
/// `require` and dynamic `import()` in one pass. Bare specifiers (`"fs"`) are deliberately not
/// matched: too many of them are ordinary local or package names to tell apart without resolving
/// the module graph, and a false positive here costs more than a miss.
fn node_specifiers(contents: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();

    for (quote, needle) in [('"', "\"node:"), ('\'', "'node:")] {
        let mut rest = contents;
        while let Some(start) = rest.find(needle) {
            let after = &rest[start + needle.len()..];
            let Some(end) = after.find(quote) else { break };
            let specifier = &after[..end];
            if !specifier.is_empty()
                && specifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
            {
                found.insert(format!("node:{specifier}"));
            }
            rest = &after[end..];
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    const MAIN: &str = r#"{
      "name": "app",
      "main": "src/index.ts",
      "compatibility_date": "2025-01-01"
    }"#;

    fn ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.id).collect()
    }

    #[test]
    fn flags_a_builtin_workers_will_never_provide() {
        let (_dir, ctx) = fixture(&[
            ("wrangler.jsonc", MAIN),
            ("src/index.ts", "import fs from 'node:fs'\nexport default {}"),
        ]);
        let findings = WorkersCompat.run(&ctx);
        assert!(ids(&findings).contains(&"workers/unsupported-node-builtin"));
        let unsupported = findings
            .iter()
            .find(|f| f.id == "workers/unsupported-node-builtin")
            .expect("finding");
        assert_eq!(unsupported.severity, Severity::High);
        assert_eq!(
            unsupported.path.as_deref().map(|p| p.as_str()),
            Some("src/index.ts"),
            "the finding must point at the importing file, not the config"
        );
    }

    #[test]
    fn flags_node_imports_without_the_compat_flag() {
        let (_dir, ctx) = fixture(&[
            ("wrangler.jsonc", MAIN),
            (
                "src/index.ts",
                r#"import { createHash } from "node:crypto""#,
            ),
        ]);
        assert_eq!(
            ids(&WorkersCompat.run(&ctx)),
            vec!["workers/node-builtins-without-compat-flag"]
        );
    }

    #[test]
    fn a_supported_builtin_behind_the_flag_is_clean() {
        let (_dir, ctx) = fixture(&[
            (
                "wrangler.jsonc",
                r#"{
                  "name": "app",
                  "main": "src/index.ts",
                  "compatibility_date": "2025-01-01",
                  "compatibility_flags": ["nodejs_compat"]
                }"#,
            ),
            (
                "src/index.ts",
                r#"import { Buffer } from "node:buffer""#,
            ),
        ]);
        assert!(WorkersCompat.run(&ctx).is_empty());
    }

    /// Wrangler replaces the top-level flag list when an environment declares its own, so a
    /// per-environment `compatibility_flags` that omits `nodejs_compat` really does deploy without
    /// it. Merging the two lists would hide the bug.
    #[test]
    fn an_environment_overriding_flags_loses_the_inherited_one() {
        let (_dir, ctx) = fixture(&[
            (
                "wrangler.jsonc",
                r#"{
                  "name": "app",
                  "main": "src/index.ts",
                  "compatibility_date": "2025-01-01",
                  "compatibility_flags": ["nodejs_compat"],
                  "env": {
                    "dev":  {},
                    "prod": { "compatibility_flags": ["global_fetch_strictly_public"] }
                  }
                }"#,
            ),
            ("src/index.ts", r#"import "node:crypto""#),
        ]);
        let findings = WorkersCompat.run(&ctx);
        assert_eq!(
            ids(&findings),
            vec!["workers/node-builtins-without-compat-flag"]
        );
        assert!(
            findings[0].title.contains("`prod`") && !findings[0].title.contains("`dev`"),
            "only the overriding environment lost the flag: {}",
            findings[0].title
        );
    }

    #[test]
    fn a_worker_without_a_compatibility_date_cannot_deploy() {
        let (_dir, ctx) = fixture(&[(
            "wrangler.jsonc",
            r#"{ "name": "app", "main": "src/index.ts" }"#,
        )]);
        let findings = WorkersCompat.run(&ctx);
        assert_eq!(ids(&findings), vec!["workers/missing-compatibility-date"]);
        assert!(findings[0].fix.is_automatic());
    }

    /// An assets-only project has no Worker code and deploys without a date.
    #[test]
    fn a_config_without_main_is_not_asked_for_a_date() {
        let (_dir, ctx) = fixture(&[(
            "wrangler.jsonc",
            r#"{ "name": "site", "assets": { "directory": "./public" } }"#,
        )]);
        assert!(WorkersCompat.run(&ctx).is_empty());
    }

    #[test]
    fn the_flag_below_its_minimum_date_is_reported() {
        let (_dir, ctx) = fixture(&[(
            "wrangler.jsonc",
            r#"{
              "name": "app",
              "main": "src/index.ts",
              "compatibility_date": "2024-01-01",
              "compatibility_flags": ["nodejs_compat"]
            }"#,
        )]);
        assert_eq!(
            ids(&WorkersCompat.run(&ctx)),
            vec!["workers/nodejs-compat-date-too-early"]
        );
    }

    /// Tests and build tooling run on a real machine, not on Workers.
    #[test]
    fn node_builtins_outside_the_bundle_are_ignored() {
        let (_dir, ctx) = fixture(&[
            ("wrangler.jsonc", MAIN),
            ("src/index.ts", "export default {}"),
            ("src/index.test.ts", "import fs from 'node:fs'"),
            ("src/__tests__/helper.ts", "import fs from 'node:fs'"),
            ("scripts/seed.ts", "import fs from 'node:fs'"),
            ("vitest.config.ts", "import path from 'node:path'"),
        ]);
        assert!(WorkersCompat.run(&ctx).is_empty());
    }

    /// A monorepo's Node server is not judged against the Workers runtime.
    #[test]
    fn only_sources_under_the_entrypoint_are_scanned() {
        let (_dir, ctx) = fixture(&[
            ("apps/api/wrangler.jsonc", MAIN),
            ("apps/api/src/index.ts", "export default {}"),
            ("apps/server/main.ts", "import fs from 'node:fs'"),
        ]);
        assert!(WorkersCompat.run(&ctx).is_empty());
    }

    #[test]
    fn toml_configs_are_left_to_the_environment_check() {
        let (_dir, ctx) = fixture(&[
            ("wrangler.toml", "name = \"app\"\nmain = \"src/index.ts\""),
            ("src/index.ts", "import fs from 'node:fs'"),
        ]);
        assert!(WorkersCompat.run(&ctx).is_empty());
    }

    #[test]
    fn finds_specifiers_in_every_import_form() {
        let found = node_specifiers(
            r#"
            import fs from "node:fs";
            const p = require('node:path');
            const c = await import("node:crypto");
            const url = "https://example.com/node:not-an-import";
            "#,
        );
        assert!(found.contains("node:fs"));
        assert!(found.contains("node:path"));
        assert!(found.contains("node:crypto"));
        assert_eq!(found.len(), 3, "the URL fragment is not a specifier: {found:?}");
    }
}
