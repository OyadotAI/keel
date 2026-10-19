//! The person's own Claude Code deny rules, applied to Antigravity lanes.
//!
//! For Claude a `defer` from `approve::ask` still reaches Claude Code's permission check, which
//! reads `permissions.deny`. For agy nothing reads anything after Keel's hook — agy runs with its
//! own permissions off — so a trusted agy lane ran exactly the commands a Claude lane refused.
//! Read from `~/.claude/settings.json` only: a repository's settings are quarantined, and the
//! person's own file is where their deny rules live.

use camino::Utf8Path;
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Clear,
    /// A rule refuses this call, named as written.
    Denied(String),
    /// A deny rule might apply and Keel cannot see far enough into the call to say: a command
    /// with syntax a heuristic cannot read past, or a URL with no host.
    Unsure,
}

/// `permissions.deny` from the person's Claude Code settings. Absent is none; unreadable is none
/// too, said once on stderr — a broken settings file must not refuse every command.
pub fn read(claude_home: &Utf8Path) -> Vec<String> {
    let path = claude_home.join("settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let rules = serde_json::from_str::<Value>(&text).ok().and_then(|v| {
        v["permissions"]["deny"].as_array().map(|a| {
            a.iter()
                .map(|r| r.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
    });
    match rules {
        Some(Some(rules)) => rules,
        None if serde_json::from_str::<Value>(&text)
            .is_ok_and(|v| v["permissions"]["deny"].is_null()) =>
        {
            Vec::new()
        }
        _ => {
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "keel: {path} could not be read as settings, so its deny rules are not \
                     applied to Antigravity lanes"
                );
            }
            Vec::new()
        }
    }
}

/// Whether any of `rules` refuses `tool` with `input`. Matching errs toward refusing.
pub fn check(rules: &[String], tool: &str, input: &Value) -> Verdict {
    let mut unsure = false;
    for rule in rules {
        let (name, arg) = match rule.split_once('(') {
            Some((n, a)) => (n, a.strip_suffix(')')),
            None => (rule.as_str(), None),
        };
        let hit = match (name, arg) {
            (n, None) if n.starts_with("mcp__") => mcp(n, tool),
            (n, None) => n == tool,
            ("Bash", Some(pattern)) if tool == "Bash" => {
                let command = input["command"].as_str().unwrap_or_default().trim();
                match crate::approve::plain_shell(command)
                    .then(|| bash(pattern, command))
                    .flatten()
                {
                    Some(hit) => hit,
                    None => {
                        unsure = true;
                        continue;
                    }
                }
            }
            ("WebFetch", Some(spec)) if tool == "WebFetch" => {
                let Some(domain) = spec.strip_prefix("domain:") else {
                    continue;
                };
                match host(input["url"].as_str().unwrap_or_default()) {
                    Some(h) => h == domain || h.ends_with(&format!(".{domain}")),
                    None => {
                        unsure = true;
                        continue;
                    }
                }
            }
            // Path rules (`Read(…)`, `Edit(src/**)`) are not applied: said in the trust dialog.
            _ => false,
        };
        if hit {
            return Verdict::Denied(rule.clone());
        }
    }
    if unsure {
        Verdict::Unsure
    } else {
        Verdict::Clear
    }
}

/// `mcp__s` and `mcp__s__*` refuse the whole server; `mcp__s__t` one tool.
fn mcp(rule: &str, tool: &str) -> bool {
    if let Some(server) = rule.strip_suffix("__*") {
        return tool.starts_with(&format!("{server}__"));
    }
    if rule.matches("__").count() == 1 {
        return tool.starts_with(&format!("{rule}__"));
    }
    tool == rule
}

