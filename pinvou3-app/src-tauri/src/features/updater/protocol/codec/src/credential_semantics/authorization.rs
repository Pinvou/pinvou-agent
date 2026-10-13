use super::*;
pub(super) fn check(claims: &Value, now: f64) -> Result<(), CredentialSemanticsError> {
    let preinstall = claims["purpose"] == "preinstall";
    let update = &claims["update"];
    rule(
        claims["authorizationJti"] == claims["jti"],
        CredentialSemanticsError::AuthorizationId,
    )?;
    rule(
        if preinstall {
            claims["freezeEpoch"].is_null() && claims["backup"].is_null()
        } else {
            !claims["freezeEpoch"].is_null()
        },
        CredentialSemanticsError::FreezeBinding,
    )?;
    rule(
        preinstall || (update["backupPolicy"] == "required") != claims["backup"].is_null(),
        CredentialSemanticsError::BackupBinding,
    )?;
    if let Some(backup) = nonnull(claims, "backup") {
        rule(
            backup["freezeEpoch"] == claims["freezeEpoch"]
                && backup["preparationEpoch"] == claims["preparation"]["ownerEpoch"],
            CredentialSemanticsError::BackupBinding,
        )?;
    }
    rule(
        (claims["purpose"] == "activate") != claims["staged"].is_null(),
        CredentialSemanticsError::StagedBinding,
    )?;
    rule(
        preinstall != claims["preinstallSlot"].is_null(),
        CredentialSemanticsError::SlotBinding,
    )?;
    rule(
        if claims["purpose"] == "install" {
            update["upgradeType"] != "silent"
        } else {
            update["upgradeType"] == "silent"
        },
        CredentialSemanticsError::UpgradePurpose,
    )?;
    rule(
        update["activationMode"]
            == if preinstall || claims["purpose"] == "activate" {
                "stagedRestart"
            } else {
                "directInstall"
            },
        CredentialSemanticsError::ActivationMode,
    )?;
    for chain in [&update["endpointChain"], &update["hopChain"]] {
        if !preinstall {
            rule(
                now >= number(&chain["installNotBefore"])?,
                CredentialSemanticsError::InstallNotYet,
            )?;
        }
        let end = &chain["installNotAfter"];
        rule(
            now >= number(&chain["releaseVisibleAt"])? && (end.is_null() || now < number(end)?),
            CredentialSemanticsError::InstallWindowClosed,
        )?;
    }
    if let Some(staged) = nonnull(claims, "staged") {
        rule(
            claims["transactionId"] != staged["preinstallTransactionId"],
            CredentialSemanticsError::StagedTransaction,
        )?;
        let start = number(&staged["stagedAt"])?;
        let sum = start + 30.0 * DAY;
        let end = &update["hopChain"]["installNotAfter"];
        let limit = if end.is_null() {
            9_007_199_254_740_991.0
        } else {
            number(end)?
        };
        let valid = number(&staged["stagedValidUntil"])?;
        rule(
            start <= number(&claims["iat"])?
                && sum.is_finite()
                && sum.fract() == 0.0
                && sum.abs() <= 9_007_199_254_740_991.0
                && valid == sum.min(limit)
                && valid > start
                && now < valid,
            CredentialSemanticsError::StagedWindow,
        )?;
    }
    Ok(())
}
