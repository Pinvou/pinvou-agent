use super::*;
pub(super) fn check(claims: &Value, role: &str) -> Result<(), CredentialSemanticsError> {
    if role.ends_with("-event") {
        let transaction = role.contains("-transaction-");
        let anchor = if transaction {
            number(&claims["transactionStartedAt"])?
        } else {
            number(nonnull(claims, "sessionStartedAt").unwrap_or(&claims["taskStartedAt"]))?
        };
        let event = number(&claims["eventNotAfter"])?;
        let nbf = number(&claims["nbf"])?;
        rule(
            number(&claims["iat"])? >= anchor
                && nbf >= anchor
                && event > nbf
                && event <= anchor + if transaction { 30.0 * DAY } else { DAY }
                && number(&claims["exp"])?
                    <= anchor + if transaction { 37.0 * DAY } else { 7.0 * DAY },
            CredentialSemanticsError::EventWindow,
        )?;
    }
    if let Some(task) = nonnull(claims, "task") {
        rule(
            task["taskPurpose"] == claims["purpose"]
                && number(&task["windowEnd"])? > number(&task["windowStart"])?,
            CredentialSemanticsError::TaskBinding,
        )?;
        rule(
            task["packageManifest"]["role"] == "package"
                && task["packageManifest"]["scope"]["targetKey"] == claims["scope"]["targetKey"]
                && same(
                    &task["packageManifest"],
                    &task["hopChain"]["packageManifest"],
                )?
                && task["packageId"] == task["hopChain"]["packageId"]
                && same(&task["package"], &task["hopChain"]["package"])?,
            CredentialSemanticsError::TaskTarget,
        )?;
        rule(
            higher(&task["targetVersion"], &task["sourceVersion"])?
                && task["activationMode"]
                    == if task["upgradeType"] == "silent" {
                        "stagedRestart"
                    } else {
                        "directInstall"
                    }
                && if task["originalPurpose"] == "install" {
                    task["upgradeType"] != "silent"
                } else {
                    task["upgradeType"] == "silent"
                },
            CredentialSemanticsError::TaskTarget,
        )?;
        rule(
            task["taskPurpose"] != "reconcile" || !task["retirementProofSha256"].is_null(),
            CredentialSemanticsError::TaskProof,
        )?;
        if ["recovery-review-event", "reconciliation-event"].contains(&role) {
            rule(
                number(&claims["nbf"])? >= number(&task["windowStart"])?
                    && number(&claims["eventNotAfter"])? <= number(&task["windowEnd"])?,
                CredentialSemanticsError::TaskWindow,
            )?;
        }
    }
    Ok(())
}
