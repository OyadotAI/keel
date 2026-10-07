//! Letting a phone reach this Keel, and nothing else.
//!
//! Keel has always bound loopback and had no authentication of any kind — no token, no origin
//! check, nothing. That was survivable precisely *because* it was loopback: the only callers were
//! processes already running as this user, which could read the repository anyway.
//!
//! Pairing changes that. The moment a port answers on a network, "any local process" becomes
//! "anyone on this Wi-Fi", and `/api/chat` runs an agent with file-write and command-run rights.
//! So the bind address and the token arrive together, in one module, and neither is optional
//! without the other:
//!
//! - Loopback stays unauthenticated. The approval hook (`keel approve --port`) is a separate
//!   process that calls in on 127.0.0.1 on every Bash tool call, and the local app does the same.
//!   Requiring a token there would buy nothing — a process that can forge the call can read
//!   `~/.keel` too — and would cost the hook its one job.
//! - Anything arriving from anywhere else needs a bearer token issued by pairing.
//! - Binding off-loopback at all is refused unless a device has been paired, so a misconfigured
//!   Keel cannot be reachable by a stranger before it is reachable by its owner.

use crate::lock::Locked;
use axum::{
    Json,
    extract::{ConnectInfo, Path, Request},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};

/// How long a pairing code is worth typing. Long enough to walk to the phone, short enough that a
/// code left on screen is not a standing invitation.
const CODE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(120);

/// A six-digit code is a million possibilities, which is only meaningful if it cannot be tried a
/// million times. The window bounds it, and so does this.
const MAX_ATTEMPTS: u32 = 5;

#[derive(Serialize, Deserialize, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Hex SHA-256 of the bearer token. The token itself is shown once, at pairing, and never
    /// stored — a stolen `paired.json` should not be a working credential.
    token_sha256: String,
    pub issued: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    #[serde(default)]
    devices: Vec<Device>,
}

fn store_path() -> Option<camino::Utf8PathBuf> {
    Some(crate::prefs::dir()?.join("paired.json"))
}

fn load() -> Store {
    store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        // A file that will not parse reads as no devices, which fails closed: no device means no
        // off-loopback bind. The other direction would grant access on a corrupt file.
        .unwrap_or_default()
}

/// Load, change and save the device list under its file lock (`writes::locked`).
fn changed<T>(change: impl FnOnce(&mut Store) -> T) -> Result<T, String> {
    let p = store_path().ok_or("no home directory")?;
    crate::writes::locked(std::path::Path::new(p.as_str()), || {
        let mut store = load();
        let out = change(&mut store);
        save(&store).map(|_| out)
    })?
}

fn save(store: &Store) -> Result<(), String> {
    let p = store_path().ok_or("no home directory")?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    crate::writes::replace(std::path::Path::new(p.as_str()), body.as_bytes())
}

pub fn any_paired() -> bool {
    !load().devices.is_empty()
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Random bytes, from the operating system.
///
/// `getrandom` (already in the build through other crates) rather than `/dev/urandom`: Windows
/// has no such file, and pairing a device there failed outright before it got as far as a code.
fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| format!("no randomness from the system: {e}"))?;
    Ok(buf)
}

struct PendingCode {
    code: String,
    expires: std::time::Instant,
    attempts: u32,
}

fn pending() -> &'static Mutex<Option<PendingCode>> {
    static P: OnceLock<Mutex<Option<PendingCode>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

#[derive(Serialize)]
pub struct CodeView {
    pub code: String,
    pub expires_in: u64,
}

/// Start pairing. Loopback only — see [`guard`].
pub async fn begin() -> Result<Json<CodeView>, (StatusCode, String)> {
    // no-blocking: four bytes from the system's random source and a mutex.
    let bytes = random_bytes(4).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    let code = format!("{n:06}");

    *pending().locked() = Some(PendingCode {
        code: code.clone(),
        expires: std::time::Instant::now() + CODE_LIFETIME,
        attempts: 0,
    });

    Ok(Json(CodeView {
        code,
        expires_in: CODE_LIFETIME.as_secs(),
    }))
}

#[derive(Deserialize)]
pub struct CompleteBody {
    pub code: String,
    #[serde(default)]
    pub device_name: String,
}

#[derive(Serialize)]
pub struct Paired {
    pub token: String,
    pub device_id: String,
}

/// Redeem a code for a token. Reachable without one, because it is how you get one.
pub async fn complete(
    Json(body): Json<CompleteBody>,
) -> Result<Json<Paired>, (StatusCode, String)> {
    crate::serve::in_blocking(move || {
        let refused = || {
            (
                StatusCode::FORBIDDEN,
                "That code is not valid. Start pairing again in Keel.".to_string(),
            )
        };

        {
            let mut slot = pending().locked();
            let Some(p) = slot.as_mut() else {
                return Err(refused());
            };
            if std::time::Instant::now() > p.expires {
                *slot = None;
                return Err(refused());
            }
            p.attempts += 1;
            // Burn the code on too many tries rather than letting the window be spent guessing.
            if p.attempts > MAX_ATTEMPTS || p.code != body.code {
                if p.attempts >= MAX_ATTEMPTS {
                    *slot = None;
                }
                return Err(refused());
            }
            // Single use.
            *slot = None;
        }

        let token: String = random_bytes(32)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let id: String = random_bytes(8)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let name = body.device_name.trim();
        let device = Device {
            id: id.clone(),
            name: if name.is_empty() {
                "A device".to_string()
            } else {
                name.chars().take(60).collect()
            },
            token_sha256: sha256_hex(&token),
            issued: now_stamp(),
        };

        // Across every daemon on this HOME: a pairing and a revoke at once each saved the list
        // without the other's change, and a revoked device came back.
        changed(|store| store.devices.push(device))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        Ok(Json(Paired {
            token,
            device_id: id,
        }))
    })
    .await
}

fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    secs.to_string()
}

#[derive(Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub issued: String,
}

pub async fn devices() -> Json<Vec<DeviceView>> {
    crate::serve::in_blocking(move || {
        Json(
            load()
                .devices
                .into_iter()
                .map(|d| DeviceView {
                    id: d.id,
                    name: d.name,
                    issued: d.issued,
                })
                .collect(),
        )
    })
    .await
}

pub async fn revoke(Path(id): Path<String>) -> Result<Json<bool>, (StatusCode, String)> {
    crate::serve::in_blocking(move || {
        let removed = changed(|store| {
            let before = store.devices.len();
            store.devices.retain(|d| d.id != id);
            store.devices.len() != before
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if !removed {
            return Err((StatusCode::NOT_FOUND, "No such device.".into()));
        }
        Ok(Json(true))
    })
    .await
}

/// Whether a bearer token matches a paired device.
///
/// Compared as hashes, and over the whole string rather than short-circuiting on the first
/// differing byte, so the answer does not leak how much of a guess was right.
fn token_ok(token: &str) -> bool {
    let given = sha256_hex(token);
    load().devices.iter().fold(false, |ok, d| {
        let same = d.token_sha256.len() == given.len()
            && d.token_sha256
                .bytes()
                .zip(given.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        ok | same
    })
}

/// Where the desktop app's webview is served from: `tauri://localhost` on macOS,
/// `http://tauri.localhost` on Windows, and Vite in a debug build.
///
/// **Not proof of anything on its own.** Every Tauri app on the machine shares these origins, so
/// script in any of them — an XSS in somebody's notes app — carries the same one, and on some
/// networks `tauri.localhost` resolves to whoever answers DNS. An origin on this list is let
/// through only with [`app_token`] beside it; the origin is the second factor, not the first.
pub const APP_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    // `tauri dev` serves the page from Vite. A debug build only.
    #[cfg(debug_assertions)]
    "http://localhost:1420",
    #[cfg(debug_assertions)]
    "http://127.0.0.1:1420",
];

/// The token the app that spawned this daemon handed it on stdin, once. Random per launch, never
/// on the command line, in the environment or in a URL, so nothing else on the machine can read it.
static APP_TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Called once, with the first line the app writes to the daemon's stdin.
pub fn set_app_token(token: &str) {
    let token = token.trim();
    if token.len() >= 32 {
        let _ = APP_TOKEN.set(token.to_string());
    }
}

/// Whether a request carries the app's token as `Authorization: Bearer`, compared over the whole
/// string. No token handed over — a daemon the Swift app or a terminal started — means no page is
/// ever the app.
fn app_token(headers: &axum::http::HeaderMap) -> bool {
    let Some(expected) = APP_TOKEN.get() else {
        return false;
    };
    let same = |given: &str| {
        given.len() == expected.len()
            && given
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    };
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    // A page's WebSocket cannot set headers, only subprotocols — so the terminal offers
    // `keel.<token>` among them. Kept out of the URL, where it would be in every log.
    let offered = headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .into_iter()
        .flat_map(|v| v.split(','))
        .filter_map(|p| p.trim().strip_prefix("keel."));
    bearer.into_iter().chain(offered).any(same)
}

/// Why a loopback request came from a web page, if it did. Loopback is unauthenticated because
/// the app and the `keel approve` hook depend on it — and so, without this, is every page in every
/// browser on the machine: a `no-cors` GET from any site, or a rebound DNS name, could add an MCP
/// server to the project. None of Keel's own callers is a browser, so none of them sends these.
fn from_a_page(headers: &axum::http::HeaderMap) -> Option<&'static str> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let name = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]") {
        return Some("a Host that is not this machine");
    }
    if let Some(origin) = headers.get(axum::http::header::ORIGIN) {
        if origin.to_str().is_ok_and(|o| APP_ORIGINS.contains(&o)) && app_token(headers) {
            return None;
        }
        return Some("a web page's Origin");
    }
    let site = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok());
    if matches!(site, Some("cross-site" | "same-site")) {
        return Some("a web page's fetch");
    }
    None
}

