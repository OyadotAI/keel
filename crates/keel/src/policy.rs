use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Policy {
    pub max_files: usize,
    pub require_isolation: bool,
    /// Whether a *policy file* demanded isolation, as opposed to the person having asked for it.
    ///
    /// The distinction decides whether a failed `git worktree add` may fall back to the project:
    /// a lane the person chose to isolate can run in the project and say so, a lane an
    /// organisation mandated cannot.
    pub isolation_by_policy: bool,
    pub allowed_providers: Vec<String>,
    pub sources: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_files: 8,
            // Keel's own default is *not* to force isolation — the person picks, per feature,
            // in New Feature. It used to be `true`, and because `tighten_from` can only ever
            // raise it there was no way back: every non-plan turn was flipped isolated at the
            // last moment, so "Sharing the working tree" was offered in three places and
            // produced an isolated lane, and the guard against two lanes writing one tree had
            // nothing left to guard. Only a policy file raises this now.
            require_isolation: false,
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

    /// A repository cannot widen what the defaults or an organisation already decided.
    #[test]
    fn repository_policy_can_only_tighten_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir(root.join(".keel")).unwrap();
        std::fs::write(
            root.join(".keel/policy.toml"),
            "max_files = 20\nallowed_providers = ['claude']\n",
        )
        .unwrap();
        let policy = Policy::load(&root);
        assert_eq!(policy.max_files, 8);
        assert_eq!(policy.allowed_providers, ["claude"]);
    }

    /// Isolation is the person's choice per feature until a policy file takes it away, and a
    /// repository cannot hand it back. Both halves matter: the first is why "Sharing the working
    /// tree" exists at all, the second is what makes an organisation's mandate a mandate.
    #[test]
    fn isolation_is_a_choice_until_a_policy_demands_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir(root.join(".keel")).unwrap();

        let plain = Policy::load(&root);
        assert!(!plain.require_isolation, "Keel does not force isolation");
        assert!(!plain.isolation_by_policy);

        std::fs::write(root.join(".keel/policy.toml"), "require_isolation = true\n").unwrap();
        let demanded = Policy::load(&root);
        assert!(demanded.require_isolation);
        assert!(
            demanded.isolation_by_policy,
            "a failed checkout must refuse rather than fall back to the project"
        );

        std::fs::write(
            root.join(".keel/policy.toml"),
            "require_isolation = false\n",
        )
        .unwrap();
        let mut org = Policy {
            require_isolation: true,
            isolation_by_policy: true,
            ..Default::default()
        };
        org.tighten_from(&root.join(".keel/policy.toml"), "repository policy");
        assert!(
            org.require_isolation,
            "a repository cannot relax an organisation's mandate"
        );
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
