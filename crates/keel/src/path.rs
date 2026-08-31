//! Finding the tools a shell would find.
//!
//! An application launched from the Dock inherits launchd's `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin`
//! — and never reads a shell profile. So `claude`, installed to `~/.local/bin` by its own native
//! installer, is not there. Neither is Homebrew at `/opt/homebrew/bin`, nor bun, nor cargo.
//!
//! The symptom is worse than "not found", because it is inconsistent: Keel reads `~/.claude.json`
//! directly and reports the signed-in account correctly, then fails to run the binary and says it
//! is not installed. "You are logged in as you@example.com" directly above "not found on your
//! PATH" reads as a bug in Keel, which is fair, because it is one.
//!
//! Every GUI developer tool has this problem and they all solve it the same way: ask the login
//! shell what its `PATH` is, once, at startup.

use std::collections::BTreeSet;

/// Directories worth having whether or not the shell answers.
///
/// The union is deliberate. A shell that fails, hangs, or has a profile that never sets `PATH`
/// should leave Keel no worse off than a hardcoded list, and a shell that answers should be able
/// to add to it rather than replace it.
fn well_known() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        // Where `claude`'s own installer puts it.
        format!("{home}/.local/bin"),
        format!("{home}/.bun/bin"),
        format!("{home}/.cargo/bin"),
        format!("{home}/.deno/bin"),
        format!("{home}/.volta/bin"),
        format!("{home}/go/bin"),
        // Homebrew, both architectures.
        "/opt/homebrew/bin".into(),
        "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        "/usr/local/sbin".into(),
        // Where gcloud's installer lands by default.
        format!("{home}/google-cloud-sdk/bin"),
    ]
    .into_iter()
    .filter(|p| std::path::Path::new(p).is_dir())
    .collect()
}

/// Ask the user's login shell what `PATH` it has.
///
/// `-l` reads the profile files, `-i` reads the interactive ones — and people put `PATH` in both,
/// so neither alone is enough. Bounded, because an interactive shell runs somebody's whole
/// `.zshrc`, which can be slow and can block on anything from a version manager to a prompt that
/// asks a network for something.
fn shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    if shell.is_empty() {
        return None;
    }

    let mut child = std::process::Command::new(&shell)
        .args(["-lic", "printf %s \"$PATH\""])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Poll rather than block: `wait_timeout` is a dependency and this is the only place that would
    // need it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }

    let out = child.wait_with_output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout);
    // A profile that prints a banner leaves it on stdout ahead of the answer. The PATH is the last
    // line with a separator in it.
    path.lines()
        .rev()
        .find(|l| l.contains('/') && l.contains(':'))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
}

/// Merge the shell's `PATH`, the well-known locations, and whatever we already had.
///
/// Order is preserved and duplicates dropped, so a tool the user shadowed deliberately stays
/// shadowed — appending our guesses after their `PATH` rather than in front of it.
pub fn adopt_shell_path() {
    let mut seen = BTreeSet::new();
    let mut parts: Vec<String> = Vec::new();

    let mut push = |dir: &str| {
        if !dir.is_empty() && seen.insert(dir.to_string()) {
            parts.push(dir.to_string());
        }
    };

    if let Some(from_shell) = shell_path() {
        for dir in from_shell.split(':') {
            push(dir);
        }
    }
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        push(dir);
    }
    for dir in well_known() {
        push(&dir);
    }

    if !parts.is_empty() {
        // Safety: called once, at the top of main, before any thread is spawned.
        unsafe { std::env::set_var("PATH", parts.join(":")) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No test sets an environment variable.
    ///
    /// `cargo test` is one process and a thread pool; an environment variable is process-global.
    /// A test that sets one is setting it for whatever else is running at that instant, and Rust
    /// 2024 made `set_var` `unsafe` because of exactly that.
    ///
    /// It cost a red CI on a release commit. `connect::tests` set `HOME=/Users/x` to check tilde
    /// expansion; on Linux the `trash` crate resolves `$HOME/.local/share/Trash`, so whichever
    /// test happened to be discarding files at that moment tried to write into `/Users/x` and
    /// failed with `PermissionDenied`. It passed on macOS, where the trash does not consult
    /// `HOME` — so `make check` was green on the machine it runs on and the failure only ever
    /// appeared in CI, and only when the scheduling lined up.
    ///
    /// The fix is never a lock around the mutation; it is a function that takes the value.
    ///
    /// The one legitimate call is in this file: `augment_path`, once, at the top of `main`,
    /// before any thread exists. Production code with the same justification may say so with
    /// `// set_var: <reason>` on the line above.
    #[test]
    fn no_test_mutates_the_environment() {
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
            .expect("the crate's own sources")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let Some(tests) = source.find("#[cfg(test)]") else {
                continue;
            };
            // Code, not prose — this test's own explanation names the call it forbids — and the
            // needle is assembled rather than written, because otherwise this line is a hit.
            let needle = concat!("set_", "var(");
            let calls_it = source[tests..]
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains(needle));
            if calls_it {
                offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        assert!(
            offenders.is_empty(),
            "these files set an environment variable from a test: {offenders:?}. \
             Pass the value to the function instead — the variable is shared with every other \
             test running at the same moment."
        );
    }

    /// Whatever else happens, the places these tools actually install to have to be reachable.
    #[test]
    fn the_search_path_covers_where_things_install() {
        adopt_shell_path();
        let path = std::env::var("PATH").unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();

        for dir in [format!("{home}/.local/bin"), "/opt/homebrew/bin".into()] {
            if std::path::Path::new(&dir).is_dir() {
                assert!(
                    path.split(':').any(|p| p == dir),
                    "{dir} exists but is not on the search path"
                );
            }
        }
    }

    /// Running it twice must not double the path, or a relaunch loop grows it without bound.
    #[test]
    fn adopting_twice_changes_nothing() {
        adopt_shell_path();
        let once = std::env::var("PATH").unwrap_or_default();
        adopt_shell_path();
        assert_eq!(once, std::env::var("PATH").unwrap_or_default());
    }
}
