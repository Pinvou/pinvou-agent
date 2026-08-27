//! Black-box adapter for the public CodeWhale app-server stdio protocol.

mod desktop_profile;
mod process;

pub use process::{CodeWhaleAdapter, CodeWhaleAdapterConfig};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const RUNTIME_API_CRATE_NAME: &str = pinvou_runtime_api::CRATE_NAME;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_and_runtime_boundary_are_stable() {
        assert_eq!(CRATE_NAME, "pinvou-agent-adapter-codewhale");
        assert_eq!(RUNTIME_API_CRATE_NAME, "pinvou-runtime-api");
    }
}
