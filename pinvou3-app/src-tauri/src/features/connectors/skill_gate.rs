//! Skill-gate shared by the four CLI connectors (tmeet/dingtalk/feishu/
//! wecom): a per-connector disabled-flag file plus skill apply/discard,
//! table-driven via [`ConnectorGate`] / [`GATES`].
//!
//! 每个连接器模块在自己的文件里声明 [`ConnectorGate`] 表项(就绪探测与技能
//! 落盘指向本模块函数),四份逐字重复的 `{apply_skills, skills_state,
//! *_skills_should_show}` 命令块与 `refresh_connector_auth_gates` 的四段
//! spawn_blocking 收编为 [`ConnectorGate`] 的公共方法。
//!
//! Disable semantics: the `~/.pinvou3/<id>_disabled` file existing means the
//! user manually disabled that connector's skills, orthogonal to connection
//! state (auth). The write side (`set_disabled_flag`) was removed together
//! with the retired `set_*_enabled` commands; connector switches now persist
//! through the unified scope state (`set_disabled_connectors` → marketplace
//! scope), so the gate only reads the flag.

use std::path::PathBuf;

use serde_json::{Value, json};

/// 单个 CLI 连接器的门控配置(表项,注册表见 [`GATES`])。
pub(crate) struct ConnectorGate {
    /// 连接器 id(事件前缀 / scope 同步键,如 `"tmeet"`)。
    pub id: &'static str,
    /// 停用标志文件名(如 `"tmeet_disabled"`)。
    pub disabled_filename: &'static str,
    /// 用户可见名(错误信息前缀:「更新{名}技能失败」/「刷新{名}技能门控失败」)。
    pub display_name: &'static str,
    /// 实时就绪探测(spawn 对应 CLI 查 auth 状态;未装返回 false)。
    pub ready_probe: fn() -> bool,
    /// 按 `visible` 增 / 删本连接器的技能文件 —— 调各自的
    /// `Pinvou3Bundle::apply_*_skills`。
    pub apply_bundle_skills: fn(bool) -> std::io::Result<()>,
}

impl ConnectorGate {
    /// 停用标志文件完整路径:`~/.pinvou3/<disabled_filename>`。
    pub fn disabled_path(&self) -> PathBuf {
        crate::platform::paths::pinvou3_home().join(self.disabled_filename)
    }

    /// 是否被手动停用(停用标志文件存在即停用)。与连接状态正交。
    pub fn is_disabled(&self) -> bool {
        self.disabled_path().exists()
    }

    /// 技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
    /// 启动时(bundle)与命令里都用它判定。注:ready_probe 会 spawn 对应 CLI。
    pub fn skills_should_show(&self) -> bool {
        !self.is_disabled() && (self.ready_probe)()
    }

    /// 按 visible 写 / 删技能文件(带用户可见错误前缀)。
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        (self.apply_bundle_skills)(visible)
            .map_err(|e| format!("更新{}技能失败: {e}", self.display_name))
    }

    /// `*_apply_skills` 命令公共体:按当前"应否可见"状态写 / 删技能文件。
    /// scope 门禁同步：连接器转为可用等同「新装」——已初始化 code 开关时加入
    /// code 禁用集，保持「code 会话外部能力默认关」语义（与 MCP 新装连接器一致）。
    // &'static self:表项都是进程级 static,线程池闭包按值捕获该共享引用。
    pub async fn apply_skills_command(&'static self) -> Result<Value, String> {
        let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
            let show = self.skills_should_show();
            self.apply_skills(show)?;
            Ok(show)
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))??;
        if show {
            crate::features::marketplace::sync_deny_all_scopes_after_install(self.id);
        }
        Ok(json!({ "visible": show }))
    }

    /// `*_skills_state` 命令公共体:给前端渲染开关态
    /// `{connected, enabled(=未停用), visible(=connected&&enabled)}`。
    pub async fn skills_state_command(&'static self) -> Result<Value, String> {
        tokio::task::spawn_blocking(|| {
            let disabled = self.is_disabled();
            let connected = (self.ready_probe)();
            Ok::<Value, String>(json!({
                "connected": connected,
                "enabled": !disabled,
                "visible": connected && !disabled,
            }))
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
    }

    /// `refresh_connector_auth_gates` 的单连接器步骤(在线程池里跑):
    /// 实时探测应否可见,按结果写 / 删技能目录。
    pub fn refresh_step(&self) -> Result<bool, String> {
        let show = self.skills_should_show();
        (self.apply_bundle_skills)(show)
            .map_err(|e| format!("刷新{}技能门控失败: {e}", self.display_name))?;
        Ok(show)
    }
}

/// 四个 CLI 连接器的门控注册表;`refresh_connector_auth_gates` 按此并行刷新。
/// 各表项与所属连接器模块同居一处,新增 CLI 连接器 = 新模块 + 在此登记一行。
pub(crate) static GATES: [&'static ConnectorGate; 4] = [
    &super::feishu::FEISHU_GATE,
    &super::wecom::WECOM_GATE,
    &super::dingtalk::DINGTALK_GATE,
    &super::tmeet::TMEET_GATE,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hand-built gate driving the default implementations: exercises
    /// the disabled-path derivation under a temporary `PINVOU3_HOME`.
    fn fake_gate() -> ConnectorGate {
        ConnectorGate {
            id: "fake",
            disabled_filename: "fake_disabled",
            display_name: "测试",
            ready_probe: || false,
            apply_bundle_skills: |_| Ok(()),
        }
    }

    /// `disabled_path` 跟随 `PINVOU3_HOME`,且文件名由 `disabled_filename` 决定。
    #[test]
    fn disabled_path_is_derived_from_pinvou3_home() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-skillgate-path-{}-{}",
            std::env::temp_dir().display(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let gate = fake_gate();
        assert_eq!(
            gate.disabled_path(),
            crate::platform::paths::pinvou3_home().join("fake_disabled")
        );
        // 标志文件不存在 → 未停用;ready_probe 恒 false → 不应显示。
        assert!(!gate.is_disabled());
        assert!(!gate.skills_should_show());

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes
            // are serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes
            // are serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 注册表恰好覆盖四个 CLI 连接器,且 id 与文件名前缀一一对应。
    #[test]
    fn gates_table_covers_the_four_cli_connectors() {
        let ids: Vec<_> = GATES.iter().map(|g| g.id).collect();
        assert_eq!(ids, vec!["feishu", "wecom", "dingtalk", "tmeet"]);
        for gate in GATES {
            let expected_filename = format!("{}_disabled", gate.id);
            let gate_id = gate.id;
            assert_eq!(
                gate.disabled_filename, expected_filename,
                "{gate_id} 的停用标志文件名应与其 id 对应"
            );
        }
    }
}
