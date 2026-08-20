//! Feature-gated composition boundary for the distributed runtime.
//!
//! Commands are added only when their daemon contracts are implemented. The
//! stage-one workspace scaffold deliberately exposes no placeholder command.

/// Confirms which protocol package is wired into the distributed build graph.
pub const PROTOCOL_CRATE_NAME: &str = pinvou_protocol::CRATE_NAME;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributed_boundary_uses_the_protocol_crate() {
        assert_eq!(PROTOCOL_CRATE_NAME, "pinvou-protocol");
    }
}
