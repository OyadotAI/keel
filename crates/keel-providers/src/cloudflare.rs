//! Cloudflare: resource provisioning, versions, and deployments.

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

/// Placeholder for the authenticated Cloudflare client.
#[derive(Debug, Default)]
pub struct Cloudflare;
