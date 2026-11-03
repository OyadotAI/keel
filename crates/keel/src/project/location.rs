//! Creation and cloning share a lazily materialized cross-platform destination.
use camino::{Utf8Path, Utf8PathBuf};
const NAME_ATTEMPTS: usize = 16;
const RANDOM_BYTES: usize = 6;

pub(super) fn default_parent() -> Result<Utf8PathBuf, String> {
    Utf8PathBuf::from_path_buf(keel_workspace::locations::projects()?)
        .map_err(|_| "The workspace path is not text.".into())
}
pub(crate) fn parent(value: Option<&str>) -> Result<Utf8PathBuf, String> {
    let default = default_parent()?;
    let path = match value.filter(|s| !s.trim().is_empty()) {
        Some(value) => Utf8PathBuf::from(super::expand(value)),
        None => default.clone(),
    };
    if path == default {
        std::fs::create_dir_all(&path).map_err(|e|format!("Keel couldn't create a project in {path}: {e}. Check folder permissions or choose another location."))?;
    }
    if !path.is_dir() {
        return Err(format!(
            "No such project location: {path}. Choose another location."
        ));
    }
    Ok(path)
}
pub(super) fn reserve(parent: &Utf8Path, name: &str) -> Result<(String, Utf8PathBuf), String> {
    let explicit = name.trim();
    if !explicit.is_empty() && !keel_generator::cloudflare::valid_name(explicit) {
        return Err("Use lowercase letters, digits and hyphens, up to 41 characters.".into());
    }
    for _ in 0..NAME_ATTEMPTS {
        let name = if explicit.is_empty() {
            automatic()?
        } else {
            explicit.to_string()
        };
        let root = parent.join(&name);
        match std::fs::create_dir(&root) {
            Ok(()) => return Ok((name, root)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && explicit.is_empty() => {
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!(
                    "A folder named '{name}' already exists here. Choose another name or location."
                ));
            }
            Err(e) => {
                return Err(format!(
                    "Keel couldn't create a project at {root}: {e}. Check folder permissions or choose another location."
                ));
            }
        }
    }
    Err("Could not reserve a unique project folder. Try again.".into())
}
fn automatic() -> Result<String, String> {
    let mut random = [0; RANDOM_BYTES];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    Ok(format!(
        "project-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_unnamed_creations_reserve_separate_folders() {
        let temp = tempfile::tempdir().unwrap();
        let parent = Utf8Path::from_path(temp.path()).unwrap();
        let roots = std::thread::scope(|s| {
            let handles = (0..8)
                .map(|_| s.spawn(|| reserve(parent, "  ").unwrap().1))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        });
        assert_eq!(roots.len(), 8);
        for root in roots {
            assert!(root.is_dir());
        }
    }
    #[test]
    fn explicit_collisions_never_reuse_or_overwrite_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let parent = Utf8Path::from_path(temp.path()).unwrap();
        let (_, root) = reserve(parent, "demo").unwrap();
        std::fs::write(root.join("keep"), "mine").unwrap();
        assert!(reserve(parent, "demo").is_err());
        assert_eq!(std::fs::read_to_string(root.join("keep")).unwrap(), "mine");
        assert!(reserve(parent, "../escape").is_err());
    }
}
