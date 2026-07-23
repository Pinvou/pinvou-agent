pub async fn reset_microphone_permission(window: tauri::WebviewWindow) -> Result<bool, String> {
    super::platform::reset_microphone_permission(window).await
}
