//! Compiled original build inputs. These DTOs are NOT native bootstrap capabilities.
use crate::{
    error::{Error, Result},
    json::{self, SAFE_MAX},
};
use pinvou_upgrade_codec::{InputKind, decode_canonical_envelope};
use serde::{Deserialize, Serialize};

pub(crate) const TARGET: &str = "windows-x86_64-nsis-perMachine";
pub(crate) const LAYOUT: &str = "windows-host-v1";
pub(crate) const MANIFEST_RESOURCE_ID: u16 = 34101;
pub(crate) use crate::inventory::InstallationManifest;
const SEED_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bootstrap-seed.json"));
const ROOT_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/initial-root.json"));

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ByteReference {
    pub(crate) size: u64,
    pub(crate) sha256: String,
}
impl ByteReference {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.size == 0 || self.size > SAFE_MAX || !json::digest(&self.sha256) {
            Err(Error::InitialTrustUnavailable)
        } else {
            Ok(())
        }
    }
    pub(crate) fn matches(&self, bytes: &[u8]) -> bool {
        self.size == bytes.len() as u64 && self.sha256 == json::sha256(bytes)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LabPublisherPolicy {
    pub(crate) policy_id: String,
    pub(crate) signer_certificate_sha256: String,
}
/// Public configuration only. Neither variant grants installation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CertificateRequirement {
    Optional,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodeImagePolicy {
    pub(crate) policy_version: u8,
    pub(crate) certificate: CertificateRequirement,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageCertificatePolicy<'a> {
    LegacySelectedSigner(&'a str),
    Optional,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Seed {
    pub(crate) host_seed_version: u8,
    pub(crate) profile_kind: String,
    // Stable installation identity across builds; never derive it from build_id.
    pub(crate) profile_id: String,
    pub(crate) product: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) build_id: String,
    pub(crate) app_version: String,
    pub(crate) layout_id: String,
    pub(crate) host_ipc_version: u8,
    pub(crate) host_state_version: u8,
    pub(crate) original_root: ByteReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) approved_initial_publisher_policy: Option<LabPublisherPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code_image_policy: Option<CodeImagePolicy>,
}
fn version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}
impl Seed {
    pub(crate) fn embedded() -> Result<Self> {
        if SEED_BYTES.is_empty() && ROOT_BYTES.is_empty() {
            return Err(Error::ProfileDisabled);
        }
        Self::parse(SEED_BYTES, ROOT_BYTES)
    }
    // Private data parsing never constructs SourceInstallerContext/InstalledServiceContext.
    fn parse(seed_bytes: &[u8], root_bytes: &[u8]) -> Result<Self> {
        let value: Self =
            json::decode(seed_bytes, true).map_err(|_| Error::InitialTrustUnavailable)?;
        if value.host_ipc_version != 1
            || value.host_state_version != 1
            || !matches!(value.profile_kind.as_str(), "lab" | "production")
            || (value.profile_kind == "production" && value.profile_id.contains("-fault-"))
            || value.target_key != TARGET
            || value.layout_id != LAYOUT
            || !(1..=48).contains(&value.profile_id.len())
            || !value
                .profile_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !value.profile_id.as_bytes()[0].is_ascii_alphanumeric()
            || !json::identifier(&value.product)
            || !json::identifier(&value.component)
            || !json::digest(&value.build_id)
            || !version(&value.app_version)
            || value.original_root.size > json::RECORD_MAX as u64
        {
            return Err(Error::InitialTrustUnavailable);
        }
        value.certificate_policy()?;
        value.original_root.validate()?;
        if !value.original_root.matches(root_bytes) || json::encode(&value, true)? != seed_bytes {
            return Err(Error::InitialTrustUnavailable);
        }
        let envelope = decode_canonical_envelope(root_bytes, InputKind::Metadata)
            .and_then(|e| {
                e.check_full_schema()
                    .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
            })
            .map_err(|_| Error::InitialTrustUnavailable)?;
        let root = envelope
            .envelope()
            .signed()
            .root_policy_shape()
            .map_err(|_| Error::InitialTrustUnavailable)?;
        if root.product() != value.product {
            return Err(Error::InitialTrustUnavailable);
        }
        // This is the ORIGINAL independent compiled seed, not an online/current Root/time proof.
        Ok(value)
    }
    /// Explicit version dispatch: missing certificates never downgrade V1.
    pub(crate) fn certificate_policy(&self) -> Result<ImageCertificatePolicy<'_>> {
        match (
            self.host_seed_version,
            self.approved_initial_publisher_policy.as_ref(),
            self.code_image_policy.as_ref(),
        ) {
            (1, Some(legacy), None)
                if json::identifier(&legacy.policy_id)
                    && json::digest(&legacy.signer_certificate_sha256) =>
            {
                Ok(ImageCertificatePolicy::LegacySelectedSigner(
                    &legacy.signer_certificate_sha256,
                ))
            }
            (2, None, Some(policy))
                if policy.policy_version == 1
                    && policy.certificate == CertificateRequirement::Optional =>
            {
                Ok(ImageCertificatePolicy::Optional)
            }
            _ => Err(Error::InitialTrustUnavailable),
        }
    }
    pub(crate) fn seed_sha256(&self) -> String {
        json::sha256(SEED_BYTES)
    }
    /// Original independently compiled anchor, never an accepted current chain.
    pub(crate) fn original_root_bytes(&self) -> &'static [u8] {
        ROOT_BYTES
    }
    pub(crate) fn bootstrap_source_leaf(&self) -> &'static str {
        if self.profile_kind == "production" {
            "UpdaterBootstrapSource"
        } else {
            "UpdaterBootstrapSourceLab"
        }
    }
    pub(crate) fn updater_leaf(&self) -> &'static str {
        if self.profile_kind == "production" {
            "Updater"
        } else {
            "UpdaterLab"
        }
    }
    pub(crate) fn app_leaf(&self) -> &'static str {
        if self.profile_kind == "production" {
            "App"
        } else {
            "AppLab"
        }
    }
    pub(crate) fn service_name(&self) -> String {
        if self.profile_kind == "production" {
            format!("PinvouUpgrade-{}", self.profile_id)
        } else {
            format!("PinvouUpgradeLab-{}", self.profile_id)
        }
    }
    pub(crate) fn pipe_name(&self) -> String {
        if self.profile_kind == "production" {
            format!(r"\\.\pipe\Pinvou.Upgrade.{}.v1", self.profile_id)
        } else {
            format!(r"\\.\pipe\Pinvou.Upgrade.Lab.{}.v1", self.profile_id)
        }
    }
    pub(crate) fn bootstrap_mutex_name(&self, lifetime: bool) -> String {
        let purpose = if lifetime {
            "ServiceLifetime"
        } else {
            "Bootstrap"
        };
        if self.profile_kind == "production" {
            format!(r"Global\Pinvou.Upgrade.{purpose}.{}", self.profile_id)
        } else {
            format!(r"Global\Pinvou.Upgrade.{purpose}.Lab.{}", self.profile_id)
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImageReference {
    pub(crate) build_id: String,
    pub(crate) version: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) layout_id: String,
}
impl ImageReference {
    pub(crate) fn validate(&self, seed: &Seed) -> Result<()> {
        ByteReference {
            size: self.size,
            sha256: self.sha256.clone(),
        }
        .validate()?;
        if self.build_id != seed.build_id
            || self.version != seed.app_version
            || self.layout_id != seed.layout_id
        {
            Err(Error::ImageRefused)
        } else {
            Ok(())
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RootManifest {
    pub(crate) host_manifest_version: u8,
    pub(crate) profile_kind: String,
    pub(crate) profile_id: String,
    pub(crate) product: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) build_id: String,
    pub(crate) seed_sha256: String,
    pub(crate) original_root: ByteReference,
    pub(crate) layout_id: String,
    pub(crate) helper: ImageReference,
    pub(crate) launcher: ImageReference,
    pub(crate) app: ImageReference,
    pub(crate) app_inventory: crate::inventory::InventoryDescriptor,
}
impl RootManifest {
    pub(crate) fn parse(bytes: &[u8], seed: &Seed) -> Result<Self> {
        let value: Self = json::decode(bytes, false).map_err(|_| Error::ImageRefused)?;
        if value.host_manifest_version != 2
            || value.profile_kind != seed.profile_kind
            || value.profile_id != seed.profile_id
            || value.product != seed.product
            || value.component != seed.component
            || value.target_key != seed.target_key
            || value.build_id != seed.build_id
            || value.seed_sha256 != seed.seed_sha256()
            || value.original_root != seed.original_root
            || value.layout_id != seed.layout_id
            || json::encode(&value, false)? != bytes
        {
            return Err(Error::ImageRefused);
        }
        value.helper.validate(seed)?;
        value.launcher.validate(seed)?;
        value.app.validate(seed)?;
        value.app_inventory.validate()?;
        Ok(value)
    }
}

/// Signed original inventory data. It is not an opened/approved namespace.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppFile {
    pub(crate) relative_name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}
pub(crate) fn relative_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 1024 || value.contains(['\\', ':', '\0']) {
        return false;
    }
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() > 32 {
        return false;
    }
    parts.iter().all(|part| {
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| {
                (c < ' ' || c == '\u{7f}') || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')
            })
        {
            return false;
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|tail| {
                matches!(
                    tail,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    })
}
pub(crate) fn validate_catalog(files: &[AppFile], app: &ImageReference) -> Result<()> {
    if files.is_empty() || files.len() > crate::inventory::MAX_FILES {
        return Err(Error::ImageRefused);
    }
    let mut previous: Option<&str> = None;
    let mut found_app = false;
    for file in files {
        if !relative_name(&file.relative_name)
            || previous.is_some_and(|p| p >= file.relative_name.as_str())
        {
            return Err(Error::ImageRefused);
        }
        if file.size > SAFE_MAX || !json::digest(&file.sha256) {
            return Err(Error::ImageRefused);
        }
        if file.relative_name == "pinvou3-tauri.exe" {
            if file.size != app.size || file.sha256 != app.sha256 {
                return Err(Error::ImageRefused);
            }
            found_app = true;
        }
        previous = Some(&file.relative_name);
    }
    // Native Windows ordinal alias checks additionally run before any publication.
    if !found_app {
        return Err(Error::ImageRefused);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn policy_seed(version: u8) -> serde_json::Value {
        let mut value = serde_json::json!({
            "hostSeedVersion": 1, "profileKind": "lab", "profileId": "policy-model-only",
            "product": "pinvou", "component": "agent", "targetKey": TARGET,
            "buildId": "1".repeat(64), "appVersion": "0.0.0", "layoutId": LAYOUT,
            "hostIpcVersion": 1, "hostStateVersion": 1,
            "originalRoot": {"size": 1, "sha256": "7".repeat(64)},
            "approvedInitialPublisherPolicy": {
                "policyId": "legacy-model-only", "signerCertificateSha256": "8".repeat(64)
            }
        });
        if version == 2 {
            value["hostSeedVersion"] = 2.into();
            value
                .as_object_mut()
                .unwrap()
                .remove("approvedInitialPublisherPolicy");
            value["codeImagePolicy"] =
                serde_json::json!({"policyVersion": 1, "certificate": "optional"});
        }
        value
    }
    fn root_shape_bytes(product: &str) -> Vec<u8> {
        // Public shape-only descriptors and a zero signature, never a real Root
        // signature threshold, production default or native bootstrap identity.
        let keys: Vec<_> = ['A', 'E', 'I']
            .into_iter()
            .map(|tail| {
                let text = format!("{}{tail}", "A".repeat(42));
                let key = pinvou_upgrade_codec::key_descriptor(&text).unwrap();
                serde_json::json!({
                    "keyId": key.key_id(), "algorithm": "ed25519",
                    "publicKey": key.public_key()
                })
            })
            .collect();
        let key_ids: Vec<_> = keys.iter().map(|key| key["keyId"].clone()).collect();
        let envelope = serde_json::json!({
            "signed": {
                "protocolVersion": 1, "role": "root", "product": product,
                "component": null, "scope": {}, "version": 1,
                "issuedAt": 0, "expiresAt": 0, "keys": keys,
                "roles": [{
                    "role": "root", "component": null, "channel": null,
                    "targetKey": null, "threshold": 2, "keyIds": key_ids
                }]
            },
            "signatures": [{"keyId": key_ids[0], "signature": "A".repeat(86)}]
        });
        json::encode(&envelope, false).unwrap()
    }
    fn seed_wire(version: u8, kind: &str, id: &str, root: &[u8]) -> Vec<u8> {
        let mut wire = policy_seed(version);
        wire["profileKind"] = kind.into();
        wire["profileId"] = id.into();
        wire["originalRoot"] = serde_json::json!({
            "size": root.len(), "sha256": json::sha256(root)
        });
        json::encode(&wire, true).unwrap()
    }
    #[test]
    fn lab_profiles_keep_original_wire_policy_and_native_names() {
        let root = root_shape_bytes("pinvou");
        for version in [1, 2] {
            for id in ["stable-installation", "stable-fault-installation"] {
                let wire = seed_wire(version, "lab", id, &root);
                let seed = Seed::parse(&wire, &root).unwrap();
                assert_eq!(json::encode(&seed, true).unwrap(), wire);
                assert_eq!(seed.service_name(), format!("PinvouUpgradeLab-{id}"));
                assert_eq!(
                    seed.pipe_name(),
                    format!(r"\\.\pipe\Pinvou.Upgrade.Lab.{id}.v1")
                );
                assert_eq!(seed.bootstrap_source_leaf(), "UpdaterBootstrapSourceLab");
                assert_eq!(seed.updater_leaf(), "UpdaterLab");
                assert_eq!(seed.app_leaf(), "AppLab");
                assert_eq!(
                    seed.bootstrap_mutex_name(false),
                    format!(r"Global\Pinvou.Upgrade.Bootstrap.Lab.{id}")
                );
                assert_eq!(
                    seed.bootstrap_mutex_name(true),
                    format!(r"Global\Pinvou.Upgrade.ServiceLifetime.Lab.{id}")
                );
            }
        }
    }
    #[test]
    fn production_profiles_use_explicit_kind_and_stable_separate_names() {
        let root = root_shape_bytes("pinvou");
        for version in [1, 2] {
            let wire = seed_wire(version, "production", "stable-installation", &root);
            let seed = Seed::parse(&wire, &root).unwrap();
            assert_eq!(json::encode(&seed, true).unwrap(), wire);
            assert_eq!(seed.service_name(), "PinvouUpgrade-stable-installation");
            assert_eq!(
                seed.pipe_name(),
                r"\\.\pipe\Pinvou.Upgrade.stable-installation.v1"
            );
            assert_eq!(seed.bootstrap_source_leaf(), "UpdaterBootstrapSource");
            assert_eq!(seed.updater_leaf(), "Updater");
            assert_eq!(seed.app_leaf(), "App");
            assert_eq!(
                seed.bootstrap_mutex_name(false),
                r"Global\Pinvou.Upgrade.Bootstrap.stable-installation"
            );
            assert_eq!(
                seed.bootstrap_mutex_name(true),
                r"Global\Pinvou.Upgrade.ServiceLifetime.stable-installation"
            );
            let mut next: serde_json::Value = serde_json::from_slice(&wire).unwrap();
            next["buildId"] = "2".repeat(64).into();
            next["appVersion"] = "1.2.3".into();
            let next = Seed::parse(&json::encode(&next, true).unwrap(), &root).unwrap();
            assert_eq!(next.service_name(), seed.service_name());
            assert_eq!(next.pipe_name(), seed.pipe_name());
            assert_eq!(next.updater_leaf(), seed.updater_leaf());
            assert_eq!(
                next.bootstrap_mutex_name(false),
                seed.bootstrap_mutex_name(false)
            );
            assert_eq!(
                next.bootstrap_mutex_name(true),
                seed.bootstrap_mutex_name(true)
            );
        }
    }
    #[test]
    fn same_profile_id_in_lab_and_production_has_disjoint_native_namespaces() {
        let root = root_shape_bytes("pinvou");
        let lab = Seed::parse(&seed_wire(2, "lab", "stable-installation", &root), &root).unwrap();
        let production = Seed::parse(
            &seed_wire(2, "production", "stable-installation", &root),
            &root,
        )
        .unwrap();
        assert_ne!(
            lab.bootstrap_source_leaf(),
            production.bootstrap_source_leaf()
        );
        assert_ne!(lab.updater_leaf(), production.updater_leaf()); // Code, state and registry parent.
        assert_ne!(lab.app_leaf(), production.app_leaf());
        assert_ne!(lab.service_name(), production.service_name()); // Includes uninstall identity.
        assert_ne!(lab.pipe_name(), production.pipe_name());
        for lifetime in [false, true] {
            assert_ne!(
                lab.bootstrap_mutex_name(lifetime),
                production.bootstrap_mutex_name(lifetime)
            );
        }
    }
    #[test]
    fn production_retains_root_binding_canonical_and_certificate_checks() {
        let root = root_shape_bytes("pinvou");
        let other_root = root_shape_bytes("other-product");
        for version in [1, 2] {
            let wire = seed_wire(version, "production", "stable-installation", &root);
            assert!(Seed::parse(&wire, &other_root).is_err());
            let wrong_product =
                seed_wire(version, "production", "stable-installation", &other_root);
            assert!(Seed::parse(&wrong_product, &other_root).is_err());
            let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
            let pretty = serde_json::to_vec_pretty(&value).unwrap();
            assert!(Seed::parse(&pretty, &root).is_err());
            if version == 1 {
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("approvedInitialPublisherPolicy");
            } else {
                value.as_object_mut().unwrap().remove("codeImagePolicy");
            }
            assert!(Seed::parse(&json::encode(&value, true).unwrap(), &root).is_err());
        }
    }
    #[test]
    fn profile_parsing_refuses_unknown_kinds_and_production_fault_profiles() {
        let root = root_shape_bytes("pinvou");
        for version in [1, 2] {
            for kind in ["", "Production", "shipping", "fault"] {
                assert!(
                    Seed::parse(
                        &seed_wire(version, kind, "stable-installation", &root),
                        &root
                    )
                    .is_err()
                );
            }
            assert!(
                Seed::parse(
                    &seed_wire(version, "production", "stable-fault-installation", &root),
                    &root
                )
                .is_err()
            );
        }
        assert!(Seed::parse(&[], &[]).is_err()); // No implicit default Root/identity.
    }
    #[test]
    fn certificate_policy_versions_preserve_legacy_bytes_without_granting_authority() {
        for version in [1, 2] {
            let wire = policy_seed(version);
            let seed: Seed = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(
                json::encode(&seed, true).unwrap(),
                json::encode(&wire, true).unwrap()
            );
            match seed.certificate_policy().unwrap() {
                ImageCertificatePolicy::LegacySelectedSigner(leaf) => {
                    assert_eq!(version, 1);
                    assert_eq!(leaf, "8".repeat(64));
                }
                ImageCertificatePolicy::Optional => assert_eq!(version, 2),
            }
        }
        // Shape/serialization checks only: neither model creates native source authority.
    }
    #[test]
    fn certificate_policy_has_no_missing_or_mixed_version_fallback() {
        let mut cases = Vec::new();
        let mut legacy = policy_seed(1);
        legacy
            .as_object_mut()
            .unwrap()
            .remove("approvedInitialPublisherPolicy");
        cases.push(legacy);
        let mut mixed = policy_seed(1);
        mixed["codeImagePolicy"] = policy_seed(2)["codeImagePolicy"].clone();
        cases.push(mixed);
        let mut mixed = policy_seed(2);
        mixed["approvedInitialPublisherPolicy"] =
            policy_seed(1)["approvedInitialPublisherPolicy"].clone();
        cases.push(mixed);
        let mut unknown = policy_seed(2);
        unknown["hostSeedVersion"] = 3.into();
        cases.push(unknown);
        let mut unknown = policy_seed(2);
        unknown["codeImagePolicy"]["policyVersion"] = 2.into();
        cases.push(unknown);
        for wire in cases {
            let seed: Seed = serde_json::from_value(wire).unwrap();
            assert!(seed.certificate_policy().is_err());
        }
        for (key, value) in [
            ("certificate", serde_json::json!("required")),
            ("extra", serde_json::json!(true)),
        ] {
            let mut wire = policy_seed(2);
            wire["codeImagePolicy"][key] = value;
            assert!(serde_json::from_value::<Seed>(wire).is_err());
        }
    }
    #[test]
    fn inventory_never_accepts_a_destination_override_or_device() {
        for bad in [
            "",
            "/x",
            "../x",
            "a/../x",
            "a\\b",
            "a:x",
            "AUX.txt",
            "a/con",
            "COM¹",
            "com².txt",
            "LPT³.ext",
            "a.",
            "a ",
            "a//b",
            "a?b",
            "bad\u{7f}name",
        ] {
            assert!(!relative_name(bad), "{bad}");
        }
        assert!("\u{e000}" < "\u{10000}"); // Inventory is UTF8 byte order, not JS default UTF16.
        for good in [
            "pinvou3-tauri.exe",
            "runtime/file.json",
            "runtime/日本語.txt",
            "runtime/file¹.txt",
            "runtime/ordinary².json",
            "runtime/ordinary$.txt",
        ] {
            assert!(relative_name(good));
        }
    }
}

/// Public compiled configuration only. No native context, source path or ready flag.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileDiagnostic<'a> {
    host_profile_version: u8,
    enabled: bool,
    profile_kind: &'a str,
    profile_id: &'a str,
    product: &'a str,
    component: &'a str,
    target_key: &'a str,
    build_id: &'a str,
    layout_id: &'a str,
    seed_sha256: String,
    original_root: &'a ByteReference,
}
impl Seed {
    pub(crate) fn diagnostic(&self) -> Result<Vec<u8>> {
        json::encode(
            &ProfileDiagnostic {
                host_profile_version: 1,
                enabled: true,
                profile_kind: &self.profile_kind,
                profile_id: &self.profile_id,
                product: &self.product,
                component: &self.component,
                target_key: &self.target_key,
                build_id: &self.build_id,
                layout_id: &self.layout_id,
                seed_sha256: self.seed_sha256(),
                original_root: &self.original_root,
            },
            true,
        )
    }
}
