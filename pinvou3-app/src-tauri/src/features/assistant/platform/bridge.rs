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

pub use crate::core::mode_state;
pub use crate::features::marketplace;
pub use crate::features::marketplace::skill_marketplace;
pub(crate) use crate::features::runtime_bundle::platform as bundle;
pub use crate::features::sessions;
pub use crate::platform::paths;
pub use crate::platform::prefs;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use deepseek_tui::config::{
    wire_model_for_provider, ApiProvider, Config as DtConfig, ProvidersConfig,
};
use deepseek_tui::core::engine::EngineConfig;
use deepseek_tui::core::ops::Op;
use deepseek_tui::hooks::{Hook, HookEvent, HookExecutor, HooksConfig};
use deepseek_tui::prompts::InstructionSource;
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;

use self::bundle::{Pinvou3Bundle, INSTRUCTIONS_MD};
use self::prefs::{ModelPreset, SavedModel, UserPrefs};
use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};

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

fn base_url_uses_loopback(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            let host = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('.');
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
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
    /// 本 engine 绑定的 session 锁定模型(per-session 不同模型)。None = 用 prefs 全局
    /// active。EnginePool spawn 时按该 session 的 model_id 注入(见 `with_session_model`)。
    pub session_model: Option<SavedModel>,
    /// 本地 vLLM `/v1/models` 探测到的 `max_model_len`(上下文窗口)。EnginePool spawn
    /// 时由 `probe_vllm_model_info` 注入。Some → 与 SavedModel 声明取较小值后填入
    /// active_route_limits，并与 output profile 一起推导压缩阈值。
    pub probed_context_tokens: Option<u32>,
}

impl crate::features::memory::MemoryReviewModel for Pinvou3Bridge {
    fn memory_provider(&self) -> String {
        self.provider()
    }

    fn memory_model(&self) -> String {
        self.model()
    }

    fn memory_base_url(&self) -> String {
        self.base_url()
    }

    fn memory_api_key(&self) -> String {
        self.api_key()
    }

    fn memory_model_preset(&self) -> ModelPreset {
        self.effective_model_owned()
            .map(|model| model.preset)
            .unwrap_or_else(|| self.prefs.advanced.model_preset.unwrap_or_default())
    }
}

