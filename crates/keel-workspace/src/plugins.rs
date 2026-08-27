use camino::Utf8Path;
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
    /// The marketplace it came from, parsed out of the `name@marketplace` key.
    pub marketplace: Option<String>,
    pub version: Option<String>,
    pub scope: Option<String>,
    pub installed_at: Option<String>,
    pub last_updated: Option<String>,
}

/// Read `~/.claude/plugins/installed_plugins.json`.
///
/// The index is keyed `"<name>@<marketplace>"` and maps to an array of installations, one per
/// scope. A malformed or absent file yields an empty list: not having plugins is normal.
pub fn discover_plugins(claude_home: &Utf8Path) -> Vec<Plugin> {
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
                .map(|install| {
                    let field = |k: &str| install.get(k).and_then(Value::as_str).map(str::to_owned);
                    Plugin {
                        name: name.clone(),
                        marketplace: marketplace.clone(),
                        version: field("version"),
                        scope: field("scope"),
                        installed_at: field("installedAt"),
                        last_updated: field("lastUpdated"),
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();

    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
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

    #[test]
    fn splits_name_from_marketplace() {
        let (_d, home) = home_with(
            r#"{"version":2,"plugins":{"frontend-design@claude-plugins-official":[
                {"scope":"user","version":"e33a9ec0973a","installedAt":"2026-05-12T21:49:01.729Z"}]}}"#,
        );
        let plugins = discover_plugins(&home);
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
        assert!(discover_plugins(&home).is_empty());
    }

    #[test]
    fn no_plugins_installed_is_normal() {
        assert!(discover_plugins(Utf8Path::new("/nonexistent")).is_empty());
    }
}
