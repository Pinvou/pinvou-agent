//! Codex 配置写入器：`~/.codex/config.toml`。
//!
//! Codex 的 `model_providers.<id>` 表只支持 `env_key` 引用环境变量，**没有明文
//! key 字段**；本写入器固定使用 `OPENAI_API_KEY`，实际 key 由 Pinvou 在 spawn
//! Codex 子进程时注入 env（仅当进程 env 未设置时）。恢复官方登录只删除受管
//! `pv-*` 表与指向它们的顶层 `model_provider` / `model`，保留用户其他配置。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml::Value;

use super::{
    atomic_write, AgentConfigWriter, EffectiveConfig, EffectiveEntry, ProviderTarget,
    PROVIDER_ID_PREFIX,
};

const ENV_KEY_NAME: &str = "OPENAI_API_KEY";
/// 受管模型 catalog 文件名（写在 ~/.codex 下，config.toml 的
/// `model_catalog_json` 指向它）。Codex 只对内置模型有元数据，中转的自定义
/// 模型名会触发「Model metadata not found」警告并按 fallback 运行；写入
/// catalog 后警告消除且上下文窗口等参数正确（格式按 codex 0.146 实测）。
const CATALOG_FILE_NAME: &str = "pinvou3-model-catalog.json";
/// catalog 里 base_instructions 必须与原值一致（codex 内置，勿自创文案）。
const CATALOG_BASE_INSTRUCTIONS: &str =
    "You are Codex, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
/// 无法获知中转模型真实窗口时的保守默认（与 kimi 侧默认一致量级）。
const CATALOG_DEFAULT_CONTEXT_WINDOW: i64 = 200_000;

pub struct CodexConfigWriter {
    config_path: PathBuf,
}

impl CodexConfigWriter {
    /// `root` = `~/.codex` 目录（单测传临时目录）。
    pub fn new(root: &Path) -> Self {
        Self {
            config_path: root.join("config.toml"),
        }
    }

    /// 读取当前配置；文件缺失时视为空表，不可解析时**拒绝覆盖**并明确报错。
    fn read_config(&self) -> Result<Value> {
        if !self.config_path.exists() {
            return Ok(Value::Table(Default::default()));
        }
        let raw = fs::read_to_string(&self.config_path)
            .with_context(|| format!("读取 {} 失败", self.config_path.display()))?;
        raw.parse::<Value>().with_context(|| {
            format!(
                "{} 不是有效的 TOML，拒绝覆盖；请手动修复或删除该文件后重试",
                self.config_path.display()
            )
        })
    }

    /// 生成/更新受管模型 catalog（幂等）。`context_window` 未指定时用保守默认。
    fn write_model_catalog(&self, model: &str, context_window: Option<i64>) -> Result<PathBuf> {
        let catalog_path = self.config_path.with_file_name(CATALOG_FILE_NAME);
        let entry = serde_json::json!({
            "models": [{
                "slug": model,
                "display_name": model,
                "context_window": context_window.unwrap_or(CATALOG_DEFAULT_CONTEXT_WINDOW),
                "max_output_tokens": 8192,
                "supported_reasoning_levels": [],
                "shell_type": "shell_command",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 1,
                "base_instructions": CATALOG_BASE_INSTRUCTIONS,
                "support_verbosity": false,
                "truncation_policy": { "mode": "bytes", "limit": 10000 },
                "supports_parallel_tool_calls": true,
                "experimental_supported_tools": [],
            }],
        });
        atomic_write(
            &catalog_path,
            serde_json::to_string_pretty(&entry)?.as_bytes(),
        )?;
        Ok(catalog_path)
    }
}

/// 把 raw TOML 解析为「relay 配置是否激活 + 激活的 provider id」。
///
/// 认证探测（`codex_authenticated`）使用其中 env_key 非空判定：App 注入的 key
/// 只在被 spawn 的子进程 env 中可见，探测进程读不到，只能以配置文件为准。
pub(crate) fn config_relay_effective(raw: &str) -> EffectiveConfig {
    let Ok(config) = raw.parse::<Value>() else {
        return EffectiveConfig::default();
    };
    let Some(provider_id) = config
        .get("model_provider")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return EffectiveConfig::default();
    };
    // 指向不存在表的 model_provider 按「未激活」处理（provider_hint 也置
    // None，与测试契约一致）：只有真正存在该表才算 relay 配置。
    let relay_active = config
        .get("model_providers")
        .and_then(|providers| providers.get(provider_id))
        .is_some();
    if !relay_active {
        return EffectiveConfig::default();
    }
    // 生效值展示（F4 可见化）：model_provider / model / 该 provider 的 base_url。
    let mut entries = Vec::new();
    entries.push(EffectiveEntry {
        key: "model_provider".to_string(),
        value: provider_id.to_string(),
        secret: false,
    });
    if let Some(value) = config.get("model").and_then(Value::as_str) {
        if !value.is_empty() {
            entries.push(EffectiveEntry {
                key: "model".to_string(),
                value: value.to_string(),
                secret: false,
            });
        }
    }
    if let Some(value) = config
        .get("model_providers")
        .and_then(|providers| providers.get(provider_id))
        .and_then(Value::as_table)
        .and_then(|table| table.get("base_url"))
        .and_then(Value::as_str)
    {
        if !value.is_empty() {
            entries.push(EffectiveEntry {
                key: format!("model_providers.{provider_id}.base_url"),
                value: value.to_string(),
                secret: false,
            });
        }
    }
    EffectiveConfig {
        relay_active,
        provider_hint: Some(provider_id.to_string()),
        entries,
    }
}

