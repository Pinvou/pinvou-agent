//! Long-lived agent runtime seam.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const PROTOCOL_CRATE_NAME: &str = pinvou_protocol::CRATE_NAME;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_and_protocol_boundary_are_stable() {
        assert_eq!(CRATE_NAME, "pinvou-runtime-api");
        assert_eq!(PROTOCOL_CRATE_NAME, "pinvou-protocol");
    }
}
