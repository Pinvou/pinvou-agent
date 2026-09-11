//! Computer Use 同意门控命令面。
//!
//! 引擎当前自动批准所有工具调用，同意门控内建于工具自身（`ComputerUseShared`
//! 是唯一事实来源）；这些命令是前端注入用户决定的唯一入口：
//! 设置开关 / 会话授权 / 吊销 / 急停 / T3 确认 / 平台权限引导。
//! 授权与确认令牌只活于内存，落盘的只有 `computer_use.enabled` 总开关。

use std::sync::Arc;

use super::prelude::*;
use crate::features::computer_use::ComputerUseShared;

/// grant/revoke（session_id）与 confirm/deny（confirm_id）的标识符空串防御
/// （评审发现）：空串既无业务意义，也不应静默成功污染守卫状态。
fn ensure_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

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

/// 授予本会话鼠标/键盘控制权（一次性会话授权，10 分钟空闲后自动失效，需
/// 重新授权；空闲时钟只由**执行成功**的输入动作续期——被拦/被拒的调用
/// 不续期）。
#[tauri::command]
pub fn computer_use_grant(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    shared.grant_session(&session_id);
    Ok(())
}

/// 吊销本会话输入授权。应用内授权即时失效；同时触发后端关闭持久 OS 级
/// 授权（Wayland portal 会话）——detached 线程执行，不阻塞本命令
/// （评审发现：授权此前会活到进程退出，与"可随时停止"承诺不符）。
/// Emergency (not plain) release: a physically held left button is
/// unpressed first, and the control lane survives an in-flight action.
#[tauri::command]
pub fn computer_use_revoke(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    shared.revoke_session(&session_id);
    shared.backends.emergency_release(&session_id);
    Ok(())
}

/// 紧急停止：置停止旗标并吊销全部会话授权，同时触发所有后端关闭持久
/// OS 级授权（detached 线程，见 [`computer_use_revoke`]）。
#[tauri::command]
pub fn computer_use_stop(shared: State<'_, Arc<ComputerUseShared>>) {
    shared.stop_all();
    shared.backends.emergency_release_all();
}

/// 用户在前端确认一个被拦截的 T3 后果性动作：铸造单次批准令牌。
/// confirm_id 必须来自 `computer_use:confirm_required` 事件（pending 中），
/// 未知 id 报错，避免为模型自造的 id 铸币。
#[tauri::command]
pub fn computer_use_confirm(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
    // mint_confirmation 只为存在且未过期的 pending 铸币并返回 true（评审
    // 发现：此前静默 no-op，前端把失败显示为成功）；false 显式报错。
    // 括注保留 "unknown or expired"：前端 bridge 以该短语识别「pending 已
    // 过期」并本地清理确认弹窗（过期不是用户拒绝），文案不得破坏该契约。
    if shared.mint_confirmation(&confirm_id) {
        Ok(())
    } else {
        Err(format!(
            "confirmation request no longer exists (unknown or expired): {confirm_id}"
        ))
    }
}

/// 用户在前端明确「拒绝」一个被拦截的 T3 动作：清除 pending。拒绝不记录
/// 任何服务端状态——模型的同动作重试会照常触发筛查并铸造**新的** pending、
/// 重发确认事件（主流模型：拒绝只是模型可见的上下文，不是存储的惩罚状态）。
#[tauri::command]
pub fn computer_use_deny(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
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
/// （guard 的既定语义），但不恢复任何会话授权；关闭时吊销全部会话授权并
/// 清空待决确认（评审发现：否则重开后旧 grant 在 10 分钟窗口内仍有效）。
/// 最后热刷 disallowed_tools（评审发现：tool_policy 闭包只在 refresh 时
/// 重算，不主动刷新则已在跑的存量引擎目录要滞后到下一次任意策略刷新；
/// 与 marketplace/connectors 命令调用 `pool.refresh_disallowed_tools()`
/// 的既有模式一致）。
#[tauri::command]
pub async fn computer_use_set_enabled(
    enabled: bool,
    shared: State<'_, Arc<ComputerUseShared>>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    UserPrefs::update_transaction(|prefs| {
        prefs.computer_use.enabled = enabled;
        Ok(())
    })?;
    shared.set_enabled(enabled);
    if enabled {
        shared.reset_stop();
    } else {
        shared.revoke_all_sessions();
        // 总开关关闭：所有后端的持久 OS 级授权一并终止（detached 线程）。
        // Emergency variant: also unpress any physically held left button.
        shared.backends.emergency_release_all();
    }
    pool.refresh_disallowed_tools().await;
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

    /// 评审修复回归：grant/revoke（session_id）与 confirm/deny（confirm_id）
    /// 的空串/纯空白标识符必须显式报错，不得静默成功。
    #[test]
    fn empty_identifiers_are_rejected() {
        assert!(ensure_non_empty("session_id", "").is_err());
        assert!(ensure_non_empty("session_id", "   ").is_err());
        assert!(ensure_non_empty("confirm_id", "").is_err());
        assert!(ensure_non_empty("confirm_id", "cu-0123456789abcdef").is_ok());
        assert!(ensure_non_empty("session_id", "s1").is_ok());
    }
}
