use super::wrangler;
use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
use camino::Utf8Path;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Environments that share a stateful resource.
///
/// The most common way a "safe" action in dev destroys real data is a dev binding that resolves to
/// a production resource id. Keel never generates that, and refuses to leave it in place when it
/// finds it in an existing repo.
pub struct SharedBindings;

/// Wrangler resource arrays and the field inside each entry that identifies the underlying store.
/// Only stateful resources are listed — sharing a Workers AI binding across environments is fine.
const STATEFUL: &[(&str, &str)] = &[
    ("d1_databases", "database_id"),
    ("kv_namespaces", "id"),
    ("r2_buckets", "bucket_name"),
    ("queues", "queue"),
];

impl Check for SharedBindings {
    fn id(&self) -> &'static str {
        "env/shared-bindings"
    }

    fn dimension(&self) -> Dimension {
        Dimension::EnvironmentHygiene
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let configs = wrangler::find_configs(ctx);

        if configs.is_empty() {
            // Not every repository is a Cloudflare service. Telling a Rust CLI or a docs site that
            // it is missing a Wrangler config is noise, and noise is how a report loses its
            // audience. Only judge repos that could plausibly deploy as a Worker.
            if !ctx.has("package.json") {
                return Vec::new();
            }
            // A repository that already runs somewhere — an image, a cluster, a platform —
            // is not missing a Wrangler config; it made a different choice. The hosting
            // check speaks to that choice; this one would only be noise beside it.
            if !crate::detect(ctx).hosting.is_empty() {
                return Vec::new();
            }
            return vec![Finding::new(
                "env/no-wrangler-config",
                self.dimension(),
                Severity::Medium,
                "No Wrangler configuration found",
                "Keel provisions dev and prod as separate environments with isolated resources. \
                 Without a Wrangler config there is nothing describing where this service runs.",
                Fix::Assisted {
                    description: "Generate a wrangler.jsonc with [env.dev] and [env.prod], each \
                                  bound to its own D1, KV and R2 resources."
                        .to_string(),
                },
            )];
        }

        configs.iter().flat_map(|p| self.analyse(ctx, p)).collect()
    }
}

impl SharedBindings {
    /// Audit one Wrangler config for environment isolation.
    fn analyse(&self, ctx: &RepoContext, path: &Utf8Path) -> Vec<Finding> {
        // TOML support is deliberately deferred: Wrangler's JSON form is the one Keel generates,
        // and reporting a confident "no problems" after failing to parse would be worse than
        // silence. This is the one place that says so — the other Wrangler checks stay quiet.
        if wrangler::is_toml(path) {
            return vec![
                Finding::new(
                    "env/toml-config-not-analysed",
                    self.dimension(),
                    Severity::Info,
                    format!("`{path}` is TOML; environment isolation not verified"),
                    "Keel reads the JSON form of Wrangler config. Migrating to wrangler.jsonc lets \
                     it verify that dev and prod never share a stateful binding.",
                    Fix::Assisted {
                        description: "Convert wrangler.toml to wrangler.jsonc.".to_string(),
                    },
                )
                .at(path.to_owned()),
            ];
        }

        let Some(raw) = ctx.read(path.as_str()) else {
            return Vec::new();
        };
        let Ok(config) = serde_json::from_str::<Value>(&wrangler::strip_jsonc_comments(&raw))
        else {
            return vec![
                Finding::new(
                    "env/unparseable-wrangler-config",
                    self.dimension(),
                    Severity::Medium,
                    format!("`{path}` could not be parsed"),
                    "Environment isolation could not be verified because the config did not parse \
                     as JSON with comments.",
                    Fix::Assisted {
                        description: "Fix the syntax error in the Wrangler config.".to_string(),
                    },
                )
                .at(path.to_owned()),
            ];
        };

        let Some(envs) = config.get("env").and_then(Value::as_object) else {
            return vec![
                Finding::new(
                    "env/single-environment",
                    self.dimension(),
                    Severity::High,
                    format!("`{path}` defines only one environment"),
                    "A project with a single environment teaches you to test in production. Keel \
                     provisions dev and prod separately, each with its own D1, KV and R2 resources.",
                    Fix::Assisted {
                        description: "Add [env.dev] and [env.prod] with isolated bindings."
                            .to_string(),
                    },
                )
                .at(path.to_owned()),
            ];
        };

        // resource id -> the environments referencing it
        let mut owners: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for (env_name, env_config) in envs {
            for (array, id_field) in STATEFUL {
                for id in resource_ids(env_config, array, id_field) {
                    owners
                        .entry(((*array).to_string(), id))
                        .or_default()
                        .insert(env_name.clone());
                }
            }
        }

        owners
            .into_iter()
            .filter(|(_, envs)| envs.len() > 1)
            .map(|((array, id), envs)| {
                let envs: Vec<_> = envs.into_iter().collect();
                Finding::new(
                    self.id(),
                    self.dimension(),
                    Severity::Critical,
                    format!(
                        "`{array}` resource `{id}` is shared by {}",
                        envs.join(" and ")
                    ),
                    "These environments write to the same store. An action taken in a \
                     non-production environment can destroy production data, and no approval gate \
                     downstream can recover from that.",
                    Fix::Assisted {
                        description: format!(
                            "Provision a separate {array} resource per environment and point each \
                             env block at its own id."
                        ),
                    },
                )
                .at(path.to_owned())
            })
            .collect()
    }
}

