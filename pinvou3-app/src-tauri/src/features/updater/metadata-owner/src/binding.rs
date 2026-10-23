use crate::Error;
use serde::Serialize;

/// Validated identity data only, not a protected scope capability. The native
/// adapter must derive every field from its original installation intent.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapBinding {
    installation_scope_id: String,
    product: String,
    component: String,
    target_key: String,
    profile_kind: String,
    profile_id: String,
    genesis_seed_sha256: String,
}
impl BootstrapBinding {
    pub fn new(
        scope: &str,
        product: &str,
        component: &str,
        target_key: &str,
        profile_kind: &str,
        profile_id: &str,
        seed_sha256: &str,
    ) -> Result<Self, Error> {
        if !valid_fields(
            scope,
            product,
            component,
            target_key,
            profile_kind,
            profile_id,
            seed_sha256,
        ) {
            return Err(Error::InvalidBinding);
        }
        let value = Self {
            installation_scope_id: scope.to_owned(),
            product: product.to_owned(),
            component: component.to_owned(),
            target_key: target_key.to_owned(),
            profile_kind: profile_kind.to_owned(),
            profile_id: profile_id.to_owned(),
            genesis_seed_sha256: seed_sha256.to_owned(),
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if valid_fields(
            &self.installation_scope_id,
            &self.product,
            &self.component,
            &self.target_key,
            &self.profile_kind,
            &self.profile_id,
            &self.genesis_seed_sha256,
        ) {
            Ok(())
        } else {
            Err(Error::InvalidBinding)
        }
    }
    pub fn product(&self) -> &str {
        &self.product
    }
}
fn valid_fields(
    scope: &str,
    product: &str,
    component: &str,
    target: &str,
    kind: &str,
    profile: &str,
    seed: &str,
) -> bool {
    digest(scope)
        && digest(seed)
        && identifier(product)
        && identifier(component)
        && matches!(
            target,
            "windows-x86_64-nsis-perMachine"
                | "linux-x86_64-deb-perMachine"
                | "linux-arm64-deb-perMachine"
                | "macos-universal-dmg-perMachine"
        )
        && matches!(kind, "lab" | "production")
        && (1..=48).contains(&profile.len())
        && profile
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && profile.as_bytes()[0].is_ascii_alphanumeric()
        && !(kind == "production" && profile.contains("-fault-"))
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_rejects_unbounded_identifiers_paths_and_unknown_profiles() {
        let scope = "1".repeat(64);
        let seed = "2".repeat(64);
        let good = [
            scope.as_str(),
            "pinvou",
            "agent",
            "windows-x86_64-nsis-perMachine",
            "production",
            "stable-installation",
            seed.as_str(),
        ];
        for (index, bad) in [
            (0, "not-a-scope"),
            (1, "../pinvou"),
            (2, ""),
            (3, "windows-any"),
            (4, "Production"),
            (5, "../profile"),
            (6, "BAD"),
        ] {
            let mut input = good;
            input[index] = bad;
            assert!(
                BootstrapBinding::new(
                    input[0], input[1], input[2], input[3], input[4], input[5], input[6]
                )
                .is_err()
            );
        }
        assert!(
            BootstrapBinding::new(
                &scope,
                &"a".repeat(129),
                "agent",
                good[3],
                "production",
                "stable-installation",
                &seed
            )
            .is_err()
        );
        assert!(
            BootstrapBinding::new(
                &scope,
                "pinvou",
                "agent",
                good[3],
                "production",
                &"a".repeat(49),
                &seed
            )
            .is_err()
        );
        assert!(
            BootstrapBinding::new(
                &scope,
                "pinvou",
                "agent",
                good[3],
                "production",
                "install-fault-test",
                &seed
            )
            .is_err()
        );
        assert!(
            BootstrapBinding::new(
                &scope,
                "pinvou",
                "agent",
                good[3],
                "lab",
                "install-fault-test",
                &seed
            )
            .is_ok()
        );
    }
}
