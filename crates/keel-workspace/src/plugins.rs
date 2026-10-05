use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

/// An installed plugin.
///
/// Plugins package skills, agents, hooks and MCP servers together, which makes them the highest-
/// leverage thing in the whole configuration surface — and the least visible. Installing one can
/// add hooks that run shell commands, and nothing in the terminal shows you that afterwards.
#[derive(Debug, Clone, Serialize)]
pub struct Plugin {
    pub name: String,
    /// Whether Claude Code currently loads it. Disabled plugins stay installed.
    pub enabled: bool,
    /// The marketplace it came from, parsed out of the `name@marketplace` key.
    pub marketplace: Option<String>,
    pub version: Option<String>,
    pub scope: Option<String>,
    pub installed_at: Option<String>,
    pub last_updated: Option<String>,
    /// Where the plugin's files are, for reading the skills inside it. Not sent: the app has no
    /// use for a cache path.
    #[serde(skip)]
    pub install_path: Option<Utf8PathBuf>,
}

/// Read `~/.claude/plugins/installed_plugins.json`.
///
/// The index is keyed `"<name>@<marketplace>"` and maps to an array of installations, one per
/// scope. A malformed or absent file yields an empty list: not having plugins is normal.
///
/// The index is machine-wide, and a `project` or `local` installation belongs to the project at
/// its `projectPath` — so only `repo`'s are listed, beside the person's own. Listing every
/// project's would show a plugin here that Claude Code does not load here.
pub fn discover_plugins(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Plugin> {
    // Enablement lives in settings.json, separately from the install index — a disabled plugin is
    // still installed, and the UI needs to tell those apart to offer the right action. Each scope
    // has its own settings file.
    let enabled_in = |file: Utf8PathBuf| -> Value {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("enabledPlugins").cloned())
            .unwrap_or(Value::Null)
    };
    let user_enabled = enabled_in(claude_home.join("settings.json"));
    let project_enabled = enabled_in(repo.join(".claude/settings.json"));
    let local_enabled = enabled_in(repo.join(".claude/settings.local.json"));
    let here = |install: &Value| {
        install
            .get("projectPath")
            .and_then(Value::as_str)
            .is_some_and(|p| same_path(Utf8Path::new(p), repo))
    };

    let path = claude_home.join("plugins").join("installed_plugins.json");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(index) = serde_json::from_str::<Value>(&contents) else {
        return Vec::new();
    };
    let Some(entries) = index.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut plugins: Vec<Plugin> = entries
        .iter()
        .flat_map(|(key, installations)| {
            let (name, marketplace) = match key.split_once('@') {
                Some((name, market)) => (name.to_string(), Some(market.to_string())),
                None => (key.clone(), None),
            };

            installations
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(|install| {
                    let field = |k: &str| install.get(k).and_then(Value::as_str).map(str::to_owned);
                    let enabled_map = match field("scope").as_deref() {
                        Some("project") if here(install) => &project_enabled,
                        Some("local") if here(install) => &local_enabled,
                        Some("project" | "local") => return None,
                        _ => &user_enabled,
                    };
                    Some(Plugin {
                        // Absent from enabledPlugins means enabled: the file records overrides.
                        enabled: enabled_map
                            .get(key)
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        name: name.clone(),
                        marketplace: marketplace.clone(),
                        version: field("version"),
                        scope: field("scope"),
                        installed_at: field("installedAt"),
                        last_updated: field("lastUpdated"),
                        install_path: field("installPath").map(Utf8PathBuf::from),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();

    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

/// The same directory, through any links: Claude Code records a resolved path, and on macOS a
/// repository's own path very often is not one (`/tmp` is `/private/tmp`).
fn same_path(a: &Utf8Path, b: &Utf8Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    fn home_with(index: &str) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(home.join("plugins/installed_plugins.json"), index).unwrap();
        (dir, home)
    }

    /// A project installation shows in its own project and nowhere else, with the project's
    /// settings deciding whether it is enabled.
    #[test]
    fn a_project_plugin_belongs_to_its_project() {
        let dir = tempfile::tempdir().unwrap();
        let base = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let (home, repo, other) = (base.join("home"), base.join("repo"), base.join("other"));
        for d in [&home.join("plugins"), &repo.join(".claude"), &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(
            home.join("plugins/installed_plugins.json"),
            format!(
                r#"{{"plugins":{{"a@m":[{{"scope":"project","projectPath":"{repo}"}}],"u@m":[{{"scope":"user"}}]}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            repo.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"a@m":false}}"#,
        )
        .unwrap();
        let here = discover_plugins(&repo, &home);
        let names: Vec<_> = here.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "u"]);
        assert!(!here[0].enabled, "the project's settings decide");
        let there: Vec<_> = discover_plugins(&other, &home)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(there, ["u"]);
    }

    #[test]
    fn a_disabled_plugin_is_still_listed() {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        std::fs::write(
            home.join("plugins/installed_plugins.json"),
            r#"{"plugins":{"a@m":[{"scope":"user"}],"b@m":[{"scope":"user"}]}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"a@m":false}}"#,
        )
        .unwrap();

        let plugins = discover_plugins(Utf8Path::new("/nonexistent-repo"), &home);
        let a = plugins.iter().find(|p| p.name == "a").expect("a");
        let b = plugins.iter().find(|p| p.name == "b").expect("b");
        assert!(!a.enabled, "explicitly disabled");
        assert!(b.enabled, "absent means enabled");
    }

    #[test]
    fn splits_name_from_marketplace() {
        let (_d, home) = home_with(
            r#"{"version":2,"plugins":{"frontend-design@claude-plugins-official":[
                {"scope":"user","version":"e33a9ec0973a","installedAt":"2026-05-12T21:49:01.729Z"}]}}"#,
        );
        let plugins = discover_plugins(Utf8Path::new("/nonexistent-repo"), &home);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "frontend-design");
        assert_eq!(
            plugins[0].marketplace.as_deref(),
            Some("claude-plugins-official")
        );
        assert_eq!(plugins[0].scope.as_deref(), Some("user"));
    }

    #[test]
    fn a_malformed_index_yields_nothing_rather_than_failing() {
        let (_d, home) = home_with("{ not json");
        assert!(discover_plugins(Utf8Path::new("/nonexistent-repo"), &home).is_empty());
    }

    #[test]
    fn no_plugins_installed_is_normal() {
        assert!(
            discover_plugins(
                Utf8Path::new("/nonexistent-repo"),
                Utf8Path::new("/nonexistent")
            )
            .is_empty()
        );
    }
}
