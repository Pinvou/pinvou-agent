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
pub mod marketplace;
pub mod mode_state;
pub mod paths;
pub mod prefs;
pub mod sessions;

use std::path::PathBuf;

use anyhow::Result;
use deepseek_tui::config::{
    wire_model_for_provider, ApiProvider, Config as DtConfig, ProvidersConfig,
};
use deepseek_tui::core::engine::EngineConfig;
use deepseek_tui::core::ops::Op;
use deepseek_tui::hooks::{Hook, HookEvent, HooksConfig};
use deepseek_tui::prompts::InstructionSource;
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;

use self::bundle::{Pinvou3Bundle, INSTRUCTIONS_MD};
use self::mode_state::PlanPhase;
use self::prefs::{ModelPreset, UserPrefs};

/// Qwen3.6 在 vLLM 里是 passthrough 字符串（不走 alias）。
///
/// 后缀 `_256k` 由 fork B1 (`context_window_for_model` 的 `_Nk` hint) 识别,
/// 让底座为本地 Qwen 派生 256K 窗口 → context_input_budget / capacity ratio /
/// compaction 派生路径全部能算对。若改名为无后缀,底座立刻退化到 `None`,
/// preflight + emergency recovery 默认不生效 (codex adversarial-review 抓到的
/// 高优 finding)。回归测试 `bridge::tests::default_model_window_recognized`
/// 锁住这个不变量。
///
/// ⚠️ ops 同步要求:vLLM 启动也要带
/// `--served-model-name qwen36_35b_256k`,否则 OpenAI-compat API 报
/// `model_not_found`。
const LOCAL_VLLM_MODEL: &str = "qwen36_35b_256k";
// 127.0.0.1 让 .deb 装到任何机器都默认连本机 vLLM(全量包 install.sh
// 起 systemd 容器 --network host 绑 0.0.0.0:8000);vLLM 与应用同机,
// 用 loopback 免疫 DHCP 换 IP,别再写具体内网 IP。
const LOCAL_VLLM_BASE_URL: &str = "http://127.0.0.1:8000/v1";
const LOCAL_VLLM_API_KEY: &str = "local-no-auth";

fn is_official_deepseek_base_url(base_url: &str) -> bool {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/beta")
        .trim_end_matches("/v1")
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "https://api.deepseek.com" | "https://api.deepseeki.com"
    )
}

