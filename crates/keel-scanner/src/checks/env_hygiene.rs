use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};
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

const CONFIG_FILES: &[&str] = &["wrangler.jsonc", "wrangler.json", "wrangler.toml"];

impl Check for SharedBindings {
    fn id(&self) -> &'static str {
        "env/shared-bindings"
    }

    fn dimension(&self) -> Dimension {
        Dimension::EnvironmentHygiene
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        let Some(path) = CONFIG_FILES.iter().find(|p| ctx.has(p)) else {
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
        };

        // TOML support is deliberately deferred: Wrangler's JSON form is the one Keel generates, and
        // reporting a confident "no problems" after failing to parse would be worse than silence.
        if path.ends_with(".toml") {
            return vec![Finding::new(
                "env/toml-config-not-analysed",
                self.dimension(),
                Severity::Info,
                "Wrangler config is TOML; environment isolation not verified",
                "Keel reads the JSON form of wrangler config. Migrating to wrangler.jsonc lets it \
                 verify that dev and prod never share a stateful binding.",
                Fix::Assisted {
                    description: "Convert wrangler.toml to wrangler.jsonc.".to_string(),
                },
            )];
        }

        let Some(raw) = ctx.read(path) else {
            return Vec::new();
        };
        let Ok(config) = serde_json::from_str::<Value>(&strip_jsonc_comments(&raw)) else {
            return vec![Finding::new(
                "env/unparseable-wrangler-config",
                self.dimension(),
                Severity::Medium,
                "Wrangler config could not be parsed",
                "Environment isolation could not be verified because the config did not parse as \
                 JSON with comments.",
                Fix::Assisted {
                    description: "Fix the syntax error in the Wrangler config.".to_string(),
                },
            )
            .at(*path)];
        };

        let Some(envs) = config.get("env").and_then(Value::as_object) else {
            return vec![
                Finding::new(
                    "env/single-environment",
                    self.dimension(),
                    Severity::High,
                    "Only one environment is defined",
                    "A project with a single environment teaches you to test in production. Keel \
                 provisions dev and prod separately, each with its own D1, KV and R2 resources.",
                    Fix::Assisted {
                        description: "Add [env.dev] and [env.prod] with isolated bindings."
                            .to_string(),
                    },
                )
                .at(*path),
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
                    format!("`{array}` resource `{id}` is shared by {}", envs.join(" and ")),
                    "These environments write to the same store. An action taken in a non-production \
                     environment can destroy production data, and no approval gate downstream can \
                     recover from that.",
                    Fix::Assisted {
                        description: format!(
                            "Provision a separate {array} resource per environment and point each \
                             env block at its own id."
                        ),
                    },
                )
                .at(*path)
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

/// Strip `//` and `/* */` comments so a `.jsonc` file can go through `serde_json`.
///
/// String literals are tracked so a `//` inside a URL is not mistaken for a comment.
fn strip_jsonc_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }

    out
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

    #[test]
    fn a_single_environment_is_itself_a_finding() {
        let (_dir, ctx) = fixture(&[("wrangler.jsonc", r#"{"name":"app"}"#)]);
        let findings = SharedBindings.run(&ctx);
        assert_eq!(findings[0].id, "env/single-environment");
    }

    #[test]
    fn strips_comments_but_not_urls() {
        let stripped =
            strip_jsonc_comments(r#"{"u":"https://x.dev","a":1 /* note */, "b":2} // end"#);
        let value: Value = serde_json::from_str(&stripped).expect("parses");
        assert_eq!(value["u"], "https://x.dev");
        assert_eq!(value["b"], 2);
    }
}
