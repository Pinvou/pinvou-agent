//! pinvou3-app 与 CodeWhale 之间的抽象层（"bridge"）。
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

pub use crate::features::marketplace;
pub use crate::features::marketplace::skill_marketplace;
pub(crate) use crate::features::runtime_bundle::platform as bundle;
pub use crate::features::sessions;
pub use crate::platform::paths;
pub use crate::platform::prefs;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use deepseek_tui::config::{
    ApiProvider, Config as DtConfig, ProviderConfig, ProvidersConfig, wire_model_for_provider,
};
use deepseek_tui::core::engine::EngineConfig;
use deepseek_tui::core::ops::Op;
use deepseek_tui::hooks::{Hook, HookCondition, HookEvent, HookExecutor, HooksConfig};
use deepseek_tui::prompts::InstructionSource;
use deepseek_tui::tui::app::AppMode;

use self::bundle::{Pinvou3Bundle, instructions_code_md, instructions_md};
use self::prefs::{ModelPreset, SavedModel, UserPrefs};
use crate::core::session_mode::SessionMode;
use crate::features::assistant::expert_roster::ExpertRosterSnapshot;
use crate::features::assistant::image_capability::{
    EffectiveImageCapability, effective_image_capability,
};
use crate::features::assistant::runtime_model::RuntimeModelCredential;
use crate::features::assistant::session_policy::SessionPolicy;
use crate::platform::credential_store::{CredentialStore, SystemCredentialStore};

/// Process-wide shared credential-store handle. `SystemCredentialStore`'s
/// backend caches (one `Secrets` per keyring service) live on the instance;
/// a per-call new() throws the probe/service resolution away and repeats it
/// several times per turn.
fn shared_credential_store() -> &'static SystemCredentialStore {
    static STORE: std::sync::OnceLock<SystemCredentialStore> = std::sync::OnceLock::new();
    STORE.get_or_init(SystemCredentialStore::new)
}

// Qwen3.6 在 vLLM 里是 passthrough 字符串（不走 alias）;`_256k` 后缀语义与
// ops 同步要求见 `ModelPreset::default_model` 的 LocalVllm 注释（prefs/model.rs）。
const LOCAL_VLLM_API_KEY: &str = "local-no-auth";
const SEPARATE_REASONING_FIELD: &str = "separate_field";

// Multi-agent is an agent cluster where the main session stays the overall
// coordinator and complex tasks nest at most one extra level — not an unbounded
// recursive tree. Plain conversations keep the original CodeWhale caps; only
// sessions with multi-agent enabled get a tighter resource budget. The tier
// constants are pub(crate) so the per-turn delegation reminder
// (app/commands/multiagent.rs) reads the same numbers and cannot drift from the
// engine's actual caps.
const MULTI_AGENT_MAX_SPAWN_DEPTH: u32 = 2;
pub(crate) const MULTI_AGENT_WORK_MAX_CONCURRENT: usize = 4;
pub(crate) const MULTI_AGENT_WORK_MAX_ADMITTED: usize = 8;
pub(crate) const MULTI_AGENT_CODE_MAX_CONCURRENT: usize = 6;
pub(crate) const MULTI_AGENT_CODE_MAX_ADMITTED: usize = 12;

fn configure_provider(
    config: &mut ProviderConfig,
    base_url: &str,
    api_key: &str,
    model: &str,
    reasoning_stream_style: Option<&str>,
) {
    config.base_url = Some(base_url.to_string());
    config.api_key = Some(api_key.to_string());
    config.model = Some(model.to_string());
    config.reasoning_stream_style = reasoning_stream_style.map(str::to_string);
}

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

/// 原生代码会话的执行根解析器与「两个根」类型统一由
/// [`crate::features::sessions`] 定义(SessionStore 与 bridge 共用同一实现),
/// 此处 re-export 保持既有调用路径不变。
pub use crate::features::sessions::{ExecutionRootResolver, SessionRoots};

#[derive(Clone)]
pub struct Pinvou3Bridge {
    pub prefs: UserPrefs,
    pub bundle: Pinvou3Bundle,
    pub workspace: PathBuf,
    /// 本 engine 绑定的 session 锁定模型(per-session 不同模型)。None = 用 prefs 全局
    /// active。EnginePool spawn 时按该 session 的 model_id 注入。
    pub session_model: Option<SavedModel>,
    /// RuntimeModelProvider 为本次引擎准备的内存凭据。Some 时是最终值，不能再被
    /// 环境变量或本地凭据库覆盖；Debug 由包装类型强制脱敏。
    pub runtime_model_credential: Option<RuntimeModelCredential>,
    /// 本地 vLLM `/v1/models` 探测到的 `max_model_len`(上下文窗口)。EnginePool spawn
    /// 时由 `probe_vllm_model_info` 注入。Some → 与 SavedModel 声明取较小值后填入
    /// active_route_limits，并与 output profile 一起推导压缩阈值。
    pub probed_context_tokens: Option<u32>,
    /// 原生代码会话的执行根（engine cwd / shell 目录）解析器；None = 无代码会话
    /// 项目绑定，所有会话都用会话私有目录。账本根（附件/审计/产物）不受其影响，
    /// 仍由 `SessionStore::session_roots` 的 `ledger` 字段统一决定。
    pub execution_root_resolver: Option<ExecutionRootResolver>,
    /// 原生代码会话判定（code_session=true，含临时与绑项目两种）。用于
    /// instructions 的 work/code 分支渲染与工具整形；lib.rs 与执行根解析器
    /// 共用 AcpPool 那份 SessionAgentStore 注入。
    pub code_session_predicate: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    /// 外部 ACP 会话判定。产品多智能体只由 Pinvou 原生 Engine 承载；该谓词
    /// 与 `code_session_predicate` 一起由同一份 AcpPool 注入，防止外部 ACP 的
    /// plain 产品模式被误当成 Work 并通过直调 IPC 开启产品开关。
    pub external_acp_session_predicate: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    /// scheduled 会话标记:EnginePool spawn 时按 scheduled_profile 注入。此类
    /// 会话的图片不走路由(命令层固定 VisionToolFallback + image_analyze 硬规则),
    /// 因此即使主模型能力 Unknown 也要注册 `image_analyze`(恢复 main 行为),
    /// 否则 prompt 硬性要求模型调用一个未注册的工具。仅影响
    /// `resolve_vision_model_config` 的规则 3 回退,不影响交互会话路由。
    pub image_analyze_always: bool,
}