fn official_deepseek_model_name(model: &str) -> String {
    let model = wire_model_for_provider(ApiProvider::Deepseek, model);
    match model.to_ascii_lowercase().as_str() {
        "deepseek-v4-pro" => "deepseek-v4-pro".to_string(),
        "deepseek-v4-flash" => "deepseek-v4-flash".to_string(),
        _ => model,
    }
}

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
        // ⓪ 注入 pinvou3 版 prompt 文案到底座 prompt 合成层(base/locale/authority)。
        // 幂等(底座 OnceLock 首次生效、后续 Err 被忽略),必须早于任何 engine spawn。
        // 编译期内嵌常量,不依赖 bundle 解包。dump_system_prompt bin 也经此 boot,故
        // dump 同样生效。见 docs/base-prompt-override-阶段2.md。
        bundle::install_prompt_overrides();
        paths::ensure_dirs()?;
        let bundle = Pinvou3Bundle::paths();
        bundle.ensure_extracted()?;
        let prefs = UserPrefs::load();
        if !paths::settings_path().exists() {
            prefs.save().ok();
        }
        let artifacts = paths::default_session_artifacts_dir();
        std::env::set_var("PINVOU3_SESSION_ARTIFACTS", &artifacts);
        let this = Self {
            prefs,
            bundle,
            workspace: paths::user_home_dir(),
        };
        this.wire_max_output_tokens_env();
        // C 方案(P-no-disk)最终版: 清理所有 pinvou3 历史 disk 残留:
        //   • `~/.pinvou3/sessions/<sid>/instructions.md`(per-session inline 前路径)
        //   • `~/.pinvou3/workspace_context.md`(workspace context 已合并进 INSTRUCTIONS_MD §0)
        //   • `~/.codewhale/instructions.md` / `~/.deepseek/instructions.md`(早期 P-brand 路径)
        // 不再生成任何 pinvou3-managed disk 文件 — 所有 prompt 内容走 Inline。
        this.cleanup_legacy_pinvou3_disk_files();
        Ok(this)
    }

    /// 清扫所有早期版本 pinvou3 写过的 prompt-related disk 文件。C-fork P-no-disk
    /// 最终态 disk 完全干净,所有 prompt 内容走 `InstructionSource::Inline` 内存注入。
    ///
    /// 清单(只清 pinvou3-managed / auto-gen 内容,用户自定义文件保留):
    ///   • `~/.pinvou3/sessions/<sid>/instructions.md` — per-session inline 前路径(全清)
    ///   • `~/.pinvou3/workspace_context.md` — workspace context 合并进 INSTRUCTIONS_MD §0 前路径
    ///   • `~/.codewhale/instructions.md` + `~/.deepseek/instructions.md` — 早期 P-brand 路径
    fn cleanup_legacy_pinvou3_disk_files(&self) {
        let mut removed = 0usize;

        // (1) sessions/*/instructions.md — 无条件清(per-session pinvou3 自家产物,不会用户编辑)
        if let Ok(entries) = std::fs::read_dir(paths::sessions_root()) {
            for entry in entries.flatten() {
                let path = entry.path().join("instructions.md");
                if path.is_file() && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }

        // (2)(3)(4) 单文件 — 只清 pinvou3-managed / auto-gen 标识的,用户自定义保留
        for legacy in [
            self.workspace.join(".pinvou3").join("workspace_context.md"),
            self.workspace.join(".codewhale").join("instructions.md"),
            self.workspace.join(".deepseek").join("instructions.md"),
        ] {
            if let Ok(existing) = std::fs::read_to_string(&legacy) {
                let head: String = existing.chars().take(200).collect();
                let is_auto_gen = head.contains("Project Structure (Auto-generated)");
                let is_pinvou3_managed = head.contains("pinvou3 workspace context");
                if (is_auto_gen || is_pinvou3_managed)
                    && std::fs::remove_file(&legacy).is_ok()
                {
                    removed += 1;
                }
            }
        }

        if removed > 0 {
            eprintln!(
                "[pinvou3-app] cleaned up {removed} legacy disk file(s) \
                 (C-fork P-no-disk: prompt content now Inline in memory)"
            );
        }
    }

    /// 把 `self.max_output_tokens()` 写到底座读取的 `DEEPSEEK_MAX_OUTPUT_TOKENS`
    /// env (核心:底座 `effective_max_output_tokens()` 只读这个 env)。
    ///
    /// 生产 Tauri 启动不走 run-dev.sh (clean env), 没这一步会让底座回到
    /// 模型表启发式。这里显式写入 pinvou3 的本地 vLLM 输出预算,确保 dev /
    /// release / headless harness 行为一致。
    ///
    /// 已有 env 时不覆盖 (允许 run-dev.sh / L1 harness / 用户 override)。
    ///
    /// 单独抽 helper 让测试可以不走 boot() (避免 ensure_dirs / extract_bundle
    /// 写盘到真实 ~/.pinvou3 + 不需要拿 PINVOU3_HOME ENV_LOCK)。
    pub fn wire_max_output_tokens_env(&self) {
        if std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS").is_none() {
            std::env::set_var(
                "DEEPSEEK_MAX_OUTPUT_TOKENS",
                self.max_output_tokens().to_string(),
            );
        }
    }

    /// 测试入口(L1 harness 用):同 [`boot`] 但 workspace 用传入的 `ws`
    /// (通常是 scenario 自己的 tempdir),而不是 `paths::user_home_dir()`。
    /// 让 L1 真 vLLM dialog harness 能给每个 scenario 一个隔离的产出目录,
    /// 避免污染用户 $HOME 也避免 scenario 之间互相干扰。
    #[allow(dead_code)] // L1 runner 接入前临时 unused
    pub fn boot_with_workspace(ws: PathBuf) -> Result<Self> {
        let mut this = Self::boot()?;
        this.workspace = ws;
        Ok(this)
    }

    pub fn locale_tag(&self) -> &'static str {
        self.prefs.language.locale_tag()
    }

    /// 用 INSTRUCTIONS_MD 模板，把 `{{PINVOU3_WORKSPACE}}` 替换成指定 session 的
    /// 独立 workspace 目录,返回渲染后的字符串(供 [`session_instructions`] 用)。
    pub fn build_session_system_prompt(&self, session_id: &str) -> String {
        let ws = paths::session_workspace_dir(session_id);
        // 同时确保目录存在,AI 写 write_file 时不会因为目录不存在而失败
        let _ = std::fs::create_dir_all(&ws);
        // [pinvou3] date/workspace 已移出静态 system → per-turn <turn_meta>:每 session
        // 变的 workspace 路径(及每天变的 date)若进 cached system prefix, vLLM prefix-cache
        // MISS 时工具调用会退化成裸文本(实测 single subagent 25%→稳态~100%)。仅保留 model
        // (固定值,不破坏 cache)与 sudo(静态文案兜底,实时状态走 super_permission::turn_reminder)。
        INSTRUCTIONS_MD
            .replace("{{PINVOU3_MODEL}}", &self.model())
            .replace(
                "{{PINVOU3_SUDO_INSTRUCTION}}",
                crate::super_permission::instruction_block(),
            )
    }

    /// 当前 active session 的 workspace 目录。
    pub fn session_workspace(&self, session_id: &str) -> std::path::PathBuf {
        paths::session_workspace_dir(session_id)
    }

    /// session 专属 `EngineConfig.instructions` 注入:
    ///   1. pinvou3 自家 INSTRUCTIONS_MD 渲染版(走 `InstructionSource::Inline`,
    ///      不写 disk — 见 C 方案 P-no-disk 决策);
    ///   2. 用户自定义 `~/.codewhale/instructions.md`(可选,仍走 `File`)。
    ///
    /// 之前版本写 `~/.pinvou3/sessions/<sid>/instructions.md` disk 文件然后传
    /// `Vec<PathBuf>` 给底座 — 改用 `InstructionSource::Inline` 后:
    ///  • disk 上没了多余的 instructions.md 给用户造成混淆
    ///  • 多引擎并发不再依赖 per-session 文件避免 race(内存对象天然隔离)
    ///  • rehydrate 不再从 disk 重读,内容跟 EngineConfig 一起在内存里活
    pub fn session_instructions(&self, session_id: &str) -> Vec<InstructionSource> {
        let mut out: Vec<InstructionSource> = Vec::new();
        let rendered = self.build_session_system_prompt(session_id);
        out.push(InstructionSource::Inline {
            name: format!("pinvou3:sessions/{session_id}/instructions"),
            content: rendered,
        });
        let user = paths::user_instructions();
        if user.is_file() {
            out.push(InstructionSource::File(user));
        }
        out
    }

    /// 当前 active provider 标识（传给底座 `DtConfig.provider`）。
    pub fn provider(&self) -> String {
        if is_official_deepseek_base_url(&self.base_url()) {
            return "deepseek".to_string();
        }
        if let Ok(v) = std::env::var("DEEPSEEK_PROVIDER") {
            return v;
        }
        match self.prefs.advanced.model_preset.unwrap_or_default() {
            ModelPreset::LocalVllm => "vllm".to_string(),
            ModelPreset::Deepseek => "deepseek".to_string(),
            ModelPreset::Kimi => "moonshot".to_string(),
            ModelPreset::OpenaiCompatible
            | ModelPreset::Qwen
            | ModelPreset::Doubao
            | ModelPreset::Minimax
            | ModelPreset::Glm
            | ModelPreset::Mimo => "openai".to_string(),
        }
    }

    /// 当前 active 模型名（传给底座 `DtConfig.default_text_model` / `EngineConfig.model`）。
    /// 环境变量 > settings.custom_model_name > 厂商默认值。
    pub fn model(&self) -> String {
        let is_official_deepseek = is_official_deepseek_base_url(&self.base_url());
        if let Ok(v) = std::env::var("DEEPSEEK_MODEL") {
            if is_official_deepseek {
                return official_deepseek_model_name(&v);
            }
            return v;
        }
        if let Some(model) = self.prefs.advanced.custom_model_name.clone() {
            if is_official_deepseek {
                return official_deepseek_model_name(&model);
            }
            return model;
        }
        if is_official_deepseek {
            return "deepseek-v4-pro".to_string();
        }
        self.default_model_for_preset()
    }

    /// 各厂商默认模型名。
    fn default_model_for_preset(&self) -> String {
        match self.prefs.advanced.model_preset.unwrap_or_default() {
            ModelPreset::LocalVllm => LOCAL_VLLM_MODEL.into(),
            ModelPreset::Deepseek => "deepseek-v4-pro".to_string(),
            ModelPreset::Kimi => "kimi-k2.6".to_string(),
            ModelPreset::OpenaiCompatible => "gpt-4o".to_string(),
            ModelPreset::Qwen => "qwen-max".to_string(),
            ModelPreset::Doubao => "doubao-pro-256k".to_string(),
            ModelPreset::Minimax => "abab6.5s-chat".to_string(),
            ModelPreset::Glm => "glm-4-plus".to_string(),
            ModelPreset::Mimo => "mimo-v2-flash".to_string(),
        }
    }

    /// 当前 active base_url（传给底座 `DtConfig.providers.*.base_url`）。
    /// 环境变量 > settings.custom_base_url > 厂商默认值。
    pub fn base_url(&self) -> String {
        if let Ok(v) = std::env::var("DEEPSEEK_BASE_URL") {
            return v;
        }
        self.prefs
            .advanced
            .custom_base_url
            .clone()
            .unwrap_or_else(|| self.default_base_url_for_preset())
    }

    /// 各厂商默认 API base URL。
    fn default_base_url_for_preset(&self) -> String {
        match self.prefs.advanced.model_preset.unwrap_or_default() {
            ModelPreset::LocalVllm => LOCAL_VLLM_BASE_URL.into(),
            ModelPreset::Deepseek => "https://api.deepseek.com".to_string(),
            ModelPreset::Kimi => "https://api.moonshot.cn/v1".to_string(),
            ModelPreset::OpenaiCompatible => "https://api.openai.com/v1".to_string(),
            ModelPreset::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            ModelPreset::Doubao => "https://ark.cn-beijing.volces.com/api/v3".to_string(),
            ModelPreset::Minimax => "https://api.minimax.chat/v1".to_string(),
            ModelPreset::Glm => "https://open.bigmodel.cn/api/paas/v4".to_string(),
            ModelPreset::Mimo => "https://api.xiaomimimo.com/v1".to_string(),
        }
    }

    /// 当前 active api_key（传给底座 `DtConfig.api_key`）。
    pub fn api_key(&self) -> String {
        if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
            return v;
        }
        if is_official_deepseek_base_url(&self.base_url()) {
            return self
                .prefs
                .advanced
                .custom_api_key
                .clone()
                .unwrap_or_default();
        }
        match self.prefs.advanced.model_preset.unwrap_or_default() {
            ModelPreset::LocalVllm => LOCAL_VLLM_API_KEY.into(),
            _ => self
                .prefs
                .advanced
                .custom_api_key
                .clone()
                .unwrap_or_default(),
        }
    }

    /// env > prefs.advanced > 默认 true。
    pub fn allow_shell(&self) -> bool {
        if let Ok(v) = std::env::var("PINVOU3_ALLOW_SHELL") {
            return matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        self.prefs.advanced.allow_shell.unwrap_or(true)
    }

    /// env > prefs.advanced > 24576 (24K)。
    /// 24K 而非 64K:系统 prompt 强制"大产物分块写"(write_file skeleton ≤8KB →
    /// append_file chunks ≤16KB/次,见 bundle/instructions.md §4 + Pinvou 审查 >20KB
    /// 单写判 CRITICAL),且 thinking 关 → 单次回复 ≈ ≤16KB chunk ≈ 3-5K tokens。
    /// 24K 覆盖该上限 + 弱模型偶尔超写到 ~24KB 的 margin;同时把输入预算从 189K(74%)
    /// 抬到 230K(90%),让自动压缩更晚触发。64K 是 ~4x 设计上限的过度预留。
    pub fn max_output_tokens(&self) -> u32 {
        if let Ok(v) = std::env::var("PINVOU3_MAX_OUTPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                return n;
            }
        }
        self.prefs.advanced.max_output_tokens.unwrap_or(24_576)
    }

    /// legacy 单引擎路径(headless harness 用):走 INSTRUCTIONS_MD inline + 用户自定义。
    /// 跟 [`session_instructions`] 区别仅在不带 session_id —— 直接用 INSTRUCTIONS_MD 原文
    /// (不替换 `{{PINVOU3_WORKSPACE}}`)。
    pub fn instructions(&self) -> Vec<InstructionSource> {
        let mut out: Vec<InstructionSource> = vec![InstructionSource::Inline {
            name: "pinvou3:bundle/instructions".to_string(),
            content: INSTRUCTIONS_MD.to_string(),
        }];
        let user = paths::user_instructions();
        if user.is_file() {
            out.push(InstructionSource::File(user));
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
            translation_enabled: _,
            vision_config: _,
            subagent_api_timeout: _, // pinvou3 自定义 (见下),本地慢推理 120s 不够
            // —— 上游 default 透传（命名后放进新结构体）——
            features,
            compaction,
            capacity,
            todos,
            plan_state,
            max_spawn_depth,
            network_policy: _, // pinvou3 显式构造 (见下),不透传 default(None)
            lsp_config,
            mut runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
            snapshots_max_workspace_bytes,
            search_provider: _, // pinvou3 显式构造 (见下),由 prefs.search 翻译
            search_api_key: _,
            // —— v0.8.47 上游新增字段,透传 default ——
            show_thinking,
            goal_state,
            tools_always_load,
            prefer_bwrap,
            // pinvou3-fork 自定义:tool_whitelist 通用白名单机制。监工废弃后(2026-06-15)
            // 已无人设值,普通 + workflow session 均 None(不限),机制保留待用。
            tool_whitelist: _,
            // pinvou3-fork 自定义:会话初始思考开关(显式构造见下)
            reasoning_effort: _,
            // —— v0.8.49 上游新增字段,透传 default ——
            allowed_tools,
            tools,
            // —— v0.8.51 上游新增字段,透传 default(speech 输出目录 / hook executor)——
            speech_output_dir,
            hook_executor,
            // —— v0.8.53 上游新增字段,透传 default(subagent 心跳超时;配 subagent
            //    lifecycle hooks feat)。⚠️ 本地慢 vLLM 下或需像 subagent_api_timeout
            //    一样调大,先透传 default,验证后再评估。——
            subagent_heartbeat_timeout,
            // —— v0.8.54-57 上游新增字段,透传 default ——
            //   search_base_url: 自定义搜索后端 base URL(pinvou3 用内置 provider → None)。
            //   stream_chunk_timeout: 单 chunk SSE 超时。⚠️ 本地慢 vLLM 下或需像
            //   subagent_api_timeout 一样调大(配 C3 SSE idle-timeout 遥测),先透传 default 验证。
            search_base_url,
            stream_chunk_timeout,
            // —— v0.8.58-60 上游新增字段,透传 default ——
            //   verbosity: concise 输出模式(CLI noninteractive 默认;GUI → None)。
            //   interactive_launch_limit: #3095 交互 fanout 闸信号量上限(default 4)。
            //   goal_token_budget / goal_status: /goal 目标管理(GUI 暂不用,透传)。
            //   disallowed_tools: codewhale exec --disallowed-tools(CLI 专用,GUI → None)。
            verbosity,
            interactive_launch_limit,
            goal_token_budget,
            goal_status,
            disallowed_tools,
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
            instructions: self.instructions(),
            project_context_pack_enabled: false,
            max_steps: self.prefs.advanced.max_steps.unwrap_or(100),
            // 2026-05-27: 默认 10 (原 1 → 10),为 PPT 工作流 fan-out 场景预留。
            // 原始锁定 2026-05-19 是避免 multi-subagent 并发在弱模型 + 单 vLLM 下 timeout。
            // 实测 single subagent + 串行 2-3 subagent 都可用,fan-out 4+ 仍有 timeout 风险,
            // 但走 SubAgentManager.max_agents fallback 不 hard crash。
            // 出问题再回退,不预先限制。
            max_subagents: self.prefs.advanced.max_subagents.unwrap_or(10),
            snapshots_enabled: false,
            memory_enabled: false,
            memory_path: paths::memory_path(),
            locale_tag: self.locale_tag().to_string(),
            strict_tool_mode: false,
            // pinvou3 中文用户已经是中文语境，不走 /translate 路径
            translation_enabled: false,
            // 视觉配置跟随主模型端点：本地 vLLM 复用同一端点；
            // 第三方 provider 也复用（若不支持 vision，底座会优雅失败）。
            vision_config: Some(deepseek_tui::config::VisionModelConfig {
                model: self.model(),
                api_key: Some(self.api_key()),
                base_url: Some(self.base_url()),
            }),
            // [pinvou3-fork] 上游默认 120s 是为 DeepSeek 云端 API 设计。
            // 本地 Qwen3.6 vLLM 慢推理下单 step 30-90s 很常见,120s 频繁误杀子 agent。
            // 300s 与 elapsed cap 对齐,给复杂研究类任务留出完整单步窗口。
            subagent_api_timeout: std::time::Duration::from_secs(300),
            // 开启 VisionModel feature(默认 Experimental 关):配合上面的
            // vision_config,tool_setup.rs 才会注册 image_analyze 工具给 LLM。
            // 两道门缺一不可——只配 vision_config 不开 feature,工具不会注册。
            features: {
                let mut f = features;
                f.enable(deepseek_tui::features::Feature::VisionModel);
                f
            },
            // compaction model 默认 deepseek-v4-pro,本地 vLLM 没这个模型,
            // 必须改成 pinvou3 当前用的 model,否则手动 /compact 报 404。
            //
            // 本地 Qwen3.6 vLLM 跑 256K (max_model_len=262144)。
            // 两条压缩触发(turn_loop 内顺序:先 should_compact:116,后 emergency:201):
            //  - should_compact(nice LLM 摘要):可摘要子集 > token_threshold − pinned,
            //    等价于 总量 > ~token_threshold + 近4条。
            //  - emergency(强制,recover_context_overflow):全量 input > context_input_budget
            //    = 窗口 − effective_max_output(24,576) − headroom(1,024) = 230,400 (256K 上)。
            //
            // ⚠️ 关键:emergency 的 230,400 是**派生硬天花板**(留满 24K 输出的输入上限,
            //    随 max_output_tokens 改而变)。token_threshold 必须**显著低于**它,否则
            //    should_compact 永远轮不到——emergency 先越线,nice 路径死掉,每个长会话
            //    都走"Emergency"强制路径(重试 2 次救不回就硬报 context_overflow)。
            //    2026-05 旧值 200K > 当时 budget 189,440 正是这个倒置 bug
            //    (详见 docs/auto-compact-256K-tuning.md「当前实测阈值」)。
            //  - token_threshold = 190K (256K × ~74%):should_compact 在总量 ~195K 触发,
            //    稳在 230,400 安全网之下 ~35K,nice=主路径 / emergency=真·安全网。
            //    回归测试 compaction_threshold_stays_below_emergency_budget 按 max_output
            //    动态算 budget 锁住这个不变式,改 output 预留会自动跟着校验。
            // 上游默认 token_threshold=800K,对 256K 窗口永远撞不到,**必须显式 set**。
            // ⚠️ v0.8.51 上游移除了 CompactionConfig.auto_floor_tokens 字段(floor 概念
            //    随 cycle removal 一并去掉),原 60K 下限设置失效,删除。
            compaction: deepseek_tui::compaction::CompactionConfig {
                model: self.model(),
                token_threshold: 190_000,
                ..compaction
            },
            // ⚠️ v0.8.51 上游整体移除 cycle 子系统(release "cycle removal"):
            //    EngineConfig.cycle 字段不复存在。原 pinvou3 在小窗口下显式关闭 cycle
            //    (防 trigger_floor saturating_sub 归零导致每轮误触发 briefing)的逻辑
            //    随之失效——目标已由上游删除子系统达成,直接删去。
            // capacity controller 保持上游 default = off (2026-05-19 codex
            // adversarial-review round 2 发现:其 low_risk_max / medium_risk_max
            // 是 p_fail 风险阈值而非 context_used_ratio,context 权重只占 15%。
            // 复杂工具轮在 context 远低于 200K 时就可能触发 VerifyAndReplan /
            // VerifyWithToolReplay 改写会话。
            // auto compact 直接用上游 turn_loop:90 的 should_compact preflight,
            // 语义干净:按 token_threshold/auto_floor 决定是否走 LLM 摘要。
            capacity,
            todos,
            plan_state,
            max_spawn_depth,
            // pinvou3 产品要跑在用户自带的 clash/透明代理 fake-ip(TUN) 环境:所有
            // 域名 DNS 解析到 fake-ip 占位段(clash 默认 198.18.0.0/15,IETF benchmark
            // 保留段、无真实服务),底座 fetch_url 自解析后被 SSRF 防护当 restricted 误杀。
            // 修法:按 **IP 段**信任 fake-ip 占位段(`with_trusted_fakeip_cidrs`),而非
            // 按 host 信任(早期 `proxy=["*"]` 会让任意域名解析到真实私网/元数据也放行 →
            // SSRF)。改成 IP 段后:198.18.x 占位放行;`*.lan→192.168.x`、`→169.254.169.254`
            // (云元数据)、IP 字面量仍被 is_restricted_ip 拦。default=Allow 仅指不按 host
            // 弹窗确认(本地可信助手),与 SSRF 兜底正交。
            // 自定义 fake-ip-range 的用户暂未暴露配置(默认段覆盖绝大多数;真有人撞再加)。
            network_policy: Some(
                deepseek_tui::network_policy::NetworkPolicyDecider::new(
                    deepseek_tui::network_policy::NetworkPolicy {
                        default: deepseek_tui::network_policy::DecisionToml::Allow,
                        allow: Vec::new(),
                        deny: Vec::new(),
                        proxy: Vec::new(),
                        audit: false,
                    },
                    None,
                )
                .with_trusted_fakeip_cidrs(&["198.18.0.0/15"]),
            ),
            lsp_config,
            runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
            snapshots_max_workspace_bytes,
            // pinvou3 search 后端: prefs 翻译。
            // Bing 是默认 (fork patch #42 在底座 SearchProvider::default());Metaso/Bocha/Baidu
            // 是 GUI 切换项。底座 web_search 对 Metaso 留空 key 用内置共享 key
            // (~100 次/天),对 Bocha/Baidu 留空 key 直接报 ToolError "requires API key"。
            search_provider: match self.prefs.search.provider {
                prefs::SearchProvider::Bing => deepseek_tui::config::SearchProvider::Bing,
                prefs::SearchProvider::Metaso => deepseek_tui::config::SearchProvider::Metaso,
                prefs::SearchProvider::Bocha => deepseek_tui::config::SearchProvider::Bocha,
                prefs::SearchProvider::Baidu => deepseek_tui::config::SearchProvider::Baidu,
                prefs::SearchProvider::Tavily => deepseek_tui::config::SearchProvider::Tavily,
            },
            search_api_key: self.prefs.search.normalized_api_key(),
            // v0.8.47 上游新增,透传 default
            show_thinking,
            goal_state,
            tools_always_load,
            prefer_bwrap,
            // tool_whitelist 通用机制保留但不再设值:对话型监工(品悟)白名单已废弃,
            // SubAgent 角色工具由 agent_registry.json 约束。
            tool_whitelist: None,
            // 会话初始思考开关:本地 vLLM(Qwen3.6)必须关 thinking。
            // 关键:工作流会话只走 SpawnSubAgent、不发 SendMessage(对话型品悟
            // 已取消),session 拿不到 SendMessage 里那份 off → 角色全员 thinking
            // 全开(6/12 taizi 思考失控实证)。在 engine 配置层钉死,不依赖对话。
            reasoning_effort: if self.provider() == "vllm" {
                Some("off".to_string())
            } else {
                None
            },
            // v0.8.49 上游新增,透传 default
            allowed_tools,
            tools,
            // v0.8.51 上游新增,透传 default
            speech_output_dir,
            hook_executor,
            // v0.8.53 上游新增,透传 default
            subagent_heartbeat_timeout,
            // v0.8.54-57 上游新增,透传 default(search_base_url=None / stream_chunk_timeout)
            search_base_url,
            stream_chunk_timeout,
            // v0.8.58-60 上游新增,透传 default(verbosity/fanout 闸/goal 管理/disallowed_tools)
            verbosity,
            interactive_launch_limit,
            goal_token_budget,
            goal_status,
            disallowed_tools,
        }
    }

    /// 构造 **session 专属** [`EngineConfig`]:在 [`build_engine_config`] 基础上把
    /// `workspace` 换成该 session 的独立工作目录、`instructions` 换成该 session 的
    /// inline 渲染版(`InstructionSource::Inline`,不走 disk)。EnginePool 给每个
    /// session spawn engine 时用这个,让 engine 从 spawn 起就绑定自己的 workspace +
    /// prompt,内存隔离不依赖 disk。
    ///
    /// [`build_engine_config`]: Self::build_engine_config
    pub fn build_engine_config_for_session(&self, session_id: &str) -> EngineConfig {
        let mut cfg = self.build_engine_config();
        cfg.workspace = self.session_workspace(session_id);
        cfg.instructions = self.session_instructions(session_id);
        cfg
    }


    /// 构造 deepseek-tui 顶层 [`DtConfig`]：按 `ModelPreset` 动态路由 provider /
    /// model / base_url / api_key，注入敏感目录拦截 hook。
    /// 环境变量优先（兼容 run-dev.sh 里既有的 `DEEPSEEK_*` 设置）。
    pub fn build_dt_config(&self) -> DtConfig {
        let mut cfg = DtConfig::default();
        let provider = self.provider();
        cfg.provider = Some(provider.clone());
        let api_key = self.api_key();
        cfg.api_key = Some(api_key.clone());
        let base_url = self.base_url();
        let providers = cfg.providers.get_or_insert_with(ProvidersConfig::default);
        // 按 provider 写对应 provider 配置的 base_url + api_key
        match provider.as_str() {
            "vllm" => {
                providers.vllm.base_url = Some(base_url);
                providers.vllm.api_key = Some(api_key);
            }
            "openai" => {
                providers.openai.base_url = Some(base_url);
                providers.openai.api_key = Some(api_key);
            }
            "deepseek" => {
                providers.deepseek.base_url = Some(base_url);
                providers.deepseek.api_key = Some(api_key);
            }
            "moonshot" => {
                providers.moonshot.base_url = Some(base_url);
                providers.moonshot.api_key = Some(api_key);
            }
            _ => {
                providers.vllm.base_url = Some(base_url);
                providers.vllm.api_key = Some(api_key);
            }
        }
        cfg.default_text_model = Some(self.model());
        // 本地 vLLM (Qwen3.6) thinking 必须关，否则 SSE idle timeout；
        // 云端 provider 保留底座默认（用户可在 settings.toml 中覆盖）。
        if self.provider() == "vllm" {
            cfg.reasoning_effort = Some("off".to_string());
        }
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

    /// 构造发给 engine 的 [`Op::SendMessage`]——按 `mode` 切换 trust/approval/sandbox。
    ///
    /// 决策来源：`docs/Plan-YOLO双模式-设计决策.md` 第 4.1 节复用底座 mode 字段。
    ///
    /// | mode | allow_shell | trust_mode | auto_approve | approval_mode | 实际效果 |
    /// |------|-------------|------------|--------------|---------------|---------|
    /// | Yolo | self.allow  | true       | true         | Auto          | 全自动 + 信任全家目录 |
    /// | Plan | true        | false      | true         | Auto          | 只读工具集 + ReadOnly sandbox（底座 tool_setup.rs 按 mode 自动切换） |
    ///
    /// **M1 弱模型加固**: 在 user content 前 prepend `<system-reminder>` 段,
    /// 内容按 `phase` 动态生成。Claude Code 同款机制对抗 long-context 遗忘 +
    /// 强制特定状态行为。Qwen3.6 短期注意力强,放 message 顶端命中率高。
    /// 见决策文档 V2 §13.1。
    ///
    /// 注：DeepSeek-TUI 当前的 `auto_approve` 字段不旁路 `await_tool_approval`
    /// （上游 bug），所以 event forwarder 仍要监听 ApprovalRequired 并主动
    /// 调 `approve_tool_call`。这条逻辑见 `engine.rs::spawn_event_forwarder`。
    pub fn build_send_message_op(
        &self,
        content: String,
        mode: AppMode,
        phase: PlanPhase,
        persona_reminder: Option<String>,
    ) -> Op {
        let (allow_shell, trust_mode) = match mode {
            AppMode::Yolo => (self.allow_shell(), true),
            // Plan: allow_shell=true 让 engine 正常路由 shell 工具，
            // 底座 tool_setup.rs 会把 sandbox 切到 ReadOnly + 工具白名单切到只读集。
            // trust_mode=true 让 list_dir/read_file 等只读工具能跨 session workspace
            // 边界（pinvou3 是本地单用户工具，无跨用户安全边界，写保护靠 ReadOnly
            // sandbox + 只读工具集，不依赖 trust_mode）。
            AppMode::Plan => (true, true),
            // Agent mode pinvou3 不暴露，但保留 default 处理避免 panic
            AppMode::Agent => (self.allow_shell(), false),
        };
        // 超级权限状态每 turn 实时注入(is_enabled() 每次读 disk),绕开
        // refresh_all_instructions no-op 导致的"切开关不生效"——静态 prompt
        // spawn 时渲染一次就过时,这里每 turn 重出。始终注入(连 mode/phase
        // reminder 为 None 的纯 Yolo 态也带上)。
        let sudo = crate::super_permission::turn_reminder();
        let mut reminder_body = match reminder_for(mode, phase) {
            Some(r) => format!("{r}\n\n{sudo}"),
            None => sudo.to_string(),
        };
        // 卡片池: 该 session 加持了专家面具时,每 turn 注入 persona 人设(粘性身份)。
        if let Some(persona) = persona_reminder {
            reminder_body = format!("{reminder_body}\n\n{persona}");
        }
        let full_content = format!(
            "<system-reminder>\n{reminder_body}\n</system-reminder>\n\n{content}"
        );
        Op::SendMessage {
            content: full_content,
            mode,
            model: self.model(),
            goal_objective: None,
            // v0.8.59 上游新增 /goal 目标管理;pinvou3 GUI 不用,取默认(无预算/Active)。
            goal_token_budget: None,
            goal_status: deepseek_tui::tools::goal::GoalStatus::Active,
            // 本地 vLLM (Qwen3.6) thinking 必须关，否则 SSE idle timeout；
            // 云端 provider 保留底座默认（传 None 让底座自行决定）。
            reasoning_effort: {
                if self.provider() == "vllm" {
                    Some("off".to_string())
                } else {
                    None
                }
            },
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell,
            trust_mode,
            auto_approve: true,
            approval_mode: ApprovalMode::Auto,
            translation_enabled: false,
            // v0.8.47 上游新增;pinvou3 reasoning_effort=off 故无实际影响,取默认。
            show_thinking: true,
            // v0.8.49 上游新增;None = 不限制本次消息可用工具,沿用 engine 全量工具表。
            allowed_tools: None,
            // v0.8.51 上游新增;None = 不挂 per-message hook executor,沿用 engine 级默认。
            hook_executor: None,
            // v0.8.59 上游新增 concise verbosity 模式;pinvou3 GUI 走默认详尽,取 None。
            verbosity: None,
        }
    }
}

