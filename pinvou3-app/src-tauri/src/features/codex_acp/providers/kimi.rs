//! Kimi 配置写入器：`~/.kimi-code/config.toml`。
//!
//! 结构与 `kimi_runtime_config_ready` 校验的官方格式一致：`providers.<id>`（type
//! / base_url / api_key）+ `models.<id>-main`（provider / model / max_context_size）
//! + 顶层 `default_model`。恢复官方登录只删除受管 `pv-*` 块与指向它们的
//! `default_model`，官方登录写入的 OAuth 表与其他配置保留。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml::Value;

use super::{
    atomic_write, AgentConfigWriter, EffectiveConfig, EffectiveEntry, ProviderTarget,
    ProviderWireApi, PROVIDER_ID_PREFIX,
};

pub struct KimiConfigWriter {
    config_path: PathBuf,
}

impl KimiConfigWriter {
    /// `root` = `~/.kimi-code` 目录（生产传 `kimi_data_root()`，单测传临时目录）。
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
        toml::from_str::<Value>(&raw).with_context(|| {
            format!(
                "{} 不是有效的 TOML，拒绝覆盖；请手动修复或删除该文件后重试",
                self.config_path.display()
            )
        })
    }
}

fn model_id_for(provider_id: &str) -> String {
    format!("{provider_id}-main")
}

impl AgentConfigWriter for KimiConfigWriter {
    fn apply(&self, target: &ProviderTarget) -> Result<()> {
        let mut config = self.read_config()?;
        let table = config.as_table_mut().context("config.toml 顶层必须是表")?;
        let providers = table
            .entry("providers")
            .or_insert_with(|| Value::Table(Default::default()));
        let providers_table = providers.as_table_mut().context("providers 必须是表")?;
        let mut entry = toml::map::Map::new();
        // type 映射遵循 Kimi Code 官方文档：`kimi` 为托管服务/Kimi Platform
        // API key 的专用类型（默认 api.moonshot.ai/v1，支持视频等专属能力）；
        // 第三方中转按协议选 openai / anthropic。
        let wire = match target.wire_api {
            ProviderWireApi::Kimi => "kimi",
            ProviderWireApi::Openai => "openai",
            ProviderWireApi::Anthropic => "anthropic",
        };
        entry.insert("type".into(), Value::String(wire.into()));
        entry.insert(
            "base_url".into(),
            Value::String(super::trim_base_url(&target.base_url)),
        );
        if let Some(key) = target.api_key.as_deref() {
            entry.insert("api_key".into(), Value::String(key.into()));
        }
        providers_table.insert(target.provider_id.clone(), Value::Table(entry));

        let models = table
            .entry("models")
            .or_insert_with(|| Value::Table(Default::default()));
        let models_table = models.as_table_mut().context("models 必须是表")?;
        let model_id = model_id_for(&target.provider_id);
        let mut model = toml::map::Map::new();
        model.insert("provider".into(), Value::String(target.provider_id.clone()));
        let model_name = target
            .model
            .clone()
            .unwrap_or_else(|| super::kimi_default_model().to_string());
        model.insert("model".into(), Value::String(model_name.clone()));
        model.insert(
            "max_context_size".into(),
            Value::Integer(
                target
                    .context_window
                    .unwrap_or_else(super::kimi_default_context_size),
            ),
        );
        models_table.insert(model_id.clone(), Value::Table(model));
        table.insert("default_model".into(), Value::String(model_id));
        let raw = toml::to_string_pretty(&config).context("序列化 kimi config.toml 失败")?;
        atomic_write(&self.config_path, raw.as_bytes())
    }

