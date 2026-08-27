//! Golden-path generation.
//!
//! The templates here are the product's real asset. Not the code an agent can write from a prompt —
//! the accumulated operational knowledge it cannot: D1's write ceiling, KV's propagation delay,
//! Durable Objects' single-writer semantics, and the fact that Cloudflare Container disk is
//! ephemeral and resets to the image on every restart.

/// A workload's shape, which determines where it is allowed to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// Stateless request handling: SSR, API, BFF, auth edge logic.
    Stateless,
    /// Needs strong per-key consistency or a coordination point.
    Coordinated,
    /// Heavy or arbitrary compute needing a full Linux runtime.
    Compute,
    /// Durable relational state.
    Relational,
}

/// Where a workload runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Workers,
    DurableObject,
    Container,
    D1,
    /// Hyperdrive in front of a managed Postgres hosted outside Cloudflare.
    ExternalPostgres,
}

/// Place a workload, given how much it writes.
///
/// The `writes_per_second` argument exists because of one specific cliff: D1 is a single-writer
/// database at roughly 50 writes/sec. Exceeding it is not a gradual slowdown, and it is the most
/// common way a Cloudflare-native app hits a wall in production. Above the threshold Keel says so
/// and reaches for Hyperdrive rather than pretending D1 scales.
pub fn place(workload: Workload, writes_per_second: u32) -> Target {
    const D1_WRITE_CEILING: u32 = 50;

    match workload {
        Workload::Stateless => Target::Workers,
        Workload::Coordinated => Target::DurableObject,
        // Container disk is ephemeral — it resets to the image on every restart — so a container
        // is never a valid home for durable state, however convenient it looks.
        Workload::Compute => Target::Container,
        Workload::Relational if writes_per_second < D1_WRITE_CEILING => Target::D1,
        Workload::Relational => Target::ExternalPostgres,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modest_relational_writes_land_on_d1() {
        assert_eq!(place(Workload::Relational, 10), Target::D1);
    }

    #[test]
    fn exceeding_the_d1_write_ceiling_reaches_for_hyperdrive() {
        assert_eq!(place(Workload::Relational, 500), Target::ExternalPostgres);
    }

    #[test]
    fn durable_state_never_lands_on_an_ephemeral_container() {
        for wps in [0, 49, 50, 10_000] {
            assert_ne!(
                place(Workload::Relational, wps),
                Target::Container,
                "container disk resets on restart; state must never be placed there"
            );
        }
    }
}
