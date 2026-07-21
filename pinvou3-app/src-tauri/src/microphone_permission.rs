#[cfg(target_os = "windows")]
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(target_os = "windows")]
async fn reset_windows_microphone_permission(window: tauri::WebviewWindow) -> Result<(), String> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
        },
        SetPermissionStateCompletedHandler,
    };
    use windows::core::{Interface, HSTRING};

    let origin = window
        .url()
        .map_err(|error| format!("读取当前页面地址失败：{error}"))?
        .origin()
        .ascii_serialization();
    if origin == "null" {
        return Err("当前页面没有可重置的 WebView2 权限来源".to_string());
    }

    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    window
        .with_webview(move |webview| {
            let callback_sender = Arc::clone(&sender);
            let schedule_result: windows::core::Result<()> = (|| unsafe {
                let webview13 = webview
                    .controller()
                    .CoreWebView2()?
                    .cast::<ICoreWebView2_13>()?;
                let profile = webview13.Profile()?.cast::<ICoreWebView2Profile4>()?;
                let callback = SetPermissionStateCompletedHandler::create(Box::new(
                    move |completion_result| {
                        if let Some(sender) = callback_sender
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take()
                        {
                            let _ =
                                sender.send(completion_result.map_err(|error| error.to_string()));
                        }
                        Ok(())
                    },
                ));
                profile.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                    &HSTRING::from(origin),
                    COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
                    &callback,
                )
            })();

            if let Err(error) = schedule_result {
                if let Some(sender) = sender
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                {
                    let _ = sender.send(Err(error.to_string()));
                }
            }
        })
        .map_err(|error| format!("访问 Windows WebView2 失败：{error}"))?;

    tokio::time::timeout(Duration::from_secs(3), receiver)
        .await
        .map_err(|_| "重置麦克风权限超时".to_string())?
        .map_err(|_| "麦克风权限重置任务被取消".to_string())??;
    Ok(())
}

#[tauri::command]
pub async fn reset_microphone_permission(window: tauri::WebviewWindow) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        reset_windows_microphone_permission(window).await?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        Ok(false)
    }
}
