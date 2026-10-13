//! The license meter, as the daemon reaches it. The work is `keel-license`'s (see LICENSE.md);
//! this only says where Keel's directory is. Under test there is none, so nothing in the
//! daemon's own tests is ever metered — the meter's tests are in its crate.

use camino::Utf8PathBuf;

fn dir() -> Option<Utf8PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::prefs::dir()
}

/// Count one agent step, or the refusal to hand the agent. No directory, no meter.
pub fn step() -> Result<(), String> {
    dir().map_or(Ok(()), |d| keel_license::step(&d))
}

/// The day's meter, plus the key's last valid day for the status bar.
#[derive(serde::Serialize)]
pub struct Meter {
    #[serde(flatten)]
    status: keel_license::Status,
    expires: Option<String>,
}

fn meter(dir: &camino::Utf8Path) -> Meter {
    Meter {
        status: keel_license::status(dir),
        expires: keel_license::license(dir).map(|l| l.expires),
    }
}

/// GET /api/license — the day's meter, for the window.
pub async fn api_status() -> axum::Json<Option<Meter>> {
    axum::Json(crate::serve::in_blocking(|| dir().map(|d| meter(&d))).await)
}

/// The daily usage report, for the life of the daemon.
pub async fn report_daily() {
    // no-blocking: keel-license reads its files in `spawn_blocking`; this only hands it the path.
    if let Some(d) = dir() {
        keel_license::report_daily(d, env!("CARGO_PKG_VERSION")).await;
    }
}

#[derive(serde::Deserialize)]
pub struct Install {
    key: String,
}

/// POST /api/license — save a pasted key as `license.key`, only once it is known to verify.
/// A key that does not verify is refused with the reason and the old file is left alone.
pub async fn api_install(
    axum::Json(body): axum::Json<Install>,
) -> Result<axum::Json<Meter>, (axum::http::StatusCode, String)> {
    crate::serve::in_blocking(move || install(dir(), body.key.trim())).await
}

fn install(
    dir: Option<Utf8PathBuf>,
    key: &str,
) -> Result<axum::Json<Meter>, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    // The variable wins over the file, so saving would show a license that is not the one in force.
    if std::env::var("KEEL_LICENSE_KEY").is_ok_and(|k| !k.trim().is_empty()) {
        return Err(bad(
            "KEEL_LICENSE_KEY is set in Keel's environment and takes precedence over a saved key. Unset it and restart Keel.",
        ));
    }
    let dir = dir.ok_or_else(|| bad("Keel has no home directory to save the key in."))?;
    // Checked in a folder of its own first, so a bad paste never replaces a working key.
    let held = tempfile::tempdir().map_err(|e| bad(&e.to_string()))?;
    let scratch =
        Utf8PathBuf::try_from(held.path().to_path_buf()).map_err(|e| bad(&e.to_string()))?;
    std::fs::write(scratch.join("license.key"), key).map_err(|e| bad(&e.to_string()))?;
    if keel_license::license(&scratch).is_none() {
        return Err(bad(
            "That is not a valid Keel license key, or it has expired.",
        ));
    }
    crate::writes::replace(dir.join("license.key").as_std_path(), key.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(axum::Json(meter(&dir)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_key_that_does_not_verify_leaves_the_saved_one_alone() {
        let home = tempfile::tempdir().unwrap();
        let dir = camino::Utf8PathBuf::try_from(home.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("license.key"), "keel1.old.key").unwrap();
        let refused = super::install(Some(dir.clone()), "keel1.not.signed");
        assert_eq!(
            refused.err().unwrap().0,
            axum::http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("license.key")).unwrap(),
            "keel1.old.key"
        );
    }
}
