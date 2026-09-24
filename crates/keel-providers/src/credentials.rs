//! Credential storage.
//!
//! Tokens live in the OS keychain and never leave the machine. This is the whole reason Keel is a
//! local binary rather than a hosted service: a developer's GitHub token and Cloudflare API token
//! are the keys to their production infrastructure, and nothing about this product needs them to
//! travel anywhere.

use keyring::Entry;

const SERVICE: &str = "dev.keel";

/// Which credential is being stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    GitHub,
    Cloudflare,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::GitHub => "github-token",
            Kind::Cloudflare => "cloudflare-token",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::GitHub => "GitHub",
            Kind::Cloudflare => "Cloudflare",
        }
    }
}

fn entry(kind: Kind) -> Result<Entry, String> {
    Entry::new(SERVICE, kind.key()).map_err(|e| e.to_string())
}

pub fn store(kind: Kind, token: &str) -> Result<(), String> {
    entry(kind)?.set_password(token).map_err(|e| e.to_string())
}

/// Read a stored token.
///
/// A missing entry is `None` rather than an error: not having connected yet is the normal state, and
/// callers should not have to distinguish it from a keychain failure.
pub fn load(kind: Kind) -> Option<String> {
    entry(kind).ok()?.get_password().ok()
}

pub fn clear(kind: Kind) -> Result<(), String> {
    match entry(kind)?.delete_credential() {
        Ok(()) => Ok(()),
        // Disconnecting something that was never connected is a no-op, not a failure.
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Fall back to the `gh` CLI's own token.
///
/// Most developers with a GitHub account already have `gh` authenticated, and asking them to mint a
/// personal access token when a working credential is already on the machine is friction for its own
/// sake. Keel never stores this one — it reads it per call, so revoking `gh` revokes Keel.
pub fn github_from_gh_cli() -> Option<String> {
    use std::io::Read;
    // Bounded, and killed past the bound: a `gh` waiting on a keychain prompt or a network it
    // cannot reach held the connections panel with no end. Its output is one token, so polling
    // cannot fill a pipe.
    const CEILING: std::time::Duration = std::time::Duration::from_secs(10);
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("gh")
        .args(["auth", "token"])
        // Its own group, so a timeout ends whatever `gh` started as well.
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() > CEILING {
            // Safety: the group this process spawned `gh` as the leader of, negated to address it.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    status
        .success()
        .then(|| out.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The GitHub token to use: an explicitly connected one first, then `gh`.
pub fn github_token() -> Option<String> {
    load(Kind::GitHub).or_else(github_from_gh_cli)
}
