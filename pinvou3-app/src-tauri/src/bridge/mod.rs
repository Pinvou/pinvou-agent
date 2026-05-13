//! pinvou3-app 与 DeepSeek-TUI 之间的抽象层（"bridge"）。
//!
//! 职责：
//! 1. 加载/持久化 [`UserPrefs`]（GUI 可调的视觉/语言偏好，序列化在
//!    `~/.pinvou3/settings.json`）
//! 2. 维护 `~/.pinvou3/` 目录布局并把内嵌 [`bundle`] 首启解包到 `bundle/`
//! 3. **把 prefs + bundle 翻译成 [`EngineConfig`] / [`DtConfig`]**——所有
//!    字段都显式列出，禁用 spread `..Default::default()`，让上游加字段时
//!    `cargo build` 报"missing field"，强制 review 是否对 pinvou3 安全。
//!
//! 用户层面看不到这一层；这层只服务 GUI 与 deepseek-tui engine 之间的
//! 转译。GUI 永远不直接操纵 EngineConfig；engine.rs 永远从这层取配置。

pub mod bundle;
pub mod paths;
pub mod prefs;
pub mod sessions;

use std::path::PathBuf;

use anyhow::Result;
use deepseek_tui::config::{Config as DtConfig, ProvidersConfig};
use deepseek_tui::core::engine::EngineConfig;
use deepseek_tui::core::ops::Op;
use deepseek_tui::hooks::{Hook, HookEvent, HooksConfig};
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;

use self::bundle::Pinvou3Bundle;
use self::prefs::{ModelPreset, UserPrefs};

/// Qwen3.6 在 vLLM 里是 passthrough 字符串（不走 alias）。
const LOCAL_VLLM_MODEL: &str = "/model";
const LOCAL_VLLM_BASE_URL: &str = "http://10.214.74.113:8000/v1";
const LOCAL_VLLM_API_KEY: &str = "local-no-auth";

#[derive(Debug, Clone)]
pub struct Pinvou3Bridge {
    pub prefs: UserPrefs,
    pub bundle: Pinvou3Bundle,
    pub workspace: PathBuf,
}

impl Pinvou3Bridge {
    /// 启动序列：确保 `~/.pinvou3/` 子目录存在 → 解包 bundle → 加载 prefs。
    /// 首次启动写一份默认 `settings.json` 让用户/开发者方便手改 advanced。
    ///
    /// **workspace 现为 `$HOME`**（阶段 C 调整）——让 AI 能用 read_file/glob
    /// 找到用户在桌面/文档/下载里的真实文件。配套敏感目录禁令在
    /// `bundle/instructions.md` 里引导，硬拦截后续走 deepseek-tui hook 注册。
    ///
    /// **$PINVOU3_SESSION_ARTIFACTS** 环境变量在这里 set：让 LLM 通过
    /// `write_file` 写"产出"时落到 `~/.pinvou3/sessions/<id>/artifacts/`，
    /// 不污染用户家目录。多 session 切换时调用方应重新 set。
    pub fn boot() -> Result<Self> {
        paths::ensure_dirs()?;
        let bundle = Pinvou3Bundle::paths();
        bundle.ensure_extracted()?;
        let prefs = UserPrefs::load();
        if !paths::settings_path().exists() {
            prefs.save().ok();
        }
        let artifacts = paths::default_session_artifacts_dir();
        std::env::set_var("PINVOU3_SESSION_ARTIFACTS", &artifacts);
        Ok(Self {
            prefs,
            bundle,
            workspace: paths::user_home_dir(),
        })
    }

