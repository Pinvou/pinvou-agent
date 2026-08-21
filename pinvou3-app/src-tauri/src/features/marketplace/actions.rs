//! 动作下发（marketplace-unification §3.3）：后端按包当前状态推导可用动作集，
//! 前端退化为动作渲染器（新增连接器零前端改动）。本刀纯增量：动作只经
//! `bundle_readiness` 的 `actions` 字段下发，前端切换消费在后续 PR。
//!
//! 统一动作词汇表：
//! - `install`    免凭据包的安装（本地 MCP / 预置技能）
//! - `configure`  带凭据/配置字段包的安装或补配（配置弹窗流程）
//! - `connect`    授权型包的连接（OAuth / CLI 扫码，`flow` 标记交互形态）
//! - `disconnect` 断开授权（CLI logout / ima 删凭据；≠ 卸载，§5.3）
//! - `update`     预置技能内容落后于嵌入资源时的覆盖重装
//! - `uninstall`  卸载（删登记 + 删资源）
//! - `edit_display` 上传包（source=Upload）的 UI 展示名/说明编辑（写 bundles.json
//!   extra 覆盖，不动包清单；仅已装上传包下发）
//! - `repair`     Degraded 修复（登记在、资源缺；按来源重新获取，§3.2），置前
//! - `enable_in(scope)` 已装包按模式 scope 的启用开关（scope 收敛后单一禁用集为
//!   包 id × SessionMode；`scope` 字段携带模式 kebab-case 名，当前开/关态由
//!   `get_disabled_bundles` 读取，本动作仅下发「该 scope 存在开关」这一事实）。
//!
//! 不纳入本刀（注释说明理由）：
//! - 占位卡（即将上线）/内置标记卡：目前仍是前端 overlay（§8 Phase 4 才改
//!   注册表 upcoming 条目），后端不下发动作。

use serde::Serialize;

use super::bundle::{BundleInfo, BundleKind, Readiness};
use crate::core::session_mode::SessionMode;

pub const ACTION_INSTALL: &str = "install";
pub const ACTION_CONFIGURE: &str = "configure";
pub const ACTION_CONNECT: &str = "connect";
pub const ACTION_DISCONNECT: &str = "disconnect";
pub const ACTION_UPDATE: &str = "update";
pub const ACTION_UNINSTALL: &str = "uninstall";
pub const ACTION_EDIT_DISPLAY: &str = "edit_display";
pub const ACTION_REPAIR: &str = "repair";
pub const ACTION_ENABLE_IN: &str = "enable_in";

/// 交互流程标记（§3.3：交互流程建模为动作的 flow payload）。本刀只给类型标记，
/// 具体交互描述（二维码、流程卡、OAuth 五态机）仍由前端现有组件承担，
/// payload 下沉（request_id 协调器、五态分类）在后续 PR。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ActionFlow {
    /// 远程 MCP 的浏览器 OAuth 授权流程
    Oauth,
    /// CLI 连接器的扫码/授权流程（飞书两段、企微/钉钉/腾讯会议单段）
    CliConnect,
}

/// 一个可下发动作。`reason` 是动作附带的用户可读提示：`enabled=false` 时为
/// 不可用原因（前端置灰 + 提示），`repair` 动作透传 degraded 详情。`scope` 仅
/// `enable_in` 动作携带（模式 kebab-case 名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleAction {
    pub id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<ActionFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

fn action(id: &str, flow: Option<ActionFlow>) -> BundleAction {
    BundleAction {
        id: id.to_string(),
        enabled: true,
        reason: None,
        flow,
        scope: None,
    }
}