/// M1: per-turn `<system-reminder>` 文案,按当前 mode+phase 选段。
/// 决策文档 V2 §13.1。
/// 命中率优先于优雅:每段都是命令式、短、列禁令清单(Qwen3.6 友好)。
fn reminder_for(mode: AppMode, phase: PlanPhase) -> Option<&'static str> {
    match (mode, phase) {
        (AppMode::Plan, PlanPhase::Planning) => Some(
            "你现在在 Plan 模式 + Planning 阶段。本 turn 你必须按这个顺序行动:\n\
             1. 任务有歧义 → 调 `request_user_input` 工具问澄清(给 2-3 个选项让用户点选)。\
             不要在 text 里列 A/B/C 选项。\n\
             2. 方案清晰后 → 调 `update_plan` 工具输出方案(explanation 字段写关键决策,\
             items 写 3-8 个执行步骤)。如果产物预计超过 300 行或 20KB(如 HTML deck / 完整网页 / 长报告),\
             plan 必须说明分块写法:`append_file` 只能追加到文件尾;要填已有文件中间或替换占位符,\
             用 `edit_file`,不要用 `append_file`。\
             可选再调 `checklist_write` 拆细。\n\
             3. **禁止**在 text 里描述方案/贴代码/写\"请点【就这么干】\"等按钮引导文字——\
             方案卡片由系统在你调 update_plan 后自动展示,你写引导是死锁。\n\
             4. **禁止**调 `write_file` / `append_file` / `edit_file` / `exec_shell` / `js_execution`——\
             它们在 Plan 模式不可用,调了一定失败。",
        ),
        (AppMode::Plan, PlanPhase::Ready) => Some(
            "Plan 模式 + Ready 阶段。AI 之前的方案已经在 plan 卡片上等用户决策。\n\
             如果用户发了新消息,说明用户在隐式修订——你必须调 `update_plan` 重出方案,\
             不要只在 text 描述,不要假定用户已批准。",
        ),
        (AppMode::Yolo, PlanPhase::Executing) => Some(
            "你现在在执行阶段(用户已批准方案)。本 turn 你必须:\n\
             1. **第一动作**:用 `write_file` / `append_file` / `edit_file` / `exec_shell` 等工具\
             **实际产出文件或代码**。\n\
             2. **禁止**只调 `update_plan` 标记 in_progress 就结束 turn——\
             那是假执行,用户什么文件都没拿到。\n\
             3. 预计超过 300 行或 20KB 的产物,**禁止**一次 `write_file` 写完整文件;\
             分块前先选策略:`append_file` 只能追加到文件尾;要填已有文件中间或替换占位符,\
             用 `edit_file`,不要用 `append_file`。\n\
             4. 一个 turn 内**连续调多个工具**直到所有步骤完成,不要中途停下来等用户。\n\
             5. 完成一步后调 `update_plan` 把对应步骤标 completed,继续下一步。\n\
             6. **禁止**在 text 里贴完整代码代替 write_file/append_file——磁盘上不会有文件。",
        ),
        // 纯 Yolo 路径(用户没进 Plan 模式直接发 task,plan_phase 一直 None):
        // 此前命中 `_ => None` 没注入"大产物拆"规则, 实测 h3c-ppt P7 阶段
        // LLM 决定"create as single mega HTML"撞 SSE timeout。跟 Plan/Executing
        // 同风格:命令式 + 短 + 一句话讲清规则,不暴露底座细节。
        (AppMode::Yolo, PlanPhase::None) => Some(
            "你在 Yolo 模式,直接调工具产出。产物预计超过 300 行或 20KB\
             (HTML deck / 完整网页 / 长报告)时,**禁止**一次 `write_file` 写完整文件;\
             分块前先选策略:`append_file` 只能追加到文件尾;要填已有文件中间或替换占位符,\
             用 `edit_file`,不要用 `append_file`;完成后读回关键片段验证。\
             **禁止**在 text 里贴完整代码代替工具调用——磁盘上不会有文件。",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        vars: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[&'static str]) -> Self {
            Self {
                vars: vars
                    .iter()
                    .map(|&name| (name, std::env::var(name).ok()))
                    .collect(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.vars {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
    }

    fn fixture_bridge() -> Pinvou3Bridge {
        Pinvou3Bridge {
            prefs: UserPrefs::default(),
            bundle: Pinvou3Bundle::paths(),
            workspace: std::env::temp_dir(),
        }
    }

    /// 默认模型名必须能被底座 `context_window_for_model` 识别出窗口,
    /// 否则 `context_input_budget` 静默返回 `None`,preflight + emergency
    /// recovery 全静默禁用 (codex adversarial-review 2026-05-19 抓到的
    /// 高优 finding)。后缀 `_256k` 由 fork B1 `_Nk` hint 解析。
    #[test]
    fn default_model_window_recognized_by_engine() {
        let bridge = fixture_bridge();
        let model = bridge.model();
        let window = deepseek_tui::models::context_window_for_model(&model);
        assert!(
            window.is_some(),
            "底座 context_window_for_model 必须识别默认模型名 (得到 None 意味着 \
             LOCAL_VLLM_MODEL 后缀漏了 _Nk 标记,B2 preflight 静默禁用)。\
             当前 model = {model:?}"
        );
        // 256K = 256_000 (hint 用 ×1000;实际 vLLM 262144 差 6K 在 2% 噪声内)
        assert_eq!(
            window,
            Some(256_000),
            "默认模型应派生 256K 窗口,得到 {window:?}"
        );
    }

    /// 超级权限状态必须每 turn 注入 system-reminder——哪怕 mode/phase reminder
    /// 为 None 的纯 Yolo 态也要带上,否则切开关对当前会话不生效(refresh no-op)。
    /// 锁住:任意 mode/phase 下 op content 都含 `<system-reminder>` + 超级权限字样。
    #[test]
    fn build_send_message_op_always_injects_super_permission_reminder() {
        let bridge = fixture_bridge();
        for (mode, phase) in [
            (AppMode::Yolo, PlanPhase::None),
            (AppMode::Yolo, PlanPhase::Executing),
            (AppMode::Plan, PlanPhase::Planning),
        ] {
            let op = bridge.build_send_message_op("用户消息".to_string(), mode, phase, None);
            let content = match op {
                Op::SendMessage { content, .. } => content,
                other => panic!("期望 SendMessage,得到 {other:?}"),
            };
            assert!(
                content.contains("<system-reminder>") && content.contains("超级权限"),
                "mode={mode:?} phase={phase:?} 的 op 必须每 turn 注入超级权限状态,得到:\n{content}"
            );
        }
    }

    /// 卡片池: 该 session 加持了专家面具时,persona reminder 必须进 per-turn
    /// `<system-reminder>`(粘性身份的核心机制)。None 时不注入(不破坏纯对话)。
    #[test]
    fn build_send_message_op_injects_persona_reminder_when_present() {
        let bridge = fixture_bridge();
        let persona = "你现在戴着【数据库架构师】专家面具。".to_string();
        let op = bridge.build_send_message_op(
            "用户消息".to_string(),
            AppMode::Yolo,
            PlanPhase::None,
            Some(persona.clone()),
        );
        let content = match op {
            Op::SendMessage { content, .. } => content,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        assert!(
            content.contains("<system-reminder>") && content.contains(&persona),
            "加持后 op 必须在 system-reminder 内注入 persona 人设,得到:\n{content}"
        );
        // None 时不应出现该文案
        let op_none =
            bridge.build_send_message_op("hi".to_string(), AppMode::Yolo, PlanPhase::None, None);
        if let Op::SendMessage { content, .. } = op_none {
            assert!(!content.contains("数据库架构师"), "未加持不应注入 persona");
        }
    }

    /// `wire_max_output_tokens_env` 必须把 self.max_output_tokens() 设给底座
    /// env,让 dev / release / headless harness 对同一个本地 vLLM cap 达成一致。
    ///
    /// 走 fixture_bridge() + helper (而非 boot()),避免:
    ///   - 写真实 ~/.pinvou3 (codex round 5 finding)
    ///   - 跟 PINVOU3_HOME ENV_LOCK 持有者冲突
    ///
    /// 两个语义合并一个测试避免并发 race (Rust env process-global,
    /// 后续多测试可以拿 DEEPSEEK_MAX_OUTPUT_TOKENS 专属锁,但目前只此一处)。
    #[test]
    fn wire_max_output_tokens_env_sets_default_then_respects_existing() {
        // clean env 路径:helper 应 set 默认 24576
        std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
        std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS");
        fixture_bridge().wire_max_output_tokens_env();
        assert_eq!(
            std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS").as_deref(),
            Ok("24576"),
            "wire helper 必须 set DEEPSEEK_MAX_OUTPUT_TOKENS=24576, 让底座 \
             effective_max_output_tokens 走 pinvou3 显式 cap (24K,见 max_output_tokens 注释)"
        );

        // 已有 env 不覆盖路径:helper 是 no-op
        std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "32768");
        fixture_bridge().wire_max_output_tokens_env();
        assert_eq!(
            std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS").as_deref(),
            Ok("32768"),
            "已有 env 必须保留,不能被 helper 覆盖 (允许 run-dev.sh / L1 / 用户 override)"
        );
        std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
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
        assert!(
            !cfg.snapshots_enabled,
            "snapshots 不开（用户没 git workspace）"
        );
        assert!(
            !cfg.project_context_pack_enabled,
            "project context pack 不开（非 dev 用户没 project）"
        );
        assert!(!cfg.memory_enabled, "memory feature 暂不开（Phase C）");
        assert_eq!(
            cfg.reasoning_effort.as_deref(),
            Some("off"),
            "本地 vLLM(Qwen3.6)会话初始 thinking 必须关。工作流会话只走 \
             SpawnSubAgent 不发 SendMessage,引擎配置层不钉死 off 角色就全员 \
             thinking 全开(6/12 taizi 思考失控实证)"
        );
        assert_eq!(cfg.locale_tag, "zh-Hans", "默认中文 locale");
        assert_eq!(
            cfg.max_subagents, 10,
            "max_subagents 默认 10：2026-05-27 从 1 解锁，为 PPT 工作流 fan-out 预留。\
             真并发 4+ 在弱模型下仍有 timeout 风险，走 SubAgentManager fallback 不 hard crash"
        );
        assert_eq!(
            cfg.subagent_api_timeout.as_secs(), 300,
            "subagent_api_timeout 必须 300s。上游默认 120s 是为 DeepSeek 云端 API 设计, \
             本地 Qwen3.6 vLLM 慢推理下单 step 30-90s 很常见,120s 频繁误杀子 agent。 \
             300s 与 elapsed cap 对齐,给复杂研究类任务留出完整单步窗口。"
        );
    }

    /// 不变式:should_compact 的 `token_threshold` 必须**显著低于** emergency 的
    /// `context_input_budget`,否则 emergency 先越线、nice LLM 摘要路径永远轮不到
    /// ——2026-06 抓到的倒置 bug(旧值 200K > budget 189,440,每个长会话都走
    /// "Emergency" 强制路径 + 2 次救不回硬报 context_overflow)。
    /// 锁住「nice 主路径 / emergency 真·安全网」的层级,谁动 token_threshold 或
    /// max_output_tokens 导致倒置都会被这条测试挡下。详见 docs/auto-compact-256K-tuning.md。
    #[test]
    fn compaction_threshold_stays_below_emergency_budget() {
        let bridge = fixture_bridge();
        let model = bridge.model();
        let window = deepseek_tui::models::context_window_for_model(&model)
            .expect("默认模型必须有已知窗口(否则 budget 静默 None)") as usize;
        // 复刻底座 context_input_budget(engine/context.rs):
        //   window<500K 时 = window − effective_max_output − headroom。
        //   effective_max_output 即 wire 给 DEEPSEEK_MAX_OUTPUT_TOKENS 的 max_output_tokens()。
        let effective_output = bridge.max_output_tokens() as usize;
        const HEADROOM: usize = 1_024; // 底座 CONTEXT_HEADROOM_TOKENS(context.rs 私有常量)
        let emergency_budget = window - effective_output - HEADROOM;

        let cfg = bridge.build_engine_config();
        let threshold = cfg.compaction.token_threshold;
        // v0.8.51 上游移除 auto_floor_tokens 字段(floor 概念随 cycle removal 去掉),
        // 原 floor < threshold 不变式已无对应字段,删除该断言。

        // ≥20K margin:should_compact 用「可摘要子集」度量、emergency 用「全量 input
        // (含 system+tools)」度量,要留够余量保证 nice 在 emergency 之前清晰触发。
        const MARGIN: usize = 20_000;
        assert!(
            threshold + MARGIN <= emergency_budget,
            "token_threshold({threshold}) 必须 ≤ emergency_budget({emergency_budget}) − {MARGIN}(margin);\
             否则 emergency 抢先、nice 路径死掉(倒置 bug)。window={window} effective_output={effective_output}"
        );
    }

    /// EngineConfig.search_provider 必须由 prefs.search 翻译,不能透传上游 default。
    /// 默认 prefs 是 Bing(国情:DDG 被 GFW + 代理 datacenter IP 反爬,基本不可用)。
    /// 切到 Metaso/Bocha 时 prefs.search.api_key 必须透传到 EngineConfig.search_api_key
    /// (Bocha 必填,Metaso 留空可走底座内置共享 key)。
    /// 下次 sync 若 destructure 块把 search_provider/search_api_key 改回透传 default,
    /// 本测试立刻报错。
    #[test]
    fn forkguard_search_provider_translates_from_prefs() {
        // 默认 prefs → Bing
        let cfg = fixture_bridge().build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Bing);
        assert!(cfg.search_api_key.is_none());

        // 切 Metaso + 自定义 key
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Metaso,
            api_key: Some("mk-user-key".to_string()),
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Metaso);
        assert_eq!(cfg.search_api_key.as_deref(), Some("mk-user-key"));

        // 切 Metaso + 空白 key: bridge 层必须归一化成 None,让底座回退内置共享 key。
        // 若透传 Some(""),旧底座会收到 Metaso HTTP 200 + errCode=2005,
        // 并可能误显示成 No results found。
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Metaso,
            api_key: Some("   ".to_string()),
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Metaso);
        assert!(cfg.search_api_key.is_none());

        // 切 Bocha + 留空 key (UX 上前端应阻止,但 bridge 层透传 None)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Bocha,
            api_key: None,
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Bocha);
        assert!(cfg.search_api_key.is_none());

        // 切 Baidu + key (千帆 AI Search,key 必填)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Baidu,
            api_key: Some("bce-v3-user-key".to_string()),
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Baidu);
        assert_eq!(cfg.search_api_key.as_deref(), Some("bce-v3-user-key"));

        // 切 Baidu + 空白 key 同样归一化为 None,由底座报明确缺 key 错误。
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Baidu,
            api_key: Some("\n\t ".to_string()),
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Baidu);
        assert!(cfg.search_api_key.is_none());

        // 切 Tavily + key (海外 agent 搜索 API,tvly- key 必填)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Tavily,
            api_key: Some("tvly-user-key".to_string()),
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(cfg.search_provider, deepseek_tui::config::SearchProvider::Tavily);
        assert_eq!(cfg.search_api_key.as_deref(), Some("tvly-user-key"));
    }

    /// [pinvou3-fork-guard #18] network_policy 必须 Some 且**只信 fake-ip 占位段**。
    /// 产品跑在用户 clash/TUN fake-ip 环境,域名全解析到 198.18/15,需放行;
    /// 但绝不能信任真实私网(早期 `proxy=["*"]` 会放行任意域名 → 内网 SSRF)。
    /// 上游改 EngineConfig 字段后 bridge 若静默传 None,fake-ip 下联网全废 /
    /// 或信任过宽。
    #[test]
    fn forkguard_network_policy_trusts_fakeip_range_only() {
        let cfg = fixture_bridge().build_engine_config();
        let decider = cfg
            .network_policy
            .as_ref()
            .expect("network_policy 必须 Some(配置 fake-ip 信任段)");
        assert!(
            decider.is_trusted_fakeip_addr(&"198.18.0.1".parse().unwrap()),
            "fake-ip 占位段(198.18/15)必须被信任,否则 TUN 下联网工具被自家 SSRF 防护误杀"
        );
        assert!(
            !decider.is_trusted_fakeip_addr(&"192.168.0.1".parse().unwrap()),
            "真实私网不得被信任(SSRF 边界)"
        );
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

    /// 把 build_send_message_op 返回的 Op 解构成 (allow_shell, trust_mode)，
    /// 失败 panic（测试用 helper）。
    fn extract_shell_trust(op: Op) -> (bool, bool) {
        match op {
            Op::SendMessage {
                allow_shell,
                trust_mode,
                ..
            } => (allow_shell, trust_mode),
            other => panic!("expected SendMessage, got {other:?}"),
        }
    }

    /// L2-5: Yolo 模式 → trust_mode=true（pinvou3 是本地单用户工具，
    /// yolo 路径默认放开 trust 让产物落任意用户授权目录）。
    #[test]
    fn bridge_yolo_mode_trust_mode_true() {
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        let bridge = fixture_bridge();
        let op = bridge.build_send_message_op("hi".into(), AppMode::Yolo, PlanPhase::None, None);
        let (_allow_shell, trust_mode) = extract_shell_trust(op);
        assert!(trust_mode, "Yolo 模式 trust_mode 必须 true");
    }

    /// L2-6: Plan 模式 → trust_mode=true（P1 修复回归，原本是 false 导致
    /// list_dir 跨 session workspace 边界报 PathEscape）。
    #[test]
    fn bridge_plan_mode_trust_mode_true_after_p1() {
        let bridge = fixture_bridge();
        let op =
            bridge.build_send_message_op("list dir".into(), AppMode::Plan, PlanPhase::Planning, None);
        let (_allow_shell, trust_mode) = extract_shell_trust(op);
        assert!(
            trust_mode,
            "Plan 模式 trust_mode 必须 true (P1 修复点，防 list_dir PathEscape 回归)"
        );
    }

    /// L2-7: Plan 模式 → allow_shell=true（让底座 tool_setup.rs 正常路由
    /// shell 工具到 ReadOnly sandbox + 只读工具白名单；allow_shell=false
    /// 会直接屏蔽掉 shell 工具入口，Plan 阶段 AI 反而连只读 exec_shell ls
    /// 都用不了）。
    #[test]
    fn bridge_plan_mode_allow_shell_true() {
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        let bridge = fixture_bridge();
        let op = bridge.build_send_message_op("exec ls".into(), AppMode::Plan, PlanPhase::Planning, None);
        let (allow_shell, _trust_mode) = extract_shell_trust(op);
        assert!(
            allow_shell,
            "Plan 模式 allow_shell 必须 true (tool_setup.rs 依赖此字段路由工具集)"
        );
    }

    /// L2-8: workspace 路径已从静态 system **移出** → per-turn `<turn_meta>` 的
    /// `Current workspace`(见 engine.rs turn_metadata_block)。每 session 变的路径若进
    /// cached system prefix 会让 vLLM prefix-cache MISS、工具调用退化成裸文本(实测 single
    /// subagent 25%→稳态~100%),故 build_session_system_prompt 不再含 session-specific
    /// 路径,保持跨 session 字节静态。
    #[test]
    fn instructions_md_session_workspace_subst() {
        let bridge = fixture_bridge();
        let session_id = "test-l2-session-9f8a-2c1b";
        let prompt = bridge.build_session_system_prompt(session_id);
        assert!(
            !prompt.contains("{{PINVOU3_WORKSPACE}}"),
            "WORKSPACE 占位符已删, 不该残留"
        );
        assert!(
            !prompt.contains(session_id),
            "workspace 路径(含 session_id)必须移出静态 system → turn_meta, 实际仍含: {}",
            &prompt.chars().take(200).collect::<String>()
        );
    }

    #[test]
    fn large_artifact_reminders_explain_append_file_tail_only() {
        for (mode, phase) in [
            (AppMode::Plan, PlanPhase::Planning),
            (AppMode::Yolo, PlanPhase::Executing),
            (AppMode::Yolo, PlanPhase::None),
        ] {
            let reminder = reminder_for(mode, phase).expect("large artifact reminder exists");
            assert!(
                reminder.contains("append_file` 只能追加到文件尾"),
                "reminder 必须锁住 append_file 尾追加语义: mode={mode:?} phase={phase:?}"
            );
            assert!(
                reminder.contains("用 `edit_file`,不要用 `append_file`"),
                "reminder 必须给中间填充/占位替换的正确工具(edit_file,apply_patch 已隐藏): mode={mode:?} phase={phase:?}"
            );
        }
    }

    /// 多引擎并发隔离基石(C 方案 P-no-disk 版): 两个不同 session 的 EngineConfig
    /// 必须 workspace 不同 + instructions 内容含各自 session_id(走 `InstructionSource::
    /// Inline`,内存对象天然隔离,不再依赖 disk 文件)。
    #[test]
    fn engine_config_for_session_paths_are_isolated() {
        let bridge = fixture_bridge();
        let (a, b) = ("sess-aaaa-1111", "sess-bbbb-2222");
        let cfg_a = bridge.build_engine_config_for_session(a);
        let cfg_b = bridge.build_engine_config_for_session(b);

        assert_ne!(
            cfg_a.workspace, cfg_b.workspace,
            "两 session 的 workspace 必须不同(否则产物冲突)"
        );
        assert!(cfg_a.workspace.to_string_lossy().contains(a));
        assert!(cfg_b.workspace.to_string_lossy().contains(b));

        // instructions 第一项是 session 专属 Inline source,name 含各自 session_id。
        let extract = |s: &InstructionSource| -> (String, String) {
            match s {
                InstructionSource::Inline { name, content } => (name.clone(), content.clone()),
                InstructionSource::File(p) => (p.display().to_string(), String::new()),
            }
        };
        let (name_a, content_a) = extract(&cfg_a.instructions[0]);
        let (name_b, content_b) = extract(&cfg_b.instructions[0]);
        assert!(
            matches!(cfg_a.instructions[0], InstructionSource::Inline { .. }),
            "session instructions 第一项必须是 Inline(C 方案 P-no-disk)"
        );
        assert_ne!(name_a, name_b, "两 session 的 inline name 必须不同");
        assert!(name_a.contains(a) && name_b.contains(b));
        // 渲染后的内容含各自 session-specific workspace 路径(占位符替换生效)。
        assert!(
            content_a.contains(a),
            "session A 的 inline content 必须含 session_id"
        );
        assert!(content_b.contains(b));
    }


    /// OpenaiCompatible preset 必须让用户提供的模型名生效，而不是回退到默认。
    #[test]
    fn openai_compatible_uses_user_provided_name() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::OpenaiCompatible);
        bridge.prefs.advanced.custom_model_name = Some("my-custom-model".to_string());
        assert_eq!(bridge.model(), "my-custom-model");
        assert_eq!(bridge.provider(), "openai");
    }

    /// OpenaiCompatible preset 必须透传任意模型名（如 gpt-4o）。
    #[test]
    fn openai_compatible_passthrough_model_name() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::OpenaiCompatible);
        bridge.prefs.advanced.custom_model_name = Some("gpt-4o".to_string());
        bridge.prefs.advanced.custom_base_url = Some("https://api.openai.com/v1".to_string());
        bridge.prefs.advanced.custom_api_key = Some("sk-xxx".to_string());
        assert_eq!(bridge.model(), "gpt-4o");
        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.base_url(), "https://api.openai.com/v1");
        assert_eq!(bridge.api_key(), "sk-xxx");
    }

    /// env 优先级始终高于 settings.json（兼容 run-dev.sh / harness）。
    #[test]
    fn env_always_overrides_settings() {
        let _env = EnvGuard::new(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::OpenaiCompatible);
        bridge.prefs.advanced.custom_model_name = Some("gpt-4o".to_string());
        std::env::set_var("DEEPSEEK_MODEL", "env-model");
        std::env::set_var("DEEPSEEK_PROVIDER", "env-provider");
        std::env::set_var("DEEPSEEK_BASE_URL", "http://env:8000/v1");
        std::env::set_var("DEEPSEEK_API_KEY", "env-key");
        assert_eq!(bridge.model(), "env-model");
        assert_eq!(bridge.provider(), "env-provider");
        assert_eq!(bridge.base_url(), "http://env:8000/v1");
        assert_eq!(bridge.api_key(), "env-key");
    }

    /// DtConfig 在 OpenaiCompatible 模式下不应强制 reasoning_effort=off。
    #[test]
    fn remote_provider_keeps_default_reasoning_effort() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::OpenaiCompatible);
        bridge.prefs.advanced.custom_model_name = Some("gpt-4o".to_string());
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.reasoning_effort, None);
    }

    /// Deepseek preset 应返回正确的默认 URL 和模型。
    #[test]
    fn deepseek_preset_defaults() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::Deepseek);
        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.model(), "deepseek-v4-pro");
        assert_eq!(bridge.base_url(), "https://api.deepseek.com");
    }

    /// 官方 DeepSeek API 只能接收裸模型名。若用户手动把 API 地址改成
    /// api.deepseek.com,bridge 必须把 provider 纠正为 deepseek,避免底座按 vLLM /
    /// sglang 形状把 deepseek-v4-flash 改写成 deepseek-ai/DeepSeek-V4-Flash。
    #[test]
    fn official_deepseek_base_url_forces_deepseek_provider() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::LocalVllm);
        bridge.prefs.advanced.custom_base_url = Some("https://api.deepseek.com/".to_string());
        bridge.prefs.advanced.custom_model_name = Some("DeepSeek-V4-Flash".to_string());
        bridge.prefs.advanced.custom_api_key = Some("sk-test".to_string());

        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.api_key(), "sk-test");
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.api_provider(), deepseek_tui::config::ApiProvider::Deepseek);
        assert_eq!(cfg.deepseek_base_url(), "https://api.deepseek.com");
        assert_eq!(cfg.default_model(), "deepseek-v4-flash");
        assert_eq!(cfg.reasoning_effort, None);
        assert_eq!(
            deepseek_tui::config::wire_model_for_provider(cfg.api_provider(), &bridge.model()),
            "deepseek-v4-flash"
        );
    }

    /// 即便环境变量残留 vLLM provider / provider-prefixed 模型,只要有效
    /// base_url 是官方 DeepSeek,bridge 就必须发官方 API 接受的 provider+模型名。
    #[test]
    fn official_deepseek_base_url_canonicalizes_env_mismatch() {
        let _env = EnvGuard::new(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::LocalVllm);
        bridge.prefs.advanced.custom_api_key = Some("sk-test".to_string());
        std::env::set_var("DEEPSEEK_PROVIDER", "vllm");
        std::env::set_var("DEEPSEEK_BASE_URL", "https://api.deepseek.com/");
        std::env::set_var("DEEPSEEK_MODEL", "deepseek-ai/DeepSeek-V4-Pro");

        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.model(), "deepseek-v4-pro");
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.api_provider(), deepseek_tui::config::ApiProvider::Deepseek);
        assert_eq!(cfg.default_model(), "deepseek-v4-pro");
        assert_eq!(cfg.reasoning_effort, None);
        assert_eq!(
            deepseek_tui::config::wire_model_for_provider(cfg.api_provider(), &bridge.model()),
            "deepseek-v4-pro"
        );
    }

    /// Qwen preset 应返回正确的默认 URL 和模型。
    #[test]
    fn qwen_preset_defaults() {
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::Qwen);
        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.model(), "qwen-max");
        assert_eq!(bridge.base_url(), "https://dashscope.aliyuncs.com/compatible-mode/v1");
    }

    /// DtConfig 在 LocalVllm 模式下必须保持 reasoning_effort=off（防 SSE timeout）。
    #[test]
    fn local_vllm_forces_reasoning_effort_off() {
        let bridge = fixture_bridge();
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("off"));
    }

}
