//! Computer Use 同意门控命令面。
//!
//! 引擎当前自动批准所有工具调用，同意门控内建于工具自身（`ComputerUseShared`
//! 是唯一事实来源）；这些命令是前端注入用户决定的唯一入口：
//! 设置开关 / 会话授权 / 吊销 / 急停 / T3 确认 / 平台权限引导。
//! 授权与确认令牌只活于内存，落盘的只有 `computer_use.enabled` 总开关。

use std::sync::Arc;

use super::prelude::*;
use crate::features::computer_use::ComputerUseShared;

/// `computer_use_get_status` 的返回投影（前端据此渲染授权/急停状态）。
#[derive(Debug, Clone, Serialize)]
pub struct ComputerUseStatus {
    /// 设置总开关（settings.json `computer_use.enabled` 的内存镜像）。
    pub enabled: bool,
    /// 该会话当前持有有效输入授权（未空闲过期、未被吊销/急停清除）。
    pub granted: bool,
    /// 急停旗标（`computer_use_stop` 置位，重新开启总开关时清除）。
    pub stopped: bool,
    /// 当前操作系统是否有 computer_use 后端实现。
    pub platform_supported: bool,
}

#[tauri::command]
pub fn computer_use_get_status(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> ComputerUseStatus {
    ComputerUseStatus {
        enabled: shared.is_enabled(),
        granted: shared.has_active_grant(&session_id),
        stopped: shared.is_stopped(),
        platform_supported: crate::features::computer_use::backend_supported(),
    }
}

/// 授予本会话鼠标/键盘控制权（一次性会话授权，10 分钟空闲或 500 次输入
/// 动作后自动失效，需重新授权）。
#[tauri::command]
pub fn computer_use_grant(session_id: String, shared: State<'_, Arc<ComputerUseShared>>) {
    shared.grant_session(&session_id);
}

/// 吊销本会话输入授权。
#[tauri::command]
pub fn computer_use_revoke(session_id: String, shared: State<'_, Arc<ComputerUseShared>>) {
    shared.revoke_session(&session_id);
}

/// 紧急停止：置停止旗标并吊销全部会话授权。
#[tauri::command]
pub fn computer_use_stop(shared: State<'_, Arc<ComputerUseShared>>) {
    shared.stop_all();
}

/// 用户在前端确认一个被拦截的 T3 后果性动作：铸造单次批准令牌。
/// confirm_id 必须来自 `computer_use:confirm_required` 事件（pending 中），
/// 未知 id 报错，避免为模型自造的 id 铸币。
#[tauri::command]
pub fn computer_use_confirm(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    if shared.pending_confirmation(&confirm_id).is_none() {
        return Err(format!("unknown or expired confirm_id: {confirm_id}"));
    }
    shared.mint_confirmation(&confirm_id);
    Ok(())
}

/// 用户在前端明确「拒绝」一个被拦截的 T3 动作：清除 pending 并短期记忆
/// 该决定。没有这条命令时，拒绝只关前端对话框，后端只能靠 TTL 过期——
/// 模型在超时窗口内重试会被再次放行到确认流程（评审发现）。
#[tauri::command]
pub fn computer_use_deny(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    if shared.deny_confirmation(&confirm_id) {
        Ok(())
    } else {
        Err(format!(
            "unknown or expired confirm_id (already decided?): {confirm_id}"
        ))
    }
}

/// 设置总开关。先落盘后翻内存旗标（同 set_voice_shortcut_enabled 的顺序）：
/// 写盘失败时内存态不得与 settings.json 不一致。重新开启时清除急停旗标
/// （guard 的既定语义），但不恢复任何会话授权。
#[tauri::command]
pub fn computer_use_set_enabled(
    enabled: bool,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    UserPrefs::update_transaction(|prefs| {
        prefs.computer_use.enabled = enabled;
        Ok(())
    })?;
    shared.set_enabled(enabled);
    if enabled {
        shared.reset_stop();
    }
    Ok(())
}

/// 触发平台授权引导：macOS 弹 Screen Recording + Accessibility 系统窗；
/// Windows/Linux 无系统授权流程，显式返回 unsupported 错误（不静默 no-op）。
#[tauri::command]
pub fn computer_use_request_permissions() -> Result<(), String> {
    crate::features::computer_use::request_permissions().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态投影的 JSON 键是前端契约（computerUse feature 直接消费）。
    #[test]
    fn status_serializes_contract_keys() {
        let status = ComputerUseStatus {
            enabled: true,
            granted: false,
            stopped: false,
            platform_supported: true,
        };
        let Ok(value) = serde_json::to_value(&status) else {
            panic!("ComputerUseStatus must serialize");
        };
        assert_eq!(
            value,
            serde_json::json!({
                "enabled": true,
                "granted": false,
                "stopped": false,
                "platform_supported": true,
            })
        );
    }
}
