//! Skills Keel can add to a project on request — never by default.
//!
//! The team in `crate::team` is written into every project; these are not. A skill here is
//! listed in the app's skill catalog and written only when the person clicks Add, into
//! `.claude/skills/<id>/`, never over a folder that exists.
//!
//! Each is vendored from its upstream with its licence, and chosen because it runs in Claude
//! Code as it stands and is for the people Keel is for. A2ABase's catalog was the shopping list:
//! nearly all of its 96 skills are Oya-sandbox tools (they read `INPUT_JSON` and call Oya's LLM
//! proxy) or sales and marketing personas, and the design skills beside this one are Anthropic's
//! and carry no licence Keel can redistribute under. The files are data, so they live beside the
//! other templates, outside `src/`, compiled in with `include_str!` — no dependency for a table.

/// One vendored skill: what the catalog row says, and every file it writes.
pub struct Vendored {
    /// The folder name under `.claude/skills/`, and the frontmatter `name`.
    pub id: &'static str,
    pub description: &'static str,
    pub license: &'static str,
    pub author: &'static str,
    /// Where it came from, pinned, so an update is a deliberate diff against a known commit.
    pub upstream: &'static str,
    /// The interpreter its scripts need, when it has any. The row says so before Add.
    pub script: Option<&'static str>,
    /// `(path inside the skill folder, contents)`.
    pub files: &'static [(&'static str, &'static str)],
}

impl Vendored {
    pub fn bytes(&self) -> usize {
        self.files.iter().map(|(_, b)| b.len()).sum()
    }
}

/// Every skill Keel offers. One today; the table is the catalog.
pub fn catalog() -> &'static [Vendored] {
    &[UI_UX_PRO_MAX]
}

pub fn find(id: &str) -> Option<&'static Vendored> {
    catalog().iter().find(|v| v.id == id)
}

macro_rules! vendored {
    ($dir:literal: $($path:literal),+ $(,)?) => {
        &[$(($path, include_str!(concat!("../skills/", $dir, "/", $path)))),+]
    };
}

/// Upstream's `.claude/skills/ui-ux-pro-max`, with its tests, its data validator and the four
/// JSON files only that validator reads left out (3.6 MB → 2 MB), and `${CLAUDE_PLUGIN_ROOT}/`
/// taken off the script paths in `SKILL.md`: that variable exists inside a plugin and nowhere
/// else, so in a project every command the skill gives would have run `/.claude/…` and failed.
pub const UI_UX_PRO_MAX: Vendored = Vendored {
    id: "ui-ux-pro-max",
    // The catalog row's copy, two lines at the sheet's width; Claude reads SKILL.md's own.
    description: "Before building a UI: a design system — style, palette, font pairing, layout and a pre-delivery checklist — from a local knowledge base.",
    license: "MIT",
    author: "Next Level Builder",
    upstream: "github.com/nextlevelbuilder/ui-ux-pro-max-skill@dcc40ff5",
    script: Some("python3"),
    // `.gitignore` is Keel's own: the skill runs Python, and `__pycache__` would otherwise
    // ride into the next turn's checkpoint in every repository that does not ignore it.
    files: vendored!("ui-ux-pro-max":
        ".gitignore",
        "LICENSE",
        "SKILL.md",
        "data/app-interface.csv",
        "data/charts.csv",
        "data/colors.csv",
        "data/google-fonts.csv",
        "data/icons.csv",
        "data/landing.csv",
        "data/motion.csv",
        "data/products.csv",
        "data/react-performance.csv",
        "data/stacks/angular.csv",
        "data/stacks/astro.csv",
        "data/stacks/avalonia.csv",
        "data/stacks/flutter.csv",
        "data/stacks/html-tailwind.csv",
        "data/stacks/javafx.csv",
        "data/stacks/jetpack-compose.csv",
        "data/stacks/laravel.csv",
        "data/stacks/nextjs.csv",
        "data/stacks/nuxt-ui.csv",
        "data/stacks/nuxtjs.csv",
        "data/stacks/react-native.csv",
        "data/stacks/react.csv",
        "data/stacks/shadcn.csv",
        "data/stacks/svelte.csv",
        "data/stacks/swiftui.csv",
        "data/stacks/threejs.csv",
        "data/stacks/uno.csv",
        "data/stacks/uwp.csv",
        "data/stacks/vue.csv",
        "data/stacks/winui.csv",
        "data/stacks/wpf.csv",
        "data/styles.csv",
        "data/typography.csv",
        "data/ui-reasoning.csv",
        "data/ux-guidelines.csv",
        "references/pro-rules.md",
        "references/quick-reference.md",
        "scripts/core.py",
        "scripts/design_system.py",
        "scripts/reasoning_contract.py",
        "scripts/search.py",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.file_name().is_some_and(|n| n != ".DS_Store")
                && !p.to_string_lossy().contains("__pycache__")
            {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }

    /// A file added to the folder and not to the table ships nothing; one in the table and not
    /// the folder does not compile. This is the first half.
    #[test]
    fn the_table_is_the_directory() {
        for v in catalog() {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("skills")
                .join(v.id);
            let mut on_disk = Vec::new();
            walk(&base, &base, &mut on_disk);
            on_disk.sort();
            let mut table: Vec<String> = v.files.iter().map(|(p, _)| p.to_string()).collect();
            table.sort();
            assert_eq!(table, on_disk, "{}", v.id);
        }
    }

    /// In a project, `${CLAUDE_PLUGIN_ROOT}` is unset and every command in the skill would fail.
    /// Every script the skill tells Claude to run must be one it ships.
    #[test]
    fn runs_from_a_project_root() {
        for v in catalog() {
            let (_, md) = v.files.iter().find(|(p, _)| *p == "SKILL.md").unwrap();
            assert!(md.starts_with(&format!("---\nname: {}\n", v.id)));
            assert!(!md.contains("CLAUDE_PLUGIN_ROOT"), "{}", v.id);
            let prefix = format!(".claude/skills/{}/", v.id);
            for (i, _) in md.match_indices(&prefix) {
                let rest = &md[i + prefix.len()..];
                let path: String = rest
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '`')
                    .collect();
                assert!(
                    v.files.iter().any(|(p, _)| *p == path),
                    "SKILL.md names {path}, which is not shipped"
                );
            }
        }
    }

    #[test]
    fn ships_its_licence_and_stays_in_its_folder() {
        for v in catalog() {
            let (_, lic) = v.files.iter().find(|(p, _)| *p == "LICENSE").unwrap();
            assert!(lic.contains("MIT") && lic.contains(v.author), "{}", v.id);
            for (p, _) in v.files {
                assert!(
                    !p.starts_with('/') && !p.split('/').any(|c| c == ".." || c.is_empty()),
                    "{p} climbs out of the skill folder"
                );
            }
        }
    }
}
