//! Which files import a given one.
//!
//! Used by the preview to decide what a change to a component might have moved on screen. Text
//! matching on import specifiers, deliberately: a real module graph would mean a bundler.

use camino::Utf8Path;

pub fn importers_of(root: &Utf8Path, file: &str) -> Vec<String> {
    let target = Utf8Path::new(file);
    let Some(stem) = target.file_stem() else {
        return Vec::new();
    };
    // A directory import resolves to its `index`, so the folder's name is what appears in the
    // specifier — `from "../steps"`, not `from "../steps/index"`.
    let names: Vec<&str> = if stem == "index" {
        target
            .parent()
            .and_then(|p| p.file_name())
            .into_iter()
            .collect()
    } else {
        vec![stem]
    };
    if names.is_empty() {
        return Vec::new();
    }

    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .require_git(false)
        .parents(false)
        .build();

    let mut found = Vec::new();
    for entry in walker.flatten() {
        let Some(path) = Utf8Path::from_path(entry.path()) else {
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if !matches!(
            path.extension(),
            Some("tsx" | "jsx" | "ts" | "js" | "vue" | "svelte" | "astro" | "mdx")
        ) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        // Relative, not absolute: a lane's checkout is under `.keel/worktrees/`, and asking of the
        // absolute path there excluded the whole repository.
        if rel.components().any(|c| {
            matches!(
                c.as_str(),
                ".git" | ".keel" | "target" | "node_modules" | "dist"
            )
        }) {
            continue;
        }
        if rel.as_str() == file {
            continue; // a file does not import itself
        }
        // Big files are almost never the page that renders one component, and reading them all is
        // what would make this slow on a large repository.
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if text.len() > 400_000 {
            continue;
        }
        if text.lines().any(|line| line_imports(line, &names)) {
            found.push(rel.to_string());
        }
        if found.len() >= 40 {
            break;
        }
    }
    found
}

/// Whether one line is an import whose specifier ends in one of these names.
fn line_imports(line: &str, names: &[&str]) -> bool {
    let trimmed = line.trim_start();
    if !(trimmed.starts_with("import")
        || trimmed.starts_with("export")
        || line.contains("require(")
        || line.contains("import("))
    {
        return false;
    }
    for quote in ['"', '\''] {
        let mut rest = line;
        while let Some(open) = rest.find(quote) {
            let after = &rest[open + 1..];
            let Some(close) = after.find(quote) else {
                break;
            };
            let spec = &after[..close];
            if specifier_names(spec).is_some_and(|last| names.contains(&last)) {
                return true;
            }
            rest = &after[close + 1..];
        }
    }
    false
}

/// The last path segment of an import specifier, without its extension.
///
/// `"@/components/steps/domain-step"` → `domain-step`. A bare package name (`react`) has no
/// separator and is not what anything in this repository is called, so it falls out naturally.
fn specifier_names(spec: &str) -> Option<&str> {
    if !spec.contains('/') {
        return None;
    }
    let last = spec.rsplit('/').next()?;
    Some(last.split('.').next().unwrap_or(last))
}

#[cfg(test)]
mod importer_tests {
    use super::*;

    /// A Next.js App Router tree with a component two levels below the page that renders it.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Utf8Path::from_path(dir.path()).expect("utf8 tempdir");
        let write = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };

        write(
            "components/steps/domain-step.tsx",
            "export function DomainStep() {}\n",
        );
        write(
            "components/onboarding.tsx",
            "import { DomainStep } from \"@/components/steps/domain-step\";\nexport function Onboarding() {}\n",
        );
        write(
            "app/onboarding/page.tsx",
            "import { Onboarding } from \"../../components/onboarding\";\nexport default function Page() {}\n",
        );
        // Noise: mentions the name without importing it, and lives in a folder that is skipped.
        write(
            "app/other/page.tsx",
            "// domain-step is mentioned here in a comment\n",
        );
        write(
            "node_modules/pkg/index.js",
            "require(\"./steps/domain-step\")\n",
        );
        dir
    }

    /// The walk the preview does: component → the thing that renders it → the page.
    #[test]
    fn a_component_leads_to_the_file_that_renders_it() {
        let dir = fixture();
        let root = Utf8Path::from_path(dir.path()).unwrap();

        let first = importers_of(root, "components/steps/domain-step.tsx");
        assert_eq!(first, vec!["components/onboarding.tsx"]);

        let second = importers_of(root, "components/onboarding.tsx");
        assert_eq!(second, vec!["app/onboarding/page.tsx"]);
    }

    /// A name in a comment is not an import, or every mention of a component would move the
    /// preview.
    #[test]
    fn a_mention_is_not_an_import() {
        let dir = fixture();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        assert!(
            !importers_of(root, "components/steps/domain-step.tsx")
                .contains(&"app/other/page.tsx".to_string())
        );
    }

    /// Dependencies are not the repository, and reading them is what would make this slow.
    #[test]
    fn node_modules_is_never_read() {
        let dir = fixture();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        assert!(
            !importers_of(root, "components/steps/domain-step.tsx")
                .iter()
                .any(|p| p.contains("node_modules"))
        );
    }

    /// A directory import resolves to its `index`, so the folder's name is what the specifier has.
    #[test]
    fn an_index_file_is_found_by_its_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("components/steps")).unwrap();
        std::fs::write(root.join("components/steps/index.ts"), "export {}\n").unwrap();
        std::fs::write(
            root.join("app/page.tsx"),
            "import { Steps } from \"../components/steps\";\n",
        )
        .ok();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(
            root.join("app/page.tsx"),
            "import { Steps } from \"../components/steps\";\n",
        )
        .unwrap();

        assert_eq!(
            importers_of(root, "components/steps/index.ts"),
            vec!["app/page.tsx"]
        );
    }

    /// A bare package name is not a file in this repository.
    #[test]
    fn a_package_import_is_not_a_local_file() {
        assert_eq!(specifier_names("react"), None);
        assert_eq!(
            specifier_names("@/components/steps/domain-step"),
            Some("domain-step")
        );
        assert_eq!(specifier_names("./domain-step.tsx"), Some("domain-step"));
    }
}