/// Pull the identifying value out of each entry in a Wrangler resource array.
fn resource_ids(env_config: &Value, array: &str, id_field: &str) -> Vec<String> {
    // `queues` nests its bindings under `producers`/`consumers`; everything else is a flat array.
    let node = match env_config.get(array) {
        Some(Value::Object(map)) => map.get("producers").cloned().unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    };

    node.as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e.get(id_field).and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    const SHARED: &str = r#"{
      // two environments, one database
      "name": "app",
      "env": {
        "dev":  { "d1_databases": [{ "binding": "DB", "database_id": "same-id" }] },
        "prod": { "d1_databases": [{ "binding": "DB", "database_id": "same-id" }] }
      }
    }"#;

    const ISOLATED: &str = r#"{
      "name": "app",
      "env": {
        "dev":  { "d1_databases": [{ "binding": "DB", "database_id": "dev-id" }] },
        "prod": { "d1_databases": [{ "binding": "DB", "database_id": "prod-id" }] }
      }
    }"#;

    #[test]
    fn flags_a_database_shared_across_environments() {
        let (_dir, ctx) = fixture(&[("wrangler.jsonc", SHARED)]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].title.contains("dev and prod"));
    }

    #[test]
    fn isolated_environments_are_clean() {
        let (_dir, ctx) = fixture(&[("wrangler.jsonc", ISOLATED)]);
        assert!(SharedBindings.run(&ctx).is_empty());
    }

    /// Monorepos keep Wrangler configs under `apps/*`. Looking only at the root reported "no
    /// Wrangler configuration" for a repo that had two.
    #[test]
    fn finds_configs_nested_in_a_monorepo() {
        let (_dir, ctx) = fixture(&[("package.json", "{}"), ("apps/api/wrangler.jsonc", SHARED)]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(
            findings.len(),
            1,
            "nested config must be analysed, not missed"
        );
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(
            findings[0].path.as_deref().map(|p| p.as_str()),
            Some("apps/api/wrangler.jsonc")
        );
    }

    #[test]
    fn ignores_example_templates() {
        let (_dir, ctx) = fixture(&[
            ("package.json", "{}"),
            ("apps/api/wrangler.jsonc.example", SHARED),
        ]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(
            findings[0].id, "env/no-wrangler-config",
            "templates carry placeholder ids"
        );
    }

    #[test]
    fn a_repo_that_is_not_a_worker_is_left_alone() {
        let (_dir, ctx) = fixture(&[("Cargo.toml", "[package]\nname = \"cli\"")]);
        assert!(
            SharedBindings.run(&ctx).is_empty(),
            "a Rust CLI is not missing a Wrangler config"
        );
    }

    #[test]
    fn a_node_project_without_wrangler_config_is_flagged() {
        let (_dir, ctx) = fixture(&[("package.json", "{}")]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(findings[0].id, "env/no-wrangler-config");
    }

    #[test]
    fn a_single_environment_is_itself_a_finding() {
        let (_dir, ctx) = fixture(&[("wrangler.jsonc", r#"{"name":"app"}"#)]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(findings[0].id, "env/single-environment");
    }
}
