//! Actual original NSIS -> native helper ownership, complete bytes and intent.
//! Public Seed/manifest/entry strings never independently grant source authority.
use super::{
    image::{SealedImage, manifest_from_installer, verify_bytes},
    layout::{Layout, ProtectedDirectory, ordinal_equal},
    process::RetainedProcess,
    registry::Store,
};
use crate::{
    error::{Error, Result},
    json,
    profile::{ImageCertificatePolicy, InstallationManifest, Seed},
    registration::LocalInitialOrigin,
};
use std::path::Path;

// Entry semantics only. The native tuple and original protected namespace below
// establish authority; neither enum can be deserialized or passed over IPC.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallerEntry {
    OriginalSignedOrRetained,
    LocalAdministrativeInstall,
}
enum InitialAuthority {
    LegacySelectedSigner,
    LocalAdministrativeInstall,
}
pub(super) struct SourceInstallerContext {
    pub(super) seed: Seed,
    pub(super) layout: Layout,
    pub(super) source: ProtectedDirectory,
    pub(super) installer: RetainedProcess,
    pub(super) installer_image: SealedImage,
    pub(super) helper_image: SealedImage,
    pub(super) manifest: InstallationManifest,
    pub(super) manifest_bytes: crate::inventory::InventoryBytes,
    pub(super) manifest_sha256: String,
    initial_authority: Option<InitialAuthority>,
}
fn exact_path(actual: &Path, expected: &Path) -> Result<()> {
    let actual = actual.to_str().ok_or(Error::InstallerContextRefused)?;
    let expected = expected.to_str().ok_or(Error::InstallerContextRefused)?;
    if ordinal_equal(actual, expected)? {
        Ok(())
    } else {
        Err(Error::InstallerContextRefused)
    }
}
impl SourceInstallerContext {
    pub(super) fn from_actual_invocation() -> Result<Self> {
        Self::from_invocation(InstallerEntry::OriginalSignedOrRetained)
    }
    pub(super) fn from_local_initial_invocation() -> Result<Self> {
        Self::from_invocation(InstallerEntry::LocalAdministrativeInstall)
    }
    fn from_invocation(entry: InstallerEntry) -> Result<Self> {
        let seed = Seed::embedded()?;
        let policy = seed.certificate_policy()?;
        if entry == InstallerEntry::LocalAdministrativeInstall
            && policy != ImageCertificatePolicy::Optional
        {
            return Err(Error::InstallerContextRefused);
        }
        let layout = Layout::discover(&seed)?;
        let current = RetainedProcess::current()?;
        let installer = RetainedProcess::actual_parent()?;
        // Real same-user includes the SID, original logon and session. This
        // tuple establishes an elevated local installer request, not a claim
        // that a fresh UAC dialog or additional human click just occurred.
        if installer.selector.creation_file_time.ticks()
            >= current.selector.creation_file_time.ticks()
            || installer.token.user != current.token.user
            || !installer.token.elevated
            || !current.token.elevated
            || !installer.token.administrator
            || !current.token.administrator
            || installer.token.integrity < 0x3000
            || current.token.integrity < 0x3000
            || installer.token.app_container
            || current.token.app_container
            || installer.token.network
            || current.token.network
        {
            return Err(Error::InstallerContextRefused);
        }
        exact_path(
            &current.image,
            &layout.source.join("pinvou-upgrade-helper.exe"),
        )?;
        exact_path(&installer.image, &layout.source.join("installer.exe"))?;
        let source = layout.source_root()?;
        let stored = match policy {
            ImageCertificatePolicy::LegacySelectedSigner(_) => None,
            ImageCertificatePolicy::Optional => Store::new(&seed).read(&seed)?,
        };
        let (helper_image, installer_image, initial_authority) = match (policy, stored.as_ref()) {
            (ImageCertificatePolicy::LegacySelectedSigner(_), _) => (
                SealedImage::open_original(&source, "pinvou-upgrade-helper.exe", &seed)?,
                SealedImage::open_original(&source, "installer.exe", &seed)?,
                Some(InitialAuthority::LegacySelectedSigner),
            ),
            (ImageCertificatePolicy::Optional, Some(record)) => (
                SealedImage::open(&source, "pinvou-upgrade-helper.exe", &record.helper, &seed)?,
                SealedImage::open_registered_installer(&source, record, &seed)?,
                None,
            ),
            (ImageCertificatePolicy::Optional, None)
                if entry == InstallerEntry::LocalAdministrativeInstall =>
            {
                (
                    SealedImage::observe_local_original(
                        &source,
                        "pinvou-upgrade-helper.exe",
                        &seed,
                    )?,
                    SealedImage::observe_local_original(&source, "installer.exe", &seed)?,
                    Some(InitialAuthority::LocalAdministrativeInstall),
                )
            }
            // An existing-stage or metadata invocation cannot bootstrap from
            // missing state, a selfhash or absent/failed certificate proof.
            _ => return Err(Error::InitialTrustUnavailable),
        };
        let (manifest, manifest_bytes) = manifest_from_installer(
            &installer_image,
            &layout.source.join("installer.exe"),
            &seed,
        )?;
        if !helper_image.matches(&manifest.helper) {
            return Err(Error::ImageRefused);
        }
        let manifest_sha256 = json::sha256(&manifest_bytes.root);
        if let Some(record) = stored.as_ref() {
            if record.genesis_manifest_sha256 != manifest_sha256
                || record.source_set.nsis_sha256 != installer_image.sha256
                || record.helper != manifest.helper
                || record.launcher != manifest.launcher
                || record.app != manifest.app
            {
                return Err(Error::ScopeConflict);
            }
        }
        // Every declared member is checked against the complete original
        // manifest. Original copy_exact later rechecks each actual held source;
        // no extraction/TEMP, new expected hash or lazy inventory fallback.
        let app = source.child("app", false, false, None)?;
        app.validate_inventory(&manifest.app_files)?;
        for member in &manifest.app_files {
            let mut file = app.relative_file(&member.relative_name)?;
            verify_bytes(&mut file, member.size, &member.sha256)?;
        }
        let _launcher = SealedImage::open(
            &source,
            "pinvou-upgrade-launcher.exe",
            &manifest.launcher,
            &seed,
        )?;
        let _app = SealedImage::open(&app, "pinvou3-tauri.exe", &manifest.app, &seed)?;
        let value = Self {
            seed,
            layout,
            source,
            installer,
            installer_image,
            helper_image,
            manifest,
            manifest_bytes,
            manifest_sha256,
            initial_authority,
        };
        value.recheck()?;
        Ok(value)
    }
    /// Called only under the original BootstrapGuard, immediately before the
    /// first original intent. A retained-intent owner cannot recreate absence.
    pub(super) fn consume_initial_origin(&mut self) -> Result<Option<LocalInitialOrigin>> {
        match self.initial_authority.take() {
            Some(InitialAuthority::LegacySelectedSigner) => Ok(None),
            Some(InitialAuthority::LocalAdministrativeInstall) => {
                Ok(Some(LocalInitialOrigin::administrator_started_installer()))
            }
            None => Err(Error::ScopeConflict),
        }
    }
    /// A fresh local request must not adopt a record that appeared after its
    /// initial absence observation. Existing-source takeover stays unchanged.
    pub(super) fn require_existing_intent(&self) -> Result<()> {
        if matches!(
            self.initial_authority.as_ref(),
            Some(InitialAuthority::LocalAdministrativeInstall)
        ) {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(super) fn recheck(&self) -> Result<()> {
        if self.installer.exited()? {
            return Err(Error::InstallerContextRefused);
        }
        self.source.recheck()?;
        if pinvou_upgrade_custody::identity(&self.installer_image.file)
            .map_err(|_| Error::ImageRefused)?
            != self.installer_image.identity
            || pinvou_upgrade_custody::identity(&self.helper_image.file)
                .map_err(|_| Error::ImageRefused)?
                != self.helper_image.identity
        {
            return Err(Error::ImageRefused);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_test_parent_is_never_an_installer() {
        assert!(SourceInstallerContext::from_actual_invocation().is_err());
        assert!(SourceInstallerContext::from_local_initial_invocation().is_err());
        // No target directory/registry/service effects are made by these refusals.
    }
}
