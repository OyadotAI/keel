//! Shared Wrangler-config plumbing.
//!
//! Two checks read the same files for different reasons — `env_hygiene` for resource isolation,
//! `workers_compat` for whether the code can run on the runtime at all. Locating and parsing the
//! config is the same work either way, and duplicating it would let the two drift apart on which
//! files count as a Wrangler config.

use crate::RepoContext;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

pub const CONFIG_FILES: &[&str] = &["wrangler.jsonc", "wrangler.json", "wrangler.toml"];

/// Every Wrangler config in the tree, not just the one at the root.
///
/// Monorepos keep them under `apps/*` or `packages/*` — the layout of every real Cloudflare project
/// of any size. Looking only at the root reported "no Wrangler configuration" for a repo that had
/// two. `.example` templates are skipped: they carry placeholder ids by design.
pub fn find_configs(ctx: &RepoContext) -> Vec<Utf8PathBuf> {
    ctx.files()
        .filter(|p| p.file_name().is_some_and(|n| CONFIG_FILES.contains(&n)))
        .map(ToOwned::to_owned)
        .collect()
}

/// True when this config is in Wrangler's TOML form.
///
/// TOML support is deliberately deferred: the JSON form is the one Keel generates, and reporting a
/// confident verdict after failing to parse would be worse than silence. `env_hygiene` raises a
/// single Info finding saying so; every other check stays quiet rather than repeating it.
pub fn is_toml(path: &Utf8Path) -> bool {
    path.as_str().ends_with(".toml")
}

/// Read and parse a JSON/JSONC Wrangler config.
///
/// `None` covers both "unreadable" and "does not parse" — callers that want to report a syntax
/// error distinguish the two themselves.
pub fn parse(ctx: &RepoContext, path: &Utf8Path) -> Option<Value> {
    let raw = ctx.read(path.as_str())?;
    serde_json::from_str(&strip_jsonc_comments(&raw)).ok()
}

/// Strip `//` and `/* */` comments so a `.jsonc` file can go through `serde_json`.
///
/// String literals are tracked so a `//` inside a URL is not mistaken for a comment.
pub fn strip_jsonc_comments(src: &str) -> String {
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

    #[test]
    fn strips_comments_but_not_urls() {
        let stripped =
            strip_jsonc_comments(r#"{"u":"https://x.dev","a":1 /* note */, "b":2} // end"#);
        let value: Value = serde_json::from_str(&stripped).expect("parses");
        assert_eq!(value["u"], "https://x.dev");
        assert_eq!(value["b"], 2);
    }
}
