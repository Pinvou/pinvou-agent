//! Controller process composition boundary.

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub fn unavailable_message() -> &'static str {
    "pinvou-controller is not implemented in the workspace scaffold"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-controller");
    }

    #[test]
    fn scaffold_does_not_claim_daemon_availability() {
        assert!(unavailable_message().contains("not implemented"));
    }
}
