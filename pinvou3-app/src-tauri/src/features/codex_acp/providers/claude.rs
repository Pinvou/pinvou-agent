//! Claude Code 配置写入器：`~/.claude/settings.json`。
//!
//! 只读写 `env` 块的受管键（ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN /
//! ANTHROPIC_MODEL + 细化模型槽位 ANTHROPIC_DEFAULT_*_MODEL /
//! CLAUDE_CODE_SUBAGENT_MODEL），其余配置原样保留；恢复官方登录只删除这些
//! 受管键。细化槽位不填时 CC 的子 agent 会回落官方模型走官方流量，因此
//! 槽位随 Provider 绑定写入。env 变量优先于 CLI 自身 OAuth 状态，切换
//! Provider 后 `claude auth status` 自然读到新值。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{atomic_write, AgentConfigWriter, EffectiveConfig, EffectiveEntry, ProviderTarget};

const ENV_BASE_URL: &str = "ANTHROPIC_BASE_URL";
const ENV_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const ENV_MODEL: &str = "ANTHROPIC_MODEL";

pub struct ClaudeConfigWriter {
    settings_path: PathBuf,
}

impl ClaudeConfigWriter {
    /// `root` = `~/.claude` 目录（单测传临时目录）。
    pub fn new(root: &Path) -> Self {
        Self {
            settings_path: root.join("settings.json"),
        }
    }

    /// 读取当前配置；文件缺失时视为空对象，不可解析时**拒绝覆盖**并明确报错。
    fn read_config(&self) -> Result<Value> {
        if !self.settings_path.exists() {
            return Ok(Value::Object(Default::default()));
        }
        let raw = fs::read_to_string(&self.settings_path)
            .with_context(|| format!("读取 {} 失败", self.settings_path.display()))?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "{} 不是有效的 JSON，拒绝覆盖；请手动修复或删除该文件后重试",
                self.settings_path.display()
            )
        })
    }
}