impl std::fmt::Debug for Pinvou3Bridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pinvou3Bridge")
            .field("prefs", &self.prefs)
            .field("bundle", &self.bundle)
            .field("workspace", &self.workspace)
            .field("session_model", &self.session_model)
            .field("runtime_model_credential", &self.runtime_model_credential)
            .field("probed_context_tokens", &self.probed_context_tokens)
            .field(
                "execution_root_resolver",
                &self.execution_root_resolver.as_ref().map(|_| "Some(..)"),
            )
            .field(
                "code_session_predicate",
                &self.code_session_predicate.as_ref().map(|_| "Some(..)"),
            )
            .field(
                "external_acp_session_predicate",
                &self
                    .external_acp_session_predicate
                    .as_ref()
                    .map(|_| "Some(..)"),
            )
            .finish()
    }
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

    fn memory_locale_tag(&self) -> String {
        self.locale_tag().to_string()
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
    /// The session artifacts dir `PINVOU3_SESSION_ARTIFACTS` is NOT injected here:
    /// boot runs in the multi-threaded phase, and process env writes are
    /// funneled to lib.rs `startup_process_env` (the single-threaded startup
    /// window).
    ///
    /// ⚠️ 不得在这里（或 boot 其它位置）注入 `DEEPSEEK_MAX_OUTPUT_TOKENS` /
    /// `PINVOU3_MAX_OUTPUT_TOKENS`：底座 `effective_max_output_tokens()` 优先读
    /// 前者，一旦回归会把所有模型（含云端）输出上限重新钉死 24576——正是本 PR
    /// 移除的根因。lib.rs `release_env_defaults_guard` 守 run() 的 release env 注入，
    /// 本函数 + `forkguard_boot_env_must_not_pin_global_output_cap` 守 boot 注入源头。
    ///
    /// ⚠️ boot runs in the Tauri setup phase (the multi-threaded runtime is
    /// already up), so it must NOT write the process env (edition 2024: a
    /// runtime write is a data race against uncoordinated concurrent readers).
    /// The `PINVOU3_SESSION_ARTIFACTS` injection (the artifacts landing dir for
    /// MCP stdio subprocesses such as PPT / official-document tools, fixed to
    /// `sessions/default/artifacts/`) has been moved up to lib.rs
    /// `startup_process_env` (the single-threaded window of the run()/headless
    /// startup sequence).
    pub fn boot() -> Result<Self> {
        // ⓪ 注入 pinvou3 版 prompt 文案到底座 prompt 合成层(base/locale/authority)。
        // 幂等(底座 OnceLock 首次生效、后续 Err 被忽略),必须早于任何 engine spawn。
        // 编译期内嵌常量,不依赖 bundle 解包。dump_system_prompt bin 也经此 boot,故
        // dump 同样生效。
        crate::platform::startup::mark("bridge_boot:prompt_overrides:start");
        bundle::install_prompt_overrides();
        // Register the foundation's MCP secret resolver (secrets live in the
        // keyring + an in-process registry, no process env writes); like the
        // prompt overrides it must run before any engine spawn.
        marketplace::install_mcp_secret_resolver();
        crate::platform::startup::mark("bridge_boot:prompt_overrides:done");
        crate::platform::startup::mark("bridge_boot:ensure_dirs:start");
        paths::ensure_dirs()?;
        crate::platform::startup::mark("bridge_boot:ensure_dirs:done");
        let bundle = Pinvou3Bundle::paths();
        crate::platform::startup::mark("bridge_boot:bundle_extract:start");
        bundle.ensure_extracted()?;
        crate::platform::startup::mark("bridge_boot:bundle_extract:done");
        // 旧布局 CLI 二进制（connectors/<platform>/bin/）→ 版本化资产库的一次性
        // 迁移（marketplace-unification §9.3）：验过 SHA-256 才移动，不匹配的不动
        // （store 侧 degraded 语义，重连重下）。放 app 侧 boot 而非 runtime_bundle
        // 内部：connectors → runtime_bundle 依赖已存在，反向调用会成环（架构守卫
        // rust_feature_cycles 基线为空）。幂等、内部不返回错误，不阻塞启动。
        crate::features::connectors::native_installer::migrate_legacy_cli_binaries();
        crate::platform::startup::mark("bridge_boot:mcp_secret_sync:start");
        if let Err(err) = marketplace::sync_mcp_secret_values() {
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
        let this = Self {
            prefs,
            bundle,
            workspace: paths::user_home_dir(),
            session_model: None,
            runtime_model_credential: None,
            probed_context_tokens: None,
            execution_root_resolver: None,
            code_session_predicate: None,
            external_acp_session_predicate: None,
            image_analyze_always: false,
        };
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

    /// 测试入口(L1 harness 用):同 [`boot`] 但 workspace 用传入的 `ws`
    /// (通常是 scenario 自己的 tempdir),而不是 `paths::user_home_dir()`。
    /// 让 L1 真 vLLM dialog harness 能给每个 scenario 一个隔离的产出目录,
    /// 避免污染用户 $HOME 也避免 scenario 之间互相干扰。
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
    pub fn build_session_system_prompt(&self, session_id: &str) -> String {
        // [pinvou3] date/workspace 已移出静态 system → per-turn <turn_meta>:每 session
        // 变的 workspace 路径(及每天变的 date)若进 cached system prefix, vLLM prefix-cache
        // MISS 时工具调用会退化成裸文本(实测 single subagent 25%→稳态~100%)。仅保留 model
        // (固定值,不破坏 cache)与 sudo(静态文案兜底,实时状态走 super_permission::turn_reminder)。
        // 分层 instructions:原生代码会话 = 共享骨架 + 代码层(编码执行循环 + 代码场景纪律,
        // 无产出物/成品卡语义);其余会话 = 共享骨架 + work 层(与历史 instructions 逐字节相等)。
        let uses_code_instructions = self.session_policy(session_id).uses_code_instructions();
        let base = if uses_code_instructions {
            let workspace_hint = self
                .execution_root_resolver
                .as_ref()
                .and_then(|resolver| resolver(session_id))
                .map(|root| {
                    format!(
                        "你正在用户的项目目录 `{}` 中工作,相对路径即相对项目根;",
                        root.display()
                    )
                })
                .unwrap_or_else(|| {
                    "你在本会话专属工作目录中工作,相对路径即相对该目录;".to_string()
                });
            instructions_code_md(&workspace_hint)
        } else {
            instructions_md().to_string()
        };
        let mut rendered = base
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
        // When browser capabilities are statically unavailable, inject a model-readable
        // reason and recovery guidance. The Browser capabilities section already explains
        // the general missing-tool fallback; this block supplies the precise reason. Add it
        // only to Work-mode sessions and only while unavailable, preserving the byte-exact
        // system prompt (and prefix cache) on the healthy path. The bridge semantic gate,
        // including exclusion of external ACP runtimes, is shared with tool registration in
        // `build_engine_config_for_session`.
        if self.exposes_browser_mcp(session_id) {
            if let Some(hint) = self.bundle.browser_unavailability_reason() {
                rendered.push_str("\n\n## Browser capabilities unavailable\n");
                rendered.push_str(&hint);
            }
        }
        rendered
    }

    /// 统一解析一个会话的两个根（执行根 + 账本根）。调用方按用途显式选择
    /// [`SessionRoots::execution`] 或 [`SessionRoots::ledger`]，避免把执行根误当
    /// 账本根写盘（或反之）。
    ///
    /// - `execution`：原生代码会话绑定了项目目录时返回项目目录（engine cwd 与
    ///   shell 执行目录由此同源），其余会话返回会话私有目录。
    /// - `ledger`：绑定了项目目录的原生代码会话恒为会话私有目录（附件/审计/产物
    ///   不污染用户项目）；其余会话与 execution 相同。
    ///
    /// 本入口不感知 scheduled 会话（bridge 拿不到 SessionStore）；scheduled 的
    /// 两个根由调用方经 [`crate::features::sessions::SessionStore::session_roots`]
    /// 解析。与 SessionStore 入口共用 [`sessions::session_roots_for`] 同一实现。
    pub fn session_roots(&self, session_id: &str) -> SessionRoots {
        let bound_project_root = self
            .execution_root_resolver
            .as_ref()
            .and_then(|resolver| resolver(session_id));
        sessions::session_roots_for(session_id, bound_project_root)
    }

    /// 当前 active session 的执行根目录：原生代码会话绑定了项目目录时返回项目目录
    /// （engine cwd 与 shell 执行目录由此同源），其余会话返回会话私有目录。
    /// 等价于 [`Self::session_roots`] 的 `execution` 字段。
    pub fn session_workspace(&self, session_id: &str) -> std::path::PathBuf {
        self.session_roots(session_id).execution
    }

    /// 注入原生代码会话的执行根解析器；由 app 组合根在 AcpPool 就绪后调用一次。
    pub fn set_execution_root_resolver(&mut self, resolver: ExecutionRootResolver) {
        self.execution_root_resolver = Some(resolver);
    }

    /// 注入原生代码会话判定（与执行根解析器同一份 SessionAgentStore）。
    pub fn set_code_session_predicate(
        &mut self,
        predicate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        self.code_session_predicate = Some(predicate);
    }

    /// 注入外部 ACP 会话判定（与原生 Code 判定同源于 AcpPool）。
    pub fn set_external_acp_session_predicate(
        &mut self,
        predicate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        self.external_acp_session_predicate = Some(predicate);
    }

    /// 该 session 是否为原生（品悟 Engine）代码会话（含临时与绑项目两种）。
    pub fn is_code_session(&self, session_id: &str) -> bool {
        self.code_session_predicate
            .as_ref()
            .is_some_and(|predicate| predicate(session_id))
    }

    /// 产品多智能体可用性同时受产品模式与运行时后端约束。SessionPolicy 只描述
    /// plain/code 轴；外部 ACP 虽然也是 plain，却不由 Pinvou Engine 执行。
    pub fn multi_agent_mode_available(&self, session_id: &str) -> bool {
        let external_acp = self
            .external_acp_session_predicate
            .as_ref()
            .is_some_and(|predicate| predicate(session_id));
        !external_acp && self.session_policy(session_id).supports_multi_agent_mode()
    }

    /// Returns whether this session exposes Browser MCP tools. Plain Work mode is necessary
    /// but not sufficient: external ACP sessions are also Plain, yet do not execute through
    /// the Pinvou Engine and must be excluded on the runtime axis.
    pub fn exposes_browser_mcp(&self, session_id: &str) -> bool {
        let external_acp = self
            .external_acp_session_predicate
            .as_ref()
            .is_some_and(|predicate| predicate(session_id));
        !external_acp && self.session_policy(session_id).exposes_browser_mcp()
    }

    /// 该 session 的会话模式策略：共享链路（发送 op 构造、工具整形、session
    /// instructions）按它取数，不再散 `is_code_session` if（D-2/D-3 统一入口）。
    /// predicate 未注入时默认 Plain——与 `is_code_session` 缺省 false 等价。
    pub fn session_policy(&self, session_id: &str) -> SessionPolicy {
        let mode = if self.is_code_session(session_id) {
            SessionMode::Code
        } else {
            SessionMode::Plain
        };
        SessionPolicy::for_mode(mode)
    }

    /// 会话级工具整形:按会话策略（[`SessionPolicy`]）并入模式差量——
    /// 无差量时原样返回。spawn 初值与全局热刷都经此整形。
    ///
    /// 传入的 `tools` 是全局(plain scope)的禁用工具名。差量项（均由编译期
    /// 静态表 `MODE_TABLE` 驱动，见 session_policy）：
    /// - 模式缺席工具（表字段 `unavailable_tools`；code: 产物卡）
    ///   ——"该模式架构上无此能力"，非用户偏好;
    /// - 连接器禁用集：非 plain 模式改用其 scope 的禁用集
    ///   ——scope 键即模式，各 scope 各自持久化(见 marketplace),互不影响;
    ///   非连接器禁用(kb_search 等)仍保留;
    /// - `load_skill` 按**该会话组合目录是否为空**动态决定（表字段
    ///   `skills_empty_hides_load_skill` 门控,V-5 联动）——目录为空（无任何
    ///   启用技能）时隐藏,避免"开关开着但没技能"的假状态;目录非空时放行。
    ///   判定在 bridge 侧做（目录检查是磁盘 I/O,策略对象保持纯数据）。
    pub fn shape_disallowed_tools(&self, session_id: &str, mut tools: Vec<String>) -> Vec<String> {
        let policy = self.session_policy(session_id);
        // 模式缺席工具（编译期常量表）：并入 disallowed（所有模式）。
        // ⚠️ 顺序约束：先于下方 connector retain——缺席名单应避开连接器全名
        // （当前 code 的 mcp_pinvou3_present_artifact 与连接器禁用集无交集），
        // 否则会被 retain 误删；新增条目时同样注意（或先做 retain 再追加）。
        for name in policy.unavailable_tools() {
            if !tools.iter().any(|tool| tool == name) {
                tools.push((*name).to_string());
            }
        }
        // 连接器禁用集：非 plain 模式用其 scope 的禁用集替换传入的 plain scope
        // 禁用集（plain 的禁用集就是传入值本身，无需替换）。
        if policy.mode() != SessionMode::Plain {
            let plain_connector = crate::features::marketplace::disabled_tool_names();
            let scoped_connector =
                crate::features::marketplace::disabled_tool_names_for(policy.mode());
            tools.retain(|tool| !plain_connector.iter().any(|blocked| blocked == tool));
            for blocked in scoped_connector {
                if !tools.iter().any(|tool| tool == &blocked) {
                    tools.push(blocked);
                }
            }
        }
        // load_skill 空目录隐藏由表字段驱动（与 scope 替换解耦）：组合目录为空 →
        // 一并隐藏（空态保护，V-5）。行为等价于原「scope 非 plain 分支内检查」：
        // 当前仅 code 该字段为 true，code 恒非 plain。
        if policy.capabilities().skills_empty_hides_load_skill
            && crate::features::assistant::skill_materialization::session_skills_is_empty(
                session_id,
            )
        {
            let load_skill = crate::features::assistant::session_policy::LOAD_SKILL;
            if !tools.iter().any(|tool| tool == load_skill) {
                tools.push(load_skill.to_string());
            }
        }
        tools
    }

    /// 应用账本根：审计等应用自有文件的落盘根。绑了项目目录的原生代码会话恒为
    /// 会话私有目录（不污染用户项目）；其余会话与传入的执行根相同——普通会话两
    /// 根本来一致，scheduled 会话继续写其项目目录，行为逐字节不变。
    ///
    /// `execution_workspace` 必须来自 [`Self::session_workspace`]（或
    /// [`Self::session_roots`] 的 `execution` 字段）。对 ledger 与 execution 相同的
    /// 会话（普通/临时代码/scheduled），直接返回调用方传入的执行根，保持
    /// scheduled 会话写其项目目录的既有行为。
    pub fn audit_workspace(
        &self,
        session_id: &str,
        execution_workspace: &std::path::Path,
    ) -> std::path::PathBuf {
        let roots = self.session_roots(session_id);
        if roots.ledger != roots.execution {
            roots.ledger
        } else {
            execution_workspace.to_path_buf()
        }
    }

    /// session 专属 `EngineConfig.instructions` 注入:
    ///   1. pinvou3 自家 INSTRUCTIONS_MD 渲染版(走 `InstructionSource::Inline`,
    ///      不写 disk — 见 C 方案 P-no-disk 决策);
    ///   2. 受限项目规则:绑定了项目目录的原生代码会话,注入项目根 → 用户家目录
    ///      (不含)路径上的 `AGENTS.md`,root→cwd 顺序(审阅建议③a;底座 C5 fork
    ///      已砍空 `PROJECT_CONTEXT_FILES`,不再自动扫描,这里按安全边界在 app 侧补齐);
    ///   3. 用户自定义 `~/.codewhale/instructions.md`(可选,仍走 `File`)。
    ///
    /// 之前版本写 `~/.pinvou3/sessions/<sid>/instructions.md` disk 文件然后传
    /// `Vec<PathBuf>` 给底座 — 改用 `InstructionSource::Inline` 后:
    ///  • disk 上没了多余的 instructions.md 给用户造成混淆
    ///  • 多引擎并发不再依赖 per-session 文件避免 race(内存对象天然隔离)
    ///  • rehydrate 不再从 disk 重读,内容跟 EngineConfig 一起在内存里活
    ///  • Inline name 保持稳定,避免纯展示标签中的 session_id 破坏跨会话前缀缓存
    pub fn session_instructions(&self, session_id: &str) -> Vec<InstructionSource> {
        let mut out: Vec<InstructionSource> = Vec::new();
        let rendered = self.build_session_system_prompt(session_id);
        out.push(InstructionSource::Inline {
            name: "pinvou3:instructions".to_string(),
            content: rendered,
        });
        for project_rule in self.code_session_project_rules(session_id) {
            out.push(InstructionSource::File(project_rule));
        }
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

    /// 受限项目规则（审阅建议③a）：仅对**绑定了项目目录的原生代码会话**注入
    /// `AGENTS.md`，覆盖项目根向上到用户家目录（不含）的路径——项目根即家目录时
    /// 一层都不注入；项目不在家目录之下时覆盖到文件系统根。
    ///
    /// 底座 C5 fork 已砍空 `PROJECT_CONTEXT_FILES`（不再自动扫描），这里在 app 侧
    /// 按安全边界补齐。行为语义：
    ///   - 注入顺序 root→cwd（祖先在前、项目根最后），与 codex/claude 惯例一致，
    ///     越靠近项目根的规则在提示词中越靠后；
    ///   - 家目录边界：home 与项目路径走同一归一化（canonicalize + 去 Windows
    ///     `\\?\` verbatim 前缀，与绑定入口 `validate_codex_project_workspace`
    ///     同源），`~/AGENTS.md` 等家目录及以上层不注入；归一化失败 fail-closed
    ///     ——项目根无法归一化时不注入，家目录无法归一化时只注入项目根本层、
    ///     不上溯祖先；
    ///   - symlink 拒读：`AGENTS.md` 是 symlink（可指向工作区外任意文件，如
    ///     ~/.ssh/id_rsa）时跳过，与底座 `project_context::load_context_file`
    ///     的防御范式对齐；
    ///   - 文件不存在或不可读时跳过。普通会话/临时代码会话不注入（行为不变）。
    fn code_session_project_rules(&self, session_id: &str) -> Vec<PathBuf> {
        let Some(project_root) = self
            .execution_root_resolver
            .as_ref()
            .and_then(|resolver| resolver(session_id))
        else {
            return Vec::new();
        };
        if !self.session_policy(session_id).binds_project() {
            return Vec::new();
        }
        // 项目根归一化失败（目录已删除/不可访问）→ fail-closed，不注入。
        let Some(project_root) = normalize_rule_boundary_path(&project_root) else {
            return Vec::new();
        };
        // 家目录归一化失败 → 无法确定上溯边界，fail-closed：只注入项目根本层。
        let home = normalize_rule_boundary_path(&crate::platform::paths::user_home_dir());
        let mut rules = Vec::new();
        for dir in collect_project_rule_chain(&project_root, home.as_deref()) {
            // 项目根及各祖先层（不含家目录本身）的 AGENTS.md 都注入，支持
            // monorepo 根规则覆盖子目录的既有语义。
            let agents = dir.join("AGENTS.md");
            if is_plain_file(&agents) {
                rules.push(agents);
            }
        }
        rules
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
    #[cfg(any(feature = "benchmark-hooks", test))]
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
        // `OpenAI compatible` 只是 wire protocol，不代表真实 provider 就是
        // OpenAI。reasoning_content 的解析、回放和思考开关都依赖底座里的
        // provider 身份，因此优先使用模型目录已经保存的 vendor 元数据。
        if let Some(vendor) = self
            .effective_model()
            .and_then(|model| model.vendor.as_deref())
            .map(str::trim)
            .filter(|vendor| !vendor.is_empty())
        {
            let provider = match vendor.to_ascii_lowercase().as_str() {
                "deepseek" => Some("deepseek"),
                "kimi" | "moonshot" => Some("moonshot"),
                "glm" | "zai" | "zhipu" => Some("zai"),
                "minimax" => Some("minimax"),
                "mimo" | "xiaomi" | "xiaomi-mimo" => Some("xiaomi-mimo"),
                "doubao" | "volcengine" => Some("volcengine"),
                // Anthropic 走底座内建 anthropic provider(Messages 原生协议,
                // x-api-key 鉴权),不能落入 OpenAI Chat Completions 路由。
                "anthropic" | "claude" => Some("anthropic"),
                "xai" | "grok" => Some("xai"),
                // DashScope、腾讯 Coding Plan 和 Gemini 暂无对应内建 provider,
                // 保留 OpenAI Chat Completions wire route(Gemini 官方提供
                // OpenAI 兼容端点),另由下方显式 reasoning_stream_style 保留
                // 独立思考字段。
                "qwen" | "tencent" | "openai" | "gemini" | "google" => Some("openai"),
                _ => None,
            };
            if let Some(provider) = provider {
                return provider.to_string();
            }
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
            ModelPreset::Doubao => "volcengine".to_string(),
            ModelPreset::Minimax => "minimax".to_string(),
            ModelPreset::Glm => "zai".to_string(),
            ModelPreset::Mimo => "xiaomi-mimo".to_string(),
            ModelPreset::Anthropic => "anthropic".to_string(),
            ModelPreset::Xai => "xai".to_string(),
            // Gemini 走官方 OpenAI 兼容端点，复用 openai wire route。
            ModelPreset::OpenaiCompatible
            | ModelPreset::Qwen
            | ModelPreset::Openai
            | ModelPreset::Gemini => "openai".to_string(),
        }
    }

    /// 当前 route 的流式思考协议。
    ///
    /// 已知厂商和官方兼容端点都把思考放在 `reasoning_content` /
    /// `reasoning` 独立字段中。显式写入 provider config，避免新模型 ID、
    /// Coding Plan 的动态别名或模型目录暂未收录时被降级成普通正文。
    /// 真正的自定义 OpenAI 兼容接口保持 None，继续采用底座的安全默认值，
    /// 不根据回答文本猜测思考内容。
    fn reasoning_stream_style(&self, provider: &str) -> Option<&'static str> {
        if matches!(
            provider,
            "deepseek" | "moonshot" | "zai" | "minimax" | "xiaomi-mimo" | "volcengine"
        ) {
            return Some(SEPARATE_REASONING_FIELD);
        }
        let vendor = self
            .effective_model()
            .and_then(|model| model.vendor.as_deref())
            .map(str::trim);
        if provider == "openai"
            && vendor.is_some_and(|vendor| {
                matches!(vendor.to_ascii_lowercase().as_str(), "qwen" | "tencent")
            })
        {
            return Some(SEPARATE_REASONING_FIELD);
        }
        if provider == "openai"
            && self
                .effective_model()
                .is_some_and(|model| model.preset == ModelPreset::Qwen)
        {
            return Some(SEPARATE_REASONING_FIELD);
        }
        None
    }

    /// 当前 route 发给模型的思考深度档位（透传底座 `reasoning_effort`）。
    ///
    /// 优先级：用户显式设置的 `SavedModel.reasoning_effort` > provider 默认
    /// （本地 vLLM 保持 off 防 SSE timeout；其余默认 high——底座自身默认是 Max，
    /// 品悟统一收口到 high，符合产品默认思考强度）。
    ///
    /// 本地 OpenAI 兼容端点（loopback 的 LM Studio/Ollama 等）保持旧行为不注入
    /// 档位（None），避免改造前不存在的 `reasoning_effort` 请求参数引起漂移。
    ///
    /// 注意：Kimi Code 的 `kimi-for-coding` 等是 always-thinking 模型，官方
    /// 接入要求 Thinking 保持开启；默认 high 由底座翻译成
    /// `thinking: {"type":"enabled"}`，天然满足该要求，无需特判模型名
    /// （探测 payload 仍需显式注入 thinking，见 image_capability.rs）。
    fn request_reasoning_effort(&self) -> Option<String> {
        if let Some(effort) = self
            .effective_model()
            .and_then(|model| model.reasoning_effort.as_deref())
        {
            return Some(effort.to_string());
        }
        match self.provider().as_str() {
            "vllm" => Some("off".to_string()),
            // 本地 OpenAI 兼容端点（loopback 服务）不注入，保持旧行为。
            "openai" if base_url_uses_loopback(&self.base_url()) => None,
            _ => Some("high".to_string()),
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

    /// 各厂商默认模型名（表在 prefs `ModelPreset::default_model`）。
    fn default_model_for_preset(&self) -> String {
        self.prefs
            .advanced
            .model_preset
            .unwrap_or_default()
            .default_model()
            .to_string()
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

    /// 各厂商默认 API base URL（表在 prefs `ModelPreset::default_base_url`）。
    fn default_base_url_for_preset(&self) -> String {
        self.prefs
            .advanced
            .model_preset
            .unwrap_or_default()
            .default_base_url()
            .to_string()
    }

    /// 当前 active api_key（传给底座 `DtConfig.api_key`）。
    pub fn api_key(&self) -> String {
        if let Some(credential) = &self.runtime_model_credential {
            return credential.expose_api_key().to_string();
        }
        if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
            if !v.trim().is_empty() {
                return v;
            }
        }
        if let Some(m) = self.effective_model() {
            let store = shared_credential_store();
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

    /// 解析一条任意 SavedModel 的凭据(视觉兜底模型专用,设计 §9.3):
    /// 复用 `credential_ref` → 系统凭据库路径,**不存第二份明文密钥**;
    /// 不回落到全局 `DEEPSEEK_API_KEY` env(那是主模型的覆盖入口)。
    /// 本地 vLLM/loopback 无鉴权场景返回占位 key(底座要求非空)。
    fn api_key_for_saved_model(model: &SavedModel) -> String {
        if let Some(reference) = &model.credential_ref {
            let store = shared_credential_store();
            match store.get(reference) {
                Ok(Some(key)) if !key.trim().is_empty() => return key,
                Ok(_) => {}
                Err(err) => {
                    eprintln!(
                        "[pinvou3-app] credential read failed for vision model {}: {}",
                        model.id,
                        err.user_message()
                    );
                }
            }
        }
        if model.preset == ModelPreset::LocalVllm || base_url_uses_loopback(&model.base_url) {
            return LOCAL_VLLM_API_KEY.to_string();
        }
        model.api_key.clone()
    }

    /// 视觉工具(`image_analyze`)配置解析(设计 §9.3,阶段 E)。规则:
    /// 1. 主模型设置了 `vision_model_id` → 用该 SavedModel 的 endpoint + 凭据;
    ///    id 失效、凭据缺失 → 记 warning 并优雅降级为不注册,不硬错。视觉模型
    ///    **自身**的图片能力不在此处拒绝——选择器已用识图探测闸门验证(supported
    ///    才允许选中),override 标记(disabled)可能是历史探测误判残留,运行时
    ///    按实际被选中的事实使用(见函数体内注释)。
    /// 2. 未设置、但主模型能力已确认为 Supported → 复用主模型作为 workspace
    ///    图片分析工具(保留旧的复用行为,但仅限 Supported)。
    /// 3. 主模型 Unsupported/Unknown 且未设置视觉模型 → 返回 None,不注册
    ///    `image_analyze`(不 enable `Feature::VisionModel`)。例外:`image_analyze_always`
    ///    (scheduled 会话)时 Unknown 回退复用主模型——scheduled 图片不走路由,
    ///    prompt 硬规则要求调用 `image_analyze`,未注册会让模型反复调用不存在的
    ///    工具;调用时 provider 拒绝的优雅失败与 main 行为一致。
    fn resolve_vision_model_config(&self) -> Option<deepseek_tui::config::VisionModelConfig> {
        let effective = self.effective_model();
        if let Some(vision_id) = effective.and_then(|model| model.vision_model_id.as_deref()) {
            let Some(vision) = self.prefs.model_by_id(vision_id) else {
                eprintln!(
                    "[pinvou3-app] vision_model_id {vision_id} not found in saved_models; \
                     image_analyze disabled"
                );
                return None;
            };
            // 视觉模型自身能力不在此处拒绝:选择器已用识图探测验证(supported
            // 才允许选中)。override 标记(disabled)可能是历史探测误判残留
            // (如 kimi-for-coding 曾因探测链路 400 被回填),运行时按实际被
            // 选中的事实使用;文本模型配成视觉模型由前端探测闸门挡住。
            let api_key = Self::api_key_for_saved_model(vision);
            if api_key.trim().is_empty() {
                eprintln!(
                    "[pinvou3-app] vision model {} has no usable credential; \
                     image_analyze disabled",
                    vision.id
                );
                return None;
            }
            return Some(deepseek_tui::config::VisionModelConfig {
                model: vision.model.clone(),
                api_key: Some(api_key),
                base_url: Some(vision.base_url.clone()),
            });
        }
        if effective.map(effective_image_capability) == Some(EffectiveImageCapability::Supported) {
            return Some(deepseek_tui::config::VisionModelConfig {
                model: self.model(),
                api_key: Some(self.api_key()),
                base_url: Some(self.base_url()),
            });
        }
        // scheduled 例外:Unknown(如本地 vLLM 模型不在内置表)也注册,见函数头
        // 注释规则 3。Unsupported 仍不注册(确认不支持的模型注册了只会持续报错)。
        if self.image_analyze_always
            && effective.map(effective_image_capability) == Some(EffectiveImageCapability::Unknown)
        {
            return Some(deepseek_tui::config::VisionModelConfig {
                model: self.model(),
                api_key: Some(self.api_key()),
                base_url: Some(self.base_url()),
            });
        }
        None
    }

    /// 当前有效模型的图片输入能力(设计 §6.3)。命令层在发送前拒绝时需要据此
    /// 区分"确认不支持"与"能力未知",给出不同的用户指引。
    pub fn effective_image_capability(&self) -> EffectiveImageCapability {
        self.effective_model()
            .map(effective_image_capability)
            // 无有效模型(配置损坏)按 Unknown 处理:不冒充支持,交给路由兜底。
            .unwrap_or(EffectiveImageCapability::Unknown)
    }

    /// 兜底视觉模型端点是否本地(§11.8/§11.9):None 表示未配置可用视觉模型。
    /// fallback 路径的图片字节发给视觉模型而非主模型,隐私提示必须按此口径。
    pub fn vision_uses_local_endpoint(&self) -> Option<bool> {
        self.resolve_vision_model_config().map(|config| {
            config
                .base_url
                .as_deref()
                .is_some_and(base_url_uses_loopback)
        })
    }

    /// 普通会话图片输入路由(设计 §9.2,阶段 D)。仅当消息含图片附件时由命令层调用。
    /// `has_vision_model` 取自 `resolve_vision_model_config`:Supported 主模型本来就走
    /// Native,该值只在 Unsupported/Unknown 时影响路由,而那时 Some 仅可能来自
    /// `vision_model_id` 命中的独立视觉模型。
    pub fn image_input_mode(&self) -> crate::features::assistant::image_capability::ImageInputMode {
        crate::features::assistant::image_capability::image_input_mode(
            self.effective_image_capability(),
            self.resolve_vision_model_config().is_some(),
        )
    }

    /// 是否配置了**可用**的独立视觉模型(或 Supported 主模型可自复用)。
    /// 与 `image_input_mode` 内部的 `has_vision_model` 同一口径
    /// (`resolve_vision_model_config`),供能力查询命令回传前端展示。
    pub fn has_vision_model(&self) -> bool {
        self.resolve_vision_model_config().is_some()
    }

    /// 当前有效模型 endpoint 是否指向本机(设计 §11.8/§11.9):前端据此决定是否在
    /// 附件区提示"图片将发送给模型服务商"——本机 loopback 场景图片字节不离开本机,
    /// 不得显示云上传字样。判定与 `api_key_for_saved_model` 同一口径:preset 为
    /// local_vllm,或有效 base_url host 为 loopback(127.0.0.1/localhost/[::1])。
    pub fn is_local_endpoint(&self) -> bool {
        let preset_is_local = match self.effective_model() {
            Some(model) => model.preset == ModelPreset::LocalVllm,
            // 无有效模型(配置损坏):按全局 preset 判定。
            None => self.prefs.advanced.model_preset.unwrap_or_default() == ModelPreset::LocalVllm,
        };
        preset_is_local || base_url_uses_loopback(&self.base_url())
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
                let store = shared_credential_store();
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
    /// 24K 而非 64K:thinking 关闭后单次回复通常显著低于该上限；24K 仍覆盖
    /// 弱模型偶尔输出较大工具参数的 margin,同时把输入预算从 189K(74%)
    /// 抬到 230K(90%),让自动压缩更晚触发。64K 是 ~4x 设计上限的过度预留。
    pub fn max_output_tokens(&self) -> u32 {
        if let Ok(v) = std::env::var("PINVOU3_MAX_OUTPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                return n;
            }
        }
        self.prefs.advanced.max_output_tokens.unwrap_or(24_576)
    }

    /// 为一个具体 wire model 生成宿主已知的 route facts：
    /// SavedModel 显式能力与实时 probe 取更小值；两者都没有时复用运行状态页同一份
    /// 模型 catalog，未知本地 vLLM 才使用 128K 保守值。
    /// output_tokens: the local vLLM explicitly carries the Pinvou 24K budget
    /// (pre-existing anti-SSE-timeout constraint); uncatalogued models on
    /// user-configured (operator-owned) openai-compatible endpoints declare
    /// the base window heuristic (see the in-function comment); other cloud
    /// models stay undeclared (SavedModel.max_output_tokens defaults to None)
    /// so the base falls back by vendor capability / conservative guess.
    fn route_limits_for_model(&self, model: &str) -> Option<codewhale_config::route::RouteLimits> {
        let saved = self.effective_model().filter(|saved| saved.model == model);
        let configured_context = saved.and_then(|saved| saved.context_window_tokens);
        let inferred_context = crate::core::model_context::resolved_context_window(model);
        let is_local_vllm = self.provider() == "vllm";
        let context_tokens = match (configured_context, self.probed_context_tokens) {
            (Some(configured), Some(probed)) => Some(configured.min(probed)),
            (Some(configured), None) => Some(configured),
            (None, Some(probed)) => Some(probed),
            (None, None) => inferred_context.or_else(|| is_local_vllm.then_some(128_000)),
        };
        let configured_output = saved.and_then(|saved| saved.max_output_tokens);
        // User-configured openai-compatible endpoints (the `OpenAI-compatible`
        // preset, or provider_kind == "custom") count as operator-owned: the
        // endpoint is configured by the user and the output ceiling is the
        // operator's responsibility. The base (upstream #5461 semantics)
        // fail-closes uncatalogued models to the 8192 conservative guess and
        // replaces it only when the route declares an explicit output_tokens
        // fact — acting as the operator's proxy, the host declares the base's
        // window heuristic (>=500K -> 64K, otherwise window/2, and half of
        // the 128K fallback when no window fact exists) as that route fact.
        // The declared value mirrors the base requested_cap window heuristic:
        // documented models keep their semantics, while uncatalogued models
        // recover the same window heuristic as documented ones instead of
        // 8192 (a 128K window -> 64000). The value sources differ (host window
        // fact vs base name-based heuristic), so they can diverge; the base
        // min() only ever merges downward, and small windows stay clamped to
        // window/2. This is a capability declaration, not the Pinvou per-turn
        // budget, and is NOT clamped by the process-level max_output_tokens()
        // (24K) — same as documented cloud models; users can tighten it
        // explicitly via SavedModel.max_output_tokens (configured_output wins
        // and is clamped by the process budget).
        // coding_plan is the official managed entry point (bigmodel/kimi/
        // tencent prefs normalize provider_kind by endpoint URL, preset
        // unchanged): even when it rides on the `OpenAI-compatible` preset it
        // is an official endpoint and must stay base fail-closed — no
        // operator declaration.
        let is_operator_owned_endpoint = saved.is_some_and(|saved| {
            (saved.preset == ModelPreset::OpenaiCompatible
                || saved.provider_kind.as_deref() == Some("custom"))
                && saved.provider_kind.as_deref() != Some("coding_plan")
        });
        let output_tokens = configured_output
            .map(|tokens| tokens.min(self.max_output_tokens()))
            .or_else(|| is_local_vllm.then(|| self.max_output_tokens()))
            .or_else(|| {
                // With a window fact <= 4K the output half is under 2K and cannot
                // fit a meaningful output budget after headroom: stay undeclared
                // (fail-closed) instead of emitting a Some(<2K) route fact.
                let declared = context_tokens.map_or(64_000, |window| {
                    if window >= 500_000 {
                        65_536
                    } else {
                        (window / 2).min(65_536)
                    }
                });
                (is_operator_owned_endpoint && declared >= 4_096).then_some(declared)
            })
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

    /// 当前 active route 的上下文窗口（供 chat:usage 事件携带给前端做
    /// token 进度条分母）。与 effective_context_window 同源（SavedModel 声明
    /// vs probe 取小），云端模型不再停留在前端 32K 假分母。
    pub fn usage_context_window(&self) -> u32 {
        self.effective_context_window(&self.model())
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
                crate::core::model_context::resolved_context_window(model).unwrap_or(128_000)
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

    /// legacy 单引擎路径(headless harness 用):走 instructions inline + 用户自定义。
    /// 跟 [`session_instructions`] 区别仅在不带 session_id —— 直接用 work 渲染原文
    /// (不替换 `{{PINVOU3_WORKSPACE}}`)。
    pub fn instructions(&self) -> Vec<InstructionSource> {
        let mut out: Vec<InstructionSource> = vec![InstructionSource::Inline {
            name: "pinvou3:bundle/instructions".to_string(),
            content: instructions_md().to_string(),
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
            plugin_registry: _,
            instructions: _,
            project_context_pack_enabled: _,
            // advanced.max_steps 显式配置时覆盖；未配置则复用底座默认值。
            max_steps: default_max_steps,
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
            goal_max_continuations,
            workshop,
            snapshots_max_workspace_bytes,
            search_provider: _, // pinvou3 显式构造 (见下),由 prefs.search 翻译
            search_api_key: _,
            goal_state,
            mut tools_always_load,
            prefer_bwrap,
            turn_tool_security: _,
            // pinvou3-fork 自定义:会话初始思考开关(显式构造见下)
            reasoning_effort: _,
            // —— v0.8.49 上游新增字段,透传 default ——
            allowed_tools: _,
            tools,
            // —— v0.8.51 上游新增字段 ——
            speech_output_dir,
            hook_executor: _, // pinvou3 注入 bundle hook（连接器自省纠正）+ CLI 环境 hook
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
            max_tool_calls,
            // —— v0.8.65 上游新增字段,透传 default ——
            //   subagents_enabled: default true（通用多智能体委派需要 SpawnSubAgent）。
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
            terminal_chrome_enabled,
            advisor_config,
            subagent_state_root,
        } = EngineConfig::default();

        // hook 有两条消费路径：turn_loop 从 EngineConfig.hook_executor 跑
        // ToolCallBefore，exec_shell 则从 RuntimeToolServices.hook_executor 收集
        // shell_env。必须共享同一个实例，不能只填前者。
        let hook_executor = self.build_hook_executor();
        runtime_services.hook_executor = Some(hook_executor.clone());
        tools_always_load.extend(
            crate::features::assistant::tool_policy::PINVOU3_ALWAYS_LOADED_TOOLS
                .iter()
                .map(|name| (*name).to_string()),
        );

        // 视觉工具(image_analyze)注册两道门(设计 §9.3,阶段 E):
        //   vision_config 有值 + Feature::VisionModel 开启,缺一不可。
        // 不再无条件复用主模型:只有「显式 vision_model_id 可解析」或
        // 「主模型能力确认 Supported」才注册;否则文本模型也会拿到
        // image_analyze,调用时才发现不支持图片(原 bridge 无条件复用 bug)。
        let vision_config = self.resolve_vision_model_config();
        let vision_tool_enabled = vision_config.is_some();

        EngineConfig {
            // pinvou3 覆盖
            model: self.model(),
            workspace: self.workspace.clone(),
            allow_shell: self.allow_shell(),
            trust_mode: true,
            notes_path: paths::notes_path(),
            // Work-mode gate: Browser MCP tools are exposed only to assistant Engine
            // sessions. Global mcp.json has no browser entry; this creates a session-specific
            // global-plus-browser configuration and falls back to global configuration when
            // prerequisites are missing. External Agents such as Codex ACP do not use this
            // path and therefore cannot receive browser tools.
            mcp_config_path: self.bundle.work_mode_mcp_config_path(),
            skills_dir: self.bundle.skills_dir.clone(),
            plugin_registry: None,
            instructions: self.instructions(),
            project_context_pack_enabled: false,
            max_steps: self.prefs.advanced.max_steps.unwrap_or(default_max_steps),
            // 默认 10，为会话级多智能体 fan-out 场景预留。
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
            // 视觉配置由 resolve_vision_model_config 按 §9.3 三规则解析;
            // None = 不注册 image_analyze(主模型不支持/未知且无可用视觉模型)。
            vision_config,
            // [pinvou3-fork] 上游默认 120s 是为 DeepSeek 云端 API 设计。
            // 本地 Qwen3.6 vLLM 慢推理下单 step 30-90s 很常见,120s 频繁误杀子 agent。
            // 300s 与 elapsed cap 对齐,给复杂研究类任务留出完整单步窗口。
            subagent_api_timeout: std::time::Duration::from_secs(300),
            // 开启 VisionModel feature(默认 Experimental 关)仅当 vision_config
            // 解析成功(见上):tool_setup.rs 才会注册 image_analyze 工具给 LLM。
            // 两道门缺一不可——只配 vision_config 不开 feature,工具不会注册。
            features: {
                let mut f = features;
                if vision_tool_enabled {
                    f.enable(deepseek_tui::features::Feature::VisionModel);
                }
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
                        proxy_fake_ip_cidrs: Vec::new(),
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
            goal_max_continuations,
            workshop,
            snapshots_max_workspace_bytes,
            // pinvou3 search 后端: prefs 翻译。
            // 底座默认仍是 DuckDuckGo;这里构造 EngineConfig 时丢弃底座默认、显式注入
            // prefs 默认 Bing (forkguard_search_provider_translates_from_prefs 锁定)。
            // Metaso/Bocha/Baidu 是 GUI 切换项。底座 web_search 对 Metaso 留空 key 用
            // 内置共享 key (~100 次/天),对 Bocha/Baidu 留空 key 直接报 ToolError "requires API key"。
            search_provider: match self.prefs.search.provider {
                prefs::SearchProvider::Bing => deepseek_tui::config::SearchProvider::Bing,
                prefs::SearchProvider::Metaso => deepseek_tui::config::SearchProvider::Metaso,
                prefs::SearchProvider::Bocha => deepseek_tui::config::SearchProvider::Bocha,
                prefs::SearchProvider::Baidu => deepseek_tui::config::SearchProvider::Baidu,
                prefs::SearchProvider::Tavily => deepseek_tui::config::SearchProvider::Tavily,
            },
            search_api_key: self.search_api_key(),
            goal_state,
            tools_always_load,
            prefer_bwrap,
            turn_tool_security: None,
            // 会话初始思考开关：本地 vLLM(Qwen3.6)必须关 thinking。在 engine
            // 配置层统一钉死，避免子智能体继承到未预期的思考模式。
            reasoning_effort: self.request_reasoning_effort(),
            // Pinvou 产品工具面使用 CodeWhale 0.9.5 原生 hard allowlist。它约束
            // 初始目录、tool_search 与 dispatch；SubAgent 角色仍会在此基础上进一步收窄。
            allowed_tools: Some(crate::features::assistant::tool_policy::allowed_tool_names()),
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
            // [多智能体] 不追加 `workflow` 禁令：主线上底座在 subagents_enabled 时
            // 注册的 WorkflowTool 对所有会话可用，本分支保持能力持平。委派提醒只教
            // agent 集群、不教 workflow；已知底座限制记录在 ADR-0006。
            disallowed_tools: {
                let n = crate::features::marketplace::disabled_tool_names();
                if n.is_empty() { None } else { Some(n) }
            },
            max_tool_calls,
            // [pinvou3-fork] 透传 default(空);kb_search 在 spawn_for_session 按 session 注入
            // —— v0.8.65 上游新增字段,透传 default ——
            //   subagents_enabled: default true（通用多智能体委派需要 SpawnSubAgent）。
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
            terminal_chrome_enabled,
            advisor_config,
            subagent_state_root,
        }
    }

    /// Build a session config with the ordinary per-session workspace.
    ///
    /// [`build_engine_config`]: Self::build_engine_config
    pub fn build_engine_config_for_session(&self, session_id: &str) -> EngineConfig {
        self.build_engine_config_for_session_roots(session_id, self.session_roots(session_id))
    }

    /// Build a session config from its execution and ledger roots.
    ///
    /// Project-bound Code sessions execute in the project while delegated-agent
    /// control-plane state remains under the session-owned ledger root. Scheduled
    /// conversations pass roots resolved by
    /// [`crate::features::sessions::SessionStore::session_roots`] so
    /// their existing shared automation workspace semantics remain unchanged.
    pub(crate) fn build_engine_config_for_session_roots(
        &self,
        session_id: &str,
        roots: SessionRoots,
    ) -> EngineConfig {
        let mut cfg = self.build_engine_config();
        let _ = std::fs::create_dir_all(&roots.execution);
        let _ = std::fs::create_dir_all(&roots.ledger);
        cfg.workspace = roots.execution;
        cfg.subagent_state_root = Some(roots.ledger);
        cfg.instructions = self.session_instructions(session_id);
        // 技能发现根按会话指向组合目录（skill 双 scope 治理：目录内容 = 该会话
        // scope 的启用技能集）。spawn 前的物化由 EnginePool 负责；此处只注入路径。
        // 目录不存在时底座 `insert_configured_skills_dir` 会跳过（发现集为空 →
        // `## Skills` 块不渲染），发送路径的自愈（`ensure_session_skills`）保证
        // 目录在下次物化时机前被重建。
        cfg.skills_dir = crate::platform::paths::session_skills_dir(session_id);
        // CLI 硬拦截（scope 门禁的 execpolicy 通道）：spawn 注入初值；开关切换
        // 后的热刷走 EnginePool::refresh_permission_rulesets，两处同一份计算。
        // 规则集 = CLI 二进制名 deny + 禁用技能脚本路径 deny（§5.1 通道③）
        // + 敏感数据/提权安全 deny（原 bundle hook deny_sensitive_paths 第 1-4
        // 段迁移，见 safety_deny_rules 模块注释）。
        cfg.exec_policy_engine = codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
            self.scope_deny_ruleset(session_id),
        ]);
        // Native Code-mode and external ACP sessions do not expose Browser MCP tools. They
        // fall back to global mcp.json, which has no browser entry. System instructions and
        // tool registration share this gate.
        if !self.exposes_browser_mcp(session_id) {
            cfg.mcp_config_path = crate::platform::paths::mcp_config_path();
        } else {
            // A Work-mode session uses its own browser-wrapper configuration, pinning the
            // Agent tool context to this session's WebView2 page so other sessions cannot
            // redirect it by opening or switching pages.
            cfg.mcp_config_path = self
                .bundle
                .work_mode_mcp_config_path_for_session(session_id);
        }
        cfg
    }

    /// 该会话 scope 被禁 CLI 连接器的 execpolicy deny 规则集（硬拦截）。
    ///
    /// 技能组合目录是软门控（模型看不见技能），本规则集是硬兜底：模型即使知道
    /// 命令，`lark-cli` 等被禁 CLI 二进制也在 spawn 前被底座硬拒（deny 全模式
    /// 生效，含 YOLO；错误直返模型）。从 scope 禁用集派生，自身无状态。
    ///
    /// 覆盖面边界（四轮评审登记，仅注释、不改行为）：规则按 CLI **二进制名**做
    /// word-boundary 前缀匹配（同 `skill_script_deny_rules` 的 DSL 现状），只拦
    /// 「首 token 即该二进制名」的直接调用；每个二进制同时发 `{bin}.exe` /
    /// `{bin}.cmd` 变体（六轮评审 R4：Windows 带扩展名拼写绕过）。已知残余绕过面：
    /// - 首 token 拼成路径（`C:\...\lark-cli.exe`、`/usr/local/bin/lark-cli`、
    ///   `./bin/lark-cli`）——typed ask 规则走底座 `allow_rule_matches` 纯前缀
    ///   比对，无 basename 折叠（底座折叠仅作用于 `denied_prefixes` 字符串通道），
    ///   路径拼写仍绕过（七轮评审实证）；
    /// - Windows PATHEXT 其余变体（`.bat`/`.com`/`.ps1` 等）未发规则——npm shim
    ///   为 `.cmd`、原生二进制为 `.exe`，其余拼写被模型生成的现实概率低；
    /// - shell 包装前缀（`cmd /c lark-cli …`、`powershell -Command …`、`sh -c …`、
    ///   `env lark-cli …`、`env python …` 等）——首 token 是包装器；
    /// - 重命名/拷贝后的同功能二进制。
    /// 以上绕过面与「禁用连接器 CLI 被模型直接调用」的主路径相比属边缘场景，
    /// 登记待底座 execpolicy 支持参数级/路径级匹配后收敛（底座缝候选）。
    pub(crate) fn cli_deny_ruleset(&self, session_id: &str) -> codewhale_execpolicy::Ruleset {
        codewhale_execpolicy::Ruleset::user(vec![], vec![])
            .with_ask_rules(self.cli_deny_rules(session_id))
    }

    fn cli_deny_rules(&self, session_id: &str) -> Vec<codewhale_execpolicy::ToolAskRule> {
        let scope = self.session_policy(session_id).mode();
        // 不可用集 = 开关关 + 不可见，两套门控都硬拒 CLI 二进制。
        crate::features::marketplace::unavailable_bundles_for(scope)
            .into_iter()
            .filter_map(|id| crate::features::marketplace::bundle::cli_bundle_bin(&id))
            .flat_map(|bin| {
                // Windows 下首 token 常带扩展名（`lark-cli.exe im send`），只发裸
                // 二进制名会被绕过（六轮评审 R4）：每个规则同时发 `{bin}.exe` /
                // `{bin}.cmd` 变体。无条件发、不加 cfg —— 非 Windows 平台上这些
                // 规则惰性无害，避免引入平台条件编译。
                [bin.to_string(), format!("{bin}.exe"), format!("{bin}.cmd")]
                    .into_iter()
                    .map(|cmd| {
                        let mut rule = codewhale_execpolicy::ToolAskRule::exec_shell(cmd);
                        rule.action = codewhale_execpolicy::PermissionAction::Deny;
                        rule
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    }

    /// scope 门禁 + 安全兜底的完整 execpolicy 硬拦截规则集 = CLI 二进制名 deny
    /// + 禁用技能的脚本路径 deny（§5.1 通道③）+ 敏感数据/提权 deny（原 bundle
    /// hook deny_sensitive_paths 第 1-4 段迁移，v1）。spawn 注入初值与开关热刷
    /// 共用这一份计算。command 型 deny 同时提升进 denied_prefixes（底座
    /// config 加载器同语义），激活 flag 感知 / basename 折叠 / wrapper 剥离的
    /// deny-always-wins 通道。
    pub(crate) fn scope_deny_ruleset(&self, session_id: &str) -> codewhale_execpolicy::Ruleset {
        self.scope_deny_ruleset_with(
            session_id,
            crate::features::assistant::safety_deny_rules::safety_deny_rules(),
        )
    }

    /// [`scope_deny_ruleset`] 的安全段可注入形态：生产传实时读盘快照
    /// [`safety_deny_rules::safety_deny_rules`]，测试注入固定 sudo 态（与宿主机
    /// `/etc/sudoers.d/pinvou3` 真实状态解耦）。回归测试必须走这个入口——规则集
    /// 组成与生产天然同源，生产增删规则源时测试同步生效，不会假绿。
    pub(crate) fn scope_deny_ruleset_with(
        &self,
        session_id: &str,
        safety_rules: Vec<codewhale_execpolicy::ToolAskRule>,
    ) -> codewhale_execpolicy::Ruleset {
        let mut rules = self.cli_deny_rules(session_id);
        rules.extend(self.skill_script_deny_rules(session_id));
        rules.extend(safety_rules);
        crate::features::assistant::safety_deny_rules::ruleset_with_denied_prefix_promotion(rules)
    }

    /// 带脚本技能的执行点硬拦截（marketplace-unification §5.1 通道③）。
    ///
    /// 缺口：物化排除只让模型「看不见」禁用技能，脚本仍物理存在于
    /// `bundle/skills/<name>/`，模型可凭路径经 exec_shell 直接执行。本规则集在
    /// spawn 前硬拒（typed Deny 短路于一切审批模式，含 YOLO）。
    ///
    /// 取数与物化排除同一口径（`disabled_skill_names_for`：scope 禁用集 +
    /// 被禁连接器的 companion 技能联动）。
    ///
    /// 底座 DSL 现状（调研结论）：`ToolAskRule.command` 是 word-boundary 前缀匹配
    /// （参数位精确 token 匹配），能表达「解释器 + 脚本完整路径」，**不能表达目录
    /// 前缀**（模式后必须是空格或结尾）。故按目录内脚本文件逐个枚举生成规则。
    /// 已知残余面（注释登记，不阻塞）：非常用解释器、stdin 喂脚本（`python < x.py`）、
    /// 拷贝后执行、引号/正反斜杠拼写差异可绕过；目录级前缀规则需底座 execpolicy
    /// 支持参数级路径前缀匹配（底座缝候选）。
    fn skill_script_deny_rules(&self, session_id: &str) -> Vec<codewhale_execpolicy::ToolAskRule> {
        let scope = self.session_policy(session_id).mode();
        // 技能目录定位经 `find_skill_dir`：新布局（bundles/<pkg>/skills/）优先、
        // 旧扁平布局回退——取数与物化排除同一口径（disabled_skill_names_for）。
        let manager =
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        let mut rules = Vec::new();
        let mut names: Vec<String> =
            crate::features::assistant::skill_materialization::disabled_skill_names_for(scope)
                .into_iter()
                .collect();
        names.sort();
        for name in names {
            let Some(dir) = manager.find_skill_dir(&name) else {
                continue; // 未安装/已物化排除：无脚本可拦
            };
            rules.extend(skill_script_deny_rules_for(&dir));
        }
        rules
    }

    /// 多智能体会话专用配置（ADR-0006）。
    ///
    /// 与普通会话的业务区别是装配专家名册与资源护栏：专家池内可执行的内置卡和用户卡
    /// 作为底座原生 `[fleet.profiles]` 内存配置，整册装进 `fleet_roster` 供裸
    /// `agent` 的 `profile` 字段选人；主模型每轮只看到按任务匹配的短候选，完整人设仅注入
    /// 被派中的子智能体。没有相关候选时模型自拟任务说明裸派。**工具目录与普通会话完全一致**
    /// ——禁用列表只来自连接器开关，`workflow` 与主线一样保持可用（委派
    /// 提醒不教学不推荐）。默认直属实例为叶子；复杂任务允许直属实例再拆一层，
    /// 第二层不得继续派生。Work 直属并行 4 / 全树准入 8，原生 Code 直属并行
    /// 6 / 全树准入 12。更深后代为避免父子互等死锁不占直属 launch gate，
    /// 但仍受整棵树的准入上限约束。
    pub(crate) fn build_engine_config_for_multi_agent(
        &self,
        session_id: &str,
        roots: SessionRoots,
        snapshot: &ExpertRosterSnapshot,
    ) -> EngineConfig {
        let mut cfg = self.build_engine_config_for_session_roots(session_id, roots);
        // 主会话是总协调者：直属子智能体处于 depth=1，复杂任务可再派生
        // depth=2；第二层不能继续。主会话侧的正数深度覆盖由专用 hook 拦截；
        // 嵌套层的工具调用不经过 ToolCallBefore，靠继承上限（省略参数即
        // 收窄）与全局准入/并发额度兜底。
        cfg.max_spawn_depth = cfg.max_spawn_depth.min(MULTI_AGENT_MAX_SPAWN_DEPTH);
        let (max_concurrent, max_admitted) = if self.is_code_session(session_id) {
            (
                MULTI_AGENT_CODE_MAX_CONCURRENT,
                MULTI_AGENT_CODE_MAX_ADMITTED,
            )
        } else {
            (
                MULTI_AGENT_WORK_MAX_CONCURRENT,
                MULTI_AGENT_WORK_MAX_ADMITTED,
            )
        };
        // 显式用户配置只做上限，不抬高更保守的值（包括 0 = 禁用）；未配置时
        // 使用当前会话模式的产品默认。
        cfg.max_subagents = self
            .prefs
            .advanced
            .max_subagents
            .map_or(max_admitted, |configured| configured.min(max_admitted));
        cfg.max_admitted_subagents = cfg
            .max_admitted_subagents
            .min(max_admitted)
            .max(cfg.max_subagents);
        cfg.launch_concurrency = cfg
            .launch_concurrency
            .min(max_concurrent)
            .min(cfg.max_subagents);
        cfg.hook_executor = Some(self.build_multi_agent_hook_executor(&cfg.workspace));
        cfg.fleet_roster = std::sync::Arc::new(deepseek_tui::FleetRoster::load(
            snapshot.fleet_config(),
            &cfg.workspace,
        ));
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
        let reasoning_stream_style = self.reasoning_stream_style(&provider);
        let providers = cfg.providers.get_or_insert_with(ProvidersConfig::default);
        // 按 provider 写对应 provider 配置的 base_url + api_key
        match provider.as_str() {
            "vllm" => configure_provider(
                &mut providers.vllm,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "openai" => configure_provider(
                &mut providers.openai,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "deepseek" => configure_provider(
                &mut providers.deepseek,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "moonshot" => configure_provider(
                &mut providers.moonshot,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "volcengine" => configure_provider(
                &mut providers.volcengine,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "zai" => configure_provider(
                &mut providers.zai,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "minimax" => configure_provider(
                &mut providers.minimax,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "xiaomi-mimo" => configure_provider(
                &mut providers.xiaomi_mimo,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "anthropic" => configure_provider(
                &mut providers.anthropic,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            "xai" => configure_provider(
                &mut providers.xai,
                &base_url,
                &api_key,
                &model,
                reasoning_stream_style,
            ),
            _ => {
                configure_provider(
                    &mut providers.vllm,
                    &base_url,
                    &api_key,
                    &model,
                    reasoning_stream_style,
                );
            }
        }
        cfg.default_text_model = Some(model);
        // 本地 vLLM 必须关 thinking（防 SSE timeout）；其余默认 high。
        cfg.reasoning_effort = self.request_reasoning_effort();
        cfg
    }

    /// 为开启多智能体的 Engine/turn 注入 Pinvou 专家池对应的原生
    /// `[fleet.profiles]`。调用方必须复用与提醒相同的 [`ExpertRosterSnapshot`]；
    /// 普通会话继续调用 [`build_dt_config`](Self::build_dt_config)，不会获得专家。
    pub(crate) fn build_multi_agent_dt_config(&self, snapshot: &ExpertRosterSnapshot) -> DtConfig {
        let mut config = self.build_dt_config();
        config.fleet = Some(snapshot.fleet_config().clone());
        config
    }

    /// 注入连接器自省纠正 hook：ToolCallBefore 时 spawn bundle 脚本，把技能型
    /// 连接器被误当 MCP 自省的调用纠正回模型（exit 2 + stdout JSON reason，
    /// Hooks v2 契约）。原脚本承担的敏感目录/危险命令/sudo 硬拦截段已迁移至
    /// execpolicy 规则引擎（`safety_deny_rules`），脚本不再做硬拦截。
    /// 脚本本体在 bundle 中,首次启动解包到
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
            problems: Vec::new(),
        }
    }

    fn build_hook_executor(&self) -> Arc<HookExecutor> {
        Arc::new(HookExecutor::new(
            self.build_hooks_config(),
            self.workspace.clone(),
        ))
    }

    /// 多智能体会话的资源护栏。`EngineConfig.max_spawn_depth = 2` 允许直属
    /// 代理为复杂任务再拆一层；该 hook 拦住**主会话**在 `agent` / `workflow`
    /// 调用中用正数深度覆盖参数扩大上限，并要求 Workflow 文件调用改用
    /// 可检查的 inline 输入。嵌套子代理的工具调用不经过 ToolCallBefore
    /// hook——那一层由继承上限与全局准入/并发额度兜底，提醒词只作教学。
    /// 普通对话不挂载此 hook。
    fn build_multi_agent_hook_executor(&self, workspace: &std::path::Path) -> Arc<HookExecutor> {
        #[cfg(windows)]
        let command = {
            let script = self.bundle.multiagent_depth_guard_ps1.to_string_lossy();
            format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{script}\"")
        };
        #[cfg(not(windows))]
        let command = {
            let script = self
                .bundle
                .multiagent_depth_guard_sh
                .to_string_lossy()
                .replace('\'', "'\\''");
            format!("bash '{script}'")
        };

        let mut config = self.build_hooks_config();
        config.hooks.push(Hook {
            event: HookEvent::ToolCallBefore,
            command,
            condition: Some(HookCondition::Any {
                conditions: vec![
                    HookCondition::ToolName {
                        name: "agent".to_string(),
                    },
                    HookCondition::ToolName {
                        name: "workflow".to_string(),
                    },
                ],
            }),
            timeout_secs: 5,
            background: false,
            continue_on_error: false,
            name: Some("pinvou3-multiagent-depth-guard".into()),
        });
        Arc::new(HookExecutor::new(config, workspace.to_path_buf()))
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
    /// 注：底座现已让 `auto_approve = true` **旁路**可绕过的 Required 审批
    /// （`turn_loop.rs::registered_tool_approval_required`，早期版本不旁路）。
    /// 需要审批事件的场景必须逐轮关掉它；Yolo 还会在底座重新折算成自动批准，
    /// 因此需要审批的运行必须同步收紧 mode、trust 与审批字段；定时任务按 profile。
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

    /// 解析携带本轮专家快照的路由。底座在真正执行 `agent(profile=...)` 前会
    /// 从 `ResolvedRuntimeRoute.config.fleet` 重新构造名册，因此只更新
    /// `EngineConfig.fleet_roster` 不足以支持 execution != ledger 的 Code 会话。
    pub(crate) fn resolve_multi_agent_runtime_route_for_model(
        &self,
        model: &str,
        snapshot: &ExpertRosterSnapshot,
    ) -> Result<deepseek_tui::route_runtime::ResolvedRuntimeRoute> {
        let config = self.build_multi_agent_dt_config(snapshot);
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
        session_id: &str,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
    ) -> Result<Op> {
        self.ensure_session_skills_for_send(session_id);
        self.build_send_message_op_with_hooks(
            session_id,
            content,
            mode,
            persona_reminder,
            restrict_tools,
            self.build_hook_executor(),
            None,
        )
    }

    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn build_eval_send_message_op(
        &self,
        session_id: &str,
        content: String,
        policy: &crate::features::assistant::product_runtime::eval_tool_policy::EvalTurnPolicy,
    ) -> Result<Op> {
        self.ensure_session_skills_for_send(session_id);
        let allowed_tools = policy
            .allowed_tools
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        let exact =
            deepseek_tui::core::ops::ExactToolDispatchPolicy::try_new(allowed_tools.clone())
                .map_err(anyhow::Error::msg)?;
        let model = self.model();
        Ok(Op::SendMessage {
            content,
            mode: AppMode::Agent,
            route: Box::new(self.resolve_runtime_route_for_model(&model)?),
            compaction: Box::new(self.compaction_config_for_model(&model)),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: deepseek_tui::tools::goal::GoalStatus::Active,
            reasoning_effort: self.request_reasoning_effort(),
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: deepseek_tui::tui::approval::ApprovalMode::Never,
            translation_enabled: false,
            allowed_tools: Some(allowed_tools),
            hook_executor: None,
            verbosity: None,
            dynamic_tools: Vec::new(),
            provenance: deepseek_tui::core::ops::UserInputProvenance::ImportedTranscript,
            turn_tool_security: Some(Arc::new(
                deepseek_tui::core::ops::TurnToolSecurityPolicy::new(Some(Vec::new()), Some(exact))
                    .with_read_only_dispatch(),
            )),
        })
    }

    /// 多智能体会话每轮都必须重新携带专用 hook；底座的 `SendMessage` 会覆盖
    /// EngineConfig 上的 hook executor，只在启动配置里设置一次并不生效。
    pub(crate) fn build_multi_agent_send_message_op(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
        workspace: &std::path::Path,
        snapshot: &ExpertRosterSnapshot,
    ) -> Result<Op> {
        self.ensure_session_skills_for_send(session_id);
        self.build_send_message_op_with_hooks(
            session_id,
            content,
            mode,
            persona_reminder,
            restrict_tools,
            self.build_multi_agent_hook_executor(workspace),
            Some(snapshot),
        )
    }

    fn ensure_session_skills_for_send(&self, session_id: &str) {
        // 发送路径自愈（skill 双 scope 治理 §2.3.3）：组合目录缺失时按当前 scope
        // 重建（微秒级 stat），防手动删除后静默丢失；不做每轮全量比对（V-7/V-10）。
        let policy = self.session_policy(session_id);
        let project_workspace = self.session_roots(session_id).execution;
        crate::features::assistant::skill_materialization::ensure_session_skills(
            session_id,
            policy.mode(),
            Some(&project_workspace),
        );
    }

    fn build_send_message_op_with_hooks(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
        hook_executor: Arc<HookExecutor>,
        expert_snapshot: Option<&ExpertRosterSnapshot>,
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
        // mode 维度的 per-turn reminder(砍 PlanPhase 后只剩 mode 维度):Plan 经会话
        // 策略产出(D-2,本期两模式同文);其余 mode 无 reminder——Yolo 大产物分块实测
        // 不再 load-bearing 已砍(只剩 sudo 动态状态),Agent pinvou3 不暴露。
        // 命中率优先于优雅:每段都是命令式、短、列禁令清单(Qwen3.6 友好)。
        let policy = self.session_policy(session_id);
        // plan_reminder() 仅 Plan 产出 Some;原两步 match 的 `Some(r) => format!(…sudo)`
        // 分支要求 Some 且 mode≠Plan,永不命中,故合并为单 match 消除死分支。
        let mut reminder_body = match mode {
            // Plan: 只读无 exec,只注入 mode reminder(不混 sudo)。
            AppMode::Plan => policy
                .plan_reminder()
                .map(str::to_string)
                .unwrap_or_else(|| sudo.to_string()),
            // 其余 mode: 无 per-turn reminder,只注入动态 sudo 状态。
            AppMode::Yolo | AppMode::Agent | AppMode::Auto | AppMode::Operate => sudo.to_string(),
        };
        // 卡片池: 该 session 加持了专家面具时,每 turn 注入 persona 人设(粘性身份)。
        if let Some(persona) = persona_reminder {
            reminder_body = format!("{reminder_body}\n\n{persona}");
        }
        let full_content =
            format!("<system-reminder>\n{reminder_body}\n</system-reminder>\n\n{content}");
        let model = self.model();
        // 审批参数经会话策略产出(R-2),与 reminder 同一 policy 来源。
        let (auto_approve, approval_mode) = policy.approval_params();
        let route = match expert_snapshot {
            Some(snapshot) => self.resolve_multi_agent_runtime_route_for_model(&model, snapshot)?,
            None => self.resolve_runtime_route_for_model(&model)?,
        };
        Ok(Op::SendMessage {
            content: full_content,
            // v0.9.5 官方方案:图片以 `[Attached image: <path>]` 标记行内嵌在
            // content 里,由底座 image_attach 展开为 ImageUrl 块并按其 route
            // 能力剥离;无需结构化 input 字段。
            mode,
            route: Box::new(route),
            compaction: Box::new(self.compaction_config_for_model(&model)),
            goal_objective: None,
            // v0.8.59 上游新增 /goal 目标管理;pinvou3 GUI 不用,取默认(无预算/Active)。
            goal_token_budget: None,
            goal_status: deepseek_tui::tools::goal::GoalStatus::Active,
            // 本地 vLLM 关 thinking（防 SSE timeout）；其余默认 high。
            reasoning_effort: self.request_reasoning_effort(),
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell,
            trust_mode,
            // 审批参数按会话策略取数(R-2):本期两模式同为全自动+Auto,与此前写死
            // 值一致;S-1 安全分化落地时改 SessionPolicy::approval_params 即可。
            auto_approve,
            approval_mode,
            translation_enabled: false,

            // v0.8.49 上游新增。Some(空表) = 本轮零工具:底座 filter_tool_catalog_for_gates
            // 直接从发给模型的 schema 里 retain 掉全部工具,模型根本看不到 write_file /
            // present_artifact 等。卡牌制造专家等"纯对话元卡"用它,从工具层杜绝小模型误走
            // 写文件路径、产出无法收藏的产物卡(不靠模型自觉遵守 prompt 硬规则)。None = 不
            // 限制,沿用 engine 全量工具表。判定源 = 每 turn 实时 active_persona(engine_pool
            // 解析后经 restrict_tools 传入),戴上即限 / 卸下即恢复,无持久状态。
            allowed_tools: if restrict_tools {
                Some(Vec::new())
            } else {
                Some(crate::features::assistant::tool_policy::allowed_tool_names())
            },
            // 底座会用这里的值覆盖 Engine 级 hook_executor；必须每轮显式携带，
            // 否则 ToolCallBefore 防火墙会在第一条消息时被 None 清掉。
            hook_executor: Some(hook_executor),
            // v0.8.59 上游新增 concise verbosity 模式;pinvou3 GUI 走默认详尽,取 None。
            verbosity: None,
            // dynamic_tools: per-message 动态工具;pinvou3 不用,空。
            dynamic_tools: Vec::new(),
            // provenance: 消息来源。build_send_message_op 是用户内容 → ExternalUser。
            provenance: deepseek_tui::core::ops::UserInputProvenance::ExternalUser,
            turn_tool_security: None,
        })
    }
}

/// 项目规则注入链的路径归一化：canonicalize（解析 symlink/8.3 短名、统一大小写）
/// 后去掉 Windows `\\?\` verbatim 前缀——与绑定入口 `validate_codex_project_workspace`
/// 的 `platform_compat_path` 归一化同源，保证 home 与项目路径按同一形式比较
/// （`canonicalize` 的 verbatim 路径与绑定链的常规盘符路径按组件比较永不相等，
/// 不做这层归一化，家目录边界在 Windows 上是死代码）。归一化失败（路径不存在/
/// 不可访问）返回 None，调用方按 fail-closed 处理。
fn normalize_rule_boundary_path(path: &std::path::Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .map(|canonical| crate::platform::os::platform_compat_path(&canonical.to_string_lossy()))
}

/// 项目规则注入的目录链（纯函数，home 可注入以便单测）：从 `project_root` 逐级
/// 向上，到达用户家目录即停止——家目录本身不入链（`~/AGENTS.md` 等全局上下文
/// 不注入），项目根即家目录时整链为空；项目不在家目录之下时上溯到文件系统根。
/// `home` 为 None（家目录归一化失败）时 fail-closed：只返回项目根本层、不上溯。
///
/// 返回顺序 root→cwd（祖先在前、项目根最后），与 codex/claude 的项目规则注入
/// 惯例一致。
fn collect_project_rule_chain(
    project_root: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let Some(home) = home else {
        return vec![project_root.to_path_buf()];
    };
    let mut chain = Vec::new();
    let mut current = Some(project_root);
    while let Some(dir) = current {
        if dir == home {
            break;
        }
        chain.push(dir.to_path_buf());
        current = dir.parent();
    }
    chain.reverse();
    chain
}

/// `AGENTS.md` 注入前的文件类型检查：只接受普通文件，拒绝 symlink——symlink
/// 可指向工作区外任意文件（如 ~/.ssh/id_rsa），`is_file()` 会跟随 symlink，
/// 不能用于安全边界。与底座 `project_context::load_context_file` 的
/// `symlink_metadata` 防御范式对齐；文件不存在或不可读时返回 false（跳过）。
fn is_plain_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| {
            let file_type = metadata.file_type();
            file_type.is_file() && !file_type.is_symlink()
        })
        .unwrap_or(false)
}

// Plan reminder 文案与按 mode 的选择已收进 `session_policy`(D-2 策略化);
// 本模块只经 `SessionPolicy::plan_reminder` 取数。

// ─── 带脚本技能的 execpolicy 硬拦截：纯函数部分（可独立测试）───────────────

/// 单个技能目录的脚本 deny 规则（纯函数：输入目录，输出规则）。
/// 每个脚本文件生成「解释器 × 脚本完整路径」+「脚本路径直跑」两类 typed Deny
/// 规则（底座 command 匹配语义见 `skill_script_deny_rules` 的调研注释）。
fn skill_script_deny_rules_for(dir: &std::path::Path) -> Vec<codewhale_execpolicy::ToolAskRule> {
    let mut scripts = Vec::new();
    collect_script_files(dir, &mut scripts);
    scripts.sort();
    let mut rules = Vec::new();
    for script in scripts {
        for interpreter in interpreters_for_script(&script) {
            let mut rule = codewhale_execpolicy::ToolAskRule::exec_shell(format!(
                "{interpreter} {}",
                script.display()
            ));
            rule.action = codewhale_execpolicy::PermissionAction::Deny;
            rules.push(rule);
        }
        // 直跑形态（shebang / 可执行位 / 双击关联）：脚本路径本身作命令词。
        let mut direct =
            codewhale_execpolicy::ToolAskRule::exec_shell(script.display().to_string());
        direct.action = codewhale_execpolicy::PermissionAction::Deny;
        rules.push(direct);
    }
    rules
}

/// 递归收集目录内的脚本文件（按扩展名识别；跳过隐藏目录如 `.git`）。
fn collect_script_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') {
                collect_script_files(&path, out);
            }
            continue;
        }
        if !interpreters_for_script(&path).is_empty() {
            out.push(path);
        }
    }
}

/// 脚本扩展名 → 常见解释器集合（空切片 = 非脚本）。
fn interpreters_for_script(path: &std::path::Path) -> &'static [&'static str] {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("py") => &["python", "python3", "pythonw", "py"],
        Some("sh") => &["bash", "sh"],
        Some("js" | "mjs" | "cjs") => &["node"],
        Some("ps1") => &["pwsh", "powershell"],
        Some("bat" | "cmd") => &["cmd"],
        Some("rb") => &["ruby"],
        Some("pl") => &["perl"],
        _ => &[],
    }
}

#[cfg(test)]
// Tests borrow platform::paths::tests::ENV_LOCK (std Mutex) to serialize global env access;
// cargo test runs test threads in parallel, but env-writing tests are mutually serialized, and
// the lock is held across await only inside a current_thread runtime with no reentrant path,
// so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
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
                    // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
                    unsafe { std::env::set_var(name, value) };
                } else {
                    // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
                    unsafe { std::env::remove_var(name) };
                }
            }
        }
    }

    /// 获取 crate 级 ENV_LOCK 并返回 (锁 guard, EnvGuard)。
    /// 供需要写 DEEPSEEK_* 等 env 的测试使用——锁保证与所有 env 写测试串行,
    /// EnvGuard 保证退出时恢复原值。切勿在已持 ENV_LOCK 时再调用(会重入死锁)。
    fn locked_env(vars: &[&'static str]) -> (std::sync::MutexGuard<'static, ()>, EnvGuard) {
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
            runtime_model_credential: None,
            probed_context_tokens: None,
            execution_root_resolver: None,
            code_session_predicate: None,
            external_acp_session_predicate: None,
            image_analyze_always: false,
        }
    }

    #[test]
    fn multi_agent_availability_combines_product_mode_and_native_runtime() {
        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id| {
            session_id == "native-code"
        }));
        bridge.set_external_acp_session_predicate(std::sync::Arc::new(|session_id| {
            session_id == "external-acp"
        }));

        assert!(bridge.multi_agent_mode_available("work"));
        assert!(bridge.multi_agent_mode_available("native-code"));
        assert!(!bridge.multi_agent_mode_available("external-acp"));
    }

    #[test]
    fn execution_root_resolver_overrides_session_workspace_only_when_hit() {
        let mut bridge = fixture_bridge();
        let private = crate::platform::paths::session_workspace_dir("sess-plain");
        // 未注入 resolver：所有会话都用会话私有目录（现状不变）。
        assert_eq!(bridge.session_workspace("sess-plain"), private);

        let project = std::env::temp_dir().join("pinvou3-resolver-test-project");
        let hit = project.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-code-project").then(|| hit.clone())
        }));
        // 命中：绑了项目目录的原生代码会话解析到项目目录（engine 与 shell 同源）。
        assert_eq!(bridge.session_workspace("sess-code-project"), project);
        // 未命中：普通会话与临时代码会话仍回退会话私有目录。
        assert_eq!(bridge.session_workspace("sess-plain"), private);
        assert_eq!(
            bridge.session_workspace("sess-code-temp"),
            crate::platform::paths::session_workspace_dir("sess-code-temp"),
        );

        // 账本根：仅绑项目的代码会话改用会话私有目录，其余会话与执行根相同。
        let execution = std::env::temp_dir().join("pinvou3-resolver-test-execution");
        assert_eq!(
            bridge.audit_workspace("sess-code-project", &execution),
            crate::platform::paths::session_workspace_dir("sess-code-project"),
        );
        assert_eq!(bridge.audit_workspace("sess-plain", &execution), execution);
        assert_eq!(
            bridge.audit_workspace("sess-code-temp", &execution),
            execution
        );

        // session_roots() 结构体取法与上面的 workspace/audit 双取法同源
        // (原 session_roots_exposes_both_roots_for_every_session_kind 的断言):
        // 命中项目的会话 execution=项目、ledger=会话私有;其余会话两根一致。
        let roots = bridge.session_roots("sess-code-project");
        assert_eq!(roots.execution, project);
        assert_eq!(
            roots.ledger,
            crate::platform::paths::session_workspace_dir("sess-code-project")
        );
        for sid in ["sess-plain", "sess-code-temp"] {
            let roots = bridge.session_roots(sid);
            let private = crate::platform::paths::session_workspace_dir(sid);
            assert_eq!(roots.execution, private);
            assert_eq!(roots.ledger, private);
        }
    }

    #[test]
    fn code_session_project_rules_inject_root_agents_only_for_bound_code_sessions() {
        let base =
            std::env::temp_dir().join(format!("pinvou3-agents-inject-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 布局：base/project/AGENTS.md（项目根规则）、base/AGENTS.md（monorepo 根规则）。
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("AGENTS.md"), "project rules").unwrap();
        std::fs::write(base.join("AGENTS.md"), "monorepo root rules").unwrap();
        // 家目录边界的纯函数语义（含伪造家目录、项目根==家目录、归一化失败
        // fail-closed）由 project_rule_chain_stops_at_home_boundary 覆盖；
        // 这里走真实 user_home_dir() 做端到端注入验证。返回的路径已按
        // normalize_rule_boundary_path 归一化，期望值同样归一化后再比较。
        let expected_base = normalize_rule_boundary_path(&base)
            .unwrap()
            .join("AGENTS.md");
        let expected_project = normalize_rule_boundary_path(&project)
            .unwrap()
            .join("AGENTS.md");

        let mut bridge = fixture_bridge();
        let hit = project.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-code-project").then(|| hit.clone())
        }));
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code-project"
                || session_id == "sess-code-temp"
                || session_id == "sess-code-project2"
        }));

        // 绑项目的代码会话：注入 project/AGENTS.md 与 base/AGENTS.md（monorepo 根）。
        let rules = bridge.code_session_project_rules("sess-code-project");
        assert!(
            rules.iter().any(|p| p == &expected_project),
            "应注入项目根 AGENTS.md: {rules:?}"
        );
        assert!(
            rules.iter().any(|p| p == &expected_base),
            "应注入 monorepo 根 AGENTS.md: {rules:?}"
        );
        // 注入顺序 root→cwd（祖先在前、项目根最后），与 codex/claude 惯例一致。
        let position = |target: &std::path::Path| rules.iter().position(|p| p == target);
        assert!(
            position(&expected_base)
                .zip(position(&expected_project))
                .is_some_and(|(base_pos, project_pos)| base_pos < project_pos),
            "注入顺序应为 root→cwd（monorepo 根在前、项目根最后）: {rules:?}"
        );

        // 临时代码会话 / 普通会话：resolver 未命中 → 不注入。
        assert!(
            bridge
                .code_session_project_rules("sess-code-temp")
                .is_empty()
        );
        assert!(bridge.code_session_project_rules("sess-plain").is_empty());

        // 没有 AGENTS.md 的目录链不注入（project2 无规则文件）。
        let project2 = base.join("project2");
        std::fs::create_dir_all(&project2).unwrap();
        let hit2 = project2.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-code-project2").then(|| hit2.clone())
        }));
        assert!(
            !bridge
                .code_session_project_rules("sess-code-project2")
                .is_empty(),
            "project2 位于 base 下,base/AGENTS.md 应仍注入"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 家目录边界的纯函数语义：伪造 home 注入，覆盖项目在家目录之下、
    /// 项目根==家目录、家目录归一化失败（fail-closed）、项目不在家目录之下
    /// 四种情形（原实现自认「无法伪造家目录」而无边界测试，抽纯函数后可测）。
    #[test]
    fn project_rule_chain_stops_at_home_boundary() {
        let base =
            std::env::temp_dir().join(format!("pinvou3-rule-chain-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 布局：home/project/sub，另有不相关目录 other。
        let home = base.join("home");
        let project = home.join("project");
        let sub = project.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        // 纯函数假定入参已归一化；测试侧自行 canonicalize（Windows 的 temp_dir
        // 可能含 8.3 短名，不归一化会与生产侧 canonicalize 结果比歪）。
        let home = home.canonicalize().unwrap();
        let project = project.canonicalize().unwrap();
        let sub = sub.canonicalize().unwrap();

        // 项目在家目录之下：链不含家目录本身，root→cwd（祖先在前）。
        assert_eq!(
            collect_project_rule_chain(&sub, Some(&home)),
            vec![project.clone(), sub.clone()]
        );
        // 项目根即家目录：整链为空（~/AGENTS.md 不注入）。
        assert!(collect_project_rule_chain(&home, Some(&home)).is_empty());
        // 家目录归一化失败（None）：fail-closed，只留项目根本层、不上溯。
        assert_eq!(collect_project_rule_chain(&sub, None), vec![sub.clone()]);
        // 项目不在家目录之下：上溯到文件系统根，仍不含 home。
        let other = base.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let other = other.canonicalize().unwrap();
        let chain = collect_project_rule_chain(&other, Some(&home));
        assert_eq!(chain.last(), Some(&other));
        assert_eq!(
            chain.first().map(std::path::PathBuf::as_path),
            other.ancestors().last()
        );
        assert!(!chain.contains(&home));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// symlink 拒读：恶意仓库把 AGENTS.md 指到工作区外文件时不得注入
    /// （与底座 load_context_file 的 symlink_metadata 防御对齐）。
    /// Windows 创建 symlink 需管理员/开发者模式，无权限时优雅跳过而非失败。
    #[test]
    fn code_session_project_rules_rejects_symlinked_agents_md() {
        #[cfg(not(any(unix, windows)))]
        {
            eprintln!("[test] 该平台不支持创建 symlink，跳过 symlink 拒绝断言");
            return;
        }
        #[cfg(any(unix, windows))]
        {
            let base = std::env::temp_dir().join(format!(
                "pinvou3-agents-symlink-test-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            let project = base.join("project");
            std::fs::create_dir_all(&project).unwrap();
            // 工作区外的"敏感文件"与指向它的 AGENTS.md symlink。
            let outside = base.join("outside-secret.md");
            std::fs::write(&outside, "secret content").unwrap();
            let link = project.join("AGENTS.md");
            #[cfg(unix)]
            let link_result = std::os::unix::fs::symlink(&outside, &link);
            #[cfg(windows)]
            let link_result = std::os::windows::fs::symlink_file(&outside, &link);
            if let Err(err) = link_result {
                eprintln!("[test] symlink 创建失败（{err}，Windows 需管理员/开发者模式），跳过");
                let _ = std::fs::remove_dir_all(&base);
                return;
            }

            let mut bridge = fixture_bridge();
            let hit = project.clone();
            bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
                (session_id == "sess-code-project").then(|| hit.clone())
            }));
            bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
                session_id == "sess-code-project"
            }));

            // symlink 的 AGENTS.md 不得注入（项目目录下唯一的 AGENTS.md 即该 symlink）。
            let rules = bridge.code_session_project_rules("sess-code-project");
            let marker = base.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                !rules.iter().any(|p| p.to_string_lossy().contains(&marker)),
                "symlink 的 AGENTS.md 不得注入: {rules:?}"
            );

            let _ = std::fs::remove_dir_all(&base);
        }
    }

    /// 双门控：resolver 命中（存在绑定的项目目录）但 predicate 判定非原生代码
    /// 会话（如索引在、sidecar 缺失的降级会话）→ 不注入，避免项目规则泄进
    /// 普通会话提示词。
    #[test]
    fn code_session_project_rules_skip_when_resolver_hits_but_not_code_session() {
        let base =
            std::env::temp_dir().join(format!("pinvou3-agents-gate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("AGENTS.md"), "project rules").unwrap();

        let mut bridge = fixture_bridge();
        let hit = project.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-degraded").then(|| hit.clone())
        }));
        bridge.set_code_session_predicate(std::sync::Arc::new(|_session_id: &str| false));

        assert!(
            bridge
                .code_session_project_rules("sess-degraded")
                .is_empty(),
            "resolver 命中但非代码会话时不应注入项目规则"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 100KB 截断端到端：bridge 注入的超限 AGENTS.md 经底座渲染系统提示词时
    /// 按 INSTRUCTIONS_FILE_MAX_BYTES（100KB）截断并带标记。
    #[test]
    fn session_instructions_oversize_agents_md_truncated_end_to_end() {
        let base =
            std::env::temp_dir().join(format!("pinvou3-agents-trunc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        // 超过底座 100KB 上限的项目规则。
        let oversized = "a".repeat(120 * 1024);
        std::fs::write(project.join("AGENTS.md"), &oversized).unwrap();

        let mut bridge = fixture_bridge();
        let hit = project.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-code-project").then(|| hit.clone())
        }));
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code-project"
        }));

        let instructions = bridge.session_instructions("sess-code-project");
        let prompt = deepseek_tui::prompts::system_prompt_for_mode_with_context_and_skills(
            &project,
            None,
            None,
            Some(&instructions),
            None,
        );
        let flat = deepseek_tui::prompts::system_prompt_flat_text(&prompt);
        assert!(
            flat.contains("[…truncated"),
            "超过 100KB 的 AGENTS.md 应被截断并带标记"
        );
        assert!(
            !flat.contains(&oversized),
            "截断后的提示词不应包含完整 120KB 内容"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn session_instructions_append_project_rules_for_bound_code_sessions() {
        let base =
            std::env::temp_dir().join(format!("pinvou3-agents-instr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("AGENTS.md"), "project rules").unwrap();

        let mut bridge = fixture_bridge();
        let hit = project.clone();
        bridge.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            (session_id == "sess-code-project").then(|| hit.clone())
        }));
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code-project"
        }));

        let instr = bridge.session_instructions("sess-code-project");
        // 第一项 Inline（自家 prompt），随后是项目规则 File 项。
        assert!(matches!(instr[0], InstructionSource::Inline { .. }));
        let files: Vec<_> = instr
            .iter()
            .filter_map(|s| match s {
                InstructionSource::File(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert!(
            files.iter().any(|p| p
                == &normalize_rule_boundary_path(&project)
                    .unwrap()
                    .join("AGENTS.md")),
            "session instructions 应含项目 AGENTS.md: {files:?}"
        );

        // 普通会话不注入项目规则。
        let plain_instr = bridge.session_instructions("sess-plain");
        let plain_files: Vec<_> = plain_instr
            .iter()
            .filter_map(|s| match s {
                InstructionSource::File(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert!(
            !plain_files.iter().any(|p| p.ends_with("AGENTS.md")),
            "普通会话不应注入 AGENTS.md: {plain_files:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn code_session_tool_shaping_hides_present_artifact_only_for_code_sessions() {
        let mut bridge = fixture_bridge();
        // 未注入 predicate：一律按非代码会话处理；plain 无模式差量。
        let plain = vec!["kb_search".to_string()];
        assert_eq!(
            bridge.shape_disallowed_tools("sess-plain", plain.clone()),
            plain.clone()
        );

        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code-temp" || session_id == "sess-code-project"
        }));
        assert!(bridge.is_code_session("sess-code-temp"));
        assert!(bridge.is_code_session("sess-code-project"));
        assert!(!bridge.is_code_session("sess-plain"));

        // 临时与绑项目的代码会话都隐藏成品卡工具并禁用 load_skill；普通会话不受影响。
        for sid in ["sess-code-temp", "sess-code-project"] {
            let shaped = bridge.shape_disallowed_tools(sid, plain.clone());
            assert!(shaped.contains(&"mcp_pinvou3_present_artifact".to_string()));
            assert!(shaped.contains(&"load_skill".to_string()));
            assert!(shaped.contains(&"kb_search".to_string()));
            // 幂等：不重复追加。
            let twice = bridge.shape_disallowed_tools(sid, shaped);
            assert_eq!(
                twice
                    .iter()
                    .filter(|tool| *tool == "mcp_pinvou3_present_artifact")
                    .count(),
                1
            );
            assert_eq!(twice.iter().filter(|tool| *tool == "load_skill").count(), 1);
        }
        assert_eq!(
            bridge.shape_disallowed_tools("sess-plain", plain.clone()),
            plain.clone()
        );
    }

    /// 代码会话的连接器禁用集来自 code scope(独立于 plain scope):
    /// plain 禁用 weather 但 code 未初始化(默认全禁已装连接器)时,weather 仍被禁;
    /// code 显式只禁用 pptx 时,weather 恢复可用、pptx 保持禁用;非连接器禁用不受影响。
    #[test]
    fn code_session_tool_shaping_uses_code_scope_for_connectors() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let dir =
            std::env::temp_dir().join(format!("pinvou3-bridge-shape-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        // 模拟已装 weather/pptx 两个连接器(code 未初始化 → 默认全禁)。
        let installed = dir.join("marketplace").join("installed.json");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(
            &installed,
            serde_json::to_string(&["weather".to_string(), "pptx".to_string()]).unwrap(),
        )
        .unwrap();
        // model_tool_names 依赖 servers_dir 下的 manifest 才能把连接器 id 映射成工具全名。
        let servers_dir = crate::platform::paths::bundle_mcp_servers_dir();
        for (id, tool) in [("weather", "get_weather"), ("pptx", "make_pptx")] {
            let mdir = servers_dir.join(id);
            std::fs::create_dir_all(&mdir).unwrap();
            std::fs::write(
                mdir.join("manifest.json"),
                format!(
                    r#"{{"id":"{id}","name":"{id}","description":"d","version":"1","icon":"x","category":"c","mcp_tools":["{tool}"],"command":"python","args":["server.py"]}}"#
                ),
            )
            .unwrap();
        }
        // 每 scope 已装连接器映射成模型可见全名。
        let weather = crate::features::marketplace::MarketplaceManager::new()
            .model_tool_names(&["weather".to_string()]);
        let pptx = crate::features::marketplace::MarketplaceManager::new()
            .model_tool_names(&["pptx".to_string()]);
        assert_eq!(weather.len(), 1);
        assert_eq!(pptx.len(), 1);

        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code"
        }));

        use crate::features::marketplace::ConnectorScope;
        // plain 禁 weather(模拟普通会话里用户关了天气)。
        crate::features::marketplace::save_disabled_connectors_for(
            ConnectorScope::Plain,
            &["weather".to_string()],
        );
        // code scope 未初始化 → 默认全禁已装连接器。
        let tools = vec!["kb_search".to_string()];
        let shaped = bridge.shape_disallowed_tools("sess-code", tools.clone());
        assert!(shaped.contains(&weather[0]));
        assert!(shaped.contains(&pptx[0]));
        assert!(shaped.contains(&"kb_search".to_string()));
        // 代码会话整体禁用 load_skill(skill 开关是进程级全局,无法按会话生效的过渡方案)。
        assert!(shaped.contains(&"load_skill".to_string()));

        // code 显式只禁 pptx → weather 恢复,pptx 仍禁;plain 的 weather 禁用不再影响代码会话。
        crate::features::marketplace::save_disabled_connectors_for(
            ConnectorScope::Code,
            &["pptx".to_string()],
        );
        let shaped = bridge.shape_disallowed_tools("sess-code", tools.clone());
        assert!(!shaped.contains(&weather[0]));
        assert!(shaped.contains(&pptx[0]));
        assert!(shaped.contains(&"load_skill".to_string()));

        // 普通会话保留 plain scope 禁用集，无模式差量（Git 已放开）。
        let shaped = bridge.shape_disallowed_tools("sess-plain", tools.clone());
        assert_eq!(shaped, tools);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CLI 硬拦截规则集（scope 门禁的 execpolicy 通道）：按会话 scope 的被禁
    /// CLI 连接器生成二进制 deny 规则——plain 默认无规则；code 未初始化默认
    /// 全禁 4 个内置 CLI 二进制；显式开启后仅余被禁者。并钉住底座执行语义：
    /// deny 在直跑 / 链式 / wrapper 形态下都硬拒（AskForApproval::Never 也拦）。
    #[test]
    fn cli_deny_ruleset_follows_scope_disabled_connectors() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let dir =
            std::env::temp_dir().join(format!("pinvou3-bridge-clidny-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code"
        }));

        use crate::features::marketplace::ConnectorScope;
        // plain 无禁用 → 无规则。
        let rs = bridge.cli_deny_ruleset("sess-plain");
        assert!(rs.ask_rules.is_empty(), "plain 默认无 CLI deny 规则");

        // plain 禁 feishu → 仅 lark-cli deny（裸名 + .exe/.cmd 变体各一条，R4）。
        crate::features::marketplace::save_disabled_connectors_for(
            ConnectorScope::Plain,
            &["feishu".to_string()],
        );
        let rs = bridge.cli_deny_ruleset("sess-plain");
        let mut cmds: Vec<&str> = rs
            .ask_rules
            .iter()
            .filter_map(|r| r.command.as_deref())
            .collect();
        cmds.sort_unstable();
        assert_eq!(cmds, ["lark-cli", "lark-cli.cmd", "lark-cli.exe"]);
        assert!(
            rs.ask_rules
                .iter()
                .all(|r| r.action == codewhale_execpolicy::PermissionAction::Deny)
        );

        // code 未初始化 → 默认全禁 4 个内置 CLI 二进制（与连接器开关默认同语义），
        // 每个二进制发裸名 + .exe/.cmd 变体共 3 条。
        let rs = bridge.cli_deny_ruleset("sess-code");
        let mut bins: Vec<&str> = rs
            .ask_rules
            .iter()
            .filter_map(|r| r.command.as_deref())
            .collect();
        bins.sort_unstable();
        assert_eq!(
            bins,
            [
                "dws",
                "dws.cmd",
                "dws.exe",
                "lark-cli",
                "lark-cli.cmd",
                "lark-cli.exe",
                "tmeet",
                "tmeet.cmd",
                "tmeet.exe",
                "wecom-cli",
                "wecom-cli.cmd",
                "wecom-cli.exe"
            ]
        );
        assert!(
            rs.ask_rules
                .iter()
                .all(|r| r.action == codewhale_execpolicy::PermissionAction::Deny)
        );

        // code 显式只禁 dingtalk → 仅剩 dws 被硬拒（含 .exe/.cmd 变体）。
        crate::features::marketplace::save_disabled_connectors_for(
            ConnectorScope::Code,
            &["dingtalk".to_string()],
        );
        let rs = bridge.cli_deny_ruleset("sess-code");
        let mut cmds: Vec<&str> = rs
            .ask_rules
            .iter()
            .filter_map(|r| r.command.as_deref())
            .collect();
        cmds.sort_unstable();
        assert_eq!(cmds, ["dws", "dws.cmd", "dws.exe"]);

        // 底座执行语义：deny 在直跑 / 链式 / wrapper 形态下都硬拒。
        let engine = codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![rs]);
        let check = |command: &str| {
            engine
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command,
                    cwd: ".",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::Never,
                    sandbox_mode: None,
                })
                .unwrap()
        };
        for cmd in [
            "dws todo create",
            "echo hi && dws todo create",
            "bash -c \"dws todo create\"",
        ] {
            assert!(!check(cmd).allow, "{cmd} 应被 deny 规则硬拒");
        }
        // 非禁用命令不受影响（lark-cli 已被显式开启）。
        assert!(check("lark-cli im send").allow, "未禁用的 CLI 不应被拦");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows 绕过面钉死（六轮评审 R4）：被禁 CLI 以 `{bin}.exe` / `{bin}.cmd`
    /// 形式首 token 调用（Windows 常见拼写）同样被硬拒——裸二进制名规则不匹配
    /// 带扩展名的首 token，此前 `lark-cli.exe im send` 可绕过。
    #[test]
    fn cli_deny_rules_cover_exe_and_cmd_variants() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-bridge-clidny-ext-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|_| false));

        use crate::features::marketplace::ConnectorScope;
        crate::features::marketplace::save_disabled_connectors_for(
            ConnectorScope::Plain,
            &["feishu".to_string()],
        );
        let rs = bridge.cli_deny_ruleset("sess-plain");

        let engine = codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![rs]);
        let check = |command: &str| {
            engine
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command,
                    cwd: ".",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::Never,
                    sandbox_mode: None,
                })
                .unwrap()
        };
        for cmd in [
            "lark-cli im send",
            "lark-cli.exe im send",
            "lark-cli.cmd im send",
        ] {
            assert!(!check(cmd).allow, "{cmd} 应被 deny 规则硬拒");
        }
        // 前缀相似的其它二进制不受影响（word-boundary 匹配）。
        assert!(
            check("lark-cli-extra im send").allow,
            "非同名的相似前缀命令不应被拦"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 脚本 deny 规则生成（纯函数）：脚本文件 × 解释器 + 直跑路径；非脚本文件与
    /// 隐藏目录不生成规则。
    #[test]
    fn skill_script_deny_rules_cover_interpreters_and_direct_exec() {
        let dir =
            std::env::temp_dir().join(format!("pinvou3-skilldeny-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let scripts = dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let py = scripts.join("make_pptx.py");
        let sh = scripts.join("render.sh");
        std::fs::write(&py, "print(1)").unwrap();
        std::fs::write(&sh, "echo hi").unwrap();
        std::fs::write(scripts.join("notes.md"), "not a script").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("hook.py"), "x").unwrap();

        let rules = skill_script_deny_rules_for(&dir);
        // x.py: 4 解释器 + 1 直跑 = 5；y.sh: 2 + 1 = 3；notes.md / .git 无规则
        assert_eq!(rules.len(), 8, "规则数: {rules:?}");
        assert!(
            rules
                .iter()
                .all(|r| r.action == codewhale_execpolicy::PermissionAction::Deny)
        );

        // 引擎级行为：直跑 / 带参数 / 链式都硬拒（Never 模式也不放水）；
        // 其它路径的同名解释器调用不受影响。
        let engine = codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
            codewhale_execpolicy::Ruleset::user(vec![], vec![]).with_ask_rules(rules),
        ]);
        let check = |command: &str| {
            engine
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command,
                    cwd: ".",
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::Never,
                    sandbox_mode: None,
                })
                .unwrap()
        };
        let py_cmd = format!("python {}", py.display());
        assert!(!check(&py_cmd).allow, "{py_cmd} 应被硬拒");
        assert!(!check(&format!("{py_cmd} --flag")).allow, "带参数应被硬拒");
        assert!(
            !check(&format!("echo a && {py_cmd}")).allow,
            "链式段应被硬拒"
        );
        assert!(
            !check(&format!("{}", sh.display())).allow,
            "直跑脚本路径应被硬拒"
        );
        let other = scripts.join("other.py");
        assert!(
            check(&format!("python {}", other.display())).allow,
            "非禁用脚本不应被拦"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 通道③ 取数口径：scope 禁用技能（含 code 未初始化默认全禁）的脚本目录生成
    /// deny 规则；启用后规则消失；与 CLI 二进制 deny 共存于同一规则集。
    #[test]
    fn scope_deny_ruleset_covers_disabled_skill_scripts() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-bridge-scriptdny-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        // 盘上放一个带脚本的市场技能目录（SKILL.md + upload 标记 → 对已装技能
        // 集合可见，code 未初始化「默认全禁」才能覆盖它）。市场技能已迁到按包聚合
        // 的新布局 `bundles/<pkg>/skills/<name>/`（旧扁平 `bundle/skills/` 仅存内置技能）。
        let script = dir.join("bundles/my-skill/skills/my-skill/scripts/run.py");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "print(1)").unwrap();
        std::fs::write(
            dir.join("bundles/my-skill/skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("bundles/my-skill/skills/my-skill/.installed-from"),
            "upload:pkg.zip",
        )
        .unwrap();
        // 上传技能的枚举自刀十起是 BundleStore 记录驱动：code「默认全禁已装技能」
        // 依赖 installed_skill_ids → list_skills → store 记录，此处对齐生产语义
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "my-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();

        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code"
        }));
        use crate::features::marketplace::ConnectorScope;

        // scope 无禁用时：无 CLI 二进制规则、无技能脚本规则（`run.py` 类）。
        // 安全兜底规则（路径型 + 命令型）常驻，由 safety_deny_rules 测试覆盖。
        assert!(bridge
            .scope_deny_ruleset("sess-plain")
            .ask_rules
            .iter()
            .all(|r| !r.command.as_deref().is_some_and(|c| c.contains("run.py"))));
        assert!(
            bridge
                .scope_deny_ruleset("sess-plain")
                .ask_rules
                .iter()
                .any(|r| r.path.is_some()
                    && r.action == codewhale_execpolicy::PermissionAction::Deny),
            "安全兜底的 File 路径规则应常驻"
        );

        // plain 禁 my-skill → 含指向该脚本的 deny 规则
        crate::features::marketplace::skill_scope::save_disabled_skills_for(
            ConnectorScope::Plain,
            &["my-skill".to_string()],
        );
        let rs = bridge.scope_deny_ruleset("sess-plain");
        assert!(
            rs.ask_rules.iter().any(|r| r
                .command
                .as_deref()
                .is_some_and(|c| c.starts_with("python ") && c.contains("run.py"))
                && r.action == codewhale_execpolicy::PermissionAction::Deny),
            "禁用技能脚本应有 deny 规则: {:?}",
            rs.ask_rules
        );

        // 开回 → 脚本规则消失（热刷重算的同一口径；安全兜底规则仍在）
        crate::features::marketplace::skill_scope::save_disabled_skills_for(
            ConnectorScope::Plain,
            &[],
        );
        assert!(bridge
            .scope_deny_ruleset("sess-plain")
            .ask_rules
            .iter()
            .all(|r| !r.command.as_deref().is_some_and(|c| c.contains("run.py"))));

        // code 未初始化 → 默认全禁已装技能 → 同样生成脚本规则
        let rs = bridge.scope_deny_ruleset("sess-code");
        assert!(
            rs.ask_rules
                .iter()
                .any(|r| r.command.as_deref().is_some_and(|c| c.contains("run.py"))),
            "code 默认全禁应覆盖技能脚本"
        );

        let _ = std::fs::remove_dir_all(&dir);
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

    /// 原失效路径的证伪回归（hook → execpolicy 迁移）：底座 v0.9.3 起模型只调
    /// `Bash`（hook 收到 `Bash`，其 exec_shell* 门控段静默放行）。构造的会话
    /// 引擎规则集必须对 `sudo rm` / `cat /etc/shadow`（原 hook 第 3/4 段的实测
    /// 失效样本）在 Never/YOLO 语义下判 deny。
    #[test]
    fn session_exec_policy_denies_migrated_hook_targets_under_bash_tool() {
        let bridge = fixture_bridge();
        // 走生产的可注入入口，规则集组成与 scope_deny_ruleset 天然同源（唯
        // sudo 段注入「关闭态」而非读宿主盘：/etc/sudoers.d/pinvou3 存在的
        // Linux 真机（超级权限已开）会让读盘快照漏掉 sudo 规则，回归测试必须
        // 与宿主状态解耦才可重复）。
        let ruleset = bridge.scope_deny_ruleset_with(
            "sess-plain",
            crate::features::assistant::safety_deny_rules::safety_deny_rules_for(false),
        );
        let engine = codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![ruleset]);
        // Never 是最严口径（deny-always-wins 才放行不通过）；Bypass/Auto 的
        // AskForApproval 映射为 OnFailure，deny 同样短路。
        let check = |command: &str| {
            engine
                .check(codewhale_execpolicy::ExecPolicyContext {
                    command,
                    cwd: ".",
                    // 引擎把 Bash(action=run) 经 canonical_action_alias 解析为
                    // exec_shell 再查规则（engine.rs exec_shell_ask_rule_decision）。
                    tool: Some("exec_shell"),
                    path: None,
                    ask_for_approval: codewhale_execpolicy::AskForApproval::Never,
                    sandbox_mode: None,
                })
                .unwrap()
        };
        for cmd in ["sudo rm -rf /tmp/x", "cat /etc/shadow"] {
            let d = check(cmd);
            assert!(!d.allow, "原失效路径应已修复（{cmd} deny）: {}", d.reason());
            assert!(
                matches!(
                    d.requirement,
                    codewhale_execpolicy::ExecApprovalRequirement::Forbidden { .. }
                ),
                "{cmd}: {:?}",
                d.requirement
            );
        }
        // 普通命令不受影响。
        assert!(check("cat README.md").allow);
        assert!(check("git status").allow);
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
            alias: None,
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: model.to_string(),
            base_url: base_url.to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: Default::default(),
            vision_model_id: None,
            api_key: api_key.to_string(),
            credential_ref: None,
            credential_state: crate::platform::credential_store::CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        }];
        bridge.prefs.advanced.active_model_id = Some("test-model".to_string());
    }

    /// 追加一条视觉兜底模型(测试辅助):plaintext api_key 直给,绕过系统凭据库。
    fn push_vision_model(bridge: &mut Pinvou3Bridge, id: &str, model: &str, api_key: &str) {
        bridge.prefs.advanced.saved_models.push(SavedModel {
            id: id.to_string(),
            name: model.to_string(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: model.to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: Default::default(),
            vision_model_id: None,
            api_key: api_key.to_string(),
            credential_ref: None,
            credential_state: crate::platform::credential_store::CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });
    }

    /// §9.3 规则 1:主模型显式设置 vision_model_id → 用该 SavedModel 的
    /// endpoint + 凭据(不回落主模型,不读第二份明文)。
    #[test]
    fn vision_config_prefers_explicit_vision_model_id() {
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        push_vision_model(&mut bridge, "vision-1", "gpt-4o", "sk-vision");
        bridge.prefs.advanced.saved_models[0].vision_model_id = Some("vision-1".to_string());

        let config = bridge
            .resolve_vision_model_config()
            .expect("explicit vision model must resolve");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.api_key.as_deref(), Some("sk-vision"));
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );

        let engine = bridge.build_engine_config();
        assert!(engine.vision_config.is_some());
        assert!(
            engine
                .features
                .enabled(deepseek_tui::features::Feature::VisionModel)
        );
    }

    #[test]
    fn vision_endpoint_locality_reflects_vision_model_base_url() {
        // 未配置视觉模型 → None;云端视觉模型 → Some(false);loopback 视觉模型 → Some(true)。
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        assert_eq!(bridge.vision_uses_local_endpoint(), None);

        push_vision_model(&mut bridge, "vision-cloud", "gpt-4o", "sk-vision");
        bridge.prefs.advanced.saved_models[0].vision_model_id = Some("vision-cloud".to_string());
        assert_eq!(bridge.vision_uses_local_endpoint(), Some(false));

        bridge.prefs.advanced.saved_models[1].base_url = "http://127.0.0.1:8000/v1".to_string();
        assert_eq!(bridge.vision_uses_local_endpoint(), Some(true));
    }

    /// §9.3 规则 2:未设置 vision_model_id、主模型能力 Supported →
    /// 复用主模型作为 workspace 图片分析工具(保留旧的复用行为,但仅限 Supported)。
    #[test]
    fn vision_config_reuses_main_model_only_when_supported() {
        let (_lock, _env) =
            locked_env(&["DEEPSEEK_API_KEY", "DEEPSEEK_MODEL", "DEEPSEEK_BASE_URL"]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MODEL") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_BASE_URL") };
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "gpt-5.6-terra",
            "https://api.openai.com/v1",
            "sk-main",
        );

        let config = bridge
            .resolve_vision_model_config()
            .expect("supported main model must be reused as vision tool");
        assert_eq!(config.model, "gpt-5.6-terra");
        assert_eq!(config.api_key.as_deref(), Some("sk-main"));
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert!(
            bridge
                .build_engine_config()
                .features
                .enabled(deepseek_tui::features::Feature::VisionModel)
        );
    }

    /// §9.3 规则 3:主模型 Unknown/Unsupported 且未设置视觉模型 →
    /// 不注册 image_analyze(vision_config=None 且不 enable Feature::VisionModel)。
    #[test]
    fn vision_config_absent_for_unknown_or_disabled_main_model() {
        // Unknown:deepseek-v4-pro 不在内置已验证能力表。
        let mut unknown = fixture_bridge();
        set_active_model(
            &mut unknown,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        assert!(unknown.resolve_vision_model_config().is_none());
        let engine = unknown.build_engine_config();
        assert!(engine.vision_config.is_none());
        assert!(
            !engine
                .features
                .enabled(deepseek_tui::features::Feature::VisionModel)
        );

        // override Disabled:即便主模型命中内置表也不得复用。
        let mut disabled = fixture_bridge();
        set_active_model(
            &mut disabled,
            ModelPreset::OpenaiCompatible,
            "gpt-4o",
            "https://api.openai.com/v1",
            "sk-main",
        );
        disabled.prefs.advanced.saved_models[0].image_capability_override =
            prefs::ImageCapabilityOverride::Disabled;
        assert!(disabled.resolve_vision_model_config().is_none());
        assert!(disabled.build_engine_config().vision_config.is_none());
    }

    /// scheduled 例外(规则 3 回退):`image_analyze_always` 时主模型 Unknown 也
    /// 注册 image_analyze——scheduled 会话的图片硬规则要求调用该工具,未注册
    /// 会让模型反复调用不存在的工具;Unsupported 仍不注册。
    #[test]
    fn vision_config_falls_back_for_unknown_main_model_when_image_analyze_always() {
        let mut scheduled = fixture_bridge();
        set_active_model(
            &mut scheduled,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        scheduled.image_analyze_always = true;
        assert!(scheduled.resolve_vision_model_config().is_some());
        let engine = scheduled.build_engine_config();
        assert!(engine.vision_config.is_some());
        assert!(
            engine
                .features
                .enabled(deepseek_tui::features::Feature::VisionModel)
        );

        // Unsupported(手动 Disabled)即使 always 也不注册:确认不支持的模型
        // 注册了 image_analyze 只会持续调用报错,与交互会话口径一致。
        let mut unsupported = fixture_bridge();
        set_active_model(
            &mut unsupported,
            ModelPreset::OpenaiCompatible,
            "gpt-4o",
            "https://api.openai.com/v1",
            "sk-main",
        );
        unsupported.prefs.advanced.saved_models[0].image_capability_override =
            prefs::ImageCapabilityOverride::Disabled;
        unsupported.image_analyze_always = true;
        assert!(unsupported.resolve_vision_model_config().is_none());
    }

    /// §9.3 规则 1 的优雅降级:vision_model_id 失效(指向不存在/已删除的模型)
    /// 或目标模型凭据缺失 → 不注册并记 warning,不硬错、不回落主模型。
    #[test]
    fn vision_config_degrades_gracefully_on_missing_id_or_credential() {
        // id 指向不存在的模型。
        let mut ghost = fixture_bridge();
        set_active_model(
            &mut ghost,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        ghost.prefs.advanced.saved_models[0].vision_model_id = Some("ghost".to_string());
        assert!(ghost.resolve_vision_model_config().is_none());

        // 目标模型无凭据(云端 base_url + 空 key + 无 credential_ref)。
        let mut no_key = fixture_bridge();
        set_active_model(
            &mut no_key,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        push_vision_model(&mut no_key, "vision-no-key", "gpt-4o", "");
        no_key.prefs.advanced.saved_models[0].vision_model_id = Some("vision-no-key".to_string());
        assert!(no_key.resolve_vision_model_config().is_none());
    }

    /// §9.3 规则 1 的视觉候选能力口径:视觉模型**自身**的 override 不在运行时
    /// 拒绝——选择器已用识图探测闸门验证(supported 才允许选中),disabled 标记
    /// 可能是历史探测误判残留,按被选中的事实使用;文本模型混入由前端闸门挡住
    /// (曾按审阅缺口 #104 拒绝 disabled,后续轮次反转,见 `resolve_vision_model_config`
    /// 规则 1 注释)。Supported/Unknown(默认态)同样放行。
    #[test]
    fn vision_config_allows_disabled_vision_model() {
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        // 候选视觉模型默认 Pinvou(Unknown):可解析。
        push_vision_model(&mut bridge, "vision-unknown", "my-finetune-7b", "sk-vision");
        bridge.prefs.advanced.saved_models[0].vision_model_id = Some("vision-unknown".to_string());
        assert!(
            bridge.resolve_vision_model_config().is_some(),
            "Unknown 能力的候选模型应允许作为视觉兜底(用户可显式确认)"
        );

        // 候选视觉模型 override Disabled:不再拒绝——选择器已用识图探测验证
        // (supported 才允许选中),disabled 可能是历史探测误判残留(kimi-for-coding
        // 曾因探测链路 400 被回填),被选中即按实际能力使用;凭据可用即可解析。
        let mut disabled = fixture_bridge();
        set_active_model(
            &mut disabled,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        push_vision_model(&mut disabled, "vision-off", "gpt-4o", "sk-vision");
        disabled.prefs.advanced.saved_models[1].image_capability_override =
            prefs::ImageCapabilityOverride::Disabled;
        disabled.prefs.advanced.saved_models[0].vision_model_id = Some("vision-off".to_string());
        assert!(
            disabled.resolve_vision_model_config().is_some(),
            "disabled 标记不再阻断视觉兜底(探测闸门在前端,运行时按被选中事实使用)"
        );
        assert!(disabled.build_engine_config().vision_config.is_some());

        // override Enabled 的候选模型:显式确认支持,放行。
        let mut enabled = fixture_bridge();
        set_active_model(
            &mut enabled,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        push_vision_model(&mut enabled, "vision-on", "my-finetune-7b", "sk-vision");
        enabled.prefs.advanced.saved_models[1].image_capability_override =
            prefs::ImageCapabilityOverride::Enabled;
        enabled.prefs.advanced.saved_models[0].vision_model_id = Some("vision-on".to_string());
        assert!(enabled.resolve_vision_model_config().is_some());
    }

    /// §9.2 路由(阶段 D):Supported → Native(无论有无视觉模型);
    /// Unknown/Unsupported → 有可用视觉模型走 VisionToolFallback,否则 Unsupported。
    #[test]
    fn image_input_mode_routes_by_capability_and_vision_model() {
        use crate::features::assistant::image_capability::ImageInputMode;

        // Supported 主模型:无视觉模型也 Native。
        let mut native = fixture_bridge();
        set_active_model(
            &mut native,
            ModelPreset::OpenaiCompatible,
            "gpt-4o",
            "https://api.openai.com/v1",
            "sk-main",
        );
        assert_eq!(native.image_input_mode(), ImageInputMode::Native);

        // Unknown 主模型、无视觉模型 → Unsupported(发送前拒绝)。
        let mut unknown = fixture_bridge();
        set_active_model(
            &mut unknown,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        assert_eq!(unknown.image_input_mode(), ImageInputMode::Unsupported);

        // Unknown 主模型 + vision_model_id 命中可用视觉模型 → VisionToolFallback。
        let mut fallback = fixture_bridge();
        set_active_model(
            &mut fallback,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "sk-main",
        );
        push_vision_model(&mut fallback, "vision-1", "gpt-4o", "sk-vision");
        fallback.prefs.advanced.saved_models[0].vision_model_id = Some("vision-1".to_string());
        assert_eq!(
            fallback.image_input_mode(),
            ImageInputMode::VisionToolFallback
        );

        // override Enabled 的未知本地模型 → Native。
        let mut forced = fixture_bridge();
        set_active_model(
            &mut forced,
            ModelPreset::LocalVllm,
            "qwen36_35b_256k",
            "http://127.0.0.1:8000/v1",
            "",
        );
        forced.prefs.advanced.saved_models[0].image_capability_override =
            prefs::ImageCapabilityOverride::Enabled;
        assert_eq!(forced.image_input_mode(), ImageInputMode::Native);

        // override Disabled 即便命中内置表也不 Native;有视觉模型 → Fallback。
        let mut disabled = fixture_bridge();
        set_active_model(
            &mut disabled,
            ModelPreset::OpenaiCompatible,
            "gpt-4o",
            "https://api.openai.com/v1",
            "sk-main",
        );
        disabled.prefs.advanced.saved_models[0].image_capability_override =
            prefs::ImageCapabilityOverride::Disabled;
        push_vision_model(&mut disabled, "vision-2", "gpt-4o", "sk-vision");
        disabled.prefs.advanced.saved_models[0].vision_model_id = Some("vision-2".to_string());
        assert_eq!(
            disabled.image_input_mode(),
            ImageInputMode::VisionToolFallback
        );
    }

    /// v0.9.5 官方方案:图片以 `[Attached image: <path>]` 标记行内嵌在 content,
    /// reminder 直接拼在 content 前缀;标记由底座 image_attach 展开,bridge 不做
    /// 结构化处理。此处验证 reminder 前缀拼接与标记行透传。
    #[test]
    fn build_send_message_op_preserves_attach_marker_in_content() {
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op(
                "sess-plain",
                "看看这张图\n[Attached image: /tmp/shot.png]".to_string(),
                AppMode::Yolo,
                None,
                false,
            )
            .expect("resolve test route");
        let Op::SendMessage { content, .. } = op else {
            panic!("期望 SendMessage");
        };
        assert!(content.contains("<system-reminder>"));
        assert!(
            content.contains("[Attached image: /tmp/shot.png]"),
            "官方标记行必须原样透传,得到:\n{content}"
        );
    }

    #[test]
    fn known_cloud_window_fills_route_limits_and_compaction_window() {
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Qwen,
            "qwen3.7-plus",
            ModelPreset::Qwen.default_base_url(),
            "",
        );

        let saved = bridge.effective_model().expect("active cloud model");
        assert_eq!(
            saved.context_window_tokens, None,
            "云端 catalog 模型默认不要求用户手填窗口"
        );
        let limits = bridge
            .route_limits_for_model(&bridge.model())
            .expect("已知云端模型必须生成 route limits");
        assert_eq!(limits.context_tokens, Some(1_000_000));
        assert_eq!(
            bridge
                .build_engine_config()
                .active_route_limits
                .and_then(|route| route.context_tokens),
            Some(1_000_000),
            "运行状态与底座 active_route_limits 必须使用同一窗口"
        );
        assert_eq!(bridge.effective_context_window(&bridge.model()), 1_000_000);
    }

    #[test]
    fn unknown_cloud_model_does_not_gain_a_speculative_route_limit() {
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::OpenaiCompatible,
            "unknown-cloud-model",
            "https://example.com/v1",
            "",
        );

        // Never fabricate context_tokens speculatively when no window fact
        // exists; the output cap declares the window heuristic under the
        // operator-owned semantics (half of the 128K fallback = 64000) for
        // the base #5461 arm to replace the uncatalogued 8192 guess (see
        // route_limits_for_model).
        let limits = bridge
            .route_limits_for_model(&bridge.model())
            .expect("operator-owned route declares the output heuristic");
        assert_eq!(limits.context_tokens, None);
        assert_eq!(limits.output_tokens, Some(64_000));
        assert_eq!(bridge.effective_context_window(&bridge.model()), 128_000);
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

    /// §11.8/§11.9:is_local_endpoint 与发送路径同一解析口径——local_vllm preset
    /// 或有效 base_url host 为 loopback 即本机;云端/局域网地址不得误判为本机。
    #[test]
    fn is_local_endpoint_detects_loopback_and_local_vllm_preset() {
        // local_vllm preset 默认部署(127.0.0.1:8000):本机。
        let mut local_preset = fixture_bridge();
        set_active_model(
            &mut local_preset,
            ModelPreset::LocalVllm,
            "qwen36_35b_256k",
            "http://127.0.0.1:8000/v1",
            "",
        );
        assert!(local_preset.is_local_endpoint());

        // local_vllm preset 即按本地对待,即使 base_url 被改成非 loopback 地址(规格口径)。
        let mut local_preset_remote = fixture_bridge();
        set_active_model(
            &mut local_preset_remote,
            ModelPreset::LocalVllm,
            "qwen36_35b_256k",
            "http://192.168.1.10:8000/v1",
            "",
        );
        assert!(local_preset_remote.is_local_endpoint());

        // 非 local preset 但 base_url 指向 loopback(自定义本机服务):本机。
        let mut loopback_compatible = fixture_bridge();
        set_active_model(
            &mut loopback_compatible,
            ModelPreset::OpenaiCompatible,
            "custom-local-model",
            "http://[::1]:9000/v1",
            "",
        );
        assert!(loopback_compatible.is_local_endpoint());

        // 云端地址:非本机,前端应提示图片发送给服务商。
        let mut cloud = fixture_bridge();
        set_active_model(
            &mut cloud,
            ModelPreset::OpenaiCompatible,
            "gpt-4o",
            "https://api.openai.com/v1",
            "sk-main",
        );
        assert!(!cloud.is_local_endpoint());

        // 局域网地址不是 loopback(见 api_key_requirement 测试同口径):非本机。
        let mut lan = fixture_bridge();
        set_active_model(
            &mut lan,
            ModelPreset::OpenaiCompatible,
            "custom-lan-model",
            "http://192.168.1.10:8000/v1",
            "",
        );
        assert!(!lan.is_local_endpoint());
    }

    /// 128K 上下文的两种情况(客户翻车场景的正反面),端到端走 build_engine_config:
    ///  A. 真实 128K 部署——vLLM max_model_len=131072、探测成功 → 窗口正确、T 按 131072 缩。
    ///  B. 客户 bug 兜底——丢 `--served-model-name`(名字无 _Nk)+ 探测失败 → 底座 legacy
    ///     128000,T 按 128000 推导。两种都 nice 主路径活(T ≪ E),不再是写死 190K 的倒置
    ///     抖动(190K > 128K 窗口的 E,必倒置——正是客户机每 1-2 工具调用一次 Emergency 的根因)。
    #[test]
    fn forkguard_compaction_128k_scenarios() {
        let (_lock, _env) =
            locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
        // [根因] derive_compaction_threshold 经底座 context_input_budget_for_route 算
        // output 预留：本地 vLLM 的 24576 由 route_limits_for_model 的 is_local_vllm
        // 分支显式携带进 RouteLimits.output_tokens，主导预留计算（min(requested_cap,
        // route_cap)=24576），不依赖 DEEPSEEK_MAX_OUTPUT_TOKENS env。云端模型不再
        // 被品悟钉死 24576，落底座 64K 兜底。
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

    /// PR #210 回归：云端模型不再被全局 DEEPSEEK_MAX_OUTPUT_TOKENS 钉死 24576。
    /// clean env（无该 env）下云端 SavedModel.max_output_tokens 为 None →
    /// route_limits.output_tokens 必须为 None（不声明 → 底座 64K/厂商能力兜底）；
    /// 本地 vLLM 的 24576 由 is_local_vllm 分支显式携带（不依赖 env），两者都要锁。
    ///
    /// ⚠️ C 段语义（评审修正 2026-08-11）：品悟中间层确实不读该 env，但底座
    /// `effective_max_output_tokens_for_route` **优先**读它——env 残留仍会把云端
    /// **最终请求**的 max_tokens 钉回 24576。因此不能声称"残留 env 不影响云端"；
    /// 真正的防线是 release/boot 不再注入（见 lib.rs `release_env_defaults_guard`
    /// 与下方 `forkguard_boot_env_must_not_pin_global_output_cap`）。
    /// C 段只锁"中间层不被 env 污染"这一层事实，D 段沿底座公开预算链验证 env
    /// 确实生效（对应 CHANGES_REQUESTED：补沿最终预算/请求构造链的回归）。
    #[test]
    fn forkguard_cloud_route_output_not_pinned_by_global_env() {
        // The base requested_cap precedence chain is CODEWHALE_ > DEEPSEEK_ >
        // window heuristic, and provider selection also reads DEEPSEEK_PROVIDER;
        // the guard asserts exact values, so all three are isolated.
        let (_lock, _env) = locked_env(&[
            "CODEWHALE_MAX_OUTPUT_TOKENS",
            "DEEPSEEK_MAX_OUTPUT_TOKENS",
            "DEEPSEEK_PROVIDER",
            "PINVOU3_MAX_OUTPUT_TOKENS",
        ]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("CODEWHALE_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_PROVIDER") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS") };

        // A. 云端（Deepseek preset）：SavedModel.max_output_tokens=None（保存云端模型
        //    时前端存 null）→ route_limits.output_tokens=None → 底座 64K/厂商能力兜底。
        let mut cloud = fixture_bridge();
        set_active_model(
            &mut cloud,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            "",
        );
        let cloud_limits = cloud.route_limits_for_model("deepseek-v4-pro");
        let cloud_output = cloud_limits.as_ref().and_then(|l| l.output_tokens);
        assert_eq!(
            cloud_output, None,
            "clean env 下云端 route_limits.output_tokens 必须为 None（不声明，落底座兜底）"
        );

        // B. 本地 vLLM：is_local_vllm 分支显式携带 24K 预算，不依赖 env → 仍 24576。
        let mut local = fixture_bridge();
        set_active_model(
            &mut local,
            ModelPreset::LocalVllm,
            ModelPreset::LocalVllm.default_model(),
            ModelPreset::LocalVllm.default_base_url(),
            "",
        );
        let local_limits = local.route_limits_for_model(&local.model());
        assert_eq!(
            local_limits.as_ref().and_then(|l| l.output_tokens),
            Some(24_576),
            "本地 vLLM 仍显式携带 24K 预算（不依赖 DEEPSEEK_MAX_OUTPUT_TOKENS env）"
        );

        // C. env 残留（旧生产双保险未清干净 / 未来有人重新注入）：品悟中间层不读
        //    该 env（route 仍不声明）——但这只是中间层事实，底座最终预算链会读
        //    （见 D 段）。此处只锁"中间层不被 env 污染"，不能据此声称残留无害。
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576") };
        let cloud_limits_env = cloud.route_limits_for_model("deepseek-v4-pro");
        assert_eq!(
            cloud_limits_env.as_ref().and_then(|l| l.output_tokens),
            None,
            "品悟中间层不读该 env（云端 route 仍不声明）；env 影响发生在底座最终预算链（见 D 段）"
        );

        // D. 沿底座公开预算链（context_input_budget_for_route，品悟 derive_compaction_threshold
        //    同款 API）验证：env 残留 24576 会把底座 output reservation 从 clean env 的 64K
        //    压回 24K → 可用输入预算随之变大。证明"残留 env 不影响云端"不成立——真正防线是
        //    release/boot 不再注入（lib.rs release_env_defaults_guard）。用显式 256K RouteLimits
        //    （<500K 窗口才走 effective_max_output_tokens_for_route，≥500K 走 TURN 分支不读 env），
        //    deepseek-v4-pro 的 provider max_output=384K 不会钳制 64K/24K 中的任一个。
        let route_limits_256k = codewhale_config::route::RouteLimits {
            context_tokens: Some(256_000),
            input_tokens: None,
            output_tokens: None,
        };
        // 先回到 clean env 基准（C 段末尾已 set 24576）。
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
        let budget_clean = deepseek_tui::core::engine::context_input_budget_for_route(
            deepseek_tui::config::ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(route_limits_256k),
            0,
        )
        .expect("显式 256K route 必须能算出输入预算");
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576") };
        let budget_env = deepseek_tui::core::engine::context_input_budget_for_route(
            deepseek_tui::config::ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(route_limits_256k),
            0,
        )
        .expect("显式 256K route 必须能算出输入预算");
        assert!(
            budget_env > budget_clean,
            "env 残留 24576 使底座 output reservation 变小（64K→24K），输入预算必须变大：\
             clean={budget_clean} env={budget_env}"
        );
        assert_eq!(
            budget_env - budget_clean,
            65_536 - 24_576,
            "reservation 差应恰为底座 64K 兜底 − 本地 24K（clamp/headroom 两侧相同）"
        );
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
    }

    /// PR #210 守卫（第四轮评审修正 2026-08-12）：bridge boot 的 env 注入结果
    /// 不得包含输出上限 env。lib.rs `release_env_defaults_guard` 只覆盖 run() 的
    /// release env 注入路径；若未来有人在 boot 路径直接 set_var
    /// `DEEPSEEK_MAX_OUTPUT_TOKENS`, which that guard cannot see — so this test
    /// actually runs `Pinvou3Bridge::boot()` under an isolated home and asserts
    /// the final env state: no matter which boot line the injection happens
    /// on, re-injecting 24576 fails the guard.
    ///
    /// edition 2024 onwards, boot (Tauri setup, multi-threaded phase) must not
    /// write the process env at all: the `PINVOU3_SESSION_ARTIFACTS` injection
    /// has been moved up to lib.rs `startup_process_env` (the single-threaded
    /// startup window). This test also locks that boundary: after boot the key
    /// must still be unset.
    ///
    /// 第五轮评审修正 2026-08-13：此前只隔离 `HOME`——Windows 的 `user_home_dir()`
    /// 优先读 `USERPROFILE`、其次 `HOMEDRIVE`+`HOMEPATH`、最后才 `HOME`，只设 `HOME`
    /// 会让 `boot()` 的 `workspace`（= `user_home_dir()`）在 Windows 上仍指向真实
    /// 用户目录，legacy 清扫可能删除真实目录里带管理标识的文件。现把三平台 home
    /// 来源全部隔离到临时目录；临时目录命名叠加 `std::process::id()`（跨进程唯一），
    /// 并用 RAII guard 保证 boot/断言 panic 时仍回收整份解包 bundle。
    #[test]
    fn forkguard_boot_env_must_not_pin_global_output_cap() {
        // 需要写 PINVOU3_HOME / HOME / Windows home 来源（隔离目录），锁 + 恢复这些与目标 key。
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MAX_OUTPUT_TOKENS",
            "PINVOU3_MAX_OUTPUT_TOKENS",
            "PINVOU3_SESSION_ARTIFACTS",
            "PINVOU3_HOME",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
        ]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_SESSION_ARTIFACTS") };

        // RAII 清理：boot 会全量解包 bundle 到隔离 home，断言/panic 时也须回收
        // （此前仅在正常结尾 remove_dir_all，中途失败会残留整份 bundle）。
        struct TempDirGuard(std::path::PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        // 叠加 pid + 进程内原子后缀：unique_suffix 只保证单进程内唯一，双终端并发
        // cargo test 会跨进程碰撞（见 paths::tests::unique_suffix 文档）。
        let root = std::env::temp_dir().join(format!(
            "pinvou3-boot-env-guard-{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        ));
        let _temp = TempDirGuard(root.clone());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("创建隔离 home");
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        // 三平台 user_home_dir() 来源全部隔离到 root：macOS/Linux 读 HOME；Windows
        // 优先 USERPROFILE，其次 HOMEDRIVE+HOMEPATH，最后 HOME。若不隔离 Windows 的
        // 前两项，boot 的 workspace 仍指向真实用户目录，legacy 清扫会触碰真实文件。
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("HOME", &root) };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("USERPROFILE", &root) };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("HOMEDRIVE") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("HOMEPATH") };

        let bridge = super::Pinvou3Bridge::boot().expect("隔离 home 下 boot 必须成功");

        assert!(
            std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS").is_none(),
            "boot 执行后不得注入 DEEPSEEK_MAX_OUTPUT_TOKENS（会重新钉死云端输出上限）"
        );
        assert!(
            std::env::var_os("PINVOU3_MAX_OUTPUT_TOKENS").is_none(),
            "boot 执行后不得注入 PINVOU3_MAX_OUTPUT_TOKENS"
        );
        // boot must not write the process env (env writes are forbidden in the
        // multi-threaded phase; PINVOU3_SESSION_ARTIFACTS is injected by lib.rs
        // `startup_process_env` in the single-threaded startup window).
        assert!(
            std::env::var_os("PINVOU3_SESSION_ARTIFACTS").is_none(),
            "boot must not write PINVOU3_SESSION_ARTIFACTS (injection is funneled to startup_process_env)"
        );

        // bridge 先于 TempDirGuard 回收（guard 声明更早、drop 更晚），避免删目录时
        // bridge 仍持有其中的 bundle 路径。
        drop(bridge);
    }

    /// route profile 属于具体部署，不属于 vLLM/Qwen 特例。任何 OpenAI-compatible
    /// 本地引擎只要在 SavedModel 声明能力，都必须走同一预算链。
    #[test]
    fn forkguard_openai_compatible_route_uses_declared_limits() {
        let (_lock, _env) =
            locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
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
        assert_eq!(
            config.compaction.token_threshold, 45_648,
            "an unknown remote OpenAI-compatible alias follows the base window heuristic: the declared output 24576 participates (E=131072-24576-1024) instead of being crushed by the 8K conservative fallback"
        );
    }

    /// Output-cap semantics guard (follows PR #210/#216; base semantics =
    /// upstream #5461): documented cloud models stay undeclared (Documented
    /// fallback); uncatalogued models on user-configured (operator-owned)
    /// openai-compatible endpoints declare the window heuristic explicitly,
    /// replacing the base 8192 conservative guess; uncatalogued models on
    /// official endpoints stay undeclared (fail-closed).
    #[test]
    fn forkguard_cloud_models_defer_output_cap_to_base() {
        // The base requested_cap precedence chain is CODEWHALE_ > DEEPSEEK_ >
        // window heuristic, and provider selection also reads DEEPSEEK_PROVIDER;
        // the guard asserts exact values, so all three are isolated.
        let (_lock, _env) = locked_env(&[
            "CODEWHALE_MAX_OUTPUT_TOKENS",
            "DEEPSEEK_MAX_OUTPUT_TOKENS",
            "DEEPSEEK_PROVIDER",
            "PINVOU3_MAX_OUTPUT_TOKENS",
        ]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("CODEWHALE_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("DEEPSEEK_PROVIDER") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS") };

        // A. Documented cloud model (deepseek-v4-pro): declares a window, no
        // output cap.
        let mut a = fixture_bridge();
        set_active_model(
            &mut a,
            ModelPreset::Deepseek,
            "deepseek-v4-pro",
            "https://api.deepseek.com/",
            "k",
        );
        let limits_a = a
            .route_limits_for_model(&a.model())
            .expect("cloud route limits");
        assert_eq!(
            limits_a.output_tokens, None,
            "documented cloud models must not declare output_tokens"
        );

        // B. Uncatalogued model on a user-configured endpoint: declares the
        // route fact per the base window heuristic. No window fact -> half of
        // the base fallback window 128K = 64000 (identical to the base
        // effective_max_output_tokens requested_cap).
        let mut b = fixture_bridge();
        set_active_model(
            &mut b,
            ModelPreset::OpenaiCompatible,
            "totally-unregistered-cloud-model",
            "https://example.com/v1",
            "k",
        );
        let limits_b = b
            .route_limits_for_model(&b.model())
            .expect("operator-owned route limits");
        assert_eq!(
            limits_b.context_tokens, None,
            "uncatalogued models must not fabricate context without a window fact"
        );
        assert_eq!(
            limits_b.output_tokens,
            Some(64_000),
            "operator-owned uncatalogued models declare the output route fact per the window heuristic"
        );

        // C. Base new-arm guard (#5461): an explicit output route fact
        // replaces the uncatalogued 8192 guess. requested_cap=64000 (128K
        // window heuristic), route_cap=64000 -> E=128000-64000-1024=62976.
        // If the base lacks #5461 (route fact crushed by the 8192 guess),
        // E=118784 and this fails.
        let provider = b.build_dt_config().api_provider();
        let budget = deepseek_tui::core::engine::context_input_budget_for_route(
            provider,
            &b.model(),
            b.route_limits_for_model(&b.model()),
            0,
        )
        .expect("unregistered cloud route must yield a budget");
        assert_eq!(
            budget, 62_976,
            "operator-owned uncatalogued models fall back to the window heuristic, ceiling=128000-64000-1024"
        );

        // D. Uncatalogued model on an official endpoint: Pinvou declares no
        // output -> the base fail-closes to 8192 (the upstream maintainer's
        // conservative boundary: uncatalogued names on official endpoints are
        // never opened by the host).
        let mut d = fixture_bridge();
        set_active_model(
            &mut d,
            ModelPreset::Openai,
            "totally-unregistered-cloud-model",
            "https://api.openai.com/v1",
            "k",
        );
        let limits_d = d.route_limits_for_model(&d.model());
        assert!(
            limits_d
                .as_ref()
                .is_none_or(|limits| limits.output_tokens.is_none()),
            "uncatalogued models on official endpoints must not declare output_tokens (fail-closed to the base)"
        );
        // Feed the host's real route product (not a literal None) into the
        // base budget to pin "official endpoints stay undeclared" as
        // behavior: if the predicate ever over-opens, the budget drifts off
        // 118784 and this case goes red.
        let provider_d = d.build_dt_config().api_provider();
        let budget_d = deepseek_tui::core::engine::context_input_budget_for_route(
            provider_d,
            &d.model(),
            limits_d,
            0,
        )
        .expect("official unregistered route must yield a budget");
        assert_eq!(
            budget_d, 118_784,
            "uncatalogued models on official endpoints keep the base 8192 guess: 128000-8192-1024"
        );

        // E. coding_plan official managed entry: even when it rides on the
        // `OpenAI-compatible` preset (prefs normalize provider_kind to
        // coding_plan by endpoint URL without touching the preset), it must
        // stay fail-closed — uncatalogued names on official endpoints are
        // never opened by the host.
        let mut e = fixture_bridge();
        set_active_model(
            &mut e,
            ModelPreset::OpenaiCompatible,
            "totally-unregistered-cloud-model",
            "https://api.kimi.com/coding/v1",
            "k",
        );
        e.prefs.advanced.saved_models[0].provider_kind = Some("coding_plan".to_string());
        assert_eq!(
            e.route_limits_for_model(&e.model())
                .and_then(|l| l.output_tokens),
            None,
            "the coding_plan official entry must not use an operator declaration (stays fail-closed)"
        );

        // F. Degenerate/tiny explicit windows: when the output half is <2K a
        // declaration is meaningless -> stay undeclared (fail-closed); the
        // 16K boundary takes window/2=8192 normally, locking the
        // small-window window/2 convention.
        let tiny = |window: Option<u32>| {
            let mut f = fixture_bridge();
            set_active_model(
                &mut f,
                ModelPreset::OpenaiCompatible,
                "totally-unregistered-cloud-model",
                "https://example.com/v1",
                "k",
            );
            f.prefs.advanced.saved_models[0].context_window_tokens = window;
            f.route_limits_for_model(&f.model())
                .and_then(|l| l.output_tokens)
        };
        assert_eq!(
            tiny(Some(2_048)),
            None,
            "a window whose output half is under 2K must not produce a degenerate declaration"
        );
        assert_eq!(
            tiny(Some(16_384)),
            Some(8_192),
            "a 16K explicit window declares the output fact as window/2"
        );
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
            eprintln!(
                "[cloud {model}] window={win} output={output} → T={t}  E={e}  conservative={conservative}"
            );
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
            .build_send_message_op("sess-plain", "用户消息".to_string(), mode, None, false)
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
                "sess-plain",
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
            .build_send_message_op("sess-plain", "hi".to_string(), AppMode::Yolo, None, false)
            .expect("resolve test route");
        if let Op::SendMessage { content, .. } = op_none {
            assert!(!content.contains("数据库架构师"), "未加持不应注入 persona");
        }
    }

    /// gating: 纯对话元卡(restrict_tools=true)→ 本轮 allowed_tools=Some(空表)=零工具;
    /// 普通卡 / 未加持(false)→ Pinvou 基础白名单。这是卡牌制造专家"只产可收藏的内联卡、绝不
    /// 写文件"的**工具层**强制手段(底座从 schema 删工具),不靠模型自觉遵守 prompt。
    /// R-2 注:op 链路不感知会话类型,该白名单对 code 会话同样生效(前端入口见
    /// CodexAcpView.jsx `restrictTools` 注释,S-1 分化时按策略驱动)。
    #[test]
    fn build_send_message_op_restricts_tools_for_conversational_persona() {
        let bridge = fixture_bridge();
        let allowed = |restrict| match bridge
            .build_send_message_op(
                "sess-plain",
                "hi".to_string(),
                AppMode::Yolo,
                None,
                restrict,
            )
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
            Some(crate::features::assistant::tool_policy::allowed_tool_names()),
            "普通卡 / 未加持必须恢复 Pinvou 基础白名单"
        );

        // code 会话同链路生效(原 build_send_message_op_restrict_tools_also_
        // applies_to_code_sessions 的断言):op 链路不感知会话类型,S-1 分化若
        // 往这里加会话类型分支,下面两条必须报警。
        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code-project"
        }));
        let allowed_code = |restrict| match bridge
            .build_send_message_op(
                "sess-code-project",
                "hi".to_string(),
                AppMode::Yolo,
                None,
                restrict,
            )
            .expect("resolve test route")
        {
            Op::SendMessage { allowed_tools, .. } => allowed_tools,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        assert_eq!(
            allowed_code(true),
            Some(Vec::new()),
            "code 会话逐轮白名单入口必须生效(R-2)"
        );
        assert_eq!(
            allowed_code(false),
            Some(crate::features::assistant::tool_policy::allowed_tool_names()),
            "code 会话未限制时必须恢复 Pinvou 基础白名单"
        );
    }

    #[test]
    fn eval_send_message_op_isolated_from_gui_authority_and_installs_exact_policy() {
        let bridge = fixture_bridge();
        let policy =
            crate::features::assistant::product_runtime::eval_tool_policy::resolve_eval_policy(
                "pinvou-gaia-offline/v1",
            )
            .unwrap();
        let op = bridge
            .build_eval_send_message_op("eval-session", "question".into(), policy)
            .unwrap();
        let Op::SendMessage {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            allowed_tools,
            dynamic_tools,
            provenance,
            turn_tool_security,
            hook_executor,
            ..
        } = op
        else {
            panic!("expected SendMessage")
        };
        assert_eq!(mode, AppMode::Agent);
        assert!(!allow_shell);
        assert!(!trust_mode);
        assert!(!auto_approve);
        assert_eq!(
            approval_mode,
            deepseek_tui::tui::approval::ApprovalMode::Never
        );
        assert_eq!(
            allowed_tools.unwrap(),
            policy
                .allowed_tools
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>()
        );
        assert!(dynamic_tools.is_empty());
        assert_eq!(
            provenance,
            deepseek_tui::core::ops::UserInputProvenance::ImportedTranscript
        );
        let security = turn_tool_security.expect("mandatory turn security");
        assert_eq!(security.trusted_external_paths_override(), Some(&[][..]));
        assert!(security.exact_dispatch().is_some());
        assert!(security.requires_read_only_dispatch());
        assert!(
            hook_executor.is_none(),
            "restricted eval turns must not launch shell-backed hooks"
        );

        let ordinary = bridge
            .build_send_message_op("gui-session", "hello".into(), AppMode::Yolo, None, false)
            .unwrap();
        assert!(matches!(
            ordinary,
            Op::SendMessage {
                turn_tool_security: None,
                ..
            }
        ));
    }

    /// 主 agent 步数预算:未显式配置时必须复用底座 `EngineConfig::default()` 的
    /// max_steps(跟随上游调整),显式配置时 settings.json 优先。
    #[test]
    fn engine_config_reuses_base_max_steps_default_and_respects_override() {
        let mut bridge = fixture_bridge();
        let base_default = EngineConfig::default().max_steps;

        assert_eq!(
            bridge.build_engine_config().max_steps,
            base_default,
            "未显式配置时，主 agent 必须复用 CodeWhale 的 max_steps 默认值"
        );

        bridge.prefs.advanced.max_steps = Some(321);
        assert_eq!(
            bridge.build_engine_config().max_steps,
            321,
            "settings.json 中的 advanced.max_steps 必须继续覆盖底座默认值"
        );
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
            "本地 vLLM(Qwen3.6)会话初始 thinking 必须关；引擎配置层统一钉死，\
             避免子智能体继承到未预期的思考模式"
        );
        assert_eq!(cfg.locale_tag, "zh-Hans", "默认中文 locale");
        assert_eq!(
            cfg.max_subagents, 10,
            "max_subagents 默认 10：为会话级多智能体 fan-out 预留。\
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
        let (_lock, _env) =
            locked_env(&["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", "24576") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS") };
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
    /// 默认 prefs 是 Bing(国情:DDG 在大陆被 DNS 污染 + SNI 重置,完全不可达;
    /// 底座自身默认仍是 DuckDuckGo,应用侧默认 Bing 由本桥接显式注入)。
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_ALLOW_SHELL") };
        assert!(fixture_bridge().allow_shell());
    }

    #[test]
    fn allow_shell_uses_advanced_preference_without_env_override() {
        let (_lock, _env) = locked_env(&["PINVOU3_ALLOW_SHELL"]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_ALLOW_SHELL") };
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_ALLOW_SHELL", "false") };
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
        assert!(hooks.enabled, "hook executor 必须启用");
        assert!(
            hooks.hooks.iter().any(|hook| {
                hook.event == HookEvent::ToolCallBefore
                    && hook.name.as_deref() == Some("pinvou3-sensitive-firewall")
            }),
            "连接器自省纠正 hook（pinvou3-sensitive-firewall，安全段已迁 execpolicy）必须保留"
        );
        // 平台脚本命令契约(原 sensitive_firewall_hook_uses_platform_script 的断言):
        // Windows 用 PowerShell 脚本,其余平台用 bash 脚本。
        let firewall_command = hooks
            .hooks
            .iter()
            .find(|hook| hook.name.as_deref() == Some("pinvou3-sensitive-firewall"))
            .map(|hook| hook.command.as_str())
            .unwrap_or_default();
        #[cfg(windows)]
        assert!(
            firewall_command.contains("powershell.exe")
                && firewall_command.contains("deny_sensitive_paths.ps1")
                && !firewall_command.contains("bash"),
            "Windows sensitive firewall hook must use PowerShell, got: {firewall_command}"
        );
        #[cfg(not(windows))]
        assert!(
            firewall_command.starts_with("bash ")
                && firewall_command.contains("deny_sensitive_paths.sh"),
            "non-Windows sensitive firewall hook must use bash script, got: {firewall_command}"
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
            .build_send_message_op("sess-plain", "test".into(), AppMode::Yolo, None, false)
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
            "每轮消息不得清掉 ToolCallBefore hook（连接器自省纠正）"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_shell_receives_filtered_shell_env_from_runtime_services() {
        use deepseek_tui::tools::ToolContext;
        use deepseek_tui::tools::shell::BashTool;
        use deepseek_tui::tools::spec::ToolSpec;
        use serde_json::json;

        let (_lock, _env) = locked_env(&["SHELL", "XDG_RUNTIME_DIR", "OPENAI_API_KEY"]);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/4242") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("OPENAI_API_KEY", "must-not-leak") };

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
        let result = BashTool::new("Bash")
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_ALLOW_SHELL") };
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op("sess-plain", "hi".into(), AppMode::Yolo, None, false)
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
            .build_send_message_op("sess-plain", "list dir".into(), AppMode::Plan, None, false)
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_ALLOW_SHELL") };
        let bridge = fixture_bridge();
        let op = bridge
            .build_send_message_op("sess-plain", "exec ls".into(), AppMode::Plan, None, false)
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
            prompt.chars().take(200).collect::<String>()
        );
    }

    #[test]
    fn yolo_has_no_mode_reminder_plan_reminder_has_no_write_content() {
        // 大产物分块实测不再 load-bearing(397 行一次写 73.8s 不撞 timeout)→ YOLO_REMINDER 砍光,
        // Yolo 生产主路径无 mode reminder(per-turn 只剩 sudo,由 build_send_message_op 的
        // mode 匹配产出 None)。
        let bridge = fixture_bridge();
        let yolo = match bridge
            .build_send_message_op("sess-plain", "hi".into(), AppMode::Yolo, None, false)
            .expect("resolve test route")
        {
            Op::SendMessage { content, .. } => content,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        assert!(
            !yolo.contains("Plan 模式(只读调研)"),
            "Yolo 不该再有 mode reminder(大产物分块已砍)"
        );
        // Plan 仍有 reminder(经会话策略产出,D-2),但只读不写,不含任何写文件/分块内容。
        let plan = SessionPolicy::for_mode(SessionMode::Plain)
            .plan_reminder()
            .expect("plan reminder exists");
        assert!(
            !plan.contains("write_file"),
            "Plan reminder 不该含写文件/分块内容: {plan}"
        );
    }

    /// D-2 行为不变断言:Plan reminder 经会话策略产出,本期 plain/code 两模式同文——
    /// code 会话 op 注入的 reminder 与 plain 逐字节相等(R-1 才按模式分化)。
    #[test]
    fn build_send_message_op_plan_reminder_same_text_for_plain_and_code() {
        let content_of = |bridge: &Pinvou3Bridge, session_id: &str| match bridge
            .build_send_message_op(
                session_id,
                "方案调研".to_string(),
                AppMode::Plan,
                None,
                false,
            )
            .expect("resolve test route")
        {
            Op::SendMessage { content, .. } => content,
            other => panic!("期望 SendMessage,得到 {other:?}"),
        };
        let mut bridge = fixture_bridge();
        // 未注入 predicate:按 plain 缺省(与历史 reminder_for 不感知会话等价)。
        let plain = content_of(&bridge, "sess-plain");
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code"
        }));
        let code = content_of(&bridge, "sess-code");
        assert!(
            plain.contains("Plan 模式(只读调研)"),
            "Plan 模式必须注入 per-turn reminder,得到:\n{plain}"
        );
        assert_eq!(plain, code, "本期两模式 Plan reminder 必须同文(行为不变)");
    }

    /// D-2 行为断言:整形按策略数据驱动。plain 会话无模式差量（Git 已放开）；
    /// code 会话按策略追加缺席工具且幂等不重复(连接器 scope 切换
    /// 由 code_session_tool_shaping_uses_code_scope_for_connectors 覆盖)。
    #[test]
    fn shape_disallowed_tools_follows_session_policy() {
        let tools = vec!["kb_search".to_string(), "custom_disabled".to_string()];
        let mut bridge = fixture_bridge();
        // 未注入 predicate → plain:保留原禁用项，无模式差量追加。
        let plain = bridge.shape_disallowed_tools("sess-plain", tools.clone());
        assert_eq!(plain, tools.clone());
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "sess-code"
        }));
        // code:按策略追加缺席工具,保留非连接器禁用项,且不重复。
        let shaped = bridge.shape_disallowed_tools("sess-code", tools.clone());
        for kept in &tools {
            assert!(shaped.contains(kept), "非连接器禁用项应保留: {shaped:?}");
        }
        for unavailable in SessionPolicy::for_mode(SessionMode::Code).unavailable_tools() {
            assert_eq!(
                shaped
                    .iter()
                    .filter(|tool| tool.as_str() == *unavailable)
                    .count(),
                1,
                "模式缺席工具应恰好出现一次: {unavailable}"
            );
        }
        let twice = bridge.shape_disallowed_tools("sess-code", shaped);
        for unavailable in SessionPolicy::for_mode(SessionMode::Code).unavailable_tools() {
            assert_eq!(
                twice
                    .iter()
                    .filter(|tool| tool.as_str() == *unavailable)
                    .count(),
                1,
                "整形应幂等(不重复追加): {unavailable}"
            );
        }
    }

    /// 多引擎并发隔离基石(C 方案 P-no-disk 版): 两个不同 session 的 EngineConfig
    /// 必须使用不同 workspace 隔离产物,同时保持静态 instructions 前缀一致以便缓存复用。
    #[test]
    fn engine_config_for_session_keeps_isolation_without_prompt_variance() {
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

        // Inline source 的 name 会被渲染进 <instructions source="...">,所以它也是
        // system prompt 文本的一部分,不能携带 session_id。
        let inline_of = |s: &InstructionSource| -> (String, String) {
            match s {
                InstructionSource::Inline { name, content } => (name.clone(), content.clone()),
                InstructionSource::File(p) => {
                    panic!(
                        "session instructions 第一项必须是 Inline,实际为 {}",
                        p.display()
                    )
                }
            }
        };
        let (name_a, content_a) = inline_of(&cfg_a.instructions[0]);
        let (name_b, content_b) = inline_of(&cfg_b.instructions[0]);
        assert_eq!(name_a, "pinvou3:instructions");
        assert_eq!(name_a, name_b, "跨 session 的静态 source name 必须一致");
        assert_eq!(
            content_a, content_b,
            "跨 session 的静态 instructions 必须一致"
        );
        // session_id / workspace 已移出静态 content,走 per-turn <turn_meta>
        // (见 build_session_system_prompt 注释:per-session 变动进 cache 前缀会
        // 触发 vLLM prefix-cache MISS → 工具调用漂移)。session 隔离由不同 workspace
        // 和每个 session 独立的 EngineConfig/Engine 实例负责,name 仅是展示标签。
    }

    #[test]
    fn engine_config_for_session_keeps_mcp_artifacts_public() {
        // locked_env 一步获取 crate 级 ENV_LOCK + EnvGuard(保护 PINVOU3_HOME 写并恢复)。
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_SESSION_ARTIFACTS"]);
        let root = std::env::temp_dir().join(format!(
            "pinvou3-mcp-artifacts-public-{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        let public_artifacts = paths::default_session_artifacts_dir();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_SESSION_ARTIFACTS", &public_artifacts) };

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

    /// OpenaiCompatible preset 必须透传任意模型名（如自定义兼容端点模型）,
    /// 而不是回退到默认。9e296c4 模型列表化后,legacy preset+custom_* 经
    /// `migrate_models()`(每次 `UserPrefs::load()` 都跑)物化成 active SavedModel
    /// 才生效——测试显式调一次模拟之。
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
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.providers
                .as_ref()
                .and_then(|providers| providers.openai.reasoning_stream_style.as_deref()),
            None,
            "generic OpenAI-compatible routes must not guess reasoning semantics"
        );
    }

    /// Verifies the bridge-side Browser MCP gate: Work-mode sessions use a session-specific
    /// mcp.work.json, Code-mode sessions fall back to global mcp.json without a browser
    /// entry, and the unavailable-capability message is injected only into Work mode.
    #[test]
    fn browser_mcp_gating_follows_session_mode() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_SESSION_ARTIFACTS"]);
        let root = std::env::temp_dir().join(format!(
            "pinvou3-browser-gating-{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        // Sessions matching the predicate use Code mode.
        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|s| s.starts_with("sess-code")));

        // An empty temporary home lacks vendored chrome-devtools-mcp, so a deterministic
        // unavailable reason must be returned for the injection path.
        let reason = bridge.bundle.browser_unavailability_reason();
        assert!(
            reason.is_some(),
            "static probing must return an unavailable reason when prerequisites are missing"
        );

        // 1) MCP configuration: Code mode falls back to global configuration and Work mode
        //    uses a session-specific configuration. Runtime-bundle tests cover the Work-mode
        //    fallback when browser prerequisites are absent; this test asserts that Code
        //    mode never receives the Work-mode path.
        let cfg_code = bridge.build_engine_config_for_session("sess-code-1");
        assert_eq!(
            cfg_code.mcp_config_path,
            crate::platform::paths::mcp_config_path(),
            "Code-mode sessions must fall back to global mcp.json without mcp_browser_*"
        );

        // 2) System instructions: Code mode never receives the unavailable-capability
        //    message, while Work mode receives it when prerequisites are absent.
        let prompt_code = bridge.build_session_system_prompt("sess-code-1");
        assert!(
            !prompt_code.contains("## Browser capabilities unavailable"),
            "Code-mode sessions must not receive the browser-unavailable message"
        );
        assert!(
            !prompt_code.contains("## Browser capabilities"),
            "Code-mode sessions use Code instructions and must not contain Work-mode Browser capabilities"
        );
        let prompt_work = bridge.build_session_system_prompt("sess-work-1");
        assert!(
            prompt_work.contains("## Browser capabilities"),
            "Work mode must render Browser capabilities"
        );
        assert!(
            prompt_work.contains("## Browser capabilities unavailable"),
            "Work mode must inject an unavailable reason when prerequisites are missing"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn browser_mcp_hidden_for_external_acp_sessions() {
        let (_lock, _env) = locked_env(&["PINVOU3_HOME", "PINVOU3_SESSION_ARTIFACTS"]);
        let root = std::env::temp_dir().join(format!(
            "pinvou3-browser-gating-ext-{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        let mut bridge = fixture_bridge();
        bridge
            .set_external_acp_session_predicate(std::sync::Arc::new(|s| s.starts_with("sess-acp")));

        let cfg = bridge.build_engine_config_for_session("sess-acp-1");
        assert_eq!(
            cfg.mcp_config_path,
            crate::platform::paths::mcp_config_path(),
            "external ACP sessions must fall back to global mcp.json without mcp_browser_*"
        );
        assert!(
            !bridge
                .build_session_system_prompt("sess-acp-1")
                .contains("## Browser capabilities unavailable"),
            "external ACP sessions must not receive the browser-unavailable message"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn glm_coding_plan_uses_zai_provider_with_canonical_base_url() {
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
            "glm-5-turbo",
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions",
            "sk-coding",
        );
        bridge.prefs.normalize_saved_model_metadata();

        assert_eq!(bridge.provider(), "zai");
        assert_eq!(bridge.model(), "glm-5-turbo");
        assert_eq!(
            bridge.base_url(),
            "https://open.bigmodel.cn/api/coding/paas/v4"
        );
        assert_eq!(bridge.api_key(), "sk-coding");
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.api_provider(), deepseek_tui::config::ApiProvider::Zai);
        assert_eq!(cfg.default_model(), "glm-5-turbo");
        assert_eq!(
            cfg.providers
                .as_ref()
                .and_then(|providers| providers.zai.reasoning_stream_style.as_deref()),
            Some(SEPARATE_REASONING_FIELD)
        );
    }

    #[test]
    fn forkguard_zai_direct_route_survives_model_casing_mismatch() {
        // The foundation zai catalog uses GLM-5.2, while Settings and existing
        // configurations may store glm-5.2. Both spellings must resolve on the
        // direct z.ai endpoint and converge to the catalog's canonical form
        // (Pinvou/CodeWhale#18).
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        for model in ["glm-5.2", "GLM-5.2"] {
            let mut bridge = fixture_bridge();
            set_active_model(
                &mut bridge,
                ModelPreset::OpenaiCompatible,
                model,
                "https://api.z.ai/api/coding/paas/v4",
                "sk-zai",
            );
            bridge.prefs.normalize_saved_model_metadata();
            assert_eq!(
                bridge.provider(),
                "zai",
                "vendor=glm must route to the zai provider"
            );

            let route = bridge
                .resolve_runtime_route_for_model(model)
                .unwrap_or_else(|error| panic!("failed to resolve route for {model}: {error}"));
            assert_eq!(
                route.model(),
                "GLM-5.2",
                "{model} must use the catalog's canonical spelling"
            );
        }
    }

    #[test]
    fn known_reasoning_routes_preserve_provider_identity_and_stream_shape() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let cases = [
            (
                ModelPreset::Kimi,
                "kimi-k3",
                "https://api.moonshot.cn/v1",
                "moonshot",
                ApiProvider::Moonshot,
            ),
            (
                ModelPreset::Glm,
                "glm-5.2",
                "https://open.bigmodel.cn/api/paas/v4",
                "zai",
                ApiProvider::Zai,
            ),
            (
                ModelPreset::Minimax,
                "MiniMax-M3",
                "https://api.minimax.chat/v1",
                "minimax",
                ApiProvider::Minimax,
            ),
            (
                ModelPreset::Mimo,
                "mimo-v2.5-pro",
                "https://api.xiaomimimo.com/v1",
                "xiaomi-mimo",
                ApiProvider::XiaomiMimo,
            ),
            (
                ModelPreset::Doubao,
                "doubao-seed-evolving",
                "https://ark.cn-beijing.volces.com/api/v3",
                "volcengine",
                ApiProvider::Volcengine,
            ),
        ];

        for (preset, model, base_url, expected_provider, expected_api_provider) in cases {
            let mut bridge = fixture_bridge();
            set_active_model(&mut bridge, preset, model, base_url, "sk-test");

            assert_eq!(bridge.provider(), expected_provider, "{model}");
            let cfg = bridge.build_dt_config();
            assert_eq!(cfg.api_provider(), expected_api_provider, "{model}");
            assert_eq!(
                cfg.reasoning_effort.as_deref(),
                Some("high"),
                "{model} must default to high reasoning effort"
            );
            bridge
                .resolve_runtime_route_for_model(model)
                .unwrap_or_else(|error| panic!("{model} route must resolve: {error}"));
            let style =
                match expected_api_provider {
                    ApiProvider::Moonshot => cfg
                        .providers
                        .as_ref()
                        .and_then(|providers| providers.moonshot.reasoning_stream_style.as_deref()),
                    ApiProvider::Zai => cfg
                        .providers
                        .as_ref()
                        .and_then(|providers| providers.zai.reasoning_stream_style.as_deref()),
                    ApiProvider::Minimax => cfg
                        .providers
                        .as_ref()
                        .and_then(|providers| providers.minimax.reasoning_stream_style.as_deref()),
                    ApiProvider::XiaomiMimo => cfg.providers.as_ref().and_then(|providers| {
                        providers.xiaomi_mimo.reasoning_stream_style.as_deref()
                    }),
                    ApiProvider::Volcengine => cfg.providers.as_ref().and_then(|providers| {
                        providers.volcengine.reasoning_stream_style.as_deref()
                    }),
                    _ => None,
                };
            assert_eq!(style, Some(SEPARATE_REASONING_FIELD), "{model}");
        }
    }

    #[test]
    fn coding_plan_vendor_routes_reasoning_without_model_name_heuristics() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let cases = [
            (
                "kimi-for-coding",
                "https://api.kimi.com/coding/v1/chat/completions",
                "moonshot",
                ApiProvider::Moonshot,
            ),
            (
                "tc-code-latest",
                "https://api.lkeap.cloud.tencent.com/coding/v3/chat/completions",
                "openai",
                ApiProvider::Openai,
            ),
            (
                "glm-5.2",
                "https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions",
                "openai",
                ApiProvider::Openai,
            ),
        ];

        for (model, base_url, expected_provider, expected_api_provider) in cases {
            let mut bridge = fixture_bridge();
            set_active_model(
                &mut bridge,
                ModelPreset::OpenaiCompatible,
                model,
                base_url,
                "sk-coding",
            );
            bridge.prefs.normalize_saved_model_metadata();

            assert_eq!(bridge.provider(), expected_provider, "{model}");
            let cfg = bridge.build_dt_config();
            assert_eq!(cfg.api_provider(), expected_api_provider, "{model}");
            assert_eq!(
                cfg.reasoning_effort.as_deref(),
                Some("high"),
                "{model} must default to high reasoning effort"
            );
            bridge
                .resolve_runtime_route_for_model(model)
                .unwrap_or_else(|error| panic!("{model} route must resolve: {error}"));
            let style = match expected_api_provider {
                ApiProvider::Moonshot => cfg
                    .providers
                    .as_ref()
                    .and_then(|providers| providers.moonshot.reasoning_stream_style.as_deref()),
                ApiProvider::Openai => cfg
                    .providers
                    .as_ref()
                    .and_then(|providers| providers.openai.reasoning_stream_style.as_deref()),
                _ => None,
            };
            assert_eq!(style, Some(SEPARATE_REASONING_FIELD), "{model}");
        }
    }

    /// 用户显式设置的 `SavedModel.reasoning_effort` 必须覆盖 provider 默认
    /// （此处验证 off 覆盖 moonshot 默认 high），且三个注入点保持一致。
    /// 注:provider 默认值本身(moonshot→high)由
    /// known_reasoning_routes_preserve_provider_identity_and_stream_shape 的
    /// Kimi 首case覆盖;未显式设置时 request_reasoning_effort() 的默认注入
    /// 断言保留在下方本测试的 baseline 段。
    #[test]
    fn explicit_reasoning_effort_overrides_provider_default() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Kimi,
            "moonshot-v1-8k",
            "https://api.moonshot.cn/v1",
            "sk-test",
        );

        // baseline:未显式设置时 Moonshot 默认 high
        // (原 moonshot_model_defaults_to_high_reasoning_effort 的默认断言)。
        assert_eq!(bridge.request_reasoning_effort().as_deref(), Some("high"));

        bridge.prefs.advanced.saved_models[0].reasoning_effort = Some("off".to_string());

        assert_eq!(bridge.request_reasoning_effort().as_deref(), Some("off"));
        assert_eq!(
            bridge.build_dt_config().reasoning_effort.as_deref(),
            Some("off"),
            "DtConfig 注入点必须透传显式档位"
        );
        assert_eq!(
            bridge.build_engine_config().reasoning_effort.as_deref(),
            Some("off"),
            "EngineConfig 注入点必须透传显式档位"
        );
        let op = bridge
            .build_send_message_op("sess-plain", "hi".to_string(), AppMode::Yolo, None, false)
            .expect("resolve test route");
        let Op::SendMessage {
            reasoning_effort, ..
        } = op
        else {
            panic!("期望 SendMessage");
        };
        assert_eq!(
            reasoning_effort.as_deref(),
            Some("off"),
            "SendMessage 注入点必须透传显式档位"
        );
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MODEL", "env-model") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_PROVIDER", "env-provider") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_BASE_URL", "http://env:8000/v1") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };
        assert_eq!(bridge.model(), "env-model");
        assert_eq!(bridge.provider(), "env-provider");
        assert_eq!(bridge.base_url(), "http://env:8000/v1");
        assert_eq!(bridge.api_key(), "env-key");
    }

    #[test]
    fn runtime_credential_is_final_and_reaches_model_client_config() {
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
            "runtime-model",
            "https://api.openai.com/v1",
            "saved-key",
        );
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };
        bridge.runtime_model_credential =
            Some(RuntimeModelCredential::api_key("runtime-key").expect("runtime credential"));

        assert_eq!(bridge.api_key(), "runtime-key");
        let config = bridge.build_dt_config();
        assert_eq!(config.api_key.as_deref(), Some("runtime-key"));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.openai.api_key.as_deref()),
            Some("runtime-key")
        );
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "  ") };
        assert_eq!(bridge.api_key(), "saved-key");
    }

    /// DtConfig 在 OpenaiCompatible 模式下默认思考深度为 high（不强制 off）。
    #[test]
    fn remote_provider_defaults_to_high_reasoning_effort() {
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
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
    }

    /// 本地 OpenAI 兼容端点（loopback，如 LM Studio/Ollama）保持旧行为：
    /// 不注入 reasoning_effort（None），避免行为漂移。
    #[test]
    fn local_openai_compatible_endpoint_keeps_none_reasoning_effort() {
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
            "local-model",
            "http://127.0.0.1:1234/v1",
            "",
        );
        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.request_reasoning_effort(), None);
        assert_eq!(bridge.build_dt_config().reasoning_effort, None);
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
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_PROVIDER", "vllm") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_BASE_URL", "https://api.deepseek.com/") };
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("DEEPSEEK_MODEL", "deepseek-ai/DeepSeek-V4-Pro") };

        assert_eq!(bridge.provider(), "deepseek");
        assert_eq!(bridge.model(), "deepseek-v4-pro");
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.api_provider(),
            deepseek_tui::config::ApiProvider::Deepseek
        );
        assert_eq!(cfg.default_model(), "deepseek-v4-pro");
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
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
        assert_eq!(bridge.model(), "qwen3.8-max");
        assert_eq!(
            bridge.base_url(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.providers
                .as_ref()
                .and_then(|providers| providers.openai.reasoning_stream_style.as_deref()),
            Some(SEPARATE_REASONING_FIELD)
        );
    }

    /// Anthropic 模型必须走底座内建 anthropic provider(Messages 原生协议),
    /// 凭证与地址写入 providers.anthropic,不得落入 openai/vllm 表。
    #[test]
    fn anthropic_preset_routes_to_native_messages_provider() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Anthropic,
            ModelPreset::Anthropic.default_model(),
            ModelPreset::Anthropic.default_base_url(),
            "sk-ant",
        );
        bridge.prefs.advanced.saved_models[0].vendor = Some("claude".to_string());

        assert_eq!(bridge.provider(), "anthropic");
        assert_eq!(bridge.model(), "claude-sonnet-5");
        assert_eq!(bridge.base_url(), "https://api.anthropic.com/v1");
        assert_eq!(bridge.api_key(), "sk-ant");
        let cfg = bridge.build_dt_config();
        assert_eq!(
            cfg.api_provider(),
            deepseek_tui::config::ApiProvider::Anthropic
        );
        let providers = cfg.providers.as_ref().expect("providers config");
        assert_eq!(
            providers.anthropic.base_url.as_deref(),
            Some("https://api.anthropic.com/v1")
        );
        assert_eq!(providers.anthropic.api_key.as_deref(), Some("sk-ant"));
        assert_eq!(providers.openai.base_url.as_deref(), None);
        assert_eq!(providers.vllm.base_url.as_deref(), None);
        // Anthropic 的思考为原生 thinking block,不注入 OpenAI 系 reasoning 字段。
        assert_eq!(providers.anthropic.reasoning_stream_style.as_deref(), None);
    }

    /// xAI 走内建 xai provider;Gemini 无内建 provider,经官方 OpenAI
    /// 兼容端点复用 openai wire route。
    #[test]
    fn xai_and_gemini_routing() {
        let (_lock, _env) = locked_env(&[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_API_KEY",
        ]);
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Xai,
            ModelPreset::Xai.default_model(),
            ModelPreset::Xai.default_base_url(),
            "xai-key",
        );
        bridge.prefs.advanced.saved_models[0].vendor = Some("grok".to_string());

        assert_eq!(bridge.provider(), "xai");
        let cfg = bridge.build_dt_config();
        assert_eq!(cfg.api_provider(), deepseek_tui::config::ApiProvider::Xai);
        let providers = cfg.providers.as_ref().expect("providers config");
        assert_eq!(
            providers.xai.base_url.as_deref(),
            Some("https://api.x.ai/v1")
        );

        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Gemini,
            ModelPreset::Gemini.default_model(),
            ModelPreset::Gemini.default_base_url(),
            "gemini-key",
        );
        bridge.prefs.advanced.saved_models[0].vendor = Some("gemini".to_string());

        assert_eq!(bridge.provider(), "openai");
        assert_eq!(bridge.model(), "gemini-3.6-flash");
        let cfg = bridge.build_dt_config();
        let providers = cfg.providers.as_ref().expect("providers config");
        assert_eq!(
            providers.openai.base_url.as_deref(),
            Some("https://generativelanguage.googleapis.com/v1beta/openai")
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

    /// 工具面与主线持平：不得往基础配置里追加 `workflow` 禁令（复核指出
    /// 全局禁用改变了主分支已有能力）。禁用列表只来自连接器开关。
    #[test]
    fn chat_engine_config_keeps_the_workflow_tool_available() {
        let bridge = fixture_bridge();
        let cfg = bridge.build_engine_config();
        let disallowed = cfg.disallowed_tools.unwrap_or_default();
        assert!(
            !disallowed.iter().any(|name| name == "workflow"),
            "不得禁用 workflow——与主线能力持平，实际: {disallowed:?}"
        );
    }

    #[test]
    fn code_multi_agent_isolates_state_and_roster_from_the_project() {
        let mut bridge = fixture_bridge();
        bridge.set_code_session_predicate(std::sync::Arc::new(|session_id: &str| {
            session_id == "code-session"
        }));
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-multiagent-gate-{}-{:p}",
            std::process::id(),
            &bridge
        ));

        let code_workspace = root.join("project");
        let code_state_root = root.join("sessions").join("code-session").join("workspace");
        let roots = SessionRoots {
            execution: code_workspace.clone(),
            ledger: code_state_root.clone(),
        };
        let ordinary_code =
            bridge.build_engine_config_for_session_roots("code-session", roots.clone());
        assert_eq!(ordinary_code.workspace, code_workspace);
        assert_eq!(
            ordinary_code.subagent_state_root.as_deref(),
            Some(code_state_root.as_path()),
            "未开启产品开关的普通 Code Engine 也必须隔离底座委派状态"
        );

        let snapshot = ExpertRosterSnapshot::capture();
        let code = bridge.build_engine_config_for_multi_agent("code-session", roots, &snapshot);

        assert!(
            code.subagents_enabled,
            "Code 会话保持底座 agent/workflow 能力"
        );
        assert_eq!(code.workspace, code_workspace);
        assert_eq!(
            code.subagent_state_root.as_deref(),
            Some(code_state_root.as_path())
        );
        let code_has_multi_agent_guard = code.hook_executor.as_ref().is_some_and(|executor| {
            executor
                .config()
                .hooks
                .iter()
                .any(|hook| hook.name.as_deref() == Some("pinvou3-multiagent-depth-guard"))
        });
        assert!(
            code_has_multi_agent_guard,
            "Code 多智能体会话必须装配资源护栏"
        );
        assert_eq!(code.max_subagents, MULTI_AGENT_CODE_MAX_ADMITTED);
        assert_eq!(code.max_admitted_subagents, MULTI_AGENT_CODE_MAX_ADMITTED);
        assert_eq!(code.launch_concurrency, MULTI_AGENT_CODE_MAX_CONCURRENT);
        assert!(
            !code_workspace.join(".codewhale").exists(),
            "Code 会话不得向用户项目写状态或专家名册"
        );
        assert!(
            !code_state_root
                .join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR)
                .exists(),
            "Code 专家名册不得复制进会话状态根"
        );
        assert!(
            code.fleet_roster
                .get("exp-engineering-frontend-developer")
                .is_some(),
            "全局 Fleet 配置必须装入 Code 引擎"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// scheduled 会话由 SessionStore::session_roots 解析为"两个根都是该
    /// automation 的共享 workspace"（sessions/mod.rs 契约）。此处验证 EngineConfig
    /// 消费该 roots 时执行根与状态根同源，且不额外创建会话私有状态根
    /// （审计补测：scheduled 语义在 EngineConfig 层保持）。
    #[test]
    fn scheduled_roots_keep_shared_automation_workspace_in_engine_config() {
        let bridge = fixture_bridge();
        let automation = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-automation-{}-{:p}",
            std::process::id(),
            &bridge
        ));
        let roots = SessionRoots {
            execution: automation.clone(),
            ledger: automation.clone(),
        };

        let ordinary = bridge.build_engine_config_for_session_roots("sched-run", roots.clone());
        assert_eq!(ordinary.workspace, automation);
        assert_eq!(
            ordinary.subagent_state_root.as_deref(),
            Some(automation.as_path()),
            "scheduled 会话的执行根与状态根必须同源（共享 automation workspace）"
        );

        let snapshot = ExpertRosterSnapshot::capture();
        let multi_agent = bridge.build_engine_config_for_multi_agent("sched-run", roots, &snapshot);
        assert_eq!(multi_agent.workspace, automation);
        assert_eq!(
            multi_agent.subagent_state_root.as_deref(),
            Some(automation.as_path())
        );

        let _ = std::fs::remove_dir_all(automation);
    }

    /// 多智能体配置装配专家名册与专用资源护栏；工具面仍与普通对话
    /// 一致（workflow 也同样可用——不教不荐，但不禁用）。
    #[test]
    fn multi_agent_engine_config_adds_roles_and_resource_guards() {
        let bridge = fixture_bridge();
        let workspace = std::env::temp_dir().join(format!(
            "pinvou3-wf-roles-{}-{:p}",
            std::process::id(),
            &bridge
        ));
        let _ = std::fs::remove_dir_all(&workspace);

        let ordinary = bridge.build_engine_config();
        let roots = SessionRoots {
            execution: workspace.clone(),
            ledger: workspace.clone(),
        };
        let snapshot = ExpertRosterSnapshot::capture();
        let cfg = bridge.build_engine_config_for_multi_agent("ma-test", roots.clone(), &snapshot);

        assert_eq!(
            cfg.disallowed_tools, ordinary.disallowed_tools,
            "多智能体会话的禁用列表必须与普通对话一字不差"
        );
        assert_eq!(cfg.max_spawn_depth, MULTI_AGENT_MAX_SPAWN_DEPTH);
        assert_eq!(cfg.max_subagents, MULTI_AGENT_WORK_MAX_ADMITTED);
        assert_eq!(cfg.max_admitted_subagents, MULTI_AGENT_WORK_MAX_ADMITTED);
        assert_eq!(cfg.launch_concurrency, MULTI_AGENT_WORK_MAX_CONCURRENT);
        assert_ne!(
            ordinary.max_spawn_depth, cfg.max_spawn_depth,
            "普通对话应保持底座原有深度，只收紧多智能体会话"
        );
        let ordinary_has_guard = ordinary.hook_executor.as_ref().is_some_and(|executor| {
            executor
                .config()
                .hooks
                .iter()
                .any(|hook| hook.name.as_deref() == Some("pinvou3-multiagent-depth-guard"))
        });
        let multi_agent_has_guard = cfg.hook_executor.as_ref().is_some_and(|executor| {
            executor
                .config()
                .hooks
                .iter()
                .any(|hook| hook.name.as_deref() == Some("pinvou3-multiagent-depth-guard"))
        });
        assert!(!ordinary_has_guard, "普通对话不得挂载多智能体深度护栏");
        assert!(multi_agent_has_guard, "多智能体会话必须拦截深度覆盖");

        // 专家池卡片使用原生 config profiles；不得再向会话或项目播种文件。
        // 底座自带的内置成员（verifier 等）也保持可用。
        let agents_dir = workspace.join(deepseek_tui::WORKSPACE_AGENT_PROFILE_DIR);
        assert!(!agents_dir.exists(), "不得创建会话级 agents 投影目录");
        assert!(
            cfg.fleet_roster
                .get("exp-engineering-frontend-developer")
                .is_some(),
            "专家池内置前端专家应注册为可派 profile"
        );
        assert!(
            cfg.fleet_roster.get("verifier").is_some(),
            "底座内置成员应保持可用"
        );

        let mut disabled_bridge = fixture_bridge();
        disabled_bridge.prefs.advanced.max_subagents = Some(0);
        let disabled =
            disabled_bridge.build_engine_config_for_multi_agent("ma-disabled", roots, &snapshot);
        assert_eq!(disabled.max_subagents, 0, "不得抬高用户原本的禁用配置");
        assert_eq!(disabled.launch_concurrency, 0);

        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// 多智能体与普通对话保持相同模式、工具面和审批语义；每轮唯一差异是
    /// 多智能体附加直属深度护栏，且普通对话不得受影响。
    #[test]
    fn multi_agent_send_path_only_adds_resource_guard() {
        let mut bridge = fixture_bridge();
        set_active_model(
            &mut bridge,
            ModelPreset::Deepseek,
            "deepseek-v4-flash",
            "https://api.deepseek.com",
            "k",
        );
        let ordinary_op = bridge
            .build_send_message_op("plain-session", "hi".into(), AppMode::Yolo, None, false)
            .expect("build op");
        let workspace = std::env::temp_dir().join("pinvou3-multiagent-send-hook");
        let snapshot = ExpertRosterSnapshot::capture();
        let multi_agent_op = bridge
            .build_multi_agent_send_message_op(
                "multi-agent-session",
                "hi".into(),
                AppMode::Yolo,
                None,
                false,
                &workspace,
                &snapshot,
            )
            .expect("build multi-agent op");
        let deepseek_tui::core::ops::Op::SendMessage {
            mode,
            allowed_tools,
            hook_executor: ordinary_hooks,
            ..
        } = ordinary_op
        else {
            panic!("SendMessage op expected");
        };
        let deepseek_tui::core::ops::Op::SendMessage {
            mode: multi_mode,
            allowed_tools: multi_allowed_tools,
            hook_executor: multi_hooks,
            ..
        } = multi_agent_op
        else {
            panic!("multi-agent SendMessage op expected");
        };
        assert_eq!(mode, AppMode::Yolo, "会话模式原样透传，不被多智能体改写");
        assert_eq!(multi_mode, mode);
        assert_eq!(
            allowed_tools,
            Some(crate::features::assistant::tool_policy::allowed_tool_names()),
            "普通会话使用 Pinvou 基础白名单"
        );
        assert_eq!(multi_allowed_tools, allowed_tools);
        let has_hook = |executor: &Option<Arc<HookExecutor>>, name: &str| {
            executor.as_ref().is_some_and(|executor| {
                executor
                    .config()
                    .hooks
                    .iter()
                    .any(|hook| hook.name.as_deref() == Some(name))
            })
        };
        assert!(
            !has_hook(&ordinary_hooks, "pinvou3-multiagent-depth-guard"),
            "普通对话每轮不得挂载多智能体深度护栏"
        );
        assert!(
            has_hook(&multi_hooks, "pinvou3-multiagent-depth-guard"),
            "多智能体每轮必须重新携带深度护栏"
        );
        assert!(
            !has_hook(&multi_hooks, "pinvou3-workflow-approval"),
            "强制 ask Hook 已随每图必停协议退役"
        );
    }
}