/// A `Bash(…)` pattern against a plain command and its canonical form (wrappers stepped over,
/// the program by its basename, so `sudo rm` and `/bin/rm` are `rm`). `X:*` is Claude Code's
/// older spelling of `X *`. A trailing ` *` is the program alone or with anything after it; any
/// other `*` is any run of characters.
fn bash(pattern: &str, command: &str) -> Option<bool> {
    // macOS and Windows open `RM` as `rm`; refusing a capitalised `rm` on Linux too errs safe.
    let (pattern, command) = (pattern.to_ascii_lowercase(), command.to_ascii_lowercase());
    let command = command.as_str();
    let pattern = pattern
        .strip_suffix(":*")
        .map(|p| format!("{p} *"))
        .unwrap_or_else(|| pattern.to_string());
    // Callers pass only `plain_shell` commands, which already rule these out; checked again so
    // the matcher fails closed on its own: an assignment or other whitespace hides the program.
    if command.contains(|c: char| c.is_whitespace() && c != ' ') || command.contains('=') {
        return None;
    }
    let words: Vec<&str> = command.split(' ').filter(|w| !w.is_empty()).collect();
    // Wrappers by their basename (`/usr/bin/sudo` is `sudo`). Ones that take a positional
    // argument before the program (`timeout 60 rm`) cannot be stepped over, so they are a card.
    const POSITIONAL: &[&str] = &["timeout", "stdbuf", "ionice", "chrt", "taskset", "watch"];
    let base = |w: &str| w.rsplit('/').next().unwrap_or(w).to_string();
    if words.iter().any(|w| POSITIONAL.contains(&base(w).as_str())) {
        return None;
    }
    let start = words
        .iter()
        .position(|w| !crate::approve::WRAPPERS.contains(&base(w).as_str()))
        .unwrap_or(0);
    // A wrapper's own flags (`sudo -u root`, `nice -n 5`) take arguments this cannot tell from
    // the program, so a flag after a wrapper is a card, not a guess.
    if start > 0 && words.get(start).is_some_and(|w| w.starts_with('-')) {
        return None;
    }
    let mut canonical: Vec<String> = words[start..].iter().map(|w| w.to_string()).collect();
    if let Some(first) = canonical.first_mut() {
        *first = first.rsplit('/').next().unwrap_or(first).to_string();
    }
    // `git -C . push` is `git push` with an option in between: a multi-word rule cannot see past
    // it, so it is a card.
    let pwords: Vec<&str> = pattern.trim_end_matches(" *").split(' ').collect();
    if pwords.len() > 1
        && canonical.first().map(String::as_str) == Some(pwords[0])
        && canonical.get(1).is_some_and(|w| w.starts_with('-'))
    {
        return None;
    }
    Some(
        [command.to_string(), canonical.join(" ")]
            .iter()
            .any(|c| glob(&pattern, c)),
    )
}

