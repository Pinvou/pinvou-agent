//! Application restart ordering. Native host identity stays in the updater host.

#[cfg(all(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
compile_error!("select exactly one upgrade host profile");

/// Preserve the stock restart unless this build explicitly enables a host profile.
/// Host failures keep the original application process alive. Cleanup may have
/// stopped browser/child processes already; this does not promise to restore them.
pub(crate) async fn restart_application(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        // This returns only after the same authenticated association is armed.
        // Its worker retains the connection and keeps it alive during cleanup.
        let prepared = tokio::task::spawn_blocking(pinvou_upgrade_host::prepare_restart)
            .await
            .map_err(|_| pinvou_upgrade_host::Error::Unavailable.to_string())?
            .map_err(|error| error.to_string())?;

        crate::prepare_app_restart(&app).await;

        // No timeout, lost reply or unknown outcome is permission to exit. The
        // original guard owns cancellation and worker shutdown on every error.
        let committed = tokio::task::spawn_blocking(move || prepared.commit_exit())
            .await
            .map_err(|_| pinvou_upgrade_host::Error::Unavailable.to_string())?
            .map_err(|error| error.to_string())?;
        // The async executor may resume after the original exit grace expired.
        // A previously valid acknowledgement cannot extend that same grant.
        if !committed.is_current() {
            return Err(pinvou_upgrade_host::Error::RestartCanceled.to_string());
        }
        app.exit(0);
        Ok(())
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        crate::prepare_app_restart(&app).await;
        app.restart();
    }
}
