//! One home-relative workspace policy is shared by the daemon and native desktop shell.
use std::path::{Path, PathBuf};

/// KEEL_HOME redirects Keel lookups for isolated QA; subprocess HOME remains unchanged.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("KEEL_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(std::env::home_dir)
}
/// Resolve without creating folders; opening or cancelling a form must leave the disk untouched.
pub fn projects() -> Result<PathBuf, String> {
    from_home(home().as_deref())
}
/// Injectable home resolution keeps scratch tests independent of the user's real project directory.
pub fn from_home(home: Option<&Path>) -> Result<PathBuf, String> {
    let home = home
        .filter(|p| p.is_absolute())
        .ok_or("No absolute home directory is available. Choose a project location.")?;
    Ok(home.join("Dev").join("KeelWorkspace"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolving_a_default_location_does_not_create_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = from_home(Some(temp.path())).unwrap();
        assert_eq!(root, temp.path().join("Dev").join("KeelWorkspace"));
        assert!(!root.exists());
        assert!(from_home(None).is_err());
        assert!(from_home(Some(Path::new("relative"))).is_err());
    }
}
