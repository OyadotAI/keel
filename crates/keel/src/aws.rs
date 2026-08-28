//! Setting up an AWS profile for IAM Identity Center.
//!
//! `aws configure sso` is the documented path and it is interactive — it "interactively prompts",
//! in its own help text — so Keel cannot drive it. But it is only a wizard around config that can
//! be written directly: `aws configure set` writes profile keys non-interactively, an `[sso-session]`
//! block is plain INI, and `aws sso login --sso-session <name>` reads whatever is there.
//!
//! Verified before building on it: with a hand-written session block, `aws sso login` got all the
//! way to `StartDeviceAuthorization` and failed only on the fake start URL. A session that does
//! not exist is rejected by name, so "not configured" and "the network failed" stay distinct.
//!
//! What this does not do is ask for an access key. Without Identity Center that is the only other
//! way in, and a long-lived AWS key is not a thing to type into an IDE — Keel hosts `aws configure`
//! in its own terminal instead, where the key goes to the CLI's stdin and never through Keel.

use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};

/// The session block Keel writes. One name, so re-running setup updates rather than accumulates.
const SESSION: &str = "keel";

#[derive(Deserialize)]
pub struct SsoSetup {
    /// e.g. `https://d-1234567890.awsapps.com/start`
    pub start_url: String,
    /// The region Identity Center itself lives in, which is not necessarily where you deploy.
    pub sso_region: String,
    pub account: String,
    pub role: String,
    /// The profile to create. Defaults to `keel`.
    #[serde(default)]
    pub profile: String,
    /// The region for the profile's own API calls.
    #[serde(default)]
    pub region: String,
}

#[derive(Serialize)]
pub struct Configured {
    pub profile: String,
}

fn bad(m: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, m.to_string())
}

/// A value that is safe as an INI value and as a CLI argument.
fn plain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && !value.contains(['\n', '\r', '[', ']', '"'])
        && !value.starts_with('-')
}

fn set(profile: &str, key: &str, value: &str) -> Result<(), String> {
    let ok = std::process::Command::new("aws")
        .args(["configure", "set", key, value, "--profile", profile])
        .output()
        .map_err(|e| e.to_string())?
        .status
        .success();
    ok.then_some(())
        .ok_or_else(|| format!("`aws configure set {key}` failed"))
}

pub async fn configure_sso(
    Json(req): Json<SsoSetup>,
) -> Result<Json<Configured>, (StatusCode, String)> {
    if !req.start_url.starts_with("https://") {
        return Err(bad(
            "The start URL is the https:// address your organisation gave you.",
        ));
    }
    for (label, value) in [
        ("start URL", &req.start_url),
        ("Identity Center region", &req.sso_region),
        ("account", &req.account),
        ("role", &req.role),
    ] {
        if !plain(value) {
            return Err(bad(format!("That {label} does not look right.")));
        }
    }
    if !req.account.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad("An AWS account id is twelve digits."));
    }

    let profile = if req.profile.trim().is_empty() {
        "keel".to_string()
    } else {
        req.profile.trim().to_string()
    };
    if !profile
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(bad(
            "A profile name is letters, digits, hyphens and underscores.",
        ));
    }

    // The session block first: a profile pointing at a session that does not exist is refused by
    // `aws sso login` with a message about the session, which reads like the wrong failure.
    write_session(&req).map_err(bad)?;

    set(&profile, "sso_session", SESSION).map_err(bad)?;
    set(&profile, "sso_account_id", &req.account).map_err(bad)?;
    set(&profile, "sso_role_name", &req.role).map_err(bad)?;
    let region = if req.region.trim().is_empty() {
        req.sso_region.clone()
    } else {
        req.region.trim().to_string()
    };
    set(&profile, "region", &region).map_err(bad)?;

    Ok(Json(Configured { profile }))
}

/// Append or replace `[sso-session keel]` in `~/.aws/config`.
///
/// Written by hand because `aws configure set` writes profile sections only. Everything else in
/// the file is left exactly as it was — this is somebody's existing AWS config, and Keel adding a
/// session to it must not be a reason for anything else in it to change.
fn write_session(req: &SsoSetup) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no home directory")?;
    let dir = std::path::Path::new(&home).join(".aws");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("config");

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let header = format!("[sso-session {SESSION}]");

    let block = format!(
        "{header}\nsso_start_url = {}\nsso_region = {}\nsso_registration_scopes = sso:account:access\n",
        req.start_url.trim(),
        req.sso_region.trim()
    );

    let updated = match existing.find(&header) {
        None => {
            let mut out = existing;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&block);
            out
        }
        Some(at) => {
            // Replace just this section: from its header to the next one, or the end.
            let after = at + header.len();
            let end = existing[after..]
                .find("\n[")
                .map(|i| after + i + 1)
                .unwrap_or(existing.len());
            format!("{}{block}{}", &existing[..at], &existing[end..])
        }
    };

    std::fs::write(&path, updated).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_cannot_carry_ini_structure() {
        assert!(plain("https://d-123.awsapps.com/start"));
        assert!(plain("PowerUserAccess"));
        assert!(!plain("x\n[sso-session evil]"));
        assert!(!plain("has \"quotes\""));
        assert!(!plain("--profile"));
        assert!(!plain(""));
    }

    /// Re-running setup replaces the session rather than stacking a second copy of it, and leaves
    /// everything else in someone's config untouched.
    #[test]
    fn the_session_block_is_replaced_not_appended() {
        let first = SsoSetup {
            start_url: "https://a.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            account: "1".into(),
            role: "r".into(),
            profile: String::new(),
            region: String::new(),
        };

        // Simulate the file surgery without touching $HOME.
        let existing = "[profile mine]\nregion = eu-west-1\n\n[sso-session keel]\n\
                        sso_start_url = https://old.awsapps.com/start\nsso_region = eu-west-1\n\n\
                        [profile other]\nregion = us-east-2\n";
        let header = "[sso-session keel]";
        let block = format!(
            "{header}\nsso_start_url = {}\nsso_region = {}\nsso_registration_scopes = sso:account:access\n",
            first.start_url, first.sso_region
        );
        let at = existing.find(header).unwrap();
        let after = at + header.len();
        let end = existing[after..]
            .find("\n[")
            .map(|i| after + i + 1)
            .unwrap_or(existing.len());
        let out = format!("{}{block}{}", &existing[..at], &existing[end..]);

        assert_eq!(out.matches(header).count(), 1, "one session block, not two");
        assert!(out.contains("https://a.awsapps.com/start"), "the new url");
        assert!(!out.contains("old.awsapps.com"), "and not the old one");
        assert!(out.contains("[profile mine]"), "other sections survive");
        assert!(
            out.contains("[profile other]"),
            "including the one after it"
        );
    }
}
