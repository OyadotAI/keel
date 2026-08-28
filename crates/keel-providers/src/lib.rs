//! Clients for the two services Keel connects to: GitHub and Cloudflare.
//!
//! Credentials live in the OS keychain and never leave the machine. Cloudflare has no OIDC or
//! keyless deploy path, so Keel provisions narrowly-scoped per-project API tokens and rotates them
//! rather than asking the user for an account-wide token.

pub mod cloudflare;
pub mod credentials;
pub mod github;
