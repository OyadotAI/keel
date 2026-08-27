use crate::{Scope, frontmatter};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

/// A subagent definition.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub name: String,
    pub description: Option<String>,
    pub scope: Scope,
    pub path: Utf8PathBuf,
}

/// A custom slash command.
#[derive(Debug, Clone, Serialize)]
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub scope: Scope,
    pub path: Utf8PathBuf,
}

pub fn discover_agents(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Agent> {
    let mut agents = read_markdown_dir(&repo.join(".claude").join("agents"), Scope::Project);
    agents.extend(read_markdown_dir(&claude_home.join("agents"), Scope::User));
    agents
        .into_iter()
        .map(|(name, description, scope, path)| Agent {
            name,
            description,
            scope,
            path,
        })
        .collect()
}

pub fn discover_commands(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Command> {
    let mut commands = read_markdown_dir(&repo.join(".claude").join("commands"), Scope::Project);
    commands.extend(read_markdown_dir(
        &claude_home.join("commands"),
        Scope::User,
    ));
    commands
        .into_iter()
        .map(|(name, description, scope, path)| Command {
            name,
            description,
            scope,
            path,
        })
        .collect()
}

/// Read a flat directory of `<name>.md` definitions.
///
/// Agents and commands share a layout, so they share a reader; the two public wrappers exist
/// because they are different things to the user even though they parse identically.
fn read_markdown_dir(
    dir: &Utf8Path,
    scope: Scope,
) -> Vec<(String, Option<String>, Scope, Utf8PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut items: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.extension() == Some("md"))
        .filter_map(|path| {
            let contents = std::fs::read_to_string(&path).ok()?;
            let (name, description) = frontmatter(&contents);
            let name = name.unwrap_or_else(|| path.file_stem().unwrap_or("unknown").to_string());
            Some((name, description, scope, path))
        })
        .collect();

    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_project_agents() {
        let dir = TempDir::new().expect("tempdir");
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::create_dir_all(repo.join(".claude/agents")).unwrap();
        std::fs::write(
            repo.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews diffs\n---\n",
        )
        .unwrap();

        let agents = discover_agents(&repo, Utf8Path::new("/nonexistent"));
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "reviewer");
        assert_eq!(agents[0].scope, Scope::Project);
    }

    #[test]
    fn a_missing_directory_yields_nothing_rather_than_failing() {
        assert!(discover_agents(Utf8Path::new("/nope"), Utf8Path::new("/nope")).is_empty());
        assert!(discover_commands(Utf8Path::new("/nope"), Utf8Path::new("/nope")).is_empty());
    }
}
