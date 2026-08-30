use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Policy {
    pub max_files: usize,
    pub require_isolation: bool,
    /// Whether a *policy file* demanded isolation, as opposed to it being Keel's own default.
    ///
    /// The distinction decides whether a failed `git worktree add` may fall back to the project.
    /// `require_isolation` defaults to true for everybody and `tighten_from` can only ever raise
    /// it, so without this flag every user looks like an organisation that mandated isolation —
    /// and a customer whose worktree could not be created was simply blocked from working.
    pub isolation_by_policy: bool,
    pub allowed_providers: Vec<String>,
    pub sources: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_files: 8,
            require_isolation: true,
            isolation_by_policy: false,
            allowed_providers: vec!["claude".into(), "codex".into()],
            sources: vec!["Keel defaults".into()],
        }
    }
}

impl Policy {
    pub fn load(repo: &Utf8Path) -> Self {
        let mut policy = Self::default();
        if let Some(path) = std::env::var_os("KEEL_ORG_POLICY")
            .and_then(|path| Utf8PathBuf::from_path_buf(path.into()).ok())
        {
            policy.tighten_from(&path, "organization policy");
        }
        policy.tighten_from(&repo.join(".keel/policy.toml"), "repository policy");
        policy
    }

    fn tighten_from(&mut self, path: &Utf8Path, label: &str) {
        let Ok(body) = std::fs::read_to_string(path) else {
            return;
        };
        let parsed = Parsed::read(&body);
        if let Some(max) = parsed.max_files.filter(|max| *max > 0) {
            self.max_files = self.max_files.min(max);
        }
        if parsed.require_isolation == Some(true) {
            self.require_isolation = true;
            self.isolation_by_policy = true;
        }
        if let Some(allowed) = parsed.allowed_providers {
            self.allowed_providers
                .retain(|provider| allowed.contains(provider));
        }
        self.sources.push(format!("{label}: {path}"));
    }
}

#[derive(Default)]
struct Parsed {
    max_files: Option<usize>,
    require_isolation: Option<bool>,
    allowed_providers: Option<Vec<String>>,
}

impl Parsed {
    fn read(body: &str) -> Self {
        let mut parsed = Self::default();
        for line in body.lines().map(str::trim) {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "max_files" => parsed.max_files = value.parse().ok(),
                "require_isolation" => parsed.require_isolation = value.parse().ok(),
                "allowed_providers" => {
                    let values = value
                        .trim_matches(['[', ']'])
                        .split(',')
                        .map(|item| item.trim().trim_matches(['\'', '"']).to_ascii_lowercase())
                        .filter(|item| !item.is_empty())
                        .collect();
                    parsed.allowed_providers = Some(values);
                }
                _ => {}
            }
        }
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_policy_can_only_tighten_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/policy.toml"),
            "max_files = 20\nrequire_isolation = false\nallowed_providers = ['claude']\n",
        )
        .unwrap();
        let policy = Policy::load(&root);
        assert_eq!(policy.max_files, 8);
        assert!(policy.require_isolation);
        assert_eq!(policy.allowed_providers, ["claude"]);
    }

    #[test]
    fn repository_policy_can_reduce_scope() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir(root.join(".keel")).unwrap();
        std::fs::write(root.join(".keel/policy.toml"), "max_files = 3\n").unwrap();
        assert_eq!(Policy::load(&root).max_files, 3);
    }
}
