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

fn save(store: &Store) -> Result<(), String> {
    let p = store_path().ok_or("no home directory")?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&p, body).map_err(|e| e.to_string())
}

pub fn any_paired() -> bool {
    !load().devices.is_empty()
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Random bytes, from the kernel.
///
/// `/dev/urandom` rather than a crate: this is four lines of std on every platform Keel runs on,
/// and a dependency whose whole job is to open that file is a dependency to keep patched forever.
fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("reading /dev/urandom: {e}"))?;
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
    let bytes = random_bytes(4).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    let code = format!("{n:06}");

    *pending().lock().expect("pairing lock") = Some(PendingCode {
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
    let refused = || {
        (
            StatusCode::FORBIDDEN,
            "That code is not valid. Start pairing again in Keel.".to_string(),
        )
    };

    {
        let mut slot = pending().lock().expect("pairing lock");
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

    let mut store = load();
    store.devices.push(device);
    save(&store).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(Paired {
        token,
        device_id: id,
    }))
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
}

pub async fn revoke(Path(id): Path<String>) -> Result<Json<bool>, (StatusCode, String)> {
    let mut store = load();
    let before = store.devices.len();
    store.devices.retain(|d| d.id != id);
    if store.devices.len() == before {
        return Err((StatusCode::NOT_FOUND, "No such device.".into()));
    }
    save(&store).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(true))
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

/// The one place that decides whether a request from off this machine is allowed in.
pub async fn guard(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    if peer.ip().is_loopback() {
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