/// The one place that decides whether a request from off this machine is allowed in.
pub async fn guard(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    // no-blocking: middleware; off loopback only, one small read of the device list.
    if peer.ip().is_loopback() {
        if let Some(why) = from_a_page(req.headers()) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Refused a request carrying {why}."),
            ));
        }
        return Ok(next.run(req).await);
    }

    let path = req.uri().path();

    // Redeeming a code is how a phone gets its first token, so it cannot itself need one.
    // Starting pairing is the Mac's decision and stays loopback-only, or anyone on the network
    // could mint themselves a code to type.
    if path == "/api/pair/begin" {
        return Err((
            StatusCode::FORBIDDEN,
            "Pairing starts on the Mac.".to_string(),
        ));
    }
    if path == "/api/pair/complete" {
        return Ok(next.run(req).await);
    }

    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();

    if token.is_empty() || !token_ok(token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Pair this device in Keel first.".to_string(),
        ));
    }
    Ok(next.run(req).await)
}

/// This machine's tailnet address, when the tunnel is actually up.
///
/// Asked of Tailscale rather than inferred from the interface list: `tailscale ip -4` is its own
/// answer to the question, and it prints nothing when the backend is stopped — which is exactly
/// the case that must not produce a bind address.
pub fn tailscale_ip() -> Option<std::net::Ipv4Addr> {
    let out = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let l = l.trim();
        (!l.is_empty()).then(|| l.parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_page_cannot_reach_the_loopback_api() {
        use axum::http::{HeaderMap, HeaderValue};
        let with = |pairs: &[(&'static str, &'static str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, HeaderValue::from_static(v));
            }
            from_a_page(&h)
        };
        // The app, the hook and `claude`'s MCP client.
        assert_eq!(with(&[("host", "127.0.0.1:7777")]), None);
        assert_eq!(
            with(&[("host", "localhost:7777"), ("sec-fetch-mode", "cors")]),
            None
        );
        // A page, a no-cors GET from one, and a rebound name.
        assert!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "https://evil.example")
            ])
            .is_some()
        );
        assert!(with(&[("host", "127.0.0.1:7777"), ("sec-fetch-site", "cross-site")]).is_some());
        // The desktop app's webview is a page, but Keel's own: its origin *and* the token it
        // handed this daemon on stdin. The origin alone is every Tauri app on the machine.
        const TOKEN: &str =
            "Bearer kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk";
        set_app_token(&TOKEN["Bearer ".len()..]);
        #[cfg(debug_assertions)]
        for origin in ["http://localhost:1420", "http://127.0.0.1:1420"] {
            assert_eq!(
                with(&[
                    ("host", "127.0.0.1:7777"),
                    ("origin", origin),
                    ("authorization", TOKEN)
                ]),
                None
            );
            assert!(
                with(&[("host", "127.0.0.1:7777"), ("origin", origin)]).is_some(),
                "a development origin still needs the app token"
            );
        }
        assert!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "http://127.0.0.1:1421"),
                ("authorization", TOKEN)
            ])
            .is_some(),
            "other local pages are not the development app"
        );

        assert_eq!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "tauri://localhost"),
                ("sec-fetch-site", "cross-site"),
                ("authorization", TOKEN)
            ]),
            None
        );
        assert!(
            with(&[("host", "127.0.0.1:7777"), ("origin", "tauri://localhost")]).is_some(),
            "another Tauri app has the same origin and no token"
        );
        assert!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "tauri://localhost"),
                ("authorization", "Bearer kkkk")
            ])
            .is_some()
        );
        // The terminal's WebSocket carries it as a subprotocol, since a page cannot set headers.
        const PROTOCOLS: &str =
            "keel, keel.kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk";
        assert_eq!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "tauri://localhost"),
                ("sec-websocket-protocol", PROTOCOLS)
            ]),
            None
        );
        assert!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "tauri://localhost"),
                ("sec-websocket-protocol", "keel, keel.kkkk")
            ])
            .is_some()
        );
        assert!(
            with(&[
                ("host", "evil.example:7777"),
                ("origin", "tauri://localhost"),
                ("authorization", TOKEN)
            ])
            .is_some(),
            "the app's token does not excuse a rebound name"
        );
        assert!(
            with(&[
                ("host", "127.0.0.1:7777"),
                ("origin", "tauri://localhost.evil.example")
            ])
            .is_some()
        );
        assert!(with(&[("host", "evil.example:7777")]).is_some());
        assert!(with(&[]).is_some(), "no Host at all is not a Keel caller");
    }

    #[test]
    fn a_code_is_six_digits_and_nothing_else() {
        let bytes = random_bytes(4).expect("urandom");
        let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
        let code = format!("{n:06}");
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    /// The token is never written down, only its hash. A copy of `paired.json` is not a key.
    #[test]
    fn the_stored_form_is_not_the_token() {
        let token = "abc123";
        let hash = sha256_hex(token);
        assert_ne!(hash, token);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, sha256_hex(token), "and it is stable");
        assert_ne!(hash, sha256_hex("abc124"));
    }

    /// A store that will not parse must read as *no* devices. Failing the other way would let a
    /// corrupt file open the door rather than close it.
    #[test]
    fn an_unparseable_store_grants_nothing() {
        let store: Store = serde_json::from_str("{ not json").unwrap_or_default();
        assert!(store.devices.is_empty());
    }
}
