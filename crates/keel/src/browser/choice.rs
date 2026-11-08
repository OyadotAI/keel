//! Where a lane browses, decided at the browser call that needs it rather than at launch.
//!
//! Each lane remembers Inside Keel or the person's own Oya Browser (`preference::resolve`). The
//! first time an agent in a lane with no choice reaches for a browser, its call is held on a card
//! (`approve::choose_browser`) and the answer is saved to the lane and used for that same call.
//! Every agent's `browser_*` call arrives through `browser::call`, so this one place covers Claude
//! Code, Codex and Antigravity, in chat and terminal lanes. When Oya Browser is not installed the
//! card offers to install it (`install.rs`), and Keel never switches browsers on its own.

use super::{
    bindings::Binding,
    detection,
    preference::{self, Backend},
};
use crate::lock::Locked;
use camino::Utf8Path;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long a call that asked for Oya to be installed waits for it before giving up. The card has
/// already used up to `approve::WAIT` of the agents' 300 s tool budget; the install carries on
/// after the call gives up, and the lane's next browser step uses it.
const INSTALL_WAIT: Duration = Duration::from_secs(45);

/// What this lane is doing about its browser right now, so a second call cannot ask or install
/// over the first. `asking` lasts as long as the card; `installing` as long as the installer.
fn busy() -> &'static Mutex<HashMap<String, &'static str>> {
    static B: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    B.get_or_init(Default::default)
}
#[derive(Debug)]
struct Busy(String);
impl Busy {
    fn take(lane: &str, what: &'static str) -> Result<Self, String> {
        let mut busy = busy().locked();
        match busy.get(lane) {
            Some(&"installing") => Err(STILL_INSTALLING.into()),
            Some(_) => Err("A browser choice is already being asked for this lane. Wait for the person to answer, then try again.".into()),
            None => {
                busy.insert(lane.into(), what);
                Ok(Busy(lane.into()))
            }
        }
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        busy().locked().remove(&self.0);
    }
}

const STILL_INSTALLING: &str =
    "Oya Browser is still installing. The next browser step will use it once it is ready.";
const UNANSWERED: &str = "Nobody chose a browser for this lane, so the browser step didn't run.";

/// Whether Oya Browser is installed on this machine. Blocking: detection runs a process.
pub(crate) fn oya_installed() -> bool {
    detection::installed().ok().flatten().is_some()
}

/// Whether detection positively found no Oya. A detection that failed — PowerShell slow to start,
/// say — is not evidence of absence, and must never set off a download.
fn oya_absent() -> bool {
    matches!(detection::installed(), Ok(None))
}

/// What to hand the system's opener to reach the installed Oya: the `.app` bundle on macOS, the
/// executable on Windows. Blocking. `None` when Oya is not installed.
pub(crate) fn oya_app() -> Option<String> {
    let installation = detection::installed().ok().flatten()?;
    Some(if cfg!(windows) {
        installation.executable
    } else {
        installation.application
    })
}

/// The browser this call runs in: the lane's saved choice, or the person's answer to the card —
/// which is saved, so they are asked once per lane. `Err` is what the agent reads.
pub(super) async fn choose(
    project: &Utf8Path,
    lane: &str,
    bound: &Binding,
) -> Result<Backend, String> {
    let (p, l) = (project.to_owned(), lane.to_string());
    let (own, resolved) = crate::serve::in_blocking(move || {
        (
            preference::lane_only(&p, &l).ok().flatten(),
            preference::resolve(&p, &l),
        )
    })
    .await;
    match resolved? {
        // This lane chose Oya in its Browser tab before Oya was installed ("installs on next step"):
        // install it now, as the card would have. Only the lane's own choice — never the device's
        // "Always Oya Browser", which would download unasked in every lane — only while not
        // connected, so a working lane never re-checks, and only when Oya is positively absent.
        Some(Backend::LocalOya)
            if own == Some(Backend::LocalOya)
                && !bound.page.try_lock().is_ok_and(|page| page.connected())
                && crate::serve::blocking(oya_absent, false).await =>
        {
            super::some_window(crate::events::windows())?;
            let installed = install_oya(project, lane).await;
            // A failure here would otherwise download again on every step: forget the lane's Oya
            // choice so the next step asks, as a failed install from the card does.
            if installed.as_ref().is_err_and(|why| why != STILL_INSTALLING) {
                let (p, l) = (project.to_owned(), lane.to_string());
                let _ = crate::serve::in_blocking(move || preference::clear_lane(&p, &l)).await;
            }
            return installed;
        }
        Some(backend) => return Ok(backend),
        None => {}
    }
    super::some_window(crate::events::windows())?;
    let asking = Busy::take(lane, "asking")?;
    let installed = crate::serve::blocking(oya_installed, false).await;
    let session = bound.session.locked().clone();
    let oya = if installed {
        "installed"
    } else {
        "installable"
    };
    let answer = crate::approve::choose_browser(lane, &session, oya)
        .await
        .ok_or(UNANSWERED)?;
    drop(asking);
    let backend = match answer.as_str() {
        "embedded" => Backend::Embedded,
        "local_oya" | "install_oya" if installed => Backend::LocalOya,
        "local_oya" | "install_oya" => return install_oya(project, lane).await,
        other => {
            return Err(format!(
                "Keel didn't understand the browser choice {other:?}."
            ));
        }
    };
    save(project, lane, backend).await?;
    Ok(backend)
}

