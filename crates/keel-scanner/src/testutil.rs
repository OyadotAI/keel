//! Test-only helpers shared by the check modules.

use crate::RepoContext;
use camino::Utf8PathBuf;
use tempfile::TempDir;

/// Build a throwaway repo on disk from `(path, contents)` pairs and load a context over it.
///
/// The `TempDir` is returned alongside the context and must be held for the duration of the test —
/// dropping it deletes the fixture out from under the checks.
pub fn fixture(files: &[(&str, &str)]) -> (TempDir, RepoContext) {
    let dir = TempDir::new().expect("tempdir");
    let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 tempdir");
    for (path, contents) in files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().expect("has parent")).expect("mkdir");
        std::fs::write(&full, contents).expect("write");
    }
    let ctx = RepoContext::load(&root).expect("load");
    (dir, ctx)
}