impl AgentConfigWriter for ClaudeConfigWriter {
    fn apply(&self, target: &ProviderTarget) -> Result<()> {
        let mut config = self.read_config()?;
        let top = config
            .as_object_mut()
            .context("settings.json 顶层必须是 JSON 对象")?;
        let env = top.entry("env").or_insert_with(|| json!({}));
        let env_obj = env
            .as_object_mut()
            .context("settings.json 的 env 必须是对象")?;
        env_obj.insert(
            ENV_BASE_URL.to_string(),
            json!(super::trim_base_url(&target.base_url)),
        );
        if let Some(key) = target.api_key.as_deref() {
            env_obj.insert(ENV_AUTH_TOKEN.to_string(), json!(key));
        }
        // 模型与当前 Provider 绑定：目标未指定模型时清除旧值，避免上一个
        // 受管 Provider 的 ANTHROPIC_MODEL 残留导致请求 404/400。
        if let Some(model) = target.model.as_deref() {
            env_obj.insert(ENV_MODEL.to_string(), json!(model));
        } else {
            env_obj.remove(ENV_MODEL);
        }
        // 细化模型槽位（opus/sonnet/haiku/fable/subagent）：随 Provider 写入；
        // 目标无槽位（旧记录）时清除受管槽位键，避免残留指向上一家中转的模型。
        for (slot, env_name) in super::CLAUDE_MODEL_SLOTS {
            match target
                .model_slots
                .as_ref()
                .and_then(|slots| slots.get(slot))
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                Some(value) => {
                    env_obj.insert(env_name.to_string(), json!(value));
                }
                None => {
                    env_obj.remove(env_name);
                }
            }
        }
        let raw = serde_json::to_string_pretty(&config)?;
        atomic_write(&self.settings_path, raw.as_bytes())
    }

    fn revert_to_official(&self, _reverted: Option<&ProviderTarget>) -> Result<()> {
        if !self.settings_path.exists() {
            return Ok(());
        }
        let mut config = self.read_config()?;
        let Some(env_obj) = config
            .as_object_mut()
            .and_then(|top| top.get_mut("env"))
            .and_then(Value::as_object_mut)
        else {
            return Ok(());
        };
        let removed = [ENV_BASE_URL, ENV_AUTH_TOKEN, ENV_MODEL]
            .into_iter()
            .chain(
                super::CLAUDE_MODEL_SLOTS
                    .iter()
                    .map(|(_, env_name)| *env_name),
            )
            .filter(|name| env_obj.remove(*name).is_some())
            .count();
        if removed == 0 {
            return Ok(());
        }
        if env_obj.is_empty() {
            if let Some(top) = config.as_object_mut() {
                top.remove("env");
            }
        }
        let raw = serde_json::to_string_pretty(&config)?;
        atomic_write(&self.settings_path, raw.as_bytes())
    }

    fn effective(&self) -> Result<EffectiveConfig> {
        if !self.settings_path.exists() {
            return Ok(EffectiveConfig::default());
        }
        let config = self.read_config()?;
        let relay_active = config
            .get("env")
            .and_then(|env| env.get(ENV_AUTH_TOKEN))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        // 生效值展示（F4 可见化）：base_url 与 model，**不含** AUTH_TOKEN。
        let mut entries = Vec::new();
        if let Some(env) = config.get("env") {
            if let Some(value) = env.get(ENV_BASE_URL).and_then(Value::as_str) {
                if !value.is_empty() {
                    entries.push(EffectiveEntry {
                        key: "ANTHROPIC_BASE_URL".to_string(),
                        value: value.to_string(),
                        secret: false,
                    });
                }
            }
            if let Some(value) = env.get(ENV_MODEL).and_then(Value::as_str) {
                if !value.is_empty() {
                    entries.push(EffectiveEntry {
                        key: "ANTHROPIC_MODEL".to_string(),
                        value: value.to_string(),
                        secret: false,
                    });
                }
            }
        }
        Ok(EffectiveConfig {
            relay_active,
            provider_hint: None,
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::codex_acp::providers::ProviderWireApi;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-writer-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target(provider_id: &str) -> ProviderTarget {
        ProviderTarget {
            provider_id: provider_id.into(),
            name: "中转".into(),
            base_url: "https://api.example.com/anthropic/".into(),
            model: Some("claude-sonnet-4-5".into()),
            model_slots: None,
            context_window: None,
            wire_api: ProviderWireApi::Anthropic,
            api_key: Some("test-api-key-1234567890".into()),
        }
    }

    fn target_with_slots(provider_id: &str) -> ProviderTarget {
        let mut target = target(provider_id);
        target.model_slots = Some(
            crate::features::codex_acp::providers::CLAUDE_MODEL_SLOTS
                .iter()
                .map(|(slot, _)| (slot.to_string(), format!("relay-{slot}")))
                .collect(),
        );
        target
    }

    #[test]
    fn apply_writes_model_slots_and_revert_removes_them() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        writer.apply(&target_with_slots("pv-aaaaaaaaaaaa")).unwrap();
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        // 五个细化槽位全部写入对应 env 键
        assert_eq!(config["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "relay-opus");
        assert_eq!(
            config["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "relay-sonnet"
        );
        assert_eq!(
            config["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "relay-haiku"
        );
        assert_eq!(
            config["env"]["ANTHROPIC_DEFAULT_FABLE_MODEL"],
            "relay-fable"
        );
        assert_eq!(
            config["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            "relay-subagent"
        );
        // 恢复官方：受管槽位键全部删除
        writer.revert_to_official(None).unwrap();
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert!(config.get("env").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_without_slots_clears_stale_slot_keys() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        writer.apply(&target_with_slots("pv-aaaaaaaaaaaa")).unwrap();
        // 切到无槽位的 Provider：受管槽位键必须清除，避免残留指向旧中转
        writer.apply(&target("pv-bbbbbbbbbbbb")).unwrap();
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert!(config["env"].get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none());
        assert!(config["env"].get("CLAUDE_CODE_SUBAGENT_MODEL").is_none());
        assert_eq!(
            config["env"]["ANTHROPIC_BASE_URL"],
            "https://api.example.com/anthropic"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_merges_and_preserves_other_keys() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        // 预置用户自己的配置
        fs::write(
            dir.join("settings.json"),
            r#"{"model":"claude-sonnet-4-5","permissions":{"defaultMode":"acceptEdits"}}"#,
        )
        .unwrap();
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            config["env"]["ANTHROPIC_BASE_URL"],
            "https://api.example.com/anthropic"
        );
        assert_eq!(
            config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "test-api-key-1234567890"
        );
        assert_eq!(config["env"]["ANTHROPIC_MODEL"], "claude-sonnet-4-5");
        // 其他键保留
        assert_eq!(config["model"], "claude-sonnet-4-5");
        assert_eq!(config["permissions"]["defaultMode"], "acceptEdits");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_only_removes_managed_env_keys() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        fs::write(
            dir.join("settings.json"),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://api.example.com","ANTHROPIC_AUTH_TOKEN":"sk-x","ANTHROPIC_MODEL":"m","CUSTOM":"v"},"model":"x"}"#,
        )
        .unwrap();
        writer.revert_to_official(None).unwrap();
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert!(config
            .get("env")
            .unwrap()
            .get("ANTHROPIC_BASE_URL")
            .is_none());
        assert!(config
            .get("env")
            .unwrap()
            .get("ANTHROPIC_AUTH_TOKEN")
            .is_none());
        assert!(config.get("env").unwrap().get("ANTHROPIC_MODEL").is_none());
        // 其他 env 键与顶层保留
        assert_eq!(config["env"]["CUSTOM"], "v");
        assert_eq!(config["model"], "x");
        // 空 env 块已清理
        assert!(config.get("env").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_noop_without_managed_keys_skips_write() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        fs::write(dir.join("settings.json"), r#"{"model":"x"}"#).unwrap();
        writer.revert_to_official(None).unwrap();
        assert!(!dir.join("settings.json.pinvou3-bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_overwrite_unparseable_file() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        let broken = "{not json";
        fs::write(dir.join("settings.json"), broken).unwrap();
        assert!(writer.apply(&target("pv-aaaaaaaaaaaa")).is_err());
        // 字节不变
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            broken
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_created_once_on_first_write() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        fs::write(dir.join("settings.json"), r#"{"model":"x"}"#).unwrap();
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        writer.apply(&target("pv-bbbbbbbbbbbb")).unwrap();
        let backup = dir.join("settings.json.pinvou3-bak");
        assert!(backup.exists());
        // 备份保留初始状态且只写一次
        assert_eq!(fs::read_to_string(backup).unwrap(), r#"{"model":"x"}"#);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_detects_relay() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        assert_eq!(
            writer.effective().unwrap(),
            EffectiveConfig {
                relay_active: false,
                provider_hint: None,
                entries: Vec::new(),
            }
        );
        fs::write(
            dir.join("settings.json"),
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-x"}}"#,
        )
        .unwrap();
        assert_eq!(
            writer.effective().unwrap(),
            EffectiveConfig {
                relay_active: true,
                provider_hint: None,
                // env 只有 AUTH_TOKEN（凭据）时 entries 为空——凭据永不展示
                entries: Vec::new(),
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_entries_expose_base_url_and_model_only() {
        let dir = tmp_dir();
        let writer = ClaudeConfigWriter::new(&dir);
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let effective = writer.effective().unwrap();
        assert_eq!(effective.relay_active, true);
        // base_url 与 model 展示；AUTH_TOKEN（凭据）绝不出现
        let keys: Vec<&str> = effective
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        assert_eq!(keys, vec!["ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL"]);
        assert!(
            !effective
                .entries
                .iter()
                .any(|entry| entry.key.contains("AUTH_TOKEN")),
            "凭据字段不得出现在生效值展示中"
        );
        assert!(
            effective
                .entries
                .iter()
                .all(|entry| !entry.value.contains("sk-") && !entry.value.contains("test-api-key")),
            "生效值不得包含 key 明文"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
