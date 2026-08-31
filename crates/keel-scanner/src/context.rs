use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use std::collections::BTreeSet;

/// A snapshot of the repository the checks run against.
///
/// Built once and shared by every check, so a scan walks the tree a single time. Paths are stored
/// repo-relative and sorted, which is what makes reports reproducible across machines.
pub struct RepoContext {
    root: Utf8PathBuf,
    files: BTreeSet<Utf8PathBuf>,
}

impl RepoContext {
    /// Walk `root`, honouring `.gitignore`, and capture the file list.
    ///
    /// Hidden files are included deliberately: `.github/`, `.claude/` and `.env` are exactly the
    /// paths several checks care about, and the default walker would skip them.
    pub fn load(root: impl AsRef<Utf8Path>) -> Result<Self> {
        let root = root.as_ref().to_owned();
        let mut files = BTreeSet::new();

        let walker = WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            // Honour the user's global gitignore too. Personal local files — a developer's own
            // .claude/settings.local.json is the common case — are not content the repository
            // ships, and reporting them as such is alarming and wrong.
            .git_global(true)
            .parents(false)
            // Honour .gitignore even when the tree is not a git checkout yet. Keel scans
            // directories before cloning as well as after, and an ignored file is ignored either
            // way; without this the walker silently skips gitignore rules outside a repo.
            .require_git(false)
            .build();

        for entry in walker {
            let entry = entry.with_context(|| format!("walking {root}"))?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = Utf8Path::from_path(entry.path())
                .context("repository contains a non-UTF-8 path")?;
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            // `.git` internals are noise for every check we have. `.keel` is Keel's own working
            // directory, and skipping it matters for correctness rather than tidiness: it holds
            // quarantined agent config, and re-reporting a file that `keel trust` already
            // neutralised would mean the finding could never be cleared.
            //
            // Asked of the path *inside* the repository, never of the absolute one. A lane's
            // checkout lives at `<project>/.keel/worktrees/<name>`, so matching on the full path
            // skipped every file it held: the scan of a lane saw an empty repository, reported
            // "no tests", "no CLAUDE.md", "no CI" whatever the code said, and no fix could ever
            // clear a finding.
            if rel
                .components()
                .any(|c| matches!(c.as_str(), ".git" | ".keel"))
            {
                continue;
            }
            files.insert(rel.to_owned());
        }

        Ok(Self { root, files })
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    pub fn files(&self) -> impl Iterator<Item = &Utf8Path> {
        self.files.iter().map(|p| p.as_path())
    }

    /// True when the repo-relative path exists.
    pub fn has(&self, path: impl AsRef<str>) -> bool {
        self.files.contains(Utf8Path::new(path.as_ref()))
    }

    /// True when any of the given repo-relative paths exist.
    pub fn has_any<'a>(&self, paths: impl IntoIterator<Item = &'a str>) -> bool {
        paths.into_iter().any(|p| self.has(p))
    }

    /// Every file whose path contains `needle`.
    pub fn matching<'a>(&'a self, needle: &'a str) -> impl Iterator<Item = &'a Utf8Path> {
        self.files().filter(move |p| p.as_str().contains(needle))
    }

    /// Read a repo-relative file. Returns `None` when it is absent or not valid UTF-8, because a
    /// check should degrade to "cannot tell" rather than failing the whole scan.
    pub fn read(&self, path: impl AsRef<str>) -> Option<String> {
        std::fs::read_to_string(self.root.join(path.as_ref())).ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::fixture;

    #[test]
    fn finds_hidden_files() {
        let (_dir, ctx) = fixture(&[(".github/workflows/ci.yml", "on: push")]);
        assert!(ctx.has(".github/workflows/ci.yml"));
    }

    #[test]
    fn honours_gitignore() {
        let (_dir, ctx) = fixture(&[(".gitignore", "secret.txt\n"), ("secret.txt", "shh")]);
        assert!(!ctx.has("secret.txt"));
        assert!(ctx.has(".gitignore"));
    }

    /// `keel trust` moves hostile config into `.keel/quarantine`. If the scanner still reported it
    /// from there, running trust would never clear the finding.
    #[test]
    fn ignores_keels_own_quarantine_directory() {
        let (_dir, ctx) = fixture(&[
            ("src/app.ts", "export const x = 1"),
            (".keel/quarantine/.claude/settings.json", r#"{"hooks":{}}"#),
        ]);
        assert!(!ctx.has(".keel/quarantine/.claude/settings.json"));
        assert_eq!(ctx.files().count(), 1);
    }

    /// The one that made a lane's readiness report fiction: a checkout under
    /// `<project>/.keel/worktrees/<name>` matched the `.keel` skip on its *absolute* path, so the
    /// scan saw no files at all and every "you have no X" check fired.
    #[test]
    fn a_checkout_under_dot_keel_still_sees_its_own_files() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("utf8 tempdir")
            .join(".keel/worktrees/a-lane");
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src/app.ts"), "export const x = 1").expect("write");
        std::fs::write(root.join("CLAUDE.md"), "# instructions").expect("write");

        let ctx = crate::RepoContext::load(&root).expect("load");
        assert!(ctx.has("CLAUDE.md"), "the lane's own files are its files");
        assert!(ctx.has("src/app.ts"));
    }

    #[test]
    fn reads_missing_file_as_none() {
        let (_dir, ctx) = fixture(&[("README.md", "hi")]);
        assert_eq!(ctx.read("README.md").as_deref(), Some("hi"));
        assert!(ctx.read("nope.md").is_none());
    }
}
