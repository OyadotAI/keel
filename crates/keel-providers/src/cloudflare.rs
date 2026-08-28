//! Cloudflare: token verification and account listing.
//!
//! Cloudflare has no OIDC or keyless path as of August 2026 — the open requests on
//! `wrangler-action` and `workers-sdk` are unimplemented — so a scoped API token is the only
//! option. Keel stores it in the OS keychain and never sends it anywhere but Cloudflare.

use crate::credentials;
use serde::{Deserialize, Serialize};

const API: &str = "https://api.cloudflare.com/client/v4";

/// A Cloudflare account the token can act on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
}

/// The stateful resources Keel provisions separately per environment.
///
/// Sharing any of these between dev and prod is the single most common way a safe-looking action
/// destroys real data, so the type exists to make "which environment does this belong to" a thing
/// the compiler asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatefulResource {
    D1,
    KvNamespace,
    R2Bucket,
    Queue,
    DurableObjectNamespace,
}

impl StatefulResource {
    /// The Wrangler config key this resource appears under.
    pub fn wrangler_key(self) -> &'static str {
        match self {
            StatefulResource::D1 => "d1_databases",
            StatefulResource::KvNamespace => "kv_namespaces",
            StatefulResource::R2Bucket => "r2_buckets",
            StatefulResource::Queue => "queues",
            StatefulResource::DurableObjectNamespace => "durable_objects",
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    result: Option<T>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

impl<T> Envelope<T> {
    fn into_result(self) -> Result<T, String> {
        if self.success && let Some(result) = self.result {
            return Ok(result);
        }
        Err(self
            .errors
            .first()
            .map(|e| e.message.clone())
            .unwrap_or_else(|| "Cloudflare rejected the request".into()))
    }
}

#[derive(Deserialize)]
struct TokenStatus {
    status: String,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("keel")
        .build()
        .expect("building an HTTP client with no custom TLS config cannot fail")
}

/// Check a token is live, then report which accounts it reaches.
///
/// Verification and listing are separate calls because they fail differently: a revoked token and a
/// token scoped to zero accounts are different problems, and telling someone "invalid token" when
/// the token is fine but unscoped sends them looking in the wrong place.
pub async fn verify(token: &str) -> Result<Vec<Account>, String> {
    let status: Envelope<TokenStatus> = client()
        .get(format!("{API}/user/tokens/verify"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("could not reach Cloudflare: {e}"))?
        .json()
        .await
        .map_err(|e| format!("unexpected response from Cloudflare: {e}"))?;

    let status = status.into_result()?;
    if status.status != "active" {
        return Err(format!("token status is `{}`, not active", status.status));
    }

    accounts(token).await
}

/// Accounts this token can act on.
pub async fn accounts(token: &str) -> Result<Vec<Account>, String> {
    let envelope: Envelope<Vec<Account>> = client()
        .get(format!("{API}/accounts"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("could not reach Cloudflare: {e}"))?
        .json()
        .await
        .map_err(|e| format!("unexpected response from Cloudflare: {e}"))?;

    let accounts = envelope.into_result()?;
    if accounts.is_empty() {
        return Err(
            "the token is valid but reaches no accounts — it needs Account scope, not just User"
                .into(),
        );
    }
    Ok(accounts)
}

/// The currently connected accounts, if a token is stored.
pub async fn current() -> Option<Vec<Account>> {
    let token = credentials::load(credentials::Kind::Cloudflare)?;
    accounts(&token).await.ok()
}