    pub fn locale_tag(&self) -> &'static str {
        self.prefs.language.locale_tag()
    }

    pub fn model(&self) -> String {
        match self.prefs.advanced.model_preset.unwrap_or_default() {
            ModelPreset::LocalVllm => {
                std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| LOCAL_VLLM_MODEL.into())
            }
        }
    }

    /// env > prefs.advanced > 默认 true。
    pub fn allow_shell(&self) -> bool {
        if let Ok(v) = std::env::var("PINVOU3_ALLOW_SHELL") {
            return matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        self.prefs.advanced.allow_shell.unwrap_or(true)
    }

    /// env > prefs.advanced > 16384。
    pub fn max_output_tokens(&self) -> u32 {
        if let Ok(v) = std::env::var("PINVOU3_MAX_OUTPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                return n;
            }
        }
        self.prefs.advanced.max_output_tokens.unwrap_or(16384)
    }

    /// system prompt 注入路径数组：bundle 必有，user 可选。
    /// 顺序：先 bundle，再 user（user 在后，覆盖效应由上游 prompt 拼接决定）。
    pub fn instruction_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if self.bundle.instructions_md.is_file() {
            out.push(self.bundle.instructions_md.clone());
        }
        let user = paths::user_instructions();
        if user.is_file() {
            out.push(user);
        }
        out
    }

    /// 构造 [`EngineConfig`]：**显式列出每个字段**。
    ///
    /// 实现技巧：先 destructure 上游 `EngineConfig::default()`——destructure 模式
    /// 不带 `..` 时，上游加新字段会让本处编译报"missing field"，强制 reviewer
    /// 决定该字段对 pinvou3 是否安全。pinvou3 自定义字段标记 `_` 忽略原 default
    /// 值；纯透传字段命名变量再放进新结构体。
    pub fn build_engine_config(&self) -> EngineConfig {
        let EngineConfig {
            // —— pinvou3 自定义（destructure 这里 `_`，新结构体里覆盖）——
            model: _,
            workspace: _,
            allow_shell: _,
            trust_mode: _,
            notes_path: _,
            mcp_config_path: _,
            skills_dir: _,
            instructions: _,
            project_context_pack_enabled: _,
            max_steps: _,
            max_subagents: _,
            snapshots_enabled: _,
            memory_enabled: _,
            memory_path: _,
            locale_tag: _,
            strict_tool_mode: _,
            // —— 上游 default 透传（命名后放进新结构体）——
            features,
            compaction,
            cycle,
            capacity,
            todos,
            plan_state,
            max_spawn_depth,
            network_policy,
            lsp_config,
            runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
        } = EngineConfig::default();

        EngineConfig {
            // pinvou3 覆盖
            model: self.model(),
            workspace: self.workspace.clone(),
            allow_shell: self.allow_shell(),
            trust_mode: true,
            notes_path: paths::notes_path(),
            mcp_config_path: paths::mcp_config_path(),
            skills_dir: self.bundle.skills_dir.clone(),
            instructions: self.instruction_paths(),
            project_context_pack_enabled: false,
            max_steps: self.prefs.advanced.max_steps.unwrap_or(100),
            max_subagents: self.prefs.advanced.max_subagents.unwrap_or(4),
            snapshots_enabled: false,
            memory_enabled: false,
            memory_path: paths::memory_path(),
            locale_tag: self.locale_tag().to_string(),
            strict_tool_mode: false,
            // 上游 default 透传
            features,
            // compaction model 默认 deepseek-v4-pro,本地 vLLM 没这个模型,
            // 必须改成 pinvou3 当前用的 model,否则手动 /compact 报 404。
            compaction: deepseek_tui::compaction::CompactionConfig {
                model: self.model(),
                ..compaction
            },
            cycle,
            capacity,
            todos,
            plan_state,
            max_spawn_depth,
            network_policy,
            lsp_config,
            runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
        }
    }

    /// 构造 deepseek-tui 顶层 [`DtConfig`]：锁定本地 vLLM + Qwen3.6 +
    /// 注入敏感目录拦截 hook。
    /// 环境变量优先（兼容 run-dev.sh 里既有的 `DEEPSEEK_*` 设置）。
    pub fn build_dt_config(&self) -> DtConfig {
        let mut cfg = DtConfig::default();
        cfg.provider = Some(
            std::env::var("DEEPSEEK_PROVIDER").unwrap_or_else(|_| "vllm".to_string()),
        );
        cfg.api_key = Some(
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_else(|_| LOCAL_VLLM_API_KEY.to_string()),
        );
        let base_url = std::env::var("DEEPSEEK_BASE_URL")
            .unwrap_or_else(|_| LOCAL_VLLM_BASE_URL.to_string());
        let providers = cfg.providers.get_or_insert_with(ProvidersConfig::default);
        providers.vllm.base_url = Some(base_url);
        cfg.default_text_model = Some(self.model());
        // Qwen3.6 thinking 必须关，否则 SSE idle timeout
        cfg.reasoning_effort = Some("off".to_string());
        cfg.hooks = Some(self.build_hooks_config());
        cfg
    }

    /// 注入硬拦截 hook：ToolCallBefore 时 spawn 一个 shell 脚本检查 tool args
    /// 是否触碰敏感目录（~/.ssh / ~/.gnupg / ~/.aws / 等），命中 exit 1
    /// 让上游拒绝该 tool 调用。脚本本体在 bundle 中,首次启动解包到
    /// `~/.pinvou3/bundle/deny_sensitive_paths.sh`。
    fn build_hooks_config(&self) -> HooksConfig {
        let script = self.bundle.deny_sensitive_sh.to_string_lossy().to_string();
        HooksConfig {
            enabled: true,
            hooks: vec![Hook {
                event: HookEvent::ToolCallBefore,
                command: format!("bash {script}"),
                condition: None,
                timeout_secs: 5,
                background: false,
                continue_on_error: false,
                name: Some("pinvou3-sensitive-firewall".into()),
            }],
            default_timeout_secs: Some(5),
            working_dir: None,
        }
    }

    /// 构造发给 engine 的 [`Op::SendMessage`]——pinvou3 永远走 Yolo + auto_approve。
    ///
    /// 注：DeepSeek-TUI 当前的 `auto_approve` 字段不旁路 `await_tool_approval`
    /// （上游 bug），所以 event forwarder 仍要监听 ApprovalRequired 并主动
    /// 调 `approve_tool_call`。这条逻辑见 `engine.rs::spawn_event_forwarder`。
    pub fn build_send_message_op(&self, content: String) -> Op {
        Op::SendMessage {
            content,
            mode: AppMode::Yolo,
            model: self.model(),
            goal_objective: None,
            reasoning_effort: Some("off".to_string()),
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: self.allow_shell(),
            trust_mode: true,
            auto_approve: true,
            approval_mode: ApprovalMode::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bridge() -> Pinvou3Bridge {
        Pinvou3Bridge {
            prefs: UserPrefs::default(),
            bundle: Pinvou3Bundle::paths(),
            workspace: std::env::temp_dir(),
        }
    }

    /// 安全敏感字段必须固定——这些值改了会让 pinvou3 出现奇怪行为或越权。
    #[test]
    fn engine_config_locks_critical_fields() {
        let cfg = fixture_bridge().build_engine_config();
        assert!(cfg.trust_mode, "trust_mode 必须 true（pinvou3 是 yolo）");
        assert!(
            !cfg.strict_tool_mode,
            "strict_tool_mode 必须 false（Qwen3.6 用宽松模式）"
        );
        assert!(!cfg.snapshots_enabled, "snapshots 不开（用户没 git workspace）");
        assert!(
            !cfg.project_context_pack_enabled,
            "project context pack 不开（非 dev 用户没 project）"
        );
        assert!(!cfg.memory_enabled, "memory feature 暂不开（Phase C）");
        assert_eq!(cfg.locale_tag, "zh-Hans", "默认中文 locale");
    }

    /// 语言切换必须传到 engine.locale_tag。
    #[test]
    fn locale_tag_follows_language_pref() {
        let mut bridge = fixture_bridge();
        bridge.prefs.language = prefs::Language::En;
        assert_eq!(bridge.locale_tag(), "en");
        assert_eq!(bridge.build_engine_config().locale_tag, "en");
    }

    /// allow_shell 默认 true（pinvou3 yolo 模式需要）。
    #[test]
    fn allow_shell_defaults_to_true() {
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        assert!(fixture_bridge().allow_shell());
    }

    /// env 优先级高于 prefs。
    #[test]
    fn allow_shell_env_overrides_prefs() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.allow_shell = Some(true);
        std::env::set_var("PINVOU3_ALLOW_SHELL", "false");
        assert!(!bridge.allow_shell());
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
    }

    /// 路径必须落在 ~/.pinvou3/ 下，绝不能落 ~/.deepseek/。
    #[test]
    fn engine_config_paths_isolated_from_deepseek() {
        let cfg = fixture_bridge().build_engine_config();
        let ds = std::env::var("HOME").unwrap_or_default() + "/.deepseek";
        assert!(
            !cfg.skills_dir.starts_with(&ds),
            "skills_dir 跑到 ~/.deepseek 了: {}",
            cfg.skills_dir.display()
        );
        assert!(!cfg.mcp_config_path.starts_with(&ds));
        assert!(!cfg.notes_path.starts_with(&ds));
        assert!(!cfg.memory_path.starts_with(&ds));
    }

    /// 阶段 C：bridge.workspace 必须透传到 EngineConfig.workspace。
    /// 不直接测 boot()——boot 会 mutate PINVOU3_HOME 跟其他测试 race。
    /// 单独验证 paths::user_home_dir() 的逻辑见 paths.rs 测试。
    #[test]
    fn engine_config_workspace_follows_bridge_field() {
        let mut bridge = fixture_bridge();
        bridge.workspace = std::path::PathBuf::from("/tmp/pinvou3-ws-fixture");
        assert_eq!(
            bridge.build_engine_config().workspace,
            std::path::PathBuf::from("/tmp/pinvou3-ws-fixture")
        );
    }
}
