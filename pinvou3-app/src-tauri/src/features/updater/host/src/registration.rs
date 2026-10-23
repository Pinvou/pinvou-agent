//! Original scope/genesis and finite native installation lifecycle.
//! These bounded records describe expectations; only native owner code writes.
use crate::{
    error::{Error, Result},
    identity::{FileTime, LogonId},
    json,
    profile::{ByteReference, ImageReference, InstallationManifest, Seed},
};
use pinvou_upgrade_custody::Identity;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Provisioning,
    FilesVerified,
    ServiceConfigured,
    Ready,
    Repairing,
    Removing,
    RepairRequired,
    RemovedTombstone,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstallerIdentity {
    pub(crate) process_id: u32,
    pub(crate) creation_file_time: FileTime,
    pub(crate) token_user_sid: String,
    pub(crate) logon_authentication_id: LogonId,
    pub(crate) session_id: u32,
    pub(crate) source_nsis_sha256: String,
    pub(crate) source_nsis_file_identity: Identity,
    pub(crate) manifest_sha256: String,
    pub(crate) operation_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServiceIdentity {
    pub(crate) service_name: String,
    pub(crate) account_sid: String,
    pub(crate) service_sid: Option<String>,
    pub(crate) sid_mode: String,
    pub(crate) creation_operation_id: Option<String>,
}
/// Historical original intent data, never a native authorization capability.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LocalInitialKind {
    AdministratorStartedInstaller,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LocalInitialOrigin {
    origin_version: u8,
    kind: LocalInitialKind,
}
impl LocalInitialOrigin {
    pub(crate) fn administrator_started_installer() -> Self {
        Self {
            origin_version: 1,
            kind: LocalInitialKind::AdministratorStartedInstaller,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceSet {
    pub(crate) layout_id: String,
    pub(crate) nsis_sha256: String,
    pub(crate) manifest_sha256: String,
    pub(crate) helper_sha256: String,
    pub(crate) launcher_sha256: String,
    pub(crate) app_sha256: String,
    pub(crate) source_operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) local_initial_origin: Option<LocalInitialOrigin>,
}
impl SourceSet {
    fn validate_origin(&self, seed: &Seed) -> Result<()> {
        use crate::profile::ImageCertificatePolicy;
        match (
            seed.certificate_policy()?,
            self.local_initial_origin.as_ref(),
        ) {
            (ImageCertificatePolicy::LegacySelectedSigner(_), None) => Ok(()),
            (ImageCertificatePolicy::Optional, Some(origin)) if origin.origin_version == 1 => {
                Ok(())
            }
            _ => Err(Error::StateCorrupt),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartupAttestation {
    pub(crate) process_id: u32,
    pub(crate) creation_file_time: FileTime,
    pub(crate) instance_nonce: String,
    pub(crate) helper_image_sha256: String,
    pub(crate) manifest_sha256: String,
    pub(crate) host_epoch: u64,
    pub(crate) observed_registration_revision: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Transition {
    pub(crate) operation_id: String,
    pub(crate) kind: String,
    pub(crate) manifest_sha256: String,
    pub(crate) previous_phase: Option<Phase>,
    pub(crate) next_phase: Phase,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HostRegistration {
    pub(crate) host_state_version: u8,
    pub(crate) profile_kind: String,
    pub(crate) profile_id: String,
    pub(crate) product: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) installation_scope_id: String,
    pub(crate) genesis_seed_sha256: String,
    pub(crate) genesis_manifest_sha256: String,
    pub(crate) initial_root_reference: ByteReference,
    pub(crate) helper: ImageReference,
    pub(crate) launcher: ImageReference,
    pub(crate) app: ImageReference,
    pub(crate) service_identity: ServiceIdentity,
    pub(crate) phase: Phase,
    pub(crate) revision: u64,
    pub(crate) host_epoch: u64,
    pub(crate) previous_record_sha256: Option<String>,
    pub(crate) original_install_operation_id: String,
    pub(crate) reference_schema_version: u8,
    pub(crate) known_owner_namespaces: Vec<String>,
    pub(crate) source_set: SourceSet,
    pub(crate) original_installer_identity: InstallerIdentity,
    pub(crate) installer_identity: InstallerIdentity,
    pub(crate) startup_attestation: Option<StartupAttestation>,
    pub(crate) last_transition: Transition,
}
impl HostRegistration {
    pub(crate) fn initial(
        seed: &Seed,
        manifest: &InstallationManifest,
        scope: String,
        installer: InstallerIdentity,
        local_initial_origin: Option<LocalInitialOrigin>,
    ) -> Result<Self> {
        let operation = installer.operation_id.clone();
        let manifest_sha256 = installer.manifest_sha256.clone();
        let value = Self {
            host_state_version: 1,
            profile_kind: seed.profile_kind.clone(),
            profile_id: seed.profile_id.clone(),
            product: seed.product.clone(),
            component: seed.component.clone(),
            target_key: seed.target_key.clone(),
            installation_scope_id: scope,
            genesis_seed_sha256: seed.seed_sha256(),
            genesis_manifest_sha256: manifest_sha256.clone(),
            initial_root_reference: seed.original_root.clone(),
            helper: manifest.helper.clone(),
            launcher: manifest.launcher.clone(),
            app: manifest.app.clone(),
            service_identity: ServiceIdentity {
                service_name: seed.service_name(),
                account_sid: "S-1-5-18".into(),
                service_sid: None,
                sid_mode: "unrestricted".into(),
                creation_operation_id: None,
            },
            phase: Phase::Provisioning,
            revision: 1,
            host_epoch: 0,
            previous_record_sha256: None,
            original_install_operation_id: operation.clone(),
            reference_schema_version: 1,
            known_owner_namespaces: crate::bootstrap::namespaces(),
            source_set: SourceSet {
                layout_id: seed.layout_id.clone(),
                nsis_sha256: installer.source_nsis_sha256.clone(),
                manifest_sha256: manifest_sha256.clone(),
                helper_sha256: manifest.helper.sha256.clone(),
                launcher_sha256: manifest.launcher.sha256.clone(),
                app_sha256: manifest.app.sha256.clone(),
                source_operation_id: operation.clone(),
                local_initial_origin,
            },
            original_installer_identity: installer.clone(),
            installer_identity: installer,
            startup_attestation: None,
            last_transition: Transition {
                operation_id: operation,
                kind: "first-install".into(),
                manifest_sha256,
                previous_phase: None,
                next_phase: Phase::Provisioning,
            },
        };
        value.validate(seed)?;
        Ok(value)
    }
    pub(crate) fn digest(&self) -> Result<String> {
        Ok(json::sha256(&json::encode(self, false)?))
    }
    pub(crate) fn validate(&self, seed: &Seed) -> Result<()> {
        self.source_set
            .validate_origin(seed)
            .map_err(|_| Error::StateCorrupt)?;
        if self.host_state_version != 1
            || self.profile_kind != seed.profile_kind
            || self.profile_id != seed.profile_id
            || self.product != seed.product
            || self.component != seed.component
            || self.target_key != seed.target_key
            || self.genesis_seed_sha256 != seed.seed_sha256()
            || self.initial_root_reference != seed.original_root
            || !json::digest(&self.installation_scope_id)
            || !json::digest(&self.genesis_manifest_sha256)
            || !json::digest(&self.original_install_operation_id)
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || self.host_epoch > json::SAFE_MAX
            || self.reference_schema_version != 1
            || !crate::bootstrap::valid_namespaces(&self.known_owner_namespaces, &seed.profile_kind)
            || self.service_identity.service_name != seed.service_name()
            || self.service_identity.account_sid != "S-1-5-18"
            || self.service_identity.sid_mode != "unrestricted"
            || self.last_transition.next_phase != self.phase
            || self.last_transition.operation_id != self.installer_identity.operation_id
            || self.last_transition.manifest_sha256 != self.genesis_manifest_sha256
            || self.source_set.manifest_sha256 != self.genesis_manifest_sha256
            || self.source_set.layout_id != seed.layout_id
            || self.source_set.helper_sha256 != self.helper.sha256
            || self.source_set.launcher_sha256 != self.launcher.sha256
            || self.source_set.app_sha256 != self.app.sha256
            || self.source_set.nsis_sha256 != self.original_installer_identity.source_nsis_sha256
            || self.source_set.source_operation_id != self.original_install_operation_id
            || self.original_installer_identity.operation_id != self.original_install_operation_id
        {
            return Err(Error::StateCorrupt);
        }
        if !matches!(
            self.last_transition.kind.as_str(),
            "first-install"
                | "takeover-original-source"
                | "verify-files"
                | "service-create-intent"
                | "service-configured"
                | "startup-incarnation"
                | "ready"
                | "repair-intent"
                | "removing"
                | "removed-tombstone"
                | "reinstall-same-build"
                | "repair-required"
        ) || self.revision == 1
            && (self.last_transition.previous_phase.is_some()
                || self.last_transition.kind != "first-install")
            || self.revision > 1 && self.last_transition.previous_phase.is_none()
        {
            return Err(Error::StateCorrupt);
        }
        self.helper.validate(seed)?;
        self.launcher.validate(seed)?;
        self.app.validate(seed)?;
        if (self.revision == 1) != (self.previous_record_sha256.is_none())
            || self
                .previous_record_sha256
                .as_ref()
                .is_some_and(|h| !json::digest(h))
        {
            return Err(Error::StateCorrupt);
        }
        for identity in [&self.original_installer_identity, &self.installer_identity] {
            if identity.process_id == 0
                || identity.creation_file_time.ticks() == 0
                || identity.token_user_sid.len() > 192
                || !identity.token_user_sid.starts_with("S-1-")
                || !json::digest(&identity.source_nsis_sha256)
                || !json::digest(&identity.operation_id)
                || identity.manifest_sha256 != self.genesis_manifest_sha256
            {
                return Err(Error::StateCorrupt);
            }
        }
        if self
            .service_identity
            .creation_operation_id
            .as_ref()
            .is_some_and(|id| !json::digest(id))
            || self.service_identity.service_sid.is_some()
                && self.service_identity.creation_operation_id.is_none()
        {
            return Err(Error::StateCorrupt);
        }
        if self
            .service_identity
            .service_sid
            .as_ref()
            .is_some_and(|sid| !sid.starts_with("S-1-5-80-") || sid.len() > 192)
        {
            return Err(Error::StateCorrupt);
        }
        if self.host_epoch == 0 {
            if self.startup_attestation.is_some() || self.phase == Phase::Ready {
                return Err(Error::StateCorrupt);
            }
        } else {
            let a = self
                .startup_attestation
                .as_ref()
                .ok_or(Error::StateCorrupt)?;
            if a.observed_registration_revision == 0
                || a.observed_registration_revision >= self.revision
                || a.process_id == 0
                || a.creation_file_time.ticks() == 0
                || !json::digest(&a.instance_nonce)
                || a.host_epoch != self.host_epoch
                || a.helper_image_sha256 != self.helper.sha256
                || a.manifest_sha256 != self.genesis_manifest_sha256
                || self.service_identity.service_sid.is_none()
            {
                return Err(Error::StateCorrupt);
            }
        }
        Ok(())
    }
    pub(crate) fn next(&self, phase: Phase, kind: &str) -> Result<Self> {
        let mut next = self.clone();
        next.previous_record_sha256 = Some(self.digest()?);
        next.revision = next
            .revision
            .checked_add(1)
            .filter(|n| *n <= json::SAFE_MAX)
            .ok_or(Error::StateCorrupt)?;
        next.phase = phase;
        next.last_transition = Transition {
            operation_id: next.installer_identity.operation_id.clone(),
            kind: kind.into(),
            manifest_sha256: self.genesis_manifest_sha256.clone(),
            previous_phase: Some(self.phase),
            next_phase: phase,
        };
        Ok(next)
    }
    /// Model-only transition. Native source must prove exact tombstone and
    /// actual namespace/service absence under its original CAS before commit.
    pub(crate) fn reinstall_same_build(&self) -> Result<Self> {
        if self.phase != Phase::RemovedTombstone {
            return Err(Error::ScopeConflict);
        }
        let mut next = self.next(Phase::Provisioning, "reinstall-same-build")?;
        next.service_identity.creation_operation_id =
            Some(next.installer_identity.operation_id.clone());
        Ok(next)
    }
    pub(crate) fn public(&self) -> PublicRegistration {
        PublicRegistration {
            host_state_version: self.host_state_version,
            profile_kind: self.profile_kind.clone(),
            profile_id: self.profile_id.clone(),
            product: self.product.clone(),
            component: self.component.clone(),
            target_key: self.target_key.clone(),
            installation_scope_id: self.installation_scope_id.clone(),
            phase: self.phase,
            revision: self.revision,
            host_epoch: self.host_epoch,
            genesis_seed_sha256: self.genesis_seed_sha256.clone(),
            genesis_manifest_sha256: self.genesis_manifest_sha256.clone(),
            initial_root_reference: self.initial_root_reference.clone(),
            helper: self.helper.clone(),
            launcher: self.launcher.clone(),
            app: self.app.clone(),
            service_identity: PublicServiceIdentity {
                service_name: self.service_identity.service_name.clone(),
                account_sid: self.service_identity.account_sid.clone(),
                service_sid: self.service_identity.service_sid.clone(),
                sid_mode: self.service_identity.sid_mode.clone(),
            },
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicRegistration {
    pub(crate) host_state_version: u8,
    pub(crate) profile_kind: String,
    pub(crate) profile_id: String,
    pub(crate) product: String,
    pub(crate) component: String,
    pub(crate) target_key: String,
    pub(crate) installation_scope_id: String,
    pub(crate) phase: Phase,
    pub(crate) revision: u64,
    pub(crate) host_epoch: u64,
    pub(crate) genesis_seed_sha256: String,
    pub(crate) genesis_manifest_sha256: String,
    pub(crate) initial_root_reference: ByteReference,
    pub(crate) helper: ImageReference,
    pub(crate) launcher: ImageReference,
    pub(crate) app: ImageReference,
    pub(crate) service_identity: PublicServiceIdentity,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicServiceIdentity {
    pub(crate) service_name: String,
    pub(crate) account_sid: String,
    pub(crate) service_sid: Option<String>,
    pub(crate) sid_mode: String,
}
#[cfg(test)]
mod privacy_tests {
    use super::*;
    #[test]
    fn complete_record_public_projection_excludes_all_private_installer_operations() {
        // Serialization-only model fixture. No native context/registration
        // store is constructed or trusted from this deserialized description.
        let image = serde_json::json!({"buildId":"1".repeat(64),"version":"0.0.0","size":1,"sha256":"2".repeat(64),"layoutId":"test-layout"});
        let operation = "9".repeat(64);
        let manifest = "3".repeat(64);
        let installer = serde_json::json!({"processId":1,"creationFileTime":"1","tokenUserSid":"S-1-5-18","logonAuthenticationId":{"high":0,"low":1},"sessionId":1,"sourceNsisSha256":"4".repeat(64),"sourceNsisFileIdentity":{"volume":1,"indexHigh":0,"indexLow":1,"createdHigh":0,"createdLow":1},"manifestSha256":manifest,"operationId":operation});
        let record:HostRegistration=serde_json::from_value(serde_json::json!({
            "hostStateVersion":1,"profileKind":"lab","profileId":"serialization-only","product":"pinvou","component":"agent","targetKey":"windows-x86_64-nsis-perMachine",
            "installationScopeId":"5".repeat(64),"genesisSeedSha256":"6".repeat(64),"genesisManifestSha256":manifest,"initialRootReference":{"size":1,"sha256":"7".repeat(64)},
            "helper":image,"launcher":image,"app":image,"serviceIdentity":{"serviceName":"serialization-only","accountSid":"S-1-5-18","serviceSid":null,"sidMode":"unrestricted","creationOperationId":operation},
            "phase":"provisioning","revision":1,"hostEpoch":0,"previousRecordSha256":null,"originalInstallOperationId":operation,"referenceSchemaVersion":1,"knownOwnerNamespaces":["bootstrap-v1"],
            "sourceSet":{"layoutId":"test-layout","nsisSha256":"4".repeat(64),"manifestSha256":manifest,"helperSha256":"2".repeat(64),"launcherSha256":"2".repeat(64),"appSha256":"2".repeat(64),"sourceOperationId":operation},
            "originalInstallerIdentity":installer,"installerIdentity":installer,"startupAttestation":null,"lastTransition":{"operationId":operation,"kind":"first-install","manifestSha256":manifest,"previousPhase":null,"nextPhase":"provisioning"}
        })).unwrap();
        let bytes = json::encode(&record.public(), false).unwrap();
        let value: serde_json::Value = json::decode(&bytes, false).unwrap();
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        let mut expected = vec![
            "hostStateVersion",
            "profileKind",
            "profileId",
            "product",
            "component",
            "targetKey",
            "installationScopeId",
            "phase",
            "revision",
            "hostEpoch",
            "genesisSeedSha256",
            "genesisManifestSha256",
            "initialRootReference",
            "helper",
            "launcher",
            "app",
            "serviceIdentity",
        ];
        expected.sort();
        assert_eq!(keys, expected);
        assert_eq!(value["serviceIdentity"].as_object().unwrap().len(), 4);
        assert!(!String::from_utf8(bytes).unwrap().contains(&operation));
    }
    #[test]
    fn local_origin_is_versioned_history_and_legacy_source_bytes_stay_exact() {
        // Closed serialization model only, never a native source/store owner.
        let legacy_wire = serde_json::json!({
            "hostSeedVersion": 1, "profileKind": "lab", "profileId": "origin-model-only",
            "product": "pinvou", "component": "agent", "targetKey": crate::profile::TARGET,
            "buildId": "1".repeat(64), "appVersion": "0.0.0", "layoutId": crate::profile::LAYOUT,
            "hostIpcVersion": 1, "hostStateVersion": 1,
            "originalRoot": {"size": 1, "sha256": "7".repeat(64)},
            "approvedInitialPublisherPolicy": {"policyId": "legacy-model-only", "signerCertificateSha256": "8".repeat(64)}
        });
        let legacy: Seed = serde_json::from_value(legacy_wire.clone()).unwrap();
        let mut local_wire = legacy_wire;
        local_wire["hostSeedVersion"] = 2.into();
        local_wire
            .as_object_mut()
            .unwrap()
            .remove("approvedInitialPublisherPolicy");
        local_wire["codeImagePolicy"] =
            serde_json::json!({"policyVersion": 1, "certificate": "optional"});
        let local: Seed = serde_json::from_value(local_wire).unwrap();
        let old_source = serde_json::json!({
            "layoutId": crate::profile::LAYOUT, "nsisSha256": "1".repeat(64),
            "manifestSha256": "2".repeat(64), "helperSha256": "3".repeat(64),
            "launcherSha256": "4".repeat(64), "appSha256": "5".repeat(64),
            "sourceOperationId": "6".repeat(64)
        });
        let mut source: SourceSet = serde_json::from_value(old_source.clone()).unwrap();
        source.validate_origin(&legacy).unwrap();
        assert_eq!(
            json::encode(&source, false).unwrap(),
            json::encode(&old_source, false).unwrap()
        );
        assert!(source.validate_origin(&local).is_err());
        source.local_initial_origin = Some(LocalInitialOrigin::administrator_started_installer());
        source.validate_origin(&local).unwrap();
        assert!(source.validate_origin(&legacy).is_err());
        let bytes = json::encode(&source, false).unwrap();
        let roundtrip: SourceSet = json::decode(&bytes, false).unwrap();
        assert_eq!(json::encode(&roundtrip, false).unwrap(), bytes);
        let mut bad = serde_json::to_value(&source).unwrap();
        bad["localInitialOrigin"]["originVersion"] = 2.into();
        assert!(
            serde_json::from_value::<SourceSet>(bad)
                .unwrap()
                .validate_origin(&local)
                .is_err()
        );
        for (key, value) in [
            ("kind", serde_json::json!("metadata-authorized")),
            ("approved", serde_json::json!(true)),
        ] {
            let mut bad = serde_json::to_value(&source).unwrap();
            bad["localInitialOrigin"][key] = value;
            assert!(serde_json::from_value::<SourceSet>(bad).is_err());
        }
    }
    #[test]
    fn historical_sid_epoch_tombstone_reinstall_rotates_exact_intent_without_reset() {
        // Closed model fixture only: no native bootstrap/store capability.
        let seed:Seed=serde_json::from_value(serde_json::json!({"hostSeedVersion":1,"profileKind":"lab","profileId":"model-only","product":"pinvou","component":"agent","targetKey":"windows-x86_64-nsis-perMachine","buildId":"1".repeat(64),"appVersion":"0.0.0","layoutId":"windows-host-v1","hostIpcVersion":1,"hostStateVersion":1,"originalRoot":{"size":1,"sha256":"7".repeat(64)},"approvedInitialPublisherPolicy":{"policyId":"model-only","signerCertificateSha256":"8".repeat(64)}})).unwrap();
        let image = ImageReference {
            build_id: seed.build_id.clone(),
            version: seed.app_version.clone(),
            size: 1,
            sha256: "2".repeat(64),
            layout_id: seed.layout_id.clone(),
        };
        let manifest = InstallationManifest {
            root: crate::profile::RootManifest {
                host_manifest_version: 2,
                profile_kind: seed.profile_kind.clone(),
                profile_id: seed.profile_id.clone(),
                product: seed.product.clone(),
                component: seed.component.clone(),
                target_key: seed.target_key.clone(),
                build_id: seed.build_id.clone(),
                seed_sha256: seed.seed_sha256(),
                original_root: seed.original_root.clone(),
                layout_id: seed.layout_id.clone(),
                helper: image.clone(),
                launcher: image.clone(),
                app: image,
                app_inventory: crate::inventory::InventoryDescriptor {
                    host_inventory_version: 1,
                    file_count: 0,
                    total_size: 0,
                    shards: vec![],
                },
            },
            app_files: vec![],
        };
        let identity=InstallerIdentity{process_id:1,creation_file_time:FileTime::from_native(0,1),token_user_sid:"S-1-5-18".into(),logon_authentication_id:LogonId{high:0,low:1},session_id:1,source_nsis_sha256:"4".repeat(64),source_nsis_file_identity:serde_json::from_value(serde_json::json!({"volume":1,"indexHigh":0,"indexLow":1,"createdHigh":0,"createdLow":1})).unwrap(),manifest_sha256:"3".repeat(64),operation_id:"9".repeat(64)};
        let initial =
            HostRegistration::initial(&seed, &manifest, "5".repeat(64), identity, None).unwrap();
        let mut configured = initial
            .next(Phase::ServiceConfigured, "service-configured")
            .unwrap();
        configured.service_identity.creation_operation_id =
            Some(configured.installer_identity.operation_id.clone());
        configured.service_identity.service_sid = Some("S-1-5-80-1-2-3-4-5".into());
        configured.host_epoch = 7;
        configured.startup_attestation = Some(StartupAttestation {
            process_id: 2,
            creation_file_time: FileTime::from_native(0, 2),
            instance_nonce: "6".repeat(64),
            helper_image_sha256: configured.helper.sha256.clone(),
            manifest_sha256: configured.genesis_manifest_sha256.clone(),
            host_epoch: 7,
            observed_registration_revision: 1,
        });
        configured.validate(&seed).unwrap();
        let removing = configured.next(Phase::Removing, "removing").unwrap();
        let tombstone = removing
            .next(Phase::RemovedTombstone, "removed-tombstone")
            .unwrap();
        tombstone.validate(&seed).unwrap();
        let mut claimed = tombstone
            .next(Phase::RemovedTombstone, "takeover-original-source")
            .unwrap();
        claimed.installer_identity.operation_id = "a".repeat(64);
        claimed.last_transition.operation_id = claimed.installer_identity.operation_id.clone();
        claimed.validate(&seed).unwrap();
        let mut old_invalid = claimed
            .next(Phase::Provisioning, "reinstall-same-build")
            .unwrap();
        old_invalid.service_identity.creation_operation_id = None;
        assert_eq!(old_invalid.validate(&seed), Err(Error::StateCorrupt));
        let next = claimed.reinstall_same_build().unwrap();
        next.validate(&seed).unwrap();
        assert_eq!(
            next.service_identity.creation_operation_id,
            Some("a".repeat(64))
        );
        assert_eq!(
            next.service_identity.service_sid,
            claimed.service_identity.service_sid
        );
        assert_eq!(next.host_epoch, 7);
        assert_eq!(next.installation_scope_id, initial.installation_scope_id);
        assert_eq!(
            next.genesis_manifest_sha256,
            initial.genesis_manifest_sha256
        );
        assert_eq!(next.genesis_seed_sha256, initial.genesis_seed_sha256);
        assert_eq!(next.previous_record_sha256, Some(claimed.digest().unwrap()));
        let verified = next.next(Phase::FilesVerified, "verify-files").unwrap();
        verified.validate(&seed).unwrap();
        let configured = verified
            .next(Phase::ServiceConfigured, "service-configured")
            .unwrap();
        configured.validate(&seed).unwrap();
        assert_eq!(
            configured.service_identity.creation_operation_id,
            next.service_identity.creation_operation_id
        );
        assert_eq!(configured.host_epoch, 7);
    }
    #[test]
    fn public_service_projection_cannot_encode_or_accept_private_creation_operation() {
        let value = PublicServiceIdentity {
            service_name: "example".into(),
            account_sid: "S-1-5-18".into(),
            service_sid: None,
            sid_mode: "unrestricted".into(),
        };
        let bytes = json::encode(&value, false).unwrap();
        let object: serde_json::Value = json::decode(&bytes, false).unwrap();
        let mut names: Vec<_> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["accountSid", "serviceName", "serviceSid", "sidMode"]
        );
        let mut bad = object;
        bad.as_object_mut().unwrap().insert(
            "creationOperationId".into(),
            serde_json::Value::String("1".repeat(64)),
        );
        assert!(
            json::decode::<PublicServiceIdentity>(&json::encode(&bad, false).unwrap(), false)
                .is_err()
        );
    }
}

impl PublicRegistration {
    pub(crate) fn validate(&self, seed: &Seed) -> Result<()> {
        if self.host_state_version != 1
            || self.profile_kind != seed.profile_kind
            || self.profile_id != seed.profile_id
            || self.product != seed.product
            || self.component != seed.component
            || self.target_key != seed.target_key
            || !json::digest(&self.installation_scope_id)
            || self.genesis_seed_sha256 != seed.seed_sha256()
            || !json::digest(&self.genesis_manifest_sha256)
            || self.initial_root_reference != seed.original_root
            || self.revision == 0
            || self.revision > json::SAFE_MAX
            || self.host_epoch > json::SAFE_MAX
            || self.service_identity.service_name != seed.service_name()
            || self.service_identity.account_sid != "S-1-5-18"
            || self.service_identity.sid_mode != "unrestricted"
        {
            return Err(Error::StateCorrupt);
        }
        self.helper.validate(seed)?;
        self.launcher.validate(seed)?;
        self.app.validate(seed)?;
        if self
            .service_identity
            .service_sid
            .as_ref()
            .is_some_and(|sid| !sid.starts_with("S-1-5-80-") || sid.len() > 192)
            || self.phase == Phase::Ready
                && (self.host_epoch == 0 || self.service_identity.service_sid.is_none())
        {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
    pub(crate) fn digest(&self) -> Result<String> {
        Ok(json::sha256(&json::encode(self, false)?))
    }
}