fn glob(pattern: &str, text: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(" *")
        && !prefix.contains('*')
    {
        return text == prefix || text.starts_with(&format!("{prefix} "));
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return text == pattern;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else {
                return false;
            };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

/// A URL's host, lowercased: past the scheme, before the path, without userinfo or port.
fn host(url: &str) -> Option<String> {
    // A backslash or an escape in the authority is read differently by different parsers, so it
    // is no host at all here — and no host is a card.
    if url.contains('\\') {
        return None;
    }
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = authority
        .split(':')
        .next()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    (!host.is_empty() && !host.contains('%')).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(r: &[&str]) -> Vec<String> {
        r.iter().map(|s| s.to_string()).collect()
    }

    fn sh(rules: &[String], command: &str) -> Verdict {
        check(rules, "Bash", &json!({ "command": command }))
    }

    #[test]
    fn a_bash_rule_refuses_the_program_however_it_is_spelled() {
        let r = rules(&["Bash(rm *)"]);
        for c in ["rm x", "rm", "/bin/rm -rf build", "sudo rm x"] {
            assert_eq!(sh(&r, c), Verdict::Denied("Bash(rm *)".into()), "{c}");
        }
        assert_eq!(sh(&r, "rmdir x"), Verdict::Clear, "a word boundary");
        assert_eq!(
            sh(&r, "sudo -u root rm x"),
            Verdict::Unsure,
            "a wrapper's flags"
        );
        assert_eq!(sh(&r, "nice -n 5 rm x"), Verdict::Unsure);
        assert_eq!(
            sh(&r, "/usr/bin/nice rm -rf build"),
            Verdict::Denied("Bash(rm *)".into())
        );
        assert_eq!(
            sh(&r, "/usr/bin/sudo rm x"),
            Verdict::Denied("Bash(rm *)".into())
        );
        assert_eq!(sh(&r, "timeout 60 rm -rf build"), Verdict::Unsure);
        assert_eq!(sh(&r, "RM -rf build"), Verdict::Denied("Bash(rm *)".into()));
        assert_eq!(bash("rm *", "FOO=1 rm x"), None);
        assert_eq!(bash("rm *", "ls\trm x"), None);
        assert_eq!(sh(&r, "/BIN/RM x"), Verdict::Denied("Bash(rm *)".into()));
        assert_eq!(sh(&r, "ls"), Verdict::Clear);
        assert_eq!(sh(&r, "ls $(rm x)"), Verdict::Unsure);
    }

    /// Multi-word rules are the common ones, and the rule leaves (`Bash(git *)`) never match them.
    #[test]
    fn a_multi_word_rule_matches_the_command() {
        let r = rules(&["Bash(git push:*)"]);
        assert_eq!(
            sh(&r, "git push origin main"),
            Verdict::Denied("Bash(git push:*)".into())
        );
        assert_eq!(sh(&r, "git status"), Verdict::Clear);
        assert_eq!(sh(&r, "git -C . push origin"), Verdict::Unsure);
        assert_eq!(sh(&r, "git --no-pager push"), Verdict::Unsure);
        assert_eq!(
            sh(&rules(&["Bash(npm run *:prod)"]), "npm run deploy:prod"),
            Verdict::Denied("Bash(npm run *:prod)".into())
        );
    }

    #[test]
    fn bare_tools_mcp_servers_and_domains() {
        assert_eq!(
            check(&rules(&["WebSearch"]), "WebSearch", &json!({})),
            Verdict::Denied("WebSearch".into())
        );
        let m = rules(&["mcp__qa"]);
        assert_eq!(
            check(&m, "mcp__qa__echo", &json!({})),
            Verdict::Denied("mcp__qa".into())
        );
        assert_eq!(check(&m, "mcp__qa2__echo", &json!({})), Verdict::Clear);
        assert_eq!(
            check(&rules(&["mcp__qa__*"]), "mcp__qa__echo", &json!({})),
            Verdict::Denied("mcp__qa__*".into())
        );
        assert_eq!(
            check(&rules(&["mcp__qa__echo"]), "mcp__qa__other", &json!({})),
            Verdict::Clear
        );
        let d = rules(&["WebFetch(domain:x.com)"]);
        let url = |u: &str| check(&d, "WebFetch", &json!({ "url": u }));
        assert_eq!(
            url("https://api.x.com/a"),
            Verdict::Denied("WebFetch(domain:x.com)".into())
        );
        assert_eq!(
            url("https://user@X.com:8443/"),
            Verdict::Denied("WebFetch(domain:x.com)".into())
        );
        assert_eq!(url("https://notx.com/"), Verdict::Clear);
        assert_eq!(url("not a url"), Verdict::Unsure);
        assert_eq!(
            url("https://x.com./a"),
            Verdict::Denied("WebFetch(domain:x.com)".into())
        );
        assert_eq!(url("https://evil.com\\@x.com/"), Verdict::Unsure);
        assert_eq!(url("https://%78.com/"), Verdict::Unsure);
        assert_eq!(
            check(&[], "Bash", &json!({"command": "ls $(rm x)"})),
            Verdict::Clear
        );
    }

    /// A broken settings file is no rules, never a refusal of everything.
    #[test]
    fn unreadable_settings_are_no_rules() {
        let d = tempfile::tempdir().unwrap();
        let home = Utf8Path::from_path(d.path()).unwrap();
        assert!(read(home).is_empty());
        std::fs::write(home.join("settings.json"), "{not json").unwrap();
        assert!(read(home).is_empty());
        std::fs::write(
            home.join("settings.json"),
            r#"{"permissions":{"deny":"Bash"}}"#,
        )
        .unwrap();
        assert!(read(home).is_empty());
        std::fs::write(
            home.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(rm *)"]}}"#,
        )
        .unwrap();
        assert_eq!(read(home), vec!["Bash(rm *)".to_string()]);
    }
}
