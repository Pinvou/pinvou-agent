use super::*;
pub(super) fn check(
    update: &Value,
    claims: &Value,
    now: f64,
) -> Result<(), CredentialSemanticsError> {
    let candidate = update["endpointKind"] == "candidate";
    let endpoint = &update["endpointChain"];
    let hop = &update["hopChain"];
    rule(
        update["activationMode"]
            == if update["upgradeType"] == "silent" {
                "stagedRestart"
            } else {
                "directInstall"
            },
        CredentialSemanticsError::UpgradeMode,
    )?;
    if !candidate {
        rule(
            endpoint["deploymentId"] == update["baselineDeploymentId"]
                && endpoint["deploymentRevision"] == update["baselineRevision"],
            CredentialSemanticsError::BaselineBinding,
        )?;
    }
    if endpoint["deploymentId"] == hop["deploymentId"] {
        rule(same(endpoint, hop)?, CredentialSemanticsError::EntityChain)?;
    }
    rule(
        (update["migrationMode"] != "none") != update["plans"]["migration"].is_null()
            && (update["backupPolicy"] == "required") != update["plans"]["backup"].is_null(),
        CredentialSemanticsError::PlanBinding,
    )?;
    rule(
        higher(&update["targetVersion"], &claims["currentVersion"])?,
        CredentialSemanticsError::VersionNotHigher,
    )?;
    rule(
        endpoint["deploymentState"] == "active"
            && hop["deploymentState"]
                == if update["hopKind"] == "bridge" {
                    "superseded"
                } else {
                    "active"
                },
        CredentialSemanticsError::EntityState,
    )?;
    rule(
        candidate != update["rollout"].is_null(),
        CredentialSemanticsError::RolloutBinding,
    )?;
    let bridge = update["hopKind"] == "bridge";
    rule(
        bridge != update["bridge"].is_null(),
        CredentialSemanticsError::BridgeBinding,
    )?;
    if bridge {
        rule(
            update["ordinaryPathApproval"].is_null(),
            CredentialSemanticsError::PathBinding,
        )?;
    } else {
        rule(
            endpoint["deploymentId"] == hop["deploymentId"]
                || (hop["deploymentId"] == update["baselineDeploymentId"]
                    && hop["deploymentRevision"] == update["baselineRevision"])
                || !update["ordinaryPathApproval"].is_null(),
            CredentialSemanticsError::PathMissing,
        )?;
    }
    rule(
        update["migrationMode"] != "irreversible" || update["backupPolicy"] == "required",
        CredentialSemanticsError::BackupPolicy,
    )?;
    rule(
        update["packageId"] == hop["packageId"],
        CredentialSemanticsError::PackageBinding,
    )?;
    for chain in [endpoint, hop] {
        rule(
            chain["releaseManifest"]["role"] == "release"
                && chain["releaseManifest"]["scope"]["releaseId"] == chain["releaseId"]
                && chain["releaseManifest"]["version"] == chain["releaseRevision"],
            CredentialSemanticsError::ReleaseBinding,
        )?;
        rule(
            chain["packageManifest"]["role"] == "package",
            CredentialSemanticsError::ReferenceRole,
        )?;
        rule(
            chain["packageManifest"]["scope"]["targetKey"] == claims["scope"]["targetKey"],
            CredentialSemanticsError::TargetBinding,
        )?;
        let visible = number(&chain["releaseVisibleAt"])?;
        let start = number(&chain["installNotBefore"])?;
        let end = &chain["installNotAfter"];
        rule(
            end.is_null() || number(end)? > visible.max(start),
            CredentialSemanticsError::InstallWindowInvalid,
        )?;
        rule(
            now >= visible && (end.is_null() || now < number(end)?),
            CredentialSemanticsError::InstallWindowClosed,
        )?;
    }
    if candidate {
        let rollout = &update["rollout"];
        let opening = &rollout["opening"];
        rule(
            opening["targetKey"] == claims["scope"]["targetKey"]
                && opening["deploymentId"] == endpoint["deploymentId"]
                && opening["deploymentRevision"] == endpoint["deploymentRevision"]
                && opening["rolloutId"] == rollout["rolloutId"]
                && opening["rolloutRevision"] == rollout["revision"]
                && opening["releaseEnvelopeSha256"]
                    == endpoint["releaseManifest"]["envelopeSha256"],
            CredentialSemanticsError::OpeningBinding,
        )?;
        let locator = number(&rollout["locatorExpiresAt"])?;
        let iat = number(&claims["iat"])?;
        rule(
            locator > iat && locator <= number(&claims["exp"])?.min(iat + 900_000.0),
            CredentialSemanticsError::LocatorWindow,
        )?;
    }
    Ok(())
}