async fn save(project: &Utf8Path, lane: &str, backend: Backend) -> Result<(), String> {
    let (p, l) = (project.to_owned(), lane.to_string());
    crate::serve::in_blocking(move || preference::set_lane(&p, &l, backend)).await?;
    super::bindings::changed(lane);
    Ok(())
}

/// Install Oya Browser for this lane and, once it is there, make it the lane's browser and open it
/// so it is ready to be driven. The install belongs to the lane, not to the call that asked: it
/// keeps going if the call gives up after [`INSTALL_WAIT`], and stops only on its own ceiling or
/// when every Keel window has closed. A failed install saves nothing, so the next step asks again.
async fn install_oya(project: &Utf8Path, lane: &str) -> Result<Backend, String> {
    let installing = Busy::take(lane, "installing")?;
    let (project, lane) = (project.to_owned(), lane.to_string());
    let task = tokio::task::spawn_blocking(move || {
        let _installing = installing;
        let say = |line: &str| {
            crate::events::emit(
                "browser.install",
                None,
                serde_json::json!({ "lane": lane, "line": line }),
            )
        };
        let result = crate::install::run("oya", say, || crate::events::windows() == 0)
            .and_then(|()| preference::set_lane(&project, &lane, Backend::LocalOya))
            .map(|()| launch());
        let error = result.as_ref().err().cloned();
        crate::events::emit(
            "browser.install",
            None,
            serde_json::json!({ "lane": lane, "done": true, "error": error }),
        );
        super::bindings::changed(&lane);
        result
    });
    match tokio::time::timeout(INSTALL_WAIT, task).await {
        Ok(Ok(Ok(()))) => Ok(Backend::LocalOya),
        Ok(Ok(Err(why))) => Err(format!("Couldn't install Oya Browser: {why}.")),
        Ok(Err(_)) => Err("The Oya Browser install was interrupted.".into()),
        Err(_) => Err(STILL_INSTALLING.into()),
    }
}

/// Open a freshly installed Oya once: Keel drives it through the connection a running Oya offers,
/// and nothing else would start it. Best effort — the next call's own connection error says so.
fn launch() {
    // The app bundle on macOS, the executable on Windows — opening the install folder there would
    // show it in Explorer and start nothing.
    if let Some(app) = oya_app() {
        let _ = open::that_detached(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One question or install per lane at a time; another lane is unaffected; the slot is freed.
    #[test]
    fn a_second_first_call_in_a_lane_is_refused_not_asked_twice() {
        let first = Busy::take("busy-lane-a", "asking").unwrap();
        assert!(
            Busy::take("busy-lane-a", "asking")
                .unwrap_err()
                .contains("already being asked")
        );
        assert!(
            Busy::take("busy-lane-b", "asking").is_ok(),
            "another lane asks freely"
        );
        drop(first);
        let installing = Busy::take("busy-lane-a", "installing").unwrap();
        assert_eq!(
            Busy::take("busy-lane-a", "asking").unwrap_err(),
            STILL_INSTALLING
        );
        drop(installing);
        assert!(Busy::take("busy-lane-a", "asking").is_ok());
    }
}
