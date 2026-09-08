use crate::{Scope, frontmatter};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

/// A skill available to the agent in this repository.
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub scope: Scope,
    pub path: Utf8PathBuf,
}

/// Every skill visible from `repo`, project scope first.
///
/// Project skills are listed ahead of user skills because they are the ones that arrived with the
/// repository — the ones worth looking at before trusting a checkout.
pub fn discover_skills(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Skill> {
    let mut skills = read_skill_dir(&repo.join(".claude").join("skills"), Scope::Project);
    skills.extend(read_skill_dir(&claude_home.join("skills"), Scope::User));
    skills
}

/// Read a directory of `<name>/SKILL.md` entries.
fn read_skill_dir(dir: &Utf8Path, scope: Scope) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut skills: Vec<Skill> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.is_dir())
        .filter_map(|dir| {
            let manifest = dir.join("SKILL.md");
            let contents = std::fs::read_to_string(&manifest).ok()?;
            let (name, description) = frontmatter(&contents);
            Some(Skill {
                // Fall back to the directory name: a skill with malformed frontmatter still exists
                // and still loads, so hiding it would misrepresent what the agent can do.
                name: name.unwrap_or_else(|| dir.file_name().unwrap_or("unknown").to_string()),
                description,
                scope,
                path: manifest,
            })
        })
        .collect();

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_skills_from_both_scopes_with_project_first() {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        let repo = root.join("repo");
        let home = root.join("home");

        std::fs::create_dir_all(repo.join(".claude/skills/deploy")).unwrap();
        std::fs::write(
            repo.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Ships it\n---\n",
        )
        .unwrap();

        std::fs::create_dir_all(home.join("skills/video-gen")).unwrap();
        std::fs::write(
            home.join("skills/video-gen/SKILL.md"),
            "---\nname: video-gen\ndescription: Makes videos\n---\n",
        )
        .unwrap();

        let skills = discover_skills(&repo, &home);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(skills[0].scope, Scope::Project);
        assert_eq!(skills[1].scope, Scope::User);
    }

    #[test]
    fn a_skill_with_broken_frontmatter_still_appears() {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::create_dir_all(root.join(".claude/skills/mystery")).unwrap();
        std::fs::write(
            root.join(".claude/skills/mystery/SKILL.md"),
            "# no frontmatter",
        )
        .unwrap();

        let skills = discover_skills(&root, Utf8Path::new("/nonexistent"));
        assert_eq!(
            skills.len(),
            1,
            "it still loads, so it must still be listed"
        );
        assert_eq!(skills[0].name, "mystery");
        assert!(skills[0].description.is_none());
    }
}
