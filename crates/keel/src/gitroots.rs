//! Which directories here are git repositories.
//!
//! The folder somebody opens is not always the repository. The common shape on a real team is a
//! workspace — `backend/` and `frontend/`, each its own repo, opened together because the work
//! crosses both. Keel assumed the opened folder *was* the repository, so that team got no diffs,
//! no history, no commits and no rewind, and was offered a `git init` that would have wrapped
//! their two repositories in a third.
//!
//! The search is `dev::searchable`'s, for the same reason it exists there: root, then children,
//! then `apps/` and `packages/`. One difference, and it is the point — for a dev server, "the root
//! declares one, that is the answer" and the search stops. Two sibling repositories are not a
//! choice between alternatives; they are both real, and both are returned.

use camino::Utf8Path;
use serde::Serialize;

/// A git repository inside the opened folder.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Root {
    /// Relative to the opened folder. Empty when the folder is itself the repository.
    pub dir: String,
}

/// Whether this exact directory is the top of a git repository.
///
/// `git status` succeeding is not the same question, and answering it that way was a quiet bug:
/// git walks *up* until it finds a `.git`, so opening a plain folder inside a repository — a home
/// directory that is a dotfiles checkout is the everyday case — reported that ancestor as the
/// project. Keel then showed the ancestor's branch and its changes, with every path relative to a
/// root the person had not opened. A blank panel is a bad answer; the wrong repository is worse.
pub fn is_root(dir: &Utf8Path) -> bool {
    let mut c = crate::git::command(dir);
    c.args(["rev-parse", "--show-toplevel"]);
    let out = crate::git::output(c);
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Canonicalised on both sides: `/tmp` is a symlink to `/private/tmp` on macOS, and comparing
    // the raw strings would call every repository under it "not the top".
    match (
        std::fs::canonicalize(top).ok(),
        std::fs::canonicalize(dir.as_std_path()).ok(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Every repository in the opened folder: the folder itself, or the children that are ones.
///
/// A repository at the root ends the search — everything below it is that repository's own
/// business, including submodules, which git already tracks.
pub fn find(root: &Utf8Path) -> Vec<Root> {
    if is_root(root) {
        return vec![Root { dir: String::new() }];
    }
    let mut out = Vec::new();
    for dir in searchable(root) {
        if is_root(&root.join(&dir)) {
            out.push(Root { dir });
        }
        // A workspace of a hundred clones must not turn one status call into a hundred `git`
        // processes.
        if out.len() >= 12 {
            break;
        }
    }
    out
}

/// Which repository a path belongs to, and what it is called inside it.
///
/// `backend/src/api.ts` in a workspace is `src/api.ts` in the `backend` repository. Every handler
/// that acts on one file — diff, stage, discard — needs both halves, and getting it wrong means
/// running `git add` in a directory that has never heard of the file.
pub fn resolve(root: &Utf8Path, path: &str) -> (camino::Utf8PathBuf, String) {
    for found in find(root) {
        if found.dir.is_empty() {
            break;
        }
        if let Some(rest) = path.strip_prefix(&format!("{}/", found.dir)) {
            return (root.join(&found.dir), rest.to_string());
        }
    }
    (root.to_path_buf(), path.to_string())
}

/// The same walk `dev::searchable` does, and deliberately so: one idea of where the projects in a
/// folder are, rather than two that disagree about it.
fn searchable(root: &Utf8Path) -> Vec<String> {
    fn children(dir: &Utf8Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
            .collect();
        names.sort();
        for n in names {
            out.push(if prefix.is_empty() {
                n
            } else {
                format!("{prefix}/{n}")
            });
        }
    }

    let mut out = Vec::new();
    children(root, "", &mut out);
    for nest in ["apps", "packages"] {
        let dir = root.join(nest);
        if dir.is_dir() {
            children(&dir, nest, &mut out);
        }
    }
    out.truncate(60);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    /// A real repository with a commit in it. Without the commit `rev-parse HEAD` fails and the
    /// branch is unnamed, which is a state almost no project is ever in.
    fn repo(at: &Utf8Path) {
        std::fs::create_dir_all(at).unwrap();
        std::fs::write(at.join("README.md"), "seed").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["add", "-A"],
            vec!["commit", "-qm", "seed"],
        ] {
            std::process::Command::new("git")
                .current_dir(at)
                .args(args)
                .output()
                .unwrap();
        }
    }

    fn tmp(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .unwrap()
            .join(format!("keel-gitroots-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The shape this exists for: a workspace folder holding two repositories.
    #[test]
    fn two_sibling_repositories_are_both_found() {
        let root = tmp("workspace");
        repo(&root.join("backend"));
        repo(&root.join("frontend"));
        std::fs::create_dir_all(root.join("docs")).unwrap();

        let found = find(&root);
        assert_eq!(
            found.iter().map(|r| r.dir.as_str()).collect::<Vec<_>>(),
            ["backend", "frontend"],
            "both are real; a folder that is not a repository is not one"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A repository at the root is the answer, and nothing below it is a separate project.
    #[test]
    fn a_repository_at_the_root_ends_the_search() {
        let root = tmp("plain");
        repo(&root);
        repo(&root.join("vendored"));
        assert_eq!(find(&root), vec![Root { dir: String::new() }]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The quiet one. Git walks up, so a plain folder inside a repository used to report that
    /// ancestor as the project — its branch, its changes, and every path relative to a root the
    /// person never opened.
    #[test]
    fn a_folder_inside_a_repository_is_not_a_repository() {
        let root = tmp("ancestor");
        repo(&root);
        let inner = root.join("just-a-folder");
        std::fs::create_dir_all(&inner).unwrap();

        assert!(
            !is_root(&inner),
            "git succeeds here by walking up; the question is whether this is the top"
        );
        assert!(find(&inner).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Through `git_status`, which is what the app actually binds to: a workspace reports both
    /// repositories, its changes carry the directory each came from, and `is_repo` is true —
    /// because the folder *is* versioned, just not at its top. Reporting false is what put
    /// "Initialise a repository" in front of people whose repositories already existed.
    #[test]
    fn a_workspace_reports_both_repositories_and_prefixes_their_paths() {
        let root = tmp("status");
        repo(&root.join("backend"));
        repo(&root.join("frontend"));
        std::fs::write(root.join("backend/api.ts"), "x").unwrap();
        std::fs::write(root.join("frontend/page.tsx"), "y").unwrap();

        let status = crate::repo::git_status(&root);
        assert!(
            status.is_repo,
            "the folder is versioned, just not at its top"
        );
        assert_eq!(status.repos.len(), 2);
        let mut paths: Vec<_> = status.changes.iter().map(|c| c.path.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            ["backend/api.ts", "frontend/page.tsx"],
            "paths are relative to the folder that was opened, so the tree groups them"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The regression that matters more than the feature: an ordinary project must answer exactly
    /// as it did before.
    #[test]
    fn one_repository_answers_exactly_as_before() {
        let root = tmp("single");
        repo(&root);
        std::fs::write(root.join("a.txt"), "x").unwrap();

        let status = crate::repo::git_status(&root);
        assert!(status.is_repo);
        assert!(status.branch.is_some(), "the one branch is still named");
        assert_eq!(
            status
                .changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["a.txt"],
            "no directory prefix when the root is the repository"
        );
        assert_eq!(status.repos.len(), 1);
        assert_eq!(status.repos[0].dir, "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A turn that changed both halves is saved in both. One commit each, same message — not
    /// atomic, because two repositories cannot be, but nothing is left uncommitted.
    #[test]
    fn committing_a_workspace_commits_in_every_repository() {
        let root = tmp("commit");
        repo(&root.join("backend"));
        repo(&root.join("frontend"));
        std::fs::write(root.join("backend/api.ts"), "x").unwrap();
        std::fs::write(root.join("frontend/page.tsx"), "y").unwrap();

        assert_eq!(crate::worktree::commit_all(&root, "one message"), Ok(true));
        for half in ["backend", "frontend"] {
            let out = std::process::Command::new("git")
                .current_dir(root.join(half))
                .args(["log", "--oneline"])
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&out.stdout);
            assert!(log.contains("one message"), "{half} did not commit: {log}");
        }
        // And the workspace folder itself is still not a repository.
        assert!(!is_root(&root));

        // Nothing left to commit is not a failure.
        assert_eq!(crate::worktree::commit_all(&root, "again"), Ok(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Clicking a file in a workspace has to diff it in the repository that owns it — the path
    /// on screen names the folder, and git inside `backend` has never heard of `backend/`.
    #[test]
    fn a_path_resolves_to_the_repository_that_owns_it() {
        let root = tmp("resolve");
        repo(&root.join("backend"));
        repo(&root.join("frontend"));

        let (dir, inner) = resolve(&root, "backend/src/api.ts");
        assert_eq!(dir, root.join("backend"));
        assert_eq!(inner, "src/api.ts");

        // A file belonging to no repository is left alone rather than guessed at.
        let (dir, inner) = resolve(&root, "docs/readme.md");
        assert_eq!(dir, root);
        assert_eq!(inner, "docs/readme.md");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// And an ordinary project must not have its paths rewritten at all.
    #[test]
    fn a_plain_repository_resolves_to_itself() {
        let root = tmp("resolve-plain");
        repo(&root);
        let (dir, inner) = resolve(&root, "src/api.ts");
        assert_eq!(dir, root);
        assert_eq!(inner, "src/api.ts");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every folder shape a person can open, through the surface the app actually calls.
    ///
    /// The property under test is not "the answer is clever" — it is that **no shape blocks the
    /// person**. Every one of these must produce a usable state: a status that returns, a gate
    /// that either exists or honestly does not, and a commit that either saves the work or says
    /// nothing needed saving. Nothing here may panic, hang, or refuse.
    #[test]
    fn every_folder_shape_is_usable() {
        struct Shape {
            name: &'static str,
            build: fn(&Utf8Path),
            /// Repository directories expected, in order.
            repos: Vec<&'static str>,
            /// Does the person get git features at all?
            versioned: bool,
        }

        let shapes = vec![
            Shape {
                name: "an ordinary project",
                build: |root| repo(root),
                repos: vec![""],
                versioned: true,
            },
            Shape {
                name: "a repository with no commits yet",
                build: |root| {
                    std::fs::create_dir_all(root).unwrap();
                    std::process::Command::new("git")
                        .current_dir(root)
                        .args(["init", "-q"])
                        .output()
                        .unwrap();
                },
                repos: vec![""],
                versioned: true,
            },
            Shape {
                name: "a plain folder with no git anywhere",
                build: |root| {
                    std::fs::create_dir_all(root.join("src")).unwrap();
                    std::fs::write(root.join("src/a.txt"), "x").unwrap();
                },
                repos: vec![],
                versioned: false,
            },
            Shape {
                name: "a workspace of two repositories",
                build: |root| {
                    repo(&root.join("backend"));
                    repo(&root.join("frontend"));
                },
                repos: vec!["backend", "frontend"],
                versioned: true,
            },
            Shape {
                name: "one repository beside a plain folder",
                build: |root| {
                    repo(&root.join("backend"));
                    std::fs::create_dir_all(root.join("docs")).unwrap();
                    std::fs::write(root.join("docs/plan.md"), "x").unwrap();
                },
                repos: vec!["backend"],
                versioned: true,
            },
            Shape {
                name: "a repository nested under apps/",
                build: |root| {
                    std::fs::create_dir_all(root.join("apps")).unwrap();
                    repo(&root.join("apps/web"));
                },
                repos: vec!["apps/web"],
                versioned: true,
            },
            Shape {
                name: "a repository holding another repository",
                build: |root| {
                    repo(root);
                    repo(&root.join("vendored"));
                },
                // The root wins: what is inside it is that repository's own business.
                repos: vec![""],
                versioned: true,
            },
            Shape {
                name: "a repository inside node_modules, which is not a project",
                build: |root| {
                    std::fs::create_dir_all(root).unwrap();
                    repo(&root.join("node_modules/some-package"));
                },
                repos: vec![],
                versioned: false,
            },
            Shape {
                name: "an empty folder",
                build: |root| {
                    std::fs::create_dir_all(root).unwrap();
                },
                repos: vec![],
                versioned: false,
            },
        ];

        for shape in shapes {
            let root = tmp(&shape.name.replace(' ', "-"));
            (shape.build)(&root);

            // 1. Which repositories are here.
            let found: Vec<String> = find(&root).into_iter().map(|r| r.dir).collect();
            assert_eq!(found, shape.repos, "repositories for {}", shape.name);

            // 2. Status always answers, and never claims to be a repository when it is not.
            let status = crate::repo::git_status(&root);
            assert_eq!(
                status.is_repo, shape.versioned,
                "is_repo for {}",
                shape.name
            );

            // 3. The gate either exists or honestly does not — it must never panic.
            let _ = crate::verify::detect_all(&root);

            // 4. Committing never blocks. Nothing to commit is `Ok(false)`, not an error, and a
            //    folder with no git at all is allowed to have nothing to do.
            match crate::worktree::commit_all(&root, "a turn") {
                Ok(_) => {}
                Err(_) if !shape.versioned => {}
                Err(e) => panic!("committing blocked the person in {}: {e}", shape.name),
            }

            // 5. Resolving a path never invents a repository.
            let (dir, _) = resolve(&root, "some/file.txt");
            assert!(dir.starts_with(&root), "resolve escaped {}", shape.name);

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn nothing_at_all_is_not_a_repository() {
        let root = tmp("empty");
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        assert!(find(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