    fn revert_to_official(&self, _reverted: Option<&ProviderTarget>) -> Result<()> {
        if !self.config_path.exists() {
            return Ok(());
        }
        let mut config = self.read_config()?;
        let Some(table) = config.as_table_mut() else {
            return Ok(());
        };
        let mut changed = false;
        let managed_providers = table
            .get("providers")
            .and_then(Value::as_table)
            .map(|providers| {
                providers
                    .keys()
                    .filter(|key| key.starts_with(PROVIDER_ID_PREFIX))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !managed_providers.is_empty() {
            let providers = table
                .get_mut("providers")
                .and_then(Value::as_table_mut)
                .expect("managed ids 来自存在的表");
            for id in managed_providers {
                providers.remove(&id);
            }
            changed = true;
            if providers.is_empty() {
                table.remove("providers");
            }
        }
        let managed_models = table
            .get("models")
            .and_then(Value::as_table)
            .map(|models| {
                models
                    .keys()
                    .filter(|key| key.starts_with(PROVIDER_ID_PREFIX))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !managed_models.is_empty() {
            let models = table
                .get_mut("models")
                .and_then(Value::as_table_mut)
                .expect("managed ids 来自存在的表");
            for id in managed_models {
                models.remove(&id);
            }
            changed = true;
            if models.is_empty() {
                table.remove("models");
            }
        }
        let default_ref = table
            .get("default_model")
            .and_then(Value::as_str)
            .map(str::to_string);
        if default_ref
            .as_deref()
            .is_some_and(|id| id.starts_with(PROVIDER_ID_PREFIX))
        {
            table.remove("default_model");
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        let raw = toml::to_string_pretty(&config).context("序列化 kimi config.toml 失败")?;
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
        let config = toml::from_str::<Value>(&raw)
            .with_context(|| format!("解析 {} 失败", self.config_path.display()))?;
        let Some(default_model) = config
            .get("default_model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(EffectiveConfig::default());
        };
        let Some(provider) = config
            .get("models")
            .and_then(|models| models.get(default_model))
            .and_then(Value::as_table)
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(EffectiveConfig::default());
        };
        // relay 判定：provider 表存在**且不含 oauth 子表**才算中转。官方
        // 登录写入的是 `providers."managed:kimi-code"` + `[…].oauth` 子表，
        // 也满足「default_model → models → provider 存在」链条，不排除会被
        // 误判为外部中转配置（externalActive 误报）。
        let relay_active = config
            .get("providers")
            .and_then(|providers| providers.get(provider))
            .and_then(Value::as_table)
            .is_some_and(|table| !table.contains_key("oauth"));
        // 生效值展示（F4 可见化）：default_model / 模型 ID / base_url，
        // **不含** api_key。
        let mut entries = Vec::new();
        entries.push(EffectiveEntry {
            key: "default_model".to_string(),
            value: default_model.to_string(),
            secret: false,
        });
        if let Some(value) = config
            .get("models")
            .and_then(|models| models.get(default_model))
            .and_then(Value::as_table)
            .and_then(|model| model.get("model"))
            .and_then(Value::as_str)
        {
            if !value.is_empty() {
                entries.push(EffectiveEntry {
                    key: format!("models.{default_model}.model"),
                    value: value.to_string(),
                    secret: false,
                });
            }
        }
        if let Some(value) = config
            .get("providers")
            .and_then(|providers| providers.get(provider))
            .and_then(Value::as_table)
            .and_then(|table| table.get("base_url"))
            .and_then(Value::as_str)
        {
            if !value.is_empty() {
                entries.push(EffectiveEntry {
                    key: format!("providers.{provider}.base_url"),
                    value: value.to_string(),
                    secret: false,
                });
            }
        }
        Ok(EffectiveConfig {
            relay_active,
            provider_hint: Some(provider.to_string()),
            entries,
        })
    }

    /// 官方登录（`kimi login` OAuth）写入的 default_model（如 "kimi-code/k3"）；
    /// 受管的 pv-* 值不算官方值，返回 None 表示无需恢复。
    fn current_default_model(&self) -> Result<Option<String>> {
        if !self.config_path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&self.config_path)
            .with_context(|| format!("读取 {} 失败", self.config_path.display()))?;
        let Ok(config) = toml::from_str::<Value>(&raw) else {
            return Ok(None);
        };
        Ok(config
            .get("default_model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && !value.starts_with(PROVIDER_ID_PREFIX))
            .map(str::to_string))
    }

    /// 恢复官方 default_model：仅在当前 default_model 缺失或仍指向受管 pv-*
    /// 时写回，避免覆盖用户手动修改的官方默认值。
    fn restore_default_model(&self, model: Option<&str>) -> Result<()> {
        let Some(model) = model.filter(|value| !value.trim().is_empty()) else {
            return Ok(());
        };
        if !self.config_path.exists() {
            return Ok(());
        }
        let mut config = self.read_config()?;
        let Some(table) = config.as_table_mut() else {
            return Ok(());
        };
        let current = table
            .get("default_model")
            .and_then(Value::as_str)
            .map(str::to_string);
        let needs_restore = match current.as_deref() {
            None => true,
            Some(current) => current.starts_with(PROVIDER_ID_PREFIX),
        };
        if !needs_restore {
            return Ok(());
        }
        table.insert("default_model".into(), Value::String(model.to_string()));
        let raw = toml::to_string_pretty(&config).context("序列化 kimi config.toml 失败")?;
        atomic_write(&self.config_path, raw.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::codex_acp::providers::ProviderWireApi;

    /// 用测试线程名（= 测试函数名）区分目录：cargo 并行跑多个测试时
    /// 同进程不同测试若共用 `{pid}` 目录会互删文件（评审发现 27 failed）。
    fn tmp_dir() -> PathBuf {
        let test = std::thread::current()
            .name()
            .unwrap_or_default()
            .replace(['/', '\\', ':'], "_");
        let dir = std::env::temp_dir().join(format!("kimi-writer-test-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target(provider_id: &str) -> ProviderTarget {
        ProviderTarget {
            provider_id: provider_id.into(),
            name: "中转".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            model: Some("kimi-k2.5".into()),
            model_slots: None,
            context_window: None,
            wire_api: ProviderWireApi::Openai,
            api_key: Some("test-api-key-1234567890".into()),
        }
    }

    #[test]
    fn apply_output_passes_runtime_config_ready() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        // 产物必须通过官方校验函数（真实解析路径）
        assert!(crate::features::codex_acp::introspect::kimi_runtime_config_ready(&raw, false));
        let config: Value = toml::from_str(&raw).unwrap();
        assert_eq!(config["default_model"], "pv-aaaaaaaaaaaa-main".into());
        assert_eq!(
            config["providers"]["pv-aaaaaaaaaaaa"]["type"],
            "openai".into()
        );
        assert_eq!(
            config["providers"]["pv-aaaaaaaaaaaa"]["api_key"],
            "test-api-key-1234567890".into()
        );
        assert_eq!(
            config["models"]["pv-aaaaaaaaaaaa-main"]["provider"],
            "pv-aaaaaaaaaaaa".into()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kimi_native_wire_writes_type_kimi() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        let mut target = target("pv-aaaaaaaaaaaa");
        target.wire_api = ProviderWireApi::Kimi;
        writer.apply(&target).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = toml::from_str(&raw).unwrap();
        // Kimi 原生协议 → type = "kimi"（Kimi Code 官方文档专用类型）
        assert_eq!(
            config["providers"]["pv-aaaaaaaaaaaa"]["type"],
            "kimi".into()
        );
        // 产物仍通过官方校验函数
        assert!(crate::features::codex_acp::introspect::kimi_runtime_config_ready(&raw, false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_uses_custom_context_window() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        let mut custom = target("pv-aaaaaaaaaaaa");
        custom.context_window = Some(262_144);
        writer.apply(&custom).unwrap();
        let config: Value =
            toml::from_str(&fs::read_to_string(dir.join("config.toml")).unwrap()).unwrap();
        assert_eq!(
            config["models"]["pv-aaaaaaaaaaaa-main"]["max_context_size"],
            Value::Integer(262_144)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_preserves_official_login_tables() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        // 模拟官方登录写入的 OAuth provider 表
        // 表名含点号必须加引号（`[models."kimi-k2.5"]`），否则 TOML 把 `.5`
        // 解析成 `models.kimi-k2` 的子表，断言取不到 `models["kimi-k2.5"]`。
        fs::write(
            dir.join("config.toml"),
            "default_model = \"kimi-k2.5\"\n\n[models.\"kimi-k2.5\"]\nprovider = \"official-oauth\"\nmodel = \"kimi-k2.5\"\nmax_context_size = 200000\n\n[providers.official-oauth]\ntype = \"anthropic\"\n\n[providers.official-oauth.oauth]\nclient_id = \"x\"\n",
        )
        .unwrap();
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let config: Value =
            toml::from_str(&fs::read_to_string(dir.join("config.toml")).unwrap()).unwrap();
        // 官方表保留
        assert!(config["providers"]["official-oauth"]["oauth"].is_table());
        assert_eq!(config["models"]["kimi-k2.5"]["model"], "kimi-k2.5".into());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_removes_only_managed_blocks() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "default_model = \"pv-aaaaaaaaaaaa-main\"\n\n[models.pv-aaaaaaaaaaaa-main]\nprovider = \"pv-aaaaaaaaaaaa\"\nmodel = \"kimi-k2.5\"\nmax_context_size = 200000\n\n[providers.pv-aaaaaaaaaaaa]\ntype = \"openai\"\napi_key = \"sk-x\"\nbase_url = \"https://api.moonshot.cn/v1\"\n\n[models.official]\nprovider = \"official-oauth\"\nmodel = \"kimi-k2.5\"\nmax_context_size = 200000\n\n[providers.official-oauth]\ntype = \"anthropic\"\n",
        )
        .unwrap();
        writer.revert_to_official(None).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = toml::from_str(&raw).unwrap();
        assert!(config.get("default_model").is_none());
        assert!(config
            .get("providers")
            .unwrap()
            .get("pv-aaaaaaaaaaaa")
            .is_none());
        assert!(config
            .get("models")
            .unwrap()
            .get("pv-aaaaaaaaaaaa-main")
            .is_none());
        // 官方表保留
        assert_eq!(config["models"]["official"]["model"], "kimi-k2.5".into());
        assert_eq!(
            config["providers"]["official-oauth"]["type"],
            "anthropic".into()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_noop_without_managed_blocks_skips_write() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        fs::write(dir.join("config.toml"), "default_model = \"kimi-k2.5\"\n").unwrap();
        writer.revert_to_official(None).unwrap();
        assert!(!dir.join("config.toml.pinvou3-bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_overwrite_unparseable_file() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        let broken = "default_model = \"x\"\n[unclosed";
        fs::write(dir.join("config.toml"), broken).unwrap();
        assert!(writer.apply(&target("pv-aaaaaaaaaaaa")).is_err());
        assert_eq!(fs::read_to_string(dir.join("config.toml")).unwrap(), broken);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_default_model_roundtrip() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        // 模拟官方登录写入的配置：官方 OAuth 登录会写 `[…].oauth` 子表，
        // `kimi_runtime_config_ready` 依赖它（缺失会被判为未就绪）。
        fs::write(
            dir.join("config.toml"),
            "default_model = \"kimi-code/k3\"\n\n[models.\"kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\n\n[providers.\"managed:kimi-code\".oauth]\nclient_id = \"x\"\n",
        )
        .unwrap();
        // 切换前记录官方 default_model
        assert_eq!(
            writer.current_default_model().unwrap(),
            Some("kimi-code/k3".to_string())
        );
        // 切换：default_model 被覆盖为受管值
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = toml::from_str(&raw).unwrap();
        assert_eq!(config["default_model"], "pv-aaaaaaaaaaaa-main".into());
        // 受管值不算官方值
        assert_eq!(writer.current_default_model().unwrap(), None);
        // 恢复官方：先 revert（删 pv-* 与指向它的 default_model），再写回官方值
        writer.revert_to_official(None).unwrap();
        writer.restore_default_model(Some("kimi-code/k3")).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        let config: Value = toml::from_str(&raw).unwrap();
        assert_eq!(config["default_model"], "kimi-code/k3".into());
        assert!(config
            .get("providers")
            .unwrap()
            .get("pv-aaaaaaaaaaaa")
            .is_none());
        // 恢复后的产物重新通过官方校验（官方登录态不断裂）
        assert!(crate::features::codex_acp::introspect::kimi_runtime_config_ready(&raw, true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_default_model_keeps_user_override() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        fs::write(
            dir.join("config.toml"),
            "default_model = \"user-model\"\n\n[models.\"user-model\"]\nprovider = \"x\"\nmodel = \"m\"\nmax_context_size = 1000\n\n[providers.x]\ntype = \"openai\"\n",
        )
        .unwrap();
        // 用户手动改过 default_model：不覆盖
        writer.restore_default_model(Some("kimi-code/k3")).unwrap();
        let raw = fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(raw.contains("default_model = \"user-model\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_errors_on_unparseable_file() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        fs::write(dir.join("config.toml"), "default_model = \"x\"\n[unclosed").unwrap();
        // 损坏文件必须返回 Err（config_unreadable 依赖该 Err），而非静默按官方
        assert!(writer.effective().is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn effective_detects_relay_provider() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        assert_eq!(writer.effective().unwrap(), EffectiveConfig::default());
        writer.apply(&target("pv-aaaaaaaaaaaa")).unwrap();
        assert_eq!(
            writer.effective().unwrap(),
            EffectiveConfig {
                relay_active: true,
                provider_hint: Some("pv-aaaaaaaaaaaa".into()),
                // relay 激活时生效值展示：default_model + model + base_url
                entries: vec![
                    EffectiveEntry {
                        key: "default_model".to_string(),
                        value: "pv-aaaaaaaaaaaa-main".to_string(),
                        secret: false,
                    },
                    EffectiveEntry {
                        key: "models.pv-aaaaaaaaaaaa-main.model".to_string(),
                        value: "kimi-k2.5".to_string(),
                        secret: false,
                    },
                    EffectiveEntry {
                        key: "providers.pv-aaaaaaaaaaaa.base_url".to_string(),
                        value: "https://api.moonshot.cn/v1".to_string(),
                        secret: false,
                    },
                ],
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_oauth_config_is_not_relay() {
        let dir = tmp_dir();
        let writer = KimiConfigWriter::new(&dir);
        // 官方登录（kimi login OAuth）写入的结构：managed provider + oauth 子表，
        // 也满足 default_model → models → provider 链条，但不得判为中转
        fs::write(
            dir.join("config.toml"),
            "default_model = \"kimi-code/k3\"\n\n[models.\"kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\n\n[providers.\"managed:kimi-code\".oauth]\nclient_id = \"x\"\n",
        )
        .unwrap();
        let effective = writer.effective().unwrap();
        assert!(!effective.relay_active, "官方 OAuth 配置不得判为中转");
        // 手写/外部工具配置的中转（无 oauth 子表）仍判为中转
        fs::write(
            dir.join("config.toml"),
            "default_model = \"kimi-code/k3\"\n\n[models.\"kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\nmax_context_size = 1048576\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\nbase_url = \"https://api.kimi.com/coding/v1\"\napi_key = \"sk-x\"\n",
        )
        .unwrap();
        assert!(
            writer.effective().unwrap().relay_active,
            "无 oauth 子表的自定义 provider 应判为中转"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