/// 由 (包信息, 就绪态) 推导可用动作集。纯函数、现算不落盘（与 ready 同纪律）。
///
/// 规则（与前端 TsActionBtn 现状语义对齐，收口到后端单一推导点）：
/// - degraded → `repair` 置前，reason 透传 degraded 原因；CLI 包的 repair 即
///   重连（flow=cli_connect），不再重复下发 `connect`；
/// - CLI 包：已连接（ready）→ `disconnect`；否则 → `connect`；
/// - ima（凭据型技能包）：凭据齐 → `disconnect`（删凭据）；否则 → `configure`；
/// - 其余 MCP/组合/技能包：
///   - 未安装：OAuth 型 → `connect`；有凭据/配置字段 → `configure`；免凭据 → `install`；
///   - 已安装：缺必填凭据 → `configure`；有可更新预置技能 → `update`；`uninstall` 恒给。
pub fn actions_for(bundle: &BundleInfo, readiness: Readiness) -> Vec<BundleAction> {
    let mut out: Vec<BundleAction> = Vec::new();
    // Degraded 修复置前（§3.2：修复动作统一为按来源重新获取）
    if let Some(reason) = &bundle.degraded {
        let flow = if bundle.kind == BundleKind::Cli {
            Some(ActionFlow::CliConnect)
        } else {
            None
        };
        let mut repair = action(ACTION_REPAIR, flow);
        repair.reason = Some(reason.clone());
        out.push(repair);
    }

    let ready = matches!(readiness, Readiness::Ready);
    let missing_credentials = matches!(readiness, Readiness::NotReady("missing_credentials"));
    let has_config = !bundle.config_fields.is_empty() || !bundle.credentials.is_empty();

    match bundle.kind {
        BundleKind::Cli => {
            if ready {
                out.push(action(ACTION_DISCONNECT, None));
            } else if bundle.degraded.is_some() {
                // degraded CLI 的 repair 即重连（flow 已带 cli_connect），不重复下发
            } else {
                out.push(action(ACTION_CONNECT, Some(ActionFlow::CliConnect)));
            }
        }
        // ima：凭据型技能包（V2 归 Skill），「断开」= 删凭据
        BundleKind::Skill if bundle.id == "ima" => {
            if ready {
                out.push(action(ACTION_DISCONNECT, None));
            } else {
                out.push(action(ACTION_CONFIGURE, None));
            }
        }
        BundleKind::Mcp | BundleKind::Bundle | BundleKind::Skill => {
            if !bundle.installed {
                if bundle.oauth {
                    out.push(action(ACTION_CONNECT, Some(ActionFlow::Oauth)));
                } else if has_config {
                    out.push(action(ACTION_CONFIGURE, None));
                } else {
                    out.push(action(ACTION_INSTALL, None));
                }
            } else {
                if missing_credentials {
                    out.push(action(ACTION_CONFIGURE, None));
                }
                if bundle.update_available {
                    out.push(action(ACTION_UPDATE, None));
                }
                // 上传包（source=Upload 的已装包）：UI 展示名/说明可用户自定义
                // （extra 覆盖，不动包清单），下发编辑动作。
                if bundle.user_uploaded {
                    out.push(action(ACTION_EDIT_DISPLAY, None));
                }
                out.push(action(ACTION_UNINSTALL, None));
            }
        }
    }
    // 已装包：每模式一个 enable_in(scope) 开关动作（scope 收敛后开关粒度 = 包 id ×
    // SessionMode）。当前开/关态由 get_disabled_bundles 读取，这里只下发「存在开关」。
    if bundle.installed {
        for mode in SessionMode::ALL {
            let mut enable = action(ACTION_ENABLE_IN, None);
            enable.scope = Some(mode.as_str().to_string());
            out.push(enable);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::marketplace::bundle::{CredentialSpec, CredentialTarget};

    fn bundle(kind: BundleKind) -> BundleInfo {
        BundleInfo {
            id: "x".into(),
            name: "x".into(),
            kind,
            mcp_servers: vec![],
            skills: vec![],
            cli: vec![],
            credentials: vec![],
            description: String::new(),
            version: String::new(),
            auth_required: false,
            config_fields: vec![],
            installed: false,
            user_uploaded: false,
            degraded: None,
            update_available: false,
            oauth: false,
            category: String::new(),
            icon: None,
            display_name: None,
            display_description: None,
        }
    }

    fn ids(actions: &[BundleAction]) -> Vec<&str> {
        // 生命周期动作与 enable_in 开关分列：这里聚焦 install/connect/... 序列。
        actions
            .iter()
            .filter(|a| a.id != ACTION_ENABLE_IN)
            .map(|a| a.id.as_str())
            .collect()
    }

    fn credential() -> CredentialSpec {
        CredentialSpec {
            key: "KEY".into(),
            target: CredentialTarget::Env,
            required: true,
        }
    }

    #[test]
    fn uninstalled_local_mcp_gets_install() {
        let b = bundle(BundleKind::Mcp);
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["install"]);
    }

    #[test]
    fn uninstalled_credential_mcp_gets_configure() {
        let mut b = bundle(BundleKind::Mcp);
        b.credentials = vec![credential()];
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["configure"]);
    }

    #[test]
    fn uninstalled_oauth_mcp_gets_connect_with_oauth_flow() {
        let mut b = bundle(BundleKind::Mcp);
        b.oauth = true;
        let actions = actions_for(&b, Readiness::Ready);
        assert_eq!(ids(&actions), ["connect"]);
        assert_eq!(actions[0].flow, Some(ActionFlow::Oauth));
    }

    #[test]
    fn installed_ready_bundle_gets_uninstall() {
        let mut b = bundle(BundleKind::Bundle);
        b.installed = true;
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["uninstall"]);
    }

    #[test]
    fn installed_missing_credentials_gets_configure_then_uninstall() {
        let mut b = bundle(BundleKind::Mcp);
        b.installed = true;
        b.credentials = vec![credential()];
        assert_eq!(
            ids(&actions_for(&b, Readiness::NotReady("missing_credentials"))),
            ["configure", "uninstall"]
        );
    }

    #[test]
    fn uninstalled_preset_skill_gets_install() {
        let b = bundle(BundleKind::Skill);
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["install"]);
    }

    #[test]
    fn installed_skill_with_update_gets_update_then_uninstall() {
        let mut b = bundle(BundleKind::Skill);
        b.installed = true;
        b.update_available = true;
        assert_eq!(
            ids(&actions_for(&b, Readiness::Ready)),
            ["update", "uninstall"]
        );
    }

    #[test]
    fn uploaded_skill_gets_edit_display_then_uninstall() {
        let mut b = bundle(BundleKind::Skill);
        b.installed = true;
        b.user_uploaded = true;
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["edit_display", "uninstall"]);
    }

    #[test]
    fn cli_connect_and_disconnect() {
        let mut b = bundle(BundleKind::Cli);
        b.installed = true;
        // 未授权 → connect（flow 标记 CLI 扫码流程）
        let actions = actions_for(&b, Readiness::NotReady("not_connected"));
        assert_eq!(ids(&actions), ["connect"]);
        assert_eq!(actions[0].flow, Some(ActionFlow::CliConnect));
        // 已授权 → disconnect
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["disconnect"]);
    }

    #[test]
    fn ima_credential_pack_configure_and_disconnect() {
        let mut b = bundle(BundleKind::Skill);
        b.id = "ima".into();
        b.credentials = vec![credential()];
        // 凭据缺 → configure；凭据齐 → disconnect（删凭据，≠ 卸载）
        assert_eq!(
            ids(&actions_for(&b, Readiness::NotReady("missing_credentials"))),
            ["configure"]
        );
        b.installed = true;
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["disconnect"]);
    }

    #[test]
    fn degraded_puts_repair_first_with_reason() {
        // 上传技能 degraded：repair 置前、reason 透传，正常动作跟后
        let mut b = bundle(BundleKind::Skill);
        b.installed = true;
        b.user_uploaded = true;
        b.degraded = Some("包内容缺失".into());
        let actions = actions_for(&b, Readiness::Ready);
        assert_eq!(ids(&actions), ["repair", "edit_display", "uninstall"]);
        assert_eq!(actions[0].reason.as_deref(), Some("包内容缺失"));
        assert_eq!(actions[0].flow, None);
    }

    #[test]
    fn degraded_cli_repair_is_reconnect_without_duplicate_connect() {
        let mut b = bundle(BundleKind::Cli);
        b.installed = true;
        b.degraded = Some("配套技能已随断开移除".into());
        let actions = actions_for(&b, Readiness::NotReady("not_connected"));
        assert_eq!(
            ids(&actions),
            ["repair"],
            "repair 即重连，不重复下发 connect"
        );
        assert_eq!(actions[0].flow, Some(ActionFlow::CliConnect));
        // 重连后（degraded 清除 + ready）回到 disconnect
        b.degraded = None;
        assert_eq!(ids(&actions_for(&b, Readiness::Ready)), ["disconnect"]);
    }

    /// 契约形态：snake_case、flow 用内部 tag（{"kind":"oauth"}）、None 字段省略。
    #[test]
    fn wire_shape_is_snake_case_with_tagged_flow() {
        let mut b = bundle(BundleKind::Mcp);
        b.oauth = true;
        let actions = actions_for(&b, Readiness::Ready);
        let json = serde_json::to_value(&actions[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "connect", "enabled": true, "flow": {"kind": "oauth"}})
        );
        // 无 flow 时不带 flow 键
        let plain =
            serde_json::to_value(&actions_for(&bundle(BundleKind::Mcp), Readiness::Ready)[0])
                .unwrap();
        assert!(plain.get("flow").is_none());
        assert!(plain.get("reason").is_none());
    }

    /// 已装包每模式下发一个 `enable_in(scope)` 开关动作；未装包不下发。
    #[test]
    fn installed_bundle_gets_enable_in_per_scope() {
        let mut b = bundle(BundleKind::Mcp);
        b.installed = true;
        let actions = actions_for(&b, Readiness::Ready);
        let enable_in: Vec<_> = actions
            .iter()
            .filter(|a| a.id == ACTION_ENABLE_IN)
            .collect();
        assert_eq!(enable_in.len(), SessionMode::ALL.len());
        for mode in SessionMode::ALL {
            assert!(
                enable_in
                    .iter()
                    .any(|a| a.scope.as_deref() == Some(mode.as_str())),
                "缺少 {mode:?} 的 enable_in 动作"
            );
        }
        // 未装包不下发 enable_in。
        let uninstalled = actions_for(&bundle(BundleKind::Mcp), Readiness::Ready);
        assert!(uninstalled.iter().all(|a| a.id != ACTION_ENABLE_IN));
    }
}