pub(crate) fn codex_config_relay_env_key_present(raw: &str) -> bool {
    let Ok(config) = raw.parse::<Value>() else {
        return false;
    };
    let Some(provider_id) = config
        .get("model_provider")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    config
        .get("model_providers")
        .and_then(|providers| providers.get(provider_id))
        .and_then(Value::as_table)
        .and_then(|table| table.get("env_key"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

impl AgentConfigWriter for CodexConfigWriter {
    fn apply(&self, target: &ProviderTarget) -> Result<()> {
        let mut config = self.read_config()?;
        let table = config.as_table_mut().context("config.toml 顶层必须是表")?;
        let providers = table
            .entry("model_providers")
            .or_insert_with(|| Value::Table(Default::default()));
        let providers_table = providers
            .as_table_mut()
            .context("model_providers 必须是表")?;
        let mut entry = toml::map::Map::new();
        entry.insert("name".into(), Value::String(target.name.clone()));
        entry.insert(
            "base_url".into(),
            Value::String(super::trim_base_url(&target.base_url)),
        );
        entry.insert("env_key".into(), Value::String(ENV_KEY_NAME.into()));
        // Codex 官方当前版本（v0.138+）wire_api 只支持 "responses"："chat"
        // 已被移除（启动即崩），无 "anthropic" 取值。统一显式写 "responses"，
        // 新旧版本均识别；第三方中转需支持 OpenAI Responses 协议（这是
        // Codex 侧硬约束，非本功能可绕过）。
        entry.insert("wire_api".into(), Value::String("responses".into()));
        providers_table.insert(target.provider_id.clone(), Value::Table(entry));
        table.insert(
            "model_provider".into(),
            Value::String(target.provider_id.clone()),
        );
        // 顶层 model 与当前 Provider 绑定：目标未指定模型时清除旧值，
        // 避免上一个受管 Provider 的模型名残留导致请求 404/400。
        if let Some(model) = target.model.as_deref() {
            table.insert("model".into(), Value::String(model.into()));
            // 为自定义模型生成受管 catalog（消除 metadata 警告 + 提供正确窗口）
            let catalog_path = self.write_model_catalog(model, target.context_window)?;
            table.insert(
                "model_catalog_json".into(),
                Value::String(catalog_path.to_string_lossy().replace('\\', "/")),
            );
        } else {
            table.remove("model");
            table.remove("model_catalog_json");
        }
        let raw = toml::to_string_pretty(&config).context("序列化 codex config.toml 失败")?;
        atomic_write(&self.config_path, raw.as_bytes())
    }

    fn revert_to_official(&self, reverted: Option<&ProviderTarget>) -> Result<()> {
        if !self.config_path.exists() {
            return Ok(());
        }
        let mut config = self.read_config()?;
        let Some(table) = config.as_table_mut() else {
            return Ok(());
        };
        let mut changed = false;
        let managed_ids = table
            .get("model_providers")
            .and_then(Value::as_table)
            .map(|providers| {
                providers
                    .keys()
                    .filter(|key| key.starts_with(PROVIDER_ID_PREFIX))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !managed_ids.is_empty() {
            let providers = table
                .get_mut("model_providers")
                .and_then(Value::as_table_mut)
                .expect("managed ids 来自存在的表");
            for id in managed_ids {
                providers.remove(&id);
            }
            changed = true;
            if providers.is_empty() {
                table.remove("model_providers");
            }
        }
        let provider_ref = table
            .get("model_provider")
            .and_then(Value::as_str)
            .map(str::to_string);
        if provider_ref
            .as_deref()
            .is_some_and(|id| id.starts_with(PROVIDER_ID_PREFIX))
        {
            table.remove("model_provider");
            changed = true;
        }
        // 只删除与刚回退的受管 Provider 相同的顶层 model，避免误删用户自己的设置。
        if let Some(target) = reverted {
            if let Some(model) = target.model.as_deref() {
                if table.get("model").and_then(Value::as_str) == Some(model) {
                    table.remove("model");
                    changed = true;
                }
            }
        }
        // 受管 catalog：值指向本功能写入的 catalog 文件时才删除（保留用户自配的）。
        let managed_catalog = table
            .get("model_catalog_json")
            .and_then(Value::as_str)
            .is_some_and(|value| value.ends_with(CATALOG_FILE_NAME));
        if managed_catalog {
            table.remove("model_catalog_json");
            changed = true;
            let catalog_path = self.config_path.with_file_name(CATALOG_FILE_NAME);
            if catalog_path.exists() {
                let _ = fs::remove_file(&catalog_path);
            }
        }
        if !changed {
            return Ok(());
        }
        let raw = toml::to_string_pretty(&config).context("序列化 codex config.toml 失败")?;
        atomic_write(&self.config_path, raw.as_bytes())
    }

    fn effective(&self) -> Result<EffectiveConfig> {
        if !self.config_path.exists() {
            return Ok(EffectiveConfig::default());
        }
        let raw = fs::read_to_string(&self.config_path)
            .with_context(|| format!("读取 {} 失败", self.config_path.display()))?;
        // 解析失败必须返回 Err（而非静默按官方处理），让 config_unreadable
        // 生效——否则损坏的 config.toml 会显示「官方登录」且无警告条。
        raw.parse::<Value>()
            .map(|_| config_relay_effective(&raw))
            .with_context(|| format!("解析 {} 失败", self.config_path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::codex_acp::providers::ProviderWireApi;

    /// 按测试名区分目录（cargo 并行跑时同 pid 共享目录会互删，见 kimi.rs）。
    fn tmp_dir() -> PathBuf {
        let test = std::thread::current()
            .name()
            .unwrap_or_default()
            .replace(['/', '\\', ':'], "_");
        let dir = std::env::temp_dir().join(format!("codex-writer-test-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target(provider_id: &str) -> ProviderTarget {
        ProviderTarget {
            provider_id: provider_id.into(),
            name: "中转".into(),
            base_url: "https://api.example.com/v1".into(),
            model: Some("gpt-5.2".into()),
            model_slots: None,
            context_window: None,
            wire_api: ProviderWireApi::Openai,
            api_key: Some("test-api-key-1234567890".into()),
        }
    }

    #[test]
    fn apply_writes_provider_and_preserves_other_tables() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "[mcp_servers.demo]\ncommand = \"echo\"\n",
        )
        .unwrap();
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let config: Value = fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let provider = &config["model_providers"]["pv-aaaaaaaaaaaa"];
        assert_eq!(provider["name"], "中转".into());
        assert_eq!(provider["base_url"], "https://api.example.com/v1".into());
        assert_eq!(provider["env_key"], "OPENAI_API_KEY".into());
        assert_eq!(provider["wire_api"], "responses".into());
        assert_eq!(config["model_provider"], "pv-aaaaaaaaaaaa".into());
        assert_eq!(config["model"], "gpt-5.2".into());
        // 其他表保留
        assert_eq!(config["mcp_servers"]["demo"]["command"], "echo".into());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_removes_only_managed_blocks() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "model_provider = \"pv-aaaaaaaaaaaa\"\nmodel = \"gpt-5.2\"\n\n[model_providers.pv-aaaaaaaaaaaa]\nname = \"中转\"\nbase_url = \"https://api.example.com/v1\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"chat\"\n\n[model_providers.user-own]\nname = \"自建\"\nbase_url = \"https://other.example.com/v1\"\nenv_key = \"MY_KEY\"\nwire_api = \"chat\"\n\n[mcp_servers.demo]\ncommand = \"echo\"\n",
        )
        .unwrap();
        writer
            .revert_to_official(Some(&target("pv-aaaaaaaaaaaa")))
            .unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = raw.parse().unwrap();
        assert!(config.get("model_provider").is_none());
        assert!(config.get("model").is_none());
        assert!(config
            .get("model_providers")
            .unwrap()
            .get("pv-aaaaaaaaaaaa")
            .is_none());
        // 用户自建 provider 与无关表保留
        assert_eq!(
            config["model_providers"]["user-own"]["env_key"],
            "MY_KEY".into()
        );
        assert_eq!(config["mcp_servers"]["demo"]["command"], "echo".into());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_uses_custom_context_window_in_catalog() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        let mut custom = target("pv-aaaaaaaaaaaa");
        custom.context_window = Some(1_048_576);
        writer.apply(&custom).unwrap();
        let catalog: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(dir.join("pinvou3-model-catalog.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            catalog["models"][0]["context_window"].as_i64().unwrap(),
            1_048_576
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_writes_model_catalog_and_revert_cleans_it() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = raw.parse().unwrap();
        let catalog_ref = config["model_catalog_json"].as_str().unwrap();
        assert!(catalog_ref.ends_with("pinvou3-model-catalog.json"));
        // catalog 文件生成且字段满足 codex 0.146 的必填结构
        let catalog: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(dir.join("pinvou3-model-catalog.json")).unwrap(),
        )
        .unwrap();
        let model = &catalog["models"][0];
        assert_eq!(model["slug"].as_str().unwrap(), "gpt-5.2");
        assert!(model["context_window"].is_i64());
        assert!(model["base_instructions"]
            .as_str()
            .unwrap()
            .starts_with("You are Codex"));
        // 恢复官方：键删除 + 文件删除
        writer
            .revert_to_official(Some(&target("pv-aaaaaaaaaaaa")))
            .unwrap();
        let config: Value = fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(config.get("model_catalog_json").is_none());
        assert!(!dir.join("pinvou3-model-catalog.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_keeps_user_owned_top_level_model() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "model_provider = \"pv-aaaaaaaaaaaa\"\nmodel = \"user-model\"\n\n[model_providers.pv-aaaaaaaaaaaa]\nname = \"中转\"\nbase_url = \"https://api.example.com/v1\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"chat\"\n",
        )
        .unwrap();
        writer
            .revert_to_official(Some(&target("pv-aaaaaaaaaaaa")))
            .unwrap();
        let config: Value = fs::read_to_string(dir.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        // 顶层 model 与受管 provider 的 model 不同 → 保留
        assert_eq!(config["model"], "user-model".into());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_noop_without_managed_blocks_skips_write() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(dir.join("config.toml"), "model = \"gpt-5.2\"\n").unwrap();
        writer.revert_to_official(None).unwrap();
        assert!(!dir.join("config.toml.pinvou3-bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_overwrite_unparseable_file() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        let broken = "model_provider = \"pv-x\"\n[unclosed";
        fs::write(dir.join("config.toml"), broken).unwrap();
        assert!(writer.apply(&target("pv-aaaaaaaaaaaa")).is_err());
        assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), broken);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_created_once() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(dir.join("config.toml"), "model = \"gpt-5.2\"\n").unwrap();
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        writer.apply(&target("pv-bbbbbbbbbbbb")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("config.toml.pinvou3-bak")).unwrap(),
            "model = \"gpt-5.2\"\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_errors_on_unparseable_file() {
        let dir = tmp_dir();
        let writer = CodexConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "model_provider = \"pv-x\"\n[unclosed",
        )
        .unwrap();
        // 损坏文件必须返回 Err（config_unreadable 依赖该 Err），而非静默按官方
        assert!(writer.effective().is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn relay_effective_and_env_key_detection() {
        let active = "model_provider = \"pv-aaaaaaaaaaaa\"\n\n[model_providers.pv-aaaaaaaaaaaa]\nname = \"x\"\nbase_url = \"https://api.example.com/v1\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"chat\"\n";
        assert_eq!(
            config_relay_effective(active),
            EffectiveConfig {
                relay_active: true,
                provider_hint: Some("pv-aaaaaaaaaaaa".into()),
                // relay 激活时生效值展示：model_provider + base_url（无顶层 model）
                entries: vec![
                    EffectiveEntry {
                        key: "model_provider".to_string(),
                        value: "pv-aaaaaaaaaaaa".to_string(),
                        secret: false,
                    },
                    EffectiveEntry {
                        key: "model_providers.pv-aaaaaaaaaaaa.base_url".to_string(),
                        value: "https://api.example.com/v1".to_string(),
                        secret: false,
                    },
                ],
            }
        );
        assert!(codex_config_relay_env_key_present(active));
        // 指向不存在表 / 无 env_key → 未激活
        assert_eq!(
            config_relay_effective("model_provider = \"missing\"\n"),
            EffectiveConfig::default()
        );
        let no_key = "model_provider = \"pv-aaaaaaaaaaaa\"\n\n[model_providers.pv-aaaaaaaaaaaa]\nname = \"x\"\nbase_url = \"https://api.example.com/v1\"\n";
        assert!(!codex_config_relay_env_key_present(no_key));
        assert!(config_relay_effective(no_key).relay_active);
    }
}
