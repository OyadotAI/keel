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

/// GET /api/license — the day's meter, for the window.
pub async fn api_status() -> axum::Json<Option<keel_license::Status>> {
    axum::Json(crate::serve::in_blocking(|| dir().map(|d| keel_license::status(&d))).await)
}

/// The daily usage report, for the life of the daemon.
pub async fn report_daily() {
    // no-blocking: keel-license reads its files in `spawn_blocking`; this only hands it the path.
    if let Some(d) = dir() {
        keel_license::report_daily(d, env!("CARGO_PKG_VERSION")).await;
    }
}
