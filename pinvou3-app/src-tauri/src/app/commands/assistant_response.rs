use std::path::Path;

use tauri_plugin_dialog::DialogExt;

const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;

fn export_extension(format: &str) -> Option<&'static str> {
    match format {
        "md" => Some("md"),
        "html" => Some("html"),
        _ => None,
    }
}

fn normalized_export_name(default_name: &str, extension: &str) -> String {
    let fallback = format!("pinvou-response.{extension}");
    let Some(name) = Path::new(default_name)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return fallback;
    };
    let stem = name
        .trim()
        .trim_end_matches(&format!(".{extension}"))
        .trim_end_matches(['.', ' ']);
    if stem.is_empty()
        || stem.len() > 120
        || stem
            .chars()
            .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
    {
        fallback
    } else {
        format!("{stem}.{extension}")
    }
}

#[tauri::command]
pub async fn export_assistant_response(
    app: tauri::AppHandle,
    content: String,
    default_name: String,
    format: String,
) -> Result<bool, String> {
    let extension =
        export_extension(&format).ok_or_else(|| "unsupported_export_format".to_string())?;
    if content.len() > MAX_EXPORT_BYTES {
        return Err("assistant_export_too_large".to_string());
    }
    let filename = normalized_export_name(&default_name, extension);
    let filter_label = if extension == "md" {
        "Markdown"
    } else {
        "HTML"
    };
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(&filename)
        .add_filter(filter_label, &[extension])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("resolve_export_path_failed: {error}"))?;
    tokio::task::spawn_blocking(move || std::fs::write(&path, content))
        .await
        .map_err(|error| format!("assistant_export_task_failed: {error}"))?
        .map_err(|error| format!("assistant_export_write_failed: {error}"))?;
    Ok(true)
}

fn share_target_uri(target: &str) -> Option<(&'static str, &'static str)> {
    match target {
        "wechat" => Some(("weixin://", "微信")),
        "wecom" => Some(("wxwork://", "企业微信")),
        "feishu" => Some(("feishu://applink/client/op/open", "飞书")),
        "dingtalk" => Some(("dingtalk://", "钉钉")),
        "qq" => Some(("tencent://", "QQ")),
        _ => None,
    }
}

#[tauri::command]
pub fn open_assistant_share_target(target: String) -> Result<(), String> {
    let (uri, label) =
        share_target_uri(&target).ok_or_else(|| "unsupported_share_target".to_string())?;
    crate::platform::os::open_target(uri, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_name_cannot_escape_or_change_extension() {
        assert_eq!(
            normalized_export_name("../answer.exe", "md"),
            "answer.exe.md"
        );
        assert_eq!(normalized_export_name("answer.md", "md"), "answer.md");
        assert_eq!(
            normalized_export_name("bad:name.html", "html"),
            "pinvou-response.html"
        );
    }

    #[test]
    fn share_targets_are_a_fixed_allowlist() {
        assert_eq!(
            share_target_uri("feishu").map(|value| value.0),
            Some("feishu://applink/client/op/open")
        );
        assert!(share_target_uri("https://example.com").is_none());
    }
}