impl Pinvou3Bridge {
    /// 启动序列：确保 `~/.pinvou3/` 子目录存在 → 解包 bundle → 加载 prefs。
    /// 首次启动写一份默认 `settings.json` 让用户/开发者方便手改 advanced。
    ///
    /// **workspace 现为 `$HOME`**（阶段 C 调整）——让 AI 能用 read_file/glob
    /// 找到用户在桌面/文档/下载里的真实文件。配套敏感目录禁令在
    /// `bundle/instructions.md` 里引导，硬拦截后续走 deepseek-tui hook 注册。
    ///
    /// **$PINVOU3_SESSION_ARTIFACTS** 环境变量在这里 set 到公共 MCP 产物目录。
    /// PPT / 公文等 MCP server 是 stdio 子进程，不能可靠感知当前 GUI session；
    /// 因此二进制办公产物固定落到 `sessions/default/artifacts/`，具体归属由带
    /// `session_id` 的工具事件和前端持久化决定。
    pub fn boot() -> Result<Self> {
        // ⓪ 注入 pinvou3 版 prompt 文案到底座 prompt 合成层(base/locale/authority)。
        // 幂等(底座 OnceLock 首次生效、后续 Err 被忽略),必须早于任何 engine spawn。
        // 编译期内嵌常量,不依赖 bundle 解包。dump_system_prompt bin 也经此 boot,故
        // dump 同样生效。见 docs/base-prompt-override-阶段2.md。
        crate::platform::startup::mark("bridge_boot:prompt_overrides:start");
        bundle::install_prompt_overrides();
        crate::platform::startup::mark("bridge_boot:prompt_overrides:done");
        crate::platform::startup::mark("bridge_boot:ensure_dirs:start");
        paths::ensure_dirs()?;
        crate::platform::startup::mark("bridge_boot:ensure_dirs:done");
        let bundle = Pinvou3Bundle::paths();
        crate::platform::startup::mark("bridge_boot:bundle_extract:start");
        bundle.ensure_extracted()?;
        crate::platform::startup::mark("bridge_boot:bundle_extract:done");
        crate::platform::startup::mark("bridge_boot:mcp_secret_sync:start");
        if let Err(err) = marketplace::sync_mcp_secret_env_vars() {
            eprintln!("[pinvou3-app] MCP secret env sync skipped: {err}");
            crate::platform::startup::mark_with_detail(
                "rust",
                "bridge_boot:mcp_secret_sync:error",
                &err,
            );
        }
        crate::platform::startup::mark("bridge_boot:mcp_secret_sync:done");
        crate::platform::startup::mark("bridge_boot:prefs_load:start");
        let prefs = UserPrefs::load();
        crate::platform::startup::mark("bridge_boot:prefs_load:done");
        if !paths::settings_path().exists() {
            prefs.save().ok();
        }
        let artifacts = paths::default_session_artifacts_dir();
        std::env::set_var("PINVOU3_SESSION_ARTIFACTS", &artifacts);
        let this = Self {
            prefs,
            bundle,
            workspace: paths::user_home_dir(),
            session_model: None,
            probed_context_tokens: None,
        };
        this.wire_max_output_tokens_env();
        // C 方案(P-no-disk)最终版: 清理所有 pinvou3 历史 disk 残留:
        //   • `~/.pinvou3/sessions/<sid>/instructions.md`(per-session inline 前路径)
        //   • `~/.pinvou3/workspace_context.md`(workspace context 已合并进 INSTRUCTIONS_MD §0)
        //   • `~/.codewhale/instructions.md` / `~/.deepseek/instructions.md`(早期 P-brand 路径)
        // 不再生成任何 pinvou3-managed disk 文件 — 所有 prompt 内容走 Inline。
        crate::platform::startup::mark("bridge_boot:legacy_cleanup:start");
        this.cleanup_legacy_pinvou3_disk_files();
        crate::platform::startup::mark("bridge_boot:legacy_cleanup:done");
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
                if (is_auto_gen || is_pinvou3_managed) && std::fs::remove_file(&legacy).is_ok() {
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

    /// Render the session-scoped inline instructions. Workspace is deliberately
    /// absent from this static prompt and is supplied through per-turn metadata.
    pub fn build_session_system_prompt(&self, _session_id: &str) -> String {
        // [pinvou3] date/workspace 已移出静态 system → per-turn <turn_meta>:每 session
        // 变的 workspace 路径(及每天变的 date)若进 cached system prefix, vLLM prefix-cache
        // MISS 时工具调用会退化成裸文本(实测 single subagent 25%→稳态~100%)。仅保留 model
        // (固定值,不破坏 cache)与 sudo(静态文案兜底,实时状态走 super_permission::turn_reminder)。
        let mut rendered = INSTRUCTIONS_MD
            .replace("{{PINVOU3_MODEL}}", &self.model())
            .replace(
                "{{PINVOU3_SUDO_INSTRUCTION}}",
                crate::platform::super_permission::instruction_block(),
            )
            // present_artifact 的 title 语言随 locale(原写死「中文 title」会把英文 UI 的产物
            // 标题/描述/后续总结整段拽回中文,见 prefs::title_language_name 注释)。
            .replace(
                "{{PINVOU3_TITLE_LANG}}",
                self.prefs.language.title_language_name(),
            );
        // [pinvou3] 非中文 locale 的语言指令补丁:底座 locale_reinforcement_preamble
        // 对 en 返回 None,而 pinvou3 整份 system prompt 是中文,会把回复语言拽回中文。
        // 这里给底座留空的 locale 补一段 mirror 指令(zh-Hans/ja 已有底座 bookend,返回
        // None 不重复)。固定值(随 language 变,不随 session 变)→ 不破 prefix-cache。
        if let Some(block) = self.prefs.language.extra_language_directive() {
            rendered.push_str("\n\n");
            rendered.push_str(block);
        }
        rendered
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
        match crate::features::memory::ensure_runtime_prompt(session_id) {
            Ok(path) => out.push(InstructionSource::File(path)),
            Err(err) => eprintln!(
                "[pinvou3-app] memory runtime prompt unavailable for session {session_id}: {err}"
            ),
        }
        out
    }

    /// 当前 active provider 标识（传给底座 `DtConfig.provider`）。
    /// 本 engine/session 实际生效的模型记录:session 锁定优先,否则全局 active。
    /// load 后 prefs.active_model() 必非空,正常返回 Some。
    fn effective_model(&self) -> Option<&SavedModel> {
        self.session_model
            .as_ref()
            .or_else(|| self.prefs.active_model())
    }

    /// 当前生效模型的副本(session > active)。EnginePool 探测 vLLM served name 后
    /// 用它克隆→改 model 名→塞回 session_model,实现「请求用 vLLM 实际名字」。
    pub fn effective_model_owned(&self) -> Option<SavedModel> {
        self.effective_model().cloned()
    }

    /// 克隆一份 bridge 并绑定 per-session 模型(EnginePool spawn 时按 session model_id 注入)。
    pub fn with_session_model(&self, model: Option<SavedModel>) -> Self {
        let mut b = self.clone();
        b.session_model = model;
        b
    }

    pub fn provider(&self) -> String {
        if is_official_deepseek_base_url(&self.base_url()) {
            return "deepseek".to_string();
        }
        if let Ok(v) = std::env::var("DEEPSEEK_PROVIDER") {
            return v;
        }
        // active model(列表化后的真实来源)优先;无 active model 时回退 legacy
        // model_preset 字段——与 model()/base_url()/api_key() 的三段式兜底保持一致,
        // 避免 provider 说 vllm 而 base_url/model 已按 legacy preset 走的分叉。
        let preset = self
            .effective_model()
            .map(|m| m.preset)
            .unwrap_or_else(|| self.prefs.advanced.model_preset.unwrap_or_default());
        match preset {
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
        if let Some(m) = self.effective_model() {
            if is_official_deepseek {
                return official_deepseek_model_name(&m.model);
            }
            return m.model.clone();
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
            ModelPreset::Kimi => "kimi-k3".to_string(),
            ModelPreset::OpenaiCompatible => "gpt-5.6-terra".to_string(),
            ModelPreset::Qwen => "qwen3.7-plus".to_string(),
            ModelPreset::Doubao => "doubao-seed-evolving".to_string(),
            ModelPreset::Minimax => "MiniMax-M3".to_string(),
            ModelPreset::Glm => "glm-5.2".to_string(),
            ModelPreset::Mimo => "mimo-v2.5-pro".to_string(),
        }
    }

    /// 当前 active base_url（传给底座 `DtConfig.providers.*.base_url`）。
    /// 环境变量 > settings.custom_base_url > 厂商默认值。
    pub fn base_url(&self) -> String {
        if let Ok(v) = std::env::var("DEEPSEEK_BASE_URL") {
            return v;
        }
        if let Some(m) = self.effective_model() {
            return m.base_url.clone();
        }
        self.default_base_url_for_preset()
    }

    /// 是否要求用户配置 API Key。local_vllm 和明确指向本机 loopback 的
    /// OpenAI-compatible 服务允许无鉴权；云端/局域网地址默认仍要求 Key。
    pub fn api_key_required(&self) -> bool {
        self.provider() != "vllm" && !base_url_uses_loopback(&self.base_url())
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
            if !v.trim().is_empty() {
                return v;
            }
        }
        if let Some(m) = self.effective_model() {
            let store = SystemCredentialStore::new();
            if let Some(reference) = &m.credential_ref {
                match store.get(reference) {
                    Ok(Some(key)) if !key.trim().is_empty() => return key,
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!(
                            "[pinvou3-app] credential read failed for model {}: {}",
                            m.id,
                            err.user_message()
                        );
                    }
                }
            }
            // 本地 vLLM 不需鉴权:用户留空 key 兜底 local-no-auth(底座要求非空)。
            if m.preset == ModelPreset::LocalVllm && self.provider() == "vllm" {
                return LOCAL_VLLM_API_KEY.to_string();
            }
            return m.api_key.clone();
        }
        match (
            self.prefs.advanced.model_preset.unwrap_or_default(),
            self.provider().as_str(),
        ) {
            (ModelPreset::LocalVllm, "vllm") => LOCAL_VLLM_API_KEY.into(),
            _ => String::new(),
        }
    }

    /// Current search API key from env or encrypted credential store.
    pub fn search_api_key(&self) -> Option<String> {
        let provider = self.prefs.search.provider;
        for name in provider.env_key_names() {
            if let Ok(value) = std::env::var(name) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        if let Some(credential) = self.prefs.search.credentials.get(&provider) {
            if let Some(reference) = &credential.credential_ref {
                let store = SystemCredentialStore::new();
                match store.get(reference) {
                    Ok(Some(key)) if !key.trim().is_empty() => return Some(key),
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!(
                            "[pinvou3-app] search credential read failed for {}: {}",
                            provider.as_str(),
                            err.user_message()
                        );
                    }
                }
            }
        }
        self.prefs.search.normalized_api_key()
    }

    /// Resolve the shared Shell switch for ordinary and scheduled Yolo runs:
    /// env > prefs.advanced > default true.
    pub(crate) fn allow_shell_for_prefs(prefs: &UserPrefs) -> bool {
        if let Ok(v) = std::env::var("PINVOU3_ALLOW_SHELL") {
            return matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        prefs.advanced.allow_shell.unwrap_or(true)
    }

    pub fn allow_shell(&self) -> bool {
        Self::allow_shell_for_prefs(&self.prefs)
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

    /// 为一个具体 wire model 生成宿主已知的 route facts。正确性不依赖模型名：
    /// SavedModel 显式能力与实时 probe 取更小值；本地 vLLM 无显式 context 时才
    /// 使用模型 hint/128K 保守值。output 始终不超过 Pinvou 全局 24K 请求意图。
    fn route_limits_for_model(&self, model: &str) -> Option<codewhale_config::route::RouteLimits> {
        let saved = self.effective_model().filter(|saved| saved.model == model);
        let configured_context = saved.and_then(|saved| saved.context_window_tokens);
        let context_tokens = match (configured_context, self.probed_context_tokens) {
            (Some(configured), Some(probed)) => Some(configured.min(probed)),
            (Some(configured), None) => Some(configured),
            (None, Some(probed)) => Some(probed),
            (None, None) if self.provider() == "vllm" => {
                Some(deepseek_tui::models::context_window_for_model(model).unwrap_or(128_000))
            }
            (None, None) => None,
        };
        let configured_output = saved.and_then(|saved| saved.max_output_tokens);
        let output_tokens = configured_output
            .or_else(|| (self.provider() == "vllm").then(|| self.max_output_tokens()))
            .map(|tokens| tokens.min(self.max_output_tokens()))
            .map(|tokens| {
                context_tokens.map_or(tokens, |context| {
                    tokens.min(context.saturating_sub(1_024).max(1))
                })
            });
        let limits = codewhale_config::route::RouteLimits {
            context_tokens: context_tokens.map(u64::from),
            input_tokens: None,
            output_tokens: output_tokens.map(u64::from),
        };
        limits.has_known_limit().then_some(limits)
    }

    /// 底座 emergency 线用的 context window。SavedModel 声明与 probe(vLLM
    /// `/v1/models` 的 `max_model_len`)取较小值；都没有时才按模型名 hint/128K。
    ///
    /// ⚠️ **填 active_route_limits 与推导 token_threshold 必须共用这一个 window**,
    /// 否则 T(正常线)/E(紧急线)用不同窗口 → 倒置(见 docs/context-compaction-设计.md)。
    fn effective_context_window(&self, model: &str) -> u32 {
        self.route_limits_for_model(model)
            .and_then(|limits| limits.context_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok())
            .unwrap_or_else(|| {
                deepseek_tui::models::context_window_for_model(model).unwrap_or(128_000)
            })
    }

    /// 按窗口推导 `should_compact` 的 `token_threshold`(nice 主路径触发线 T)。
    /// 公式与常数见 docs/context-compaction-设计.md §3(2026-07-02 实测校准):
    ///
    ///   T = (E − S)/1.5 − FIXED,   clamp[4096, 0.75·W]
    ///   E = W − O − 1024           (底座 emergency 线,conservative 全量尺)
    ///   O 来自同一 route profile；未声明 route 时才由底座 provider/model fallback 推导
    ///
    /// ÷1.5 把 conservative 全量尺换算回 `should_compact` 的 raw 子集尺(k=1.5 实测精确,
    /// pinvou 关 thinking 无偏移)。S=4000(system 保守估算,dump 实测 ~1.4K + 余量);
    /// FIXED=22000(framing ~2.5K + pinned/recent R ~4.5K + safety margin ~15K)。
    /// 写死单值对任一窗口非倒置即过保守,故按窗口推导(实证:262K→~133K / 131K→~46K)。
    fn derive_compaction_threshold(&self, model: &str) -> usize {
        let window = self.effective_context_window(model) as usize;
        // [pinvou3-fork 根治 2026-07-03] 直接问底座要 emergency input budget E,**不再镜像**
        // 500K/262144/output 预留/`E=W−O−1024` 公式。那些常数一旦上游 sync 改动,pinvou3 编译
        // 不报错却静默算出不一致的 E → 倒置(与 tool_search 折叠单名同类:依赖上游不变的假设,
        // 间接检查抓不到)。底座 `context_input_budget_for_route` 已封装窗口分档(≥500K→262144 /
        // 否则 effective_max_output)+ headroom;传 input_tokens=0 取总 budget E。上游改这些
        // pinvou3 自动跟随、永不倒置。route_limits 与 build_engine_config.active_route_limits 同源。
        let route_limits = self.route_limits_for_model(model);
        let provider = self.build_dt_config().api_provider();
        let emergency = deepseek_tui::core::engine::context_input_budget_for_route(
            provider,
            model,
            route_limits,
            0,
        )
        .unwrap_or_else(|| {
            // 底座返 None(未知 model + 无探测 route_limits):与底座禁用 preflight 时同路,保守兜底。
            window
                .saturating_sub(
                    route_limits
                        .and_then(|limits| limits.output_tokens)
                        .and_then(|tokens| usize::try_from(tokens).ok())
                        .unwrap_or_else(|| self.max_output_tokens() as usize),
                )
                .saturating_sub(1_024)
        });
        // 以下 S/FIXED/÷1.5/clamp 是 pinvou3 自己的 T 推导(同尺换算 + 守护余量),**非镜像底座**。
        const S: usize = 4_000;
        const FIXED: usize = 22_000;
        // ÷1.5 == ×2/3:把 conservative 全量尺换算回 should_compact 的 raw 子集尺。
        let raw_equiv = emergency.saturating_sub(S).saturating_mul(2) / 3;
        let threshold = raw_equiv.saturating_sub(FIXED);
        // ⚠️ 上界 `.max(4_096)`:病态小窗口(W<5461 → W*3/4<4096)时裸 clamp 的 min>max 会触发
        // `Ord::clamp` 的 `assert!(min<=max)` panic → build_engine_config 崩。抬高上界使合法。
        threshold.clamp(4_096, (window * 3 / 4).max(4_096))
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
            // —— v0.8.51 上游新增字段 ——
            speech_output_dir,
            hook_executor: _, // pinvou3 注入敏感目录防火墙 + CLI 环境 hook
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
            launch_concurrency,
            goal_token_budget,
            goal_status,
            disallowed_tools: _, // pinvou3 从持久列表算初值(见构造处),默认值忽略
            // —— v0.8.65 上游新增字段,透传 default ——
            //   subagents_enabled: default true(三省六部走 SpawnSubAgent,必须开)。
            //   launch_concurrency/max_admitted_subagents/subagent_token_budget: subagent
            //   资源闸(决策③ fork 基底用步数上限,token_budget 透传 default 不启用)。
            //   auto_review_policy/exec_policy_engine: 审查/exec 策略。
            //   active_route_limits/skills_scan_codewhale_only/workspace_follow_symlinks: 透传。
            active_route_limits: _, // pinvou3 按 SavedModel + probe 显式构造，不透传 default
            skills_scan_codewhale_only,
            max_admitted_subagents,
            subagents_enabled,
            auto_review_policy,
            subagent_token_budget,
            workspace_follow_symlinks,
            exec_policy_engine,
            extra_tools,
            fleet_roster,
            moraine_fallback,
            terminal_chrome_enabled,
        } = EngineConfig::default();

        // hook 有两条消费路径：turn_loop 从 EngineConfig.hook_executor 跑
        // ToolCallBefore，exec_shell 则从 RuntimeToolServices.hook_executor 收集
        // shell_env。必须共享同一个实例，不能只填前者。
        let hook_executor = self.build_hook_executor();
        runtime_services.hook_executor = Some(hook_executor.clone());

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
            // 两条压缩触发(turn_loop 内顺序:先 should_compact,后 emergency),用**两把尺**:
            //  - should_compact(nice LLM 摘要,正常线 T):可摘要**子集**的 raw 尺 > T − pinned。
            //  - emergency(强制 recover_context_overflow,紧急线 E):**全量** input 的
            //    conservative 尺(raw×1.5 + system + framing) > W − O − 1024。
            //
            // ⚠️ 两把尺差 ×1.5 乘性,T 必须换算后仍显著低于 E,否则 emergency 抢先、nice 死
            //    (倒置 bug)。且 W 必须探测真实 max_model_len:写死单值(旧 190K)对任一窗口
            //    非倒置即过保守——2026-07-02 实证坐实 190K 在健康 256K 机也倒置(emergency@198
            //    早于 should_compact@255)。故 token_threshold 改**按窗口推导**:
            //    derive_compaction_threshold() = (E−S)/1.5 − 22000, clamp[4096, 0.75W];
            //    W/O 来源 = SavedModel 显式 route profile + probe；vLLM 缺省才走保守值。
            //    公式常数与实证见 docs/context-compaction-设计.md;同尺不变式由回归测试
            //    forkguard_compaction_threshold_below_emergency_all_windows 四窗口锁住。
            // 上游默认 token_threshold=800K,对本地窗口永远撞不到,**必须显式 set**。
            // ⚠️ v0.8.51 上游移除了 CompactionConfig.auto_floor_tokens 字段(floor 概念
            //    随 cycle removal 一并去掉),原 60K 下限设置失效,删除。
            compaction: deepseek_tui::compaction::CompactionConfig {
                model: self.model(),
                token_threshold: self.derive_compaction_threshold(&self.model()),
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
            search_api_key: self.search_api_key(),
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
            hook_executor: Some(hook_executor),
            // v0.8.53 上游新增,透传 default
            subagent_heartbeat_timeout,
            // v0.8.54-57 上游新增,透传 default(search_base_url=None / stream_chunk_timeout)
            search_base_url,
            stream_chunk_timeout,
            // v0.8.58-60 上游新增,透传 default(verbosity/fanout 闸/goal 管理/disallowed_tools)
            verbosity,
            launch_concurrency,
            goal_token_budget,
            goal_status,
            // pinvou3 工具开关:从全局持久的"被禁用连接器"算出禁用工具全名作为初值,
            // 让新对话/新窗口的引擎都继承用户的开关状态(持久语义)。
            disallowed_tools: {
                let n = crate::features::marketplace::disabled_tool_names();
                if n.is_empty() {
                    None
                } else {
                    Some(n)
                }
            },
            // [pinvou3-fork] 透传 default(空);kb_search 在 spawn_for_session 按 session 注入
            // —— v0.8.65 上游新增字段,透传 default ——
            //   subagents_enabled: default true(三省六部走 SpawnSubAgent,必须开)。
            //   launch_concurrency/max_admitted_subagents/subagent_token_budget: subagent
            //   资源闸(决策③ fork 基底用步数上限,token_budget 透传 default 不启用)。
            //   auto_review_policy/exec_policy_engine: 审查/exec 策略。
            //   active_route_limits/skills_scan_codewhale_only/workspace_follow_symlinks: 透传。
            // [pinvou3-fork] active_route_limits:把 SavedModel 声明和实时 probe 收敛成同一份
            // context/output route facts，让底座 emergency 线、Compact 与真实请求上限同尺。
            // 未登记的 vLLM 才回退 128K/24K；其他兼容引擎可在 SavedModel 显式声明。
            active_route_limits: self.route_limits_for_model(&self.model()),
            skills_scan_codewhale_only,
            max_admitted_subagents,
            subagents_enabled,
            auto_review_policy,
            subagent_token_budget,
            workspace_follow_symlinks,
            exec_policy_engine,
            extra_tools,
            fleet_roster,
            moraine_fallback,
            terminal_chrome_enabled,
        }
    }

    /// Build a session config with the ordinary per-session workspace.
    ///
    /// [`build_engine_config`]: Self::build_engine_config
    pub fn build_engine_config_for_session(&self, session_id: &str) -> EngineConfig {
        self.build_engine_config_for_session_at(session_id, self.session_workspace(session_id))
    }

    /// Build a session config at an explicit workspace. Scheduled conversations
    /// use this path so their transcripts stay independent while every run shares
    /// the ordinary global workspace, without creating a misleading empty
    /// `sessions/<id>/workspace` directory first.
    pub(crate) fn build_engine_config_for_session_at(
        &self,
        session_id: &str,
        workspace: PathBuf,
    ) -> EngineConfig {
        let mut cfg = self.build_engine_config();
        let _ = std::fs::create_dir_all(&workspace);
        cfg.workspace = workspace;
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
        let model = self.model();
        let providers = cfg.providers.get_or_insert_with(ProvidersConfig::default);
        // 按 provider 写对应 provider 配置的 base_url + api_key
        match provider.as_str() {
            "vllm" => {
                providers.vllm.base_url = Some(base_url);
                providers.vllm.api_key = Some(api_key);
                providers.vllm.model = Some(model.clone());
            }
            "openai" => {
                providers.openai.base_url = Some(base_url);
                providers.openai.api_key = Some(api_key);
                providers.openai.model = Some(model.clone());
            }
            "deepseek" => {
                providers.deepseek.base_url = Some(base_url);
                providers.deepseek.api_key = Some(api_key);
                providers.deepseek.model = Some(model.clone());
            }
            "moonshot" => {
                providers.moonshot.base_url = Some(base_url);
                providers.moonshot.api_key = Some(api_key);
                providers.moonshot.model = Some(model.clone());
            }
            _ => {
                providers.vllm.base_url = Some(base_url);
                providers.vllm.api_key = Some(api_key);
                providers.vllm.model = Some(model.clone());
            }
        }
        cfg.default_text_model = Some(model);
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
        #[cfg(windows)]
        let sensitive_command = {
            let script = self.bundle.deny_sensitive_ps1.to_string_lossy();
            format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{script}\"")
        };
        #[cfg(not(windows))]
        let sensitive_command = {
            let script = self
                .bundle
                .deny_sensitive_sh
                .to_string_lossy()
                .replace('\'', "'\\''");
            format!("bash '{script}'")
        };
        let hooks = vec![Hook {
            event: HookEvent::ToolCallBefore,
            command: sensitive_command,
            condition: None,
            timeout_secs: 5,
            background: false,
            continue_on_error: false,
            name: Some("pinvou3-sensitive-firewall".into()),
        }];

        // Linux/macOS 桌面安装通常不继承用户登录 shell 的 PATH/SDK 环境。
        // 复用底座现有 shell_env 扩展点，仅给 exec_shell 注入过滤后的终端环境；
        // MCP、RLM、JS、其他 hooks 仍保持各自原有环境策略，底座无需 fork patch。
        #[cfg(unix)]
        let hooks = {
            let mut hooks = hooks;
            let script = self
                .bundle
                .shell_env_sh
                .to_string_lossy()
                .replace('\'', "'\\''");
            hooks.push(Hook {
                event: HookEvent::ShellEnv,
                command: format!("bash '{script}'"),
                condition: None,
                timeout_secs: 5,
                background: false,
                continue_on_error: false,
                name: Some("pinvou3-cli-shell-env".into()),
            });
            hooks
        };

        HooksConfig {
            enabled: true,
            hooks,
            default_timeout_secs: Some(5),
            working_dir: None,
        }
    }

    fn build_hook_executor(&self) -> Arc<HookExecutor> {
        Arc::new(HookExecutor::new(
            self.build_hooks_config(),
            self.workspace.clone(),
        ))
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
    pub fn resolve_runtime_route_for_model(
        &self,
        model: &str,
    ) -> Result<deepseek_tui::route_runtime::ResolvedRuntimeRoute> {
        let config = self.build_dt_config();
        let provider = config.api_provider();
        let route = if let Some(limits) = self.route_limits_for_model(model) {
            deepseek_tui::route_runtime::resolve_runtime_route_with_limits(
                &config,
                provider,
                Some(model),
                limits,
            )
        } else {
            deepseek_tui::route_runtime::resolve_runtime_route(&config, provider, Some(model))
        };
        route.map_err(anyhow::Error::msg)
    }

    pub fn compaction_config_for_model(
        &self,
        model: &str,
    ) -> deepseek_tui::compaction::CompactionConfig {
        deepseek_tui::compaction::CompactionConfig {
            model: model.to_string(),
            token_threshold: self.derive_compaction_threshold(model),
            ..Default::default()
        }
    }

    pub fn build_send_message_op(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
    ) -> Result<Op> {
        let (allow_shell, trust_mode) = match mode {
            AppMode::Yolo => (self.allow_shell(), true),
            // Plan: allow_shell=true 让 engine 正常路由 shell 工具，
            // 底座 tool_setup.rs 会把 sandbox 切到 ReadOnly + 工具白名单切到只读集。
            // trust_mode=true 让 list_dir/read_file 等只读工具能跨 session workspace
            // 边界（pinvou3 是本地单用户工具，无跨用户安全边界，写保护靠 ReadOnly
            // sandbox + 只读工具集，不依赖 trust_mode）。
            AppMode::Plan => (true, true),
            // Agent mode pinvou3 不暴露，但保留 default 处理避免 panic
            AppMode::Agent | AppMode::Auto | AppMode::Operate => (self.allow_shell(), false),
        };
        // 超级权限状态每 turn 实时注入(is_enabled() 每次读 disk),绕开
        // refresh_all_instructions no-op 导致的"切开关不生效"——静态 prompt
        // spawn 时渲染一次就过时,这里每 turn 重出。
        // 但只对**能跑命令**的 mode 注入:Plan 是只读、无 exec_shell(底座只读工具集),
        // sudo 用不用对它毫无意义,注入纯浪费 ~110 字/turn。
        let sudo = crate::platform::super_permission::turn_reminder();
        let mut reminder_body = match reminder_for(mode) {
            // Plan: 无 exec,不带 sudo。
            Some(r) if matches!(mode, AppMode::Plan) => r.to_string(),
            // Yolo: reminder + sudo(能 exec,sudo 状态 load-bearing)。
            Some(r) => format!("{r}\n\n{sudo}"),
            // Agent(pinvou3 不暴露,reminder_for=None): 能 exec,带 sudo。
            None => sudo.to_string(),
        };
        // 卡片池: 该 session 加持了专家面具时,每 turn 注入 persona 人设(粘性身份)。
        if let Some(persona) = persona_reminder {
            reminder_body = format!("{reminder_body}\n\n{persona}");
        }
        let full_content =
            format!("<system-reminder>\n{reminder_body}\n</system-reminder>\n\n{content}");
        let model = self.model();
        Ok(Op::SendMessage {
            content: full_content,
            mode,
            route: Box::new(self.resolve_runtime_route_for_model(&model)?),
            compaction: Box::new(self.compaction_config_for_model(&model)),
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
            // v0.8.49 上游新增。Some(空表) = 本轮零工具:底座 filter_tool_catalog_for_gates
            // 直接从发给模型的 schema 里 retain 掉全部工具,模型根本看不到 write_file /
            // present_artifact 等。卡牌制造专家等"纯对话元卡"用它,从工具层杜绝小模型误走
            // 写文件路径、产出无法收藏的产物卡(不靠模型自觉遵守 prompt 硬规则)。None = 不
            // 限制,沿用 engine 全量工具表。判定源 = 每 turn 实时 active_persona(engine_pool
            // 解析后经 restrict_tools 传入),戴上即限 / 卸下即恢复,无持久状态。
            allowed_tools: if restrict_tools {
                Some(Vec::new())
            } else {
                None
            },
            // 底座会用这里的值覆盖 Engine 级 hook_executor；必须每轮显式携带，
            // 否则 ToolCallBefore 防火墙会在第一条消息时被 None 清掉。
            hook_executor: Some(self.build_hook_executor()),
            // v0.8.59 上游新增 concise verbosity 模式;pinvou3 GUI 走默认详尽,取 None。
            verbosity: None,
            // dynamic_tools: per-message 动态工具;pinvou3 不用,空。
            dynamic_tools: Vec::new(),
            // provenance: 消息来源。build_send_message_op 是用户内容 → ExternalUser。
            provenance: deepseek_tui::core::ops::UserInputProvenance::ExternalUser,
        })
    }
}

/// Plan 模式 per-turn reminder:命令式、短、列禁令(Qwen3.6 友好)。写保护真防线是底座
/// 只读工具集 + ReadOnly sandbox,禁写条只是减少弱模型撞墙的引导(消融证非 load-bearing)。
const PLAN_REMINDER: &str = "你现在在 Plan 模式(只读调研)。本 turn:\n\
     1. 想清楚后 → 调 `update_plan` 工具输出方案(explanation 字段写关键决策,\
     items 写 3-8 个执行步骤),可选再调 `checklist_write` 拆细。\n\
     2. **禁止**在 text 里描述方案/贴代码/写\"请点【就这么干】\"等按钮引导文字——\
     方案卡片由系统在你调 update_plan 后自动展示,你写引导是死锁。";

/// M1: per-turn `<system-reminder>` 文案,按当前 mode 选段(砍 PlanPhase 后只剩 mode 维度)。
/// 命中率优先于优雅:每段都是命令式、短、列禁令清单(Qwen3.6 友好)。
fn reminder_for(mode: AppMode) -> Option<&'static str> {
    match mode {
        AppMode::Plan => Some(PLAN_REMINDER),
        // Yolo: 大产物分块实测不再 load-bearing(landing.html 397 行一次 write_file、73.8s 不撞
        // timeout——idle timeout 早从 90s 提到 240-280s;且 h3c-ppt 等大-deck workflow 已下线,
        // 无依赖)→ 砍光 YOLO_REMINDER。Yolo per-turn 只剩 sudo(动态状态)。Agent pinvou3 不暴露。
        AppMode::Yolo | AppMode::Agent | AppMode::Auto | AppMode::Operate => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // env 写测试统一借用 bridge::paths::tests::ENV_LOCK(crate 级唯一 env 锁),
    // 避免本模块自建锁与其它模块的 PINVOU3_HOME/DEEPSEEK_* 写测试并发竞争
    // (曾经的 ENV_GUARD_LOCK 与 paths::ENV_LOCK 不通,导致 qwen_preset/vllm flaky,
    // 进而被迫全局 --test-threads=1)。
    //
    // EnvGuard **本身不持锁**(避免与外部 ENV_LOCK 获取重入死锁);调用方负责先拿锁。
    // 所有写 env 的本模块测试统一用 `locked_env` helper 一步到位(锁 + guard):
    //   let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
    // 切勿在已持 ENV_LOCK 时再调 locked_env(同一 Mutex 不可重入,会死锁)。
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

    /// 获取 crate 级 ENV_LOCK 并返回 (锁 guard, EnvGuard)。
    /// 供需要写 DEEPSEEK_* 等 env 的测试使用——锁保证与所有 env 写测试串行,
    /// EnvGuard 保证退出时恢复原值。切勿在已持 ENV_LOCK 时再调用(会重入死锁)。
    fn locked_env(
        vars: &[&'static str],
    ) -> (
        std::sync::MutexGuard<'static, ()>,
        EnvGuard,
    ) {
        let lock = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        (lock, EnvGuard::new(vars))
    }

    fn fixture_bridge() -> Pinvou3Bridge {
        Pinvou3Bridge {
            prefs: UserPrefs::default(),
            bundle: Pinvou3Bundle::paths(),
            workspace: std::env::temp_dir(),
            session_model: None,
            probed_context_tokens: None,
        }
    }

    #[test]
    fn sensitive_firewall_hook_uses_platform_script() {
        let bridge = fixture_bridge();
        let hooks = bridge.build_hooks_config();
        let command = &hooks.hooks[0].command;

        #[cfg(windows)]
        {
            assert!(
                command.contains("powershell.exe") && command.contains("deny_sensitive_paths.ps1"),
                "Windows sensitive firewall hook must use PowerShell, got: {command}"
            );
            assert!(
                !command.contains("bash"),
                "Windows sensitive firewall hook must not require bash, got: {command}"
            );
        }

        #[cfg(not(windows))]
        {
            assert!(
                command.starts_with("bash ") && command.contains("deny_sensitive_paths.sh"),
                "non-Windows sensitive firewall hook must use bash script, got: {command}"
            );
        }
    }

    #[test]
    fn engine_config_registers_sensitive_firewall_hook() {
        let bridge = fixture_bridge();
        let config = bridge.build_engine_config();
        let executor = config
            .hook_executor
            .as_ref()
            .expect("engine config must register pinvou3 sensitive firewall hook");
        let hooks = executor.config();
        assert!(hooks.enabled);
        assert!(hooks.hooks.iter().any(|hook| {
            hook.name.as_deref() == Some("pinvou3-sensitive-firewall")
                && hook.event == HookEvent::ToolCallBefore
        }));
        #[cfg(unix)]
        assert!(hooks.hooks.iter().any(|hook| {
            hook.name.as_deref() == Some("pinvou3-cli-shell-env")
                && hook.event == HookEvent::ShellEnv
        }));
    }

    fn set_active_model(
        bridge: &mut Pinvou3Bridge,
        preset: ModelPreset,
        model: &str,
        base_url: &str,
        api_key: &str,
    ) {
        bridge.prefs.advanced.saved_models = vec![SavedModel {
            id: "test-model".to_string(),
            name: model.to_string(),
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            model: model.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            credential_ref: None,
            credential_state: crate::platform::credential_store::CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        }];
        bridge.prefs.advanced.active_model_id = Some("test-model".to_string());
    }

    #[test]
    fn api_key_requirement_allows_only_vllm_or_loopback_without_key() {
        assert!(base_url_uses_loopback("http://localhost:8000/v1"));
        assert!(base_url_uses_loopback("http://localhost.:8000/v1"));
        assert!(base_url_uses_loopback("http://127.0.0.42:8000/v1"));
        assert!(base_url_uses_loopback("http://[::1]:8000/v1"));
        assert!(!base_url_uses_loopback("https://localhost.example.com/v1"));
        assert!(!base_url_uses_loopback("https://127.0.0.10.example.com/v1"));
        assert!(!base_url_uses_loopback("not a url"));

        let mut local_compatible = fixture_bridge();
        set_active_model(
            &mut local_compatible,
            ModelPreset::OpenaiCompatible,
            "custom-local-model",
            "http://127.0.0.1:9000/v1",
            "",
        );
        assert!(!local_compatible.api_key_required());

        let mut cloud_compatible = fixture_bridge();
        set_active_model(
            &mut cloud_compatible,
            ModelPreset::OpenaiCompatible,
            "custom-cloud-model",
            "https://gateway.example.com/v1",
            "",
        );
        assert!(cloud_compatible.api_key_required());
    }

    /// 128K 上下文的两种情况(客户翻车场景的正反面),端到端走 build_engine_config:
    ///  A. 真实 128K 部署——vLLM max_model_len=131072、探测成功 → 窗口正确、T 按 131072 缩。
    ///  B. 客户 bug 兜底——丢 `--served-model-name`(名字无 _Nk)+ 探测失败 → 底座 legacy
    ///     128000,T 按 128000 推导。两种都 nice 主路径活(T ≪ E),不再是写死 190K 的倒置
    ///     抖动(190K > 128K 窗口的 E,必倒置——正是客户机每 1-2 工具调用一次 Emergency 的根因)。
    #[test]
    fn forkguard_compaction_128k_scenarios() {
        let (_lock, _env) = locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
        // [根治后] derive_compaction_threshold 经底座 context_input_budget_for_route 读
        // DEEPSEEK_MAX_OUTPUT_TOKENS 算 output 预留(不再镜像 24576)。测试须钉死生产 env 值,
        // 否则底座默认 API_MAX_OUTPUT_TOKENS=65536 → E 偏小 → T 偏小(fixture 的 wire 用 is_none
        // 检查,env 被其它测试污染时不覆盖)。生产由 Bridge::boot → wire_max_output_tokens_env 保证。
        std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576");
        // A. 真实 128K 部署:探测拿到 131072
        // 默认预设已平台感知(macOS/Windows→Deepseek),显式设 LocalVllm 才测 128K vLLM compaction。
        let mut a = fixture_bridge();
        set_active_model(
            &mut a,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        a.probed_context_tokens = Some(131_072);
        let cfg_a = a.build_engine_config();
        let t_a = cfg_a.compaction.token_threshold;
        let e_a = 131_072usize - a.max_output_tokens() as usize - 1_024;
        eprintln!(
            "[A 真实128K部署] probed=131072 → T={t_a}  E={e_a}  route_limits={:?}",
            cfg_a.active_route_limits.and_then(|l| l.context_tokens)
        );
        assert_eq!(
            cfg_a.active_route_limits.and_then(|l| l.context_tokens),
            Some(131_072),
            "探测成功必须填 route_limits.context_tokens"
        );
        assert_eq!(
            cfg_a.active_route_limits.and_then(|l| l.output_tokens),
            Some(24_576),
            "128K 本地 route 必须显式携带 Pinvou 24K output"
        );
        assert!(
            (40_000..=55_000).contains(&t_a),
            "128K 窗口 T 应 ~46K,实得 {t_a}"
        );
        assert!(t_a < e_a, "T 必须低于 E(nice 先于 emergency)");

        // B. 客户 bug 兜底:名字无 _Nk 后缀 + 探测失败(vLLM 没起)
        let mut b = fixture_bridge();
        set_active_model(
            &mut b,
            ModelPreset::LocalVllm,
            "qwen3.6-35b",
            "http://x/v1",
            "",
        );
        b.probed_context_tokens = None;
        let win_b = b.effective_context_window(&b.model());
        let cfg_b = b.build_engine_config();
        let t_b = cfg_b.compaction.token_threshold;
        let e_b = win_b as usize - b.max_output_tokens() as usize - 1_024;
        eprintln!(
            "[B 客户bug兜底] name=qwen3.6-35b probed=None → window={win_b}  T={t_b}  E={e_b}  route_limits={:?}",
            cfg_b.active_route_limits.and_then(|l| l.context_tokens)
        );
        assert_eq!(
            win_b, 128_000,
            "无 _Nk 名字 + 探测失败 → 底座 legacy 128000(与 provider_capability generic 分支对齐)"
        );
        assert_eq!(
            cfg_b.active_route_limits,
            Some(codewhale_config::route::RouteLimits {
                context_tokens: Some(128_000),
                input_tokens: None,
                output_tokens: Some(24_576),
            }),
            "未知本地 alias 也必须携带明确的 128K/24K 保守 profile"
        );
        assert!(
            (38_000..=50_000).contains(&t_b),
            "128000 兜底 T 应 ~44K,实得 {t_b}"
        );
        assert!(
            t_b < e_b,
            "T({t_b}) 必须低于紧急线 E({e_b})——nice 先于 emergency(不倒置)"
        );

        // C. Pinvou 默认健康部署:SavedModel 明确 262144/24576，不依赖 wire alias。
        // 默认预设已平台感知,显式设 LocalVllm 后 migrate 才得到 262144/24576 profile。
        let mut c = fixture_bridge();
        c.prefs.advanced.model_preset = Some(ModelPreset::LocalVllm);
        c.prefs.migrate_models();
        let cfg_c = c.build_engine_config();
        let t_c = cfg_c.compaction.token_threshold;
        assert_eq!(
            cfg_c.active_route_limits,
            Some(codewhale_config::route::RouteLimits {
                context_tokens: Some(262_144),
                input_tokens: None,
                output_tokens: Some(24_576),
            })
        );
        assert_eq!(
            t_c, 133_029,
            "256K/24K profile 的 Compact 阈值应稳定为 133029"
        );
    }

    /// route profile 属于具体部署，不属于 vLLM/Qwen 特例。任何 OpenAI-compatible
    /// 本地引擎只要在 SavedModel 声明能力，都必须走同一预算链。
    #[test]
    fn forkguard_openai_compatible_route_uses_declared_limits() {
        let (_lock, _env) = locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "custom-local-model",
            "http://127.0.0.1:9000/v1",
            "",
        );
        let saved = bridge
            .prefs
            .advanced
            .saved_models
            .first_mut()
            .expect("active model");
        saved.context_window_tokens = Some(131_072);
        saved.max_output_tokens = Some(24_576);

        let config = bridge.build_engine_config();
        assert_eq!(
            config.active_route_limits,
            Some(codewhale_config::route::RouteLimits {
                context_tokens: Some(131_072),
                input_tokens: None,
                output_tokens: Some(24_576),
            })
        );
        assert_eq!(config.compaction.token_threshold, 45_648);
    }

    /// 实际会用的云端大窗口模型(不探测 → probed=None,window 走 catalog/名字 hint)。
    /// 断言 derive 的 T 换算 conservative 后 < 底座 E(不倒置)。deepseek-v4-pro(1M,≥500K)
    /// 走 output 预留分档(底座 TURN_MAX_OUTPUT=262144),锁住 2026-07-02 修的大窗口倒置。
    #[test]
    fn compaction_cloud_large_window_models() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        // T(raw 子集尺)换算回 emergency 的 conservative 全量尺(同 forkguard 常数)
        const K_NUM: usize = 3;
        const K_DEN: usize = 2;
        const R: usize = 4_500;
        const S: usize = 4_000;
        const FRAMING: usize = 2_500;
        // (模型名, 期望窗口, 底座该窗口的 output 预留)
        let cases = [
            ("deepseek-v4-pro", 1_000_000usize, 262_144usize), // ≥500K → 底座 TURN_MAX
            ("kimi-k2.6", 262_144, 24_576),                    // <500K → effective_max_output
            ("doubao-pro-256k", 256_000, 24_576),
        ];
        for (model, want_window, output) in cases {
            let mut b = fixture_bridge();
            set_active_model(&mut b, ModelPreset::Deepseek, model, "https://x/v1", "");
            b.probed_context_tokens = None; // 云端不探测
            let win = b.effective_context_window(&b.model()) as usize;
            assert_eq!(
                win, want_window,
                "{model} 窗口应 {want_window}(catalog/hint),实得 {win}"
            );
            let t = b.build_engine_config().compaction.token_threshold;
            let e = win - output - 1_024;
            let conservative = (t + R) * K_NUM / K_DEN + S + FRAMING;
            eprintln!("[云端 {model}] window={win} output={output} → T={t}  E={e}  conservative={conservative}");
            assert!(
                conservative <= e,
                "{model}: T={t} 换算 conservative={conservative} 必须 ≤ E={e}(不倒置)"
            );
        }
    }

    /// 默认模型名必须能被底座 `context_window_for_model` 识别出窗口,
    /// 否则 `context_input_budget` 静默返回 `None`,preflight + emergency
    /// recovery 全静默禁用 (codex adversarial-review 2026-05-19 抓到的
    /// 高优 finding)。后缀 `_256k` 由 fork B1 `_Nk` hint 解析。
    #[test]
    fn default_model_window_recognized_by_engine() {
        // 本测试钉死 LocalVllm 的 256K 窗口识别(默认预设已平台感知:macOS/Windows 默认
        // Deepseek),故显式设 LocalVllm preset 再断言其窗口派生。
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
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

    /// 超级权限状态对**能 exec 的 mode**(Yolo)必须每 turn 注入(切开关即时生效,
    /// refresh no-op);Plan 只读无 exec,sudo 无意义→不注入(省 ~110 字/turn)。
    #[test]
    fn build_send_message_op_injects_sudo_for_yolo_not_plan() {
        let bridge = fixture_bridge();
        let content_of = |mode| match bridge
            .build_send_message_op("用户消息".to_string(), mode, None, false)
            .expect("resolve test route")
        {
            Op::SendMessage { content, .. } => content,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        let yolo = content_of(AppMode::Yolo);
        assert!(
            yolo.contains("<system-reminder>") && yolo.contains("超级权限"),
            "Yolo 能 exec,必须每 turn 注入超级权限状态,得到:\n{yolo}"
        );
        let plan = content_of(AppMode::Plan);
        assert!(
            !plan.contains("超级权限"),
            "Plan 无 exec,不该注入 sudo reminder(纯浪费),得到:\n{plan}"
        );
    }

    /// 卡片池: 该 session 加持了专家面具时,persona reminder 必须进 per-turn
    /// `<system-reminder>`(粘性身份的核心机制)。None 时不注入(不破坏纯对话)。
    #[test]
    fn build_send_message_op_injects_persona_reminder_when_present() {
        let bridge = fixture_bridge();
        let persona = "你现在戴着【数据库架构师】专家面具。".to_string();
        let op = bridge
            .build_send_message_op(
                "用户消息".to_string(),
                AppMode::Yolo,
                Some(persona.clone()),
                false,
            )
            .expect("resolve test route");
        let content = match op {
            Op::SendMessage { content, .. } => content,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        assert!(
            content.contains("<system-reminder>") && content.contains(&persona),
            "加持后 op 必须在 system-reminder 内注入 persona 人设,得到:\n{content}"
        );
        // None 时不应出现该文案
        let op_none = bridge
            .build_send_message_op("hi".to_string(), AppMode::Yolo, None, false)
            .expect("resolve test route");
        if let Op::SendMessage { content, .. } = op_none {
            assert!(!content.contains("数据库架构师"), "未加持不应注入 persona");
        }
    }

    /// gating: 纯对话元卡(restrict_tools=true)→ 本轮 allowed_tools=Some(空表)=零工具;
    /// 普通卡 / 未加持(false)→ None=不限制。这是卡牌制造专家"只产可收藏的内联卡、绝不
    /// 写文件"的**工具层**强制手段(底座从 schema 删工具),不靠模型自觉遵守 prompt。
    #[test]
    fn build_send_message_op_restricts_tools_for_conversational_persona() {
        let bridge = fixture_bridge();
        let allowed = |restrict| match bridge
            .build_send_message_op("hi".to_string(), AppMode::Yolo, None, restrict)
            .expect("resolve test route")
        {
            Op::SendMessage { allowed_tools, .. } => allowed_tools,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        assert_eq!(
            allowed(true),
            Some(Vec::new()),
            "纯对话元卡本轮必须零工具(空白名单),把 write_file/present_artifact 挡在模型视野外"
        );
        assert_eq!(
            allowed(false),
            None,
            "普通卡 / 未加持不限制工具,沿用 engine 全量工具表"
        );
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
        let (_lock, _env) = locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
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
        // reasoning_effort=off 断言钉的是 LocalVllm 行为(默认预设已平台感知),显式设 LocalVllm。
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        let cfg = bridge.build_engine_config();
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
            cfg.subagent_api_timeout.as_secs(),
            300,
            "subagent_api_timeout 必须 300s。上游默认 120s 是为 DeepSeek 云端 API 设计, \
             本地 Qwen3.6 vLLM 慢推理下单 step 30-90s 很常见,120s 频繁误杀子 agent。 \
             300s 与 elapsed cap 对齐,给复杂研究类任务留出完整单步窗口。"
        );
    }

    /// 同尺守护:把推导出的 `token_threshold`(T,should_compact 的 raw 子集尺)换算回
    /// conservative 全量尺后,必须 ≤ emergency 线 E,否则 emergency 抢先、nice LLM 摘要
    /// 路径永远轮不到(倒置 bug)。四窗口参数化——2026-07-02 实证坐实写死 190K 在健康
    /// 256K 机也倒置(emergency@198 早于 should_compact@255),故按窗口推导。
    ///
    /// 换算用实测常数(docs/context-compaction-设计.md §6):k=1.5、pinned/recent R=4500、
    /// system S=4000、framing=2500。旧测试锁的 `threshold + 20K ≤ budget` 是**跨尺**假
    /// 不变式(T 用子集 raw、budget 用全量 conservative,差 ×1.5 乘性),已废弃。
    /// 谁改 derive_compaction_threshold 或 max_output_tokens 导致倒置都会被这条挡下。
    #[test]
    fn forkguard_compaction_threshold_below_emergency_all_windows() {
        let (_lock, _env) = locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
        std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576");
        std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS");
        // 把 T 从 should_compact 的 raw 子集尺 → emergency 的 conservative 全量尺
        const K_NUM: usize = 3; // ÷ K_DEN == ×1.5
        const K_DEN: usize = 2;
        const R: usize = 4_500; // pinned(近 4 条 + query)raw,实测 4,358,与会话长无关
        const S: usize = 4_000; // system 保守估算
        const FRAMING: usize = 2_500; // messages.len()×12+48,长会话量级

        // 正常窗口:同尺不变式必须成立(nice 先于 emergency)。emergency **直接对拍底座**
        // context_input_budget_for_route(与 derive 同源)——不再镜像 `window-output-1024`。上游改
        // output 预留/公式,本测试自动跟随、始终验**真跨仓一致**(根治的守护核心;不依赖 env 值)。
        for window in [262_144u32, 131_072, 65_536] {
            let mut bridge = fixture_bridge();
            bridge.probed_context_tokens = Some(window);
            let route_limits = bridge.route_limits_for_model(&bridge.model());
            let emergency = deepseek_tui::core::engine::context_input_budget_for_route(
                bridge.build_dt_config().api_provider(),
                &bridge.model(),
                route_limits,
                0,
            )
            .expect("探测窗口 → 底座必给出 budget");
            let t = bridge.build_engine_config().compaction.token_threshold;
            // T(raw 子集)换算回 conservative 全量:≈ k·(T + R) + S + framing
            let conservative_equiv = (t + R) * K_NUM / K_DEN + S + FRAMING;
            assert!(
                conservative_equiv <= emergency,
                "W={window}: T={t} 换算 conservative={conservative_equiv} 必须 ≤ 底座 emergency E={emergency},\
                 否则倒置(nice 死)。"
            );
        }

        // 病态小窗口(O > W):公式自然算成负值 → saturating + clamp 到 floor,
        // 只保证不 panic / 不归零(threshold=0 会退化成按消息数触发的压缩风暴)。
        let mut tiny = fixture_bridge();
        tiny.probed_context_tokens = Some(16_384);
        let t = tiny.build_engine_config().compaction.token_threshold;
        assert!(
            t >= 4_096,
            "病态小窗口 T 必须 clamp 到 floor ≥4096(防压缩风暴),实得 {t}"
        );

        // 极端小窗口(W<5461 → W*3/4<4096):clamp 上界 < floor,若不 `.max(4_096)`
        // 则 `Ord::clamp` 的 min>max 断言 panic(build_engine_config 崩、engine 起不来)。
        // LM Studio 默认 4096 / 小窗口 vLLM 探测即触发。断言不 panic 且 T 落 floor。
        for w in [4_096u32, 5_460, 8_192] {
            let mut b = fixture_bridge();
            b.probed_context_tokens = Some(w);
            let t = b.build_engine_config().compaction.token_threshold; // 不得 panic
            assert_eq!(t, 4_096, "极端小窗口 W={w} 应 clamp 到 floor 4096,实得 {t}");
        }
    }

    /// probed_context_tokens=Some → 必须填进 active_route_limits.context_tokens
    /// (底座 emergency 线 + footer 百分比据此按真实 max_model_len 计);None → 本地 vLLM
    /// 仍给 model hint/128K + 24K 保守 profile。下次 sync 若构造块改回透传 default，
    /// 本测试立刻报错。
    #[test]
    fn forkguard_probed_window_fills_route_limits() {
        // 本测试钉死本地 vLLM 的 route_limits 行为(默认预设已平台感知),两处 fixture 都显式设 LocalVllm。
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        bridge.probed_context_tokens = Some(262_144);
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.active_route_limits.and_then(|l| l.context_tokens),
            Some(262_144),
            "probed_context_tokens=Some 必须填进 active_route_limits.context_tokens"
        );

        let mut no_probe = fixture_bridge();
        set_active_model(
            &mut no_probe,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        let expected_context =
            deepseek_tui::models::context_window_for_model(&no_probe.model()).unwrap_or(128_000);
        let cfg_none = no_probe.build_engine_config();
        assert_eq!(
            cfg_none.active_route_limits,
            Some(codewhale_config::route::RouteLimits {
                context_tokens: Some(u64::from(expected_context)),
                input_tokens: None,
                output_tokens: Some(24_576),
            }),
            "未探测的本地 vLLM 应使用 model hint/128K + 24K 保守 profile"
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
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Bing
        );
        assert!(cfg.search_api_key.is_none());

        // 切 Metaso + 自定义 key
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Metaso,
            api_key: Some("mk-user-key".to_string()),
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Metaso
        );
        assert_eq!(cfg.search_api_key.as_deref(), Some("mk-user-key"));

        // 切 Metaso + 空白 key: bridge 层必须归一化成 None,让底座回退内置共享 key。
        // 若透传 Some(""),旧底座会收到 Metaso HTTP 200 + errCode=2005,
        // 并可能误显示成 No results found。
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Metaso,
            api_key: Some("   ".to_string()),
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Metaso
        );
        assert!(cfg.search_api_key.is_none());

        // 切 Bocha + 留空 key (UX 上前端应阻止,但 bridge 层透传 None)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Bocha,
            api_key: None,
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Bocha
        );
        assert!(cfg.search_api_key.is_none());

        // 切 Baidu + key (千帆 AI Search,key 必填)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Baidu,
            api_key: Some("bce-v3-user-key".to_string()),
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Baidu
        );
        assert_eq!(cfg.search_api_key.as_deref(), Some("bce-v3-user-key"));

        // 切 Baidu + 空白 key 同样归一化为 None,由底座报明确缺 key 错误。
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Baidu,
            api_key: Some("\n\t ".to_string()),
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Baidu
        );
        assert!(cfg.search_api_key.is_none());

        // 切 Tavily + key (海外 agent 搜索 API,tvly- key 必填)
        let mut bridge = fixture_bridge();
        bridge.prefs.search = prefs::SearchPrefs {
            provider: prefs::SearchProvider::Tavily,
            api_key: Some("tvly-user-key".to_string()),
            ..Default::default()
        };
        let cfg = bridge.build_engine_config();
        assert_eq!(
            cfg.search_provider,
            deepseek_tui::config::SearchProvider::Tavily
        );
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

    /// en locale 的 system prompt 必须带英文语言指令(底座 en→None,pinvou3 补)。
    /// zh-Hans 走底座 bookend,不在 inline instructions 里重复补。
    #[test]
    fn en_locale_injects_english_language_directive() {
        let mut bridge = fixture_bridge();
        let sid = "__test_lang__";

        bridge.prefs.language = prefs::Language::En;
        let en_prompt = bridge.build_session_system_prompt(sid);
        assert!(
            en_prompt.contains("## Language") && en_prompt.contains("Respond in English"),
            "en system prompt 缺英文语言指令:\n{en_prompt}"
        );

        bridge.prefs.language = prefs::Language::ZhHans;
        let zh_prompt = bridge.build_session_system_prompt(sid);
        assert!(
            !zh_prompt.contains("Respond in English"),
            "zh-Hans 不应在 inline instructions 重复注入英文指令(底座 bookend 已覆盖)"
        );
    }

    /// allow_shell 默认 true（pinvou3 yolo 模式需要）。
    #[test]
    fn allow_shell_defaults_to_true() {
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        assert!(fixture_bridge().allow_shell());
    }

    #[test]
    fn allow_shell_uses_advanced_preference_without_env_override() {
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.allow_shell = Some(false);
        assert!(!bridge.allow_shell());
    }

    /// env 优先级高于 prefs。
    #[test]
    fn allow_shell_env_overrides_prefs() {
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.allow_shell = Some(true);
        std::env::set_var("PINVOU3_ALLOW_SHELL", "false");
        assert!(!bridge.allow_shell());
    }

    #[test]
    fn hooks_include_cli_shell_env_without_replacing_sensitive_firewall() {
        let bridge = fixture_bridge();
        let config = bridge.build_engine_config();
        let engine_executor = config
            .hook_executor
            .as_ref()
            .expect("PINVOU Engine 必须注入 hook executor");
        let runtime_executor = config
            .runtime_services
            .hook_executor
            .as_ref()
            .expect("exec_shell runtime 必须注入 hook executor");
        assert!(
            Arc::ptr_eq(engine_executor, runtime_executor),
            "Engine hooks 与 exec_shell shell_env 必须共享同一 executor"
        );
        let hooks = engine_executor.config();
        assert!(
            hooks.hooks.iter().any(|hook| {
                hook.event == HookEvent::ToolCallBefore
                    && hook.name.as_deref() == Some("pinvou3-sensitive-firewall")
            }),
            "敏感目录硬拦截 hook 必须保留"
        );
        #[cfg(unix)]
        assert!(
            hooks.hooks.iter().any(|hook| {
                hook.event == HookEvent::ShellEnv
                    && hook.name.as_deref() == Some("pinvou3-cli-shell-env")
                    && hook.command.contains("shell_env.sh")
            }),
            "Unix PINVOU 必须通过底座现有 shell_env hook 注入 CLI 环境"
        );

        let Op::SendMessage {
            hook_executor: Some(message_executor),
            ..
        } = bridge
            .build_send_message_op("test".into(), AppMode::Yolo, None, false)
            .expect("resolve test route")
        else {
            panic!("每轮 SendMessage 必须显式携带 hook executor");
        };
        assert!(
            message_executor
                .config()
                .hooks
                .iter()
                .any(|hook| hook.event == HookEvent::ToolCallBefore),
            "每轮消息不得清掉敏感目录防火墙"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_shell_receives_filtered_shell_env_from_runtime_services() {
        use deepseek_tui::tools::shell::ExecShellTool;
        use deepseek_tui::tools::spec::ToolSpec;
        use deepseek_tui::tools::ToolContext;
        use serde_json::json;

        let (_lock, _env) = locked_env(&["SHELL", "XDG_RUNTIME_DIR", "OPENAI_API_KEY"]);
        std::env::set_var("SHELL", "/bin/bash");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/4242");
        std::env::set_var("OPENAI_API_KEY", "must-not-leak");

        let workspace =
            std::env::temp_dir().join(format!("pinvou3-shell-env-runtime-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let script = workspace.join("shell_env.sh");
        std::fs::write(&script, bundle::SHELL_ENV_SH).unwrap();

        let mut bridge = fixture_bridge();
        bridge.workspace.clone_from(&workspace);
        bridge.bundle.shell_env_sh = script;
        let config = bridge.build_engine_config();
        let context = ToolContext::new(&workspace).with_runtime_services(config.runtime_services);
        let result = ExecShellTool
            .execute(
                json!({
                    "command": "printf '%s|%s' \"${XDG_RUNTIME_DIR-unset}\" \"${OPENAI_API_KEY-unset}\""
                }),
                &context,
            )
            .await
            .expect("exec_shell 应执行成功");

        assert!(result.success, "exec_shell failed: {}", result.content);
        assert!(
            result.content.contains("/run/user/4242|unset"),
            "exec_shell 必须收到桌面运行时环境且不能泄露 API key: {}",
            result.content
        );
        assert!(!result.content.contains("must-not-leak"));
        let _ = std::fs::remove_dir_all(workspace);
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
        // remove_var 是无保护的 env 写,须持 crate 级 ENV_LOCK 与 allow_shell_* 组串行,
        // 否则去掉 --test-threads=1 后会与同组测试并发污染 PINVOU3_ALLOW_SHELL。
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op("hi".into(), AppMode::Yolo, None, false)
            .expect("resolve test route");
        let (_allow_shell, trust_mode) = extract_shell_trust(op);
        assert!(trust_mode, "Yolo 模式 trust_mode 必须 true");
    }

    /// L2-6: Plan 模式 → trust_mode=true（P1 修复回归，原本是 false 导致
    /// list_dir 跨 session workspace 边界报 PathEscape）。
    #[test]
    fn bridge_plan_mode_trust_mode_true_after_p1() {
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op("list dir".into(), AppMode::Plan, None, false)
            .expect("resolve test route");
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
        // 本测试既 remove_var 又经 allow_shell_for_prefs() 读 PINVOU3_ALLOW_SHELL 断言 true;
        // 不持锁时会与 allow_shell_env_overrides_prefs(临界区内 set "false")竞态 → 断言偶发失败。
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        std::env::remove_var("PINVOU3_ALLOW_SHELL");
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op("exec ls".into(), AppMode::Plan, None, false)
            .expect("resolve test route");
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
    fn yolo_has_no_mode_reminder_plan_reminder_has_no_write_content() {
        // 大产物分块实测不再 load-bearing(397 行一次写 73.8s 不撞 timeout)→ YOLO_REMINDER 砍光,
        // Yolo 生产主路径无 mode reminder(per-turn 只剩 sudo)。
        assert!(
            reminder_for(AppMode::Yolo).is_none(),
            "Yolo 不该再有 mode reminder(大产物分块已砍)"
        );
        // Plan 仍有 reminder,但只读不写,不含任何写文件/分块内容。
        let plan = reminder_for(AppMode::Plan).expect("plan reminder exists");
        assert!(
            !plan.contains("append_file") && !plan.contains("write_file"),
            "Plan reminder 不该含写文件/分块内容: {plan}"
        );
    }

    /// 多引擎并发隔离基石(C 方案 P-no-disk 版): 两个不同 session 的 EngineConfig
    /// 必须 workspace 不同 + instructions 的 inline name 含各自 session_id(走
    /// `InstructionSource::Inline`,内存对象天然隔离,不再依赖 disk 文件)。
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
        let name_of = |s: &InstructionSource| -> String {
            match s {
                InstructionSource::Inline { name, .. } => name.clone(),
                InstructionSource::File(p) => p.display().to_string(),
            }
        };
        let name_a = name_of(&cfg_a.instructions[0]);
        let name_b = name_of(&cfg_b.instructions[0]);
        assert!(
            matches!(cfg_a.instructions[0], InstructionSource::Inline { .. }),
            "session instructions 第一项必须是 Inline(C 方案 P-no-disk)"
        );
        assert_ne!(name_a, name_b, "两 session 的 inline name 必须不同");
        assert!(name_a.contains(a) && name_b.contains(b));
        // session_id / workspace 已移出静态 content,走 per-turn <turn_meta>
        // (见 build_session_system_prompt 注释:per-session 变动进 cache 前缀会
        // 触发 vLLM prefix-cache MISS → 工具调用漂移)。故 content 不再含 session_id,
        // 隔离已由上面"workspace 目录不同 + inline name 含 session_id"覆盖。
    }

    #[test]
    fn engine_config_for_session_keeps_mcp_artifacts_public() {
        // locked_env 一步获取 crate 级 ENV_LOCK + EnvGuard(保护 PINVOU3_HOME 写并恢复)。
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_SESSION_ARTIFACTS"]);
        let root = std::env::temp_dir().join(format!(
            "pinvou3-mcp-artifacts-public-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);

        let public_artifacts = paths::default_session_artifacts_dir();
        std::env::set_var("PINVOU3_SESSION_ARTIFACTS", &public_artifacts);

        let bridge = fixture_bridge();
        let a = "sess-artifacts-a";
        let b = "sess-artifacts-b";
        let _cfg_a = bridge.build_engine_config_for_session(a);
        let _cfg_b = bridge.build_engine_config_for_session(b);

        let actual = std::env::var("PINVOU3_SESSION_ARTIFACTS")
            .expect("PINVOU3_SESSION_ARTIFACTS should remain set");
        assert_eq!(
            actual,
            public_artifacts.to_string_lossy(),
            "MCP stdio server 共享进程不能拿 session 专属 artifacts；env 必须保持公共落点"
        );
        assert_ne!(public_artifacts, paths::session_artifacts_dir(a));
        assert_ne!(public_artifacts, paths::session_artifacts_dir(b));

        let _ = std::fs::remove_dir_all(root);
    }

    /// OpenaiCompatible preset 必须让用户提供的模型名生效，而不是回退到默认。
    /// 9e296c4 模型列表化后,legacy preset+custom_* 经 `migrate_models()`(每次
    /// `UserPrefs::load()` 都跑)物化成 active SavedModel 才生效——测试显式调一次模拟之。
    #[test]
    fn openai_compatible_uses_user_provided_name() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "my-custom-model",
            "https://api.openai.com/v1",
            "",
        );
        assert_eq!(bridge.model(), "my-custom-model");
        assert_eq!(bridge.provider(), "openai");
    }

    /// OpenaiCompatible preset 必须透传任意模型名（如自定义兼容端点模型）。
    #[test]
    fn openai_compatible_passthrough_model_name() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "custom-openai-model",
            "https://api.openai.com/v1",
            "sk-xxx",
        );
        assert_eq!(bridge.model(), "custom-openai-model");
        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.base_url(), "https://api.openai.com/v1");
        assert_eq!(bridge.api_key(), "sk-xxx");
    }

    /// env 优先级始终高于 settings.json（兼容 run-dev.sh / harness）。
    #[test]
    fn env_always_overrides_settings() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        bridge.prefs.advanced.model_preset = Some(ModelPreset::OpenaiCompatible);
        bridge.prefs.advanced.custom_model_name = Some("custom-openai-model".to_string());
        std::env::set_var("DEEPSEEK_MODEL", "env-model");
        std::env::set_var("DEEPSEEK_PROVIDER", "env-provider");
        std::env::set_var("DEEPSEEK_BASE_URL", "http://env:8000/v1");
        std::env::set_var("DEEPSEEK_API_KEY", "env-key");
        assert_eq!(bridge.model(), "env-model");
        assert_eq!(bridge.provider(), "env-provider");
        assert_eq!(bridge.base_url(), "http://env:8000/v1");
        assert_eq!(bridge.api_key(), "env-key");
    }

    #[test]
    fn empty_api_key_env_does_not_hide_saved_credential() {
        let (_lock, _env) = locked_env(&["DEEPSEEK_API_KEY"]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "custom-openai-model",
            "https://api.openai.com/v1",
            "saved-key",
        );
        std::env::set_var("DEEPSEEK_API_KEY", "  ");
        assert_eq!(bridge.api_key(), "saved-key");
    }

    /// DtConfig 在 OpenaiCompatible 模式下不应强制 reasoning_effort=off。
    #[test]
    fn remote_provider_keeps_default_reasoning_effort() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "custom-openai-model",
            "https://api.openai.com/v1",
            "",
        );
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.reasoning_effort, None);
    }

    /// Deepseek preset 应返回正确的默认 URL 和模型。
    #[test]
    fn deepseek_preset_defaults() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
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
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            "deepseek-v4-pro",
            "https://api.deepseek.com/",
            "sk-test",
        );

        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.api_key(), "sk-test");
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.api_provider(),
            deepseek_tui::config::ApiProvider::Deepseek
        );
        assert_eq!(cfg.deepseek_base_url(), "https://api.deepseek.com");
        assert_eq!(cfg.default_model(), "deepseek-v4-pro");
        assert_eq!(cfg.reasoning_effort, None);
        assert_eq!(
            deepseek_tui::config::wire_model_for_provider(cfg.api_provider(), &bridge.model()),
            "deepseek-v4-pro"
        );
    }

    /// 即便环境变量残留 vLLM provider / provider-prefixed 模型,只要有效
    /// base_url 是官方 DeepSeek,bridge 就必须发官方 API 接受的 provider+模型名。
    #[test]
    fn official_deepseek_base_url_canonicalizes_env_mismatch() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            "qwen36_35b_256k",
            "http://127.0.0.1:8000/v1",
            "",
        );
        std::env::set_var("DEEPSEEK_PROVIDER", "vllm");
        std::env::set_var("DEEPSEEK_BASE_URL", "https://api.deepseek.com/");
        std::env::set_var("DEEPSEEK_MODEL", "deepseek-ai/DeepSeek-V4-Pro");

        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.model(), "deepseek-v4-pro");
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.api_provider(),
            deepseek_tui::config::ApiProvider::Deepseek
        );
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
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Qwen,
            ModelPreset::Qwen.default_model(),
            ModelPreset::Qwen.default_base_url(),
            "",
        );
        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.model(), "qwen3.7-plus");
        assert_eq!(
            bridge.base_url(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
    }

    /// DtConfig 在 LocalVllm 模式下必须保持 reasoning_effort=off（防 SSE timeout）。
    #[test]
    fn local_vllm_forces_reasoning_effort_off() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        // 默认预设已平台感知(macOS/Windows→Deepseek),显式设 LocalVllm 才测其 reasoning_effort=off。
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("off"));
    }
}
