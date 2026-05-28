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
pub mod mode_state;
pub mod paths;
pub mod prefs;
pub mod review_gate;
pub mod sessions;

use std::path::PathBuf;

use anyhow::Result;
use deepseek_tui::config::{Config as DtConfig, ProvidersConfig};
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
/// 占据底座 `PROJECT_CONTEXT_FILES` 唯一一条路径的精简内容,挡掉
/// `auto_generate_context($HOME)` 产生的 500 行树状 dump。
///
/// 写到 `~/.pinvou3/workspace_context.md`(P-brand cleanup 后路径,配套底座 fork
/// patch 砍掉 PROJECT_CONTEXT_FILES 6 条品牌路径只留这一条)。设计要点:
/// - **不**自称 "Project Structure" — 避免 auto-gen 检测把它当 pinvou3 自己写的可覆盖文件
/// - 引导模型把"产出根目录"从 $HOME 转向 session 的真实 workspace(具体路径在 6a 层
///   `<instructions source=".../sessions/<sid>/...">` 段内)
/// - 重申敏感目录禁令的位置,不在这里重复展开(避免双源真相)
const PINVOU3_WORKSPACE_CONTEXT_MD: &str = "\
# pinvou3 workspace context

工作目录是 pinvou3 用户的家目录 (`$HOME`)。**这不是一个项目**——pinvou3 是本地 AI 助手 GUI,$HOME 不需要按 git repo 的方式建立项目认知。

## 产出根目录
真正的产出目录由 session 决定,具体路径见下文 `<instructions>` 段(每个 session 一份独立 workspace,新会话 = 全新空目录)。**不要**把产物写到 `$HOME` 顶层或 `~/Documents` 等用户私有目录,除非用户明确要求。

## 用户文件位置
常见: `~/Documents/` `~/Desktop/` `~/Downloads/` `~/桌面/` `~/下载/` `~/文档/`(中/英文桌面环境两种命名都可能)。用 `glob` / `file_search` 找,**不要硬猜路径**。

## 禁止行为
- **不要** `list_dir ~/` 或 `find ~/ ...` 探整个家目录——噪音大且对当前任务无意义
- 敏感目录禁读/禁写见 `<instructions>` 段 §8(`~/.ssh/`、`~/.gnupg/`、`~/.aws/`、`credentials*`、`.env` 等)。本文件**不**列敏感路径名,避免与 §8 形成双源真相
";

const LOCAL_VLLM_MODEL: &str = "qwen36_35b_256k";
// 127.0.0.1 让 .deb 装到任何机器都默认连本机 vLLM(全量包 install.sh
// 起 systemd 容器 --network host 绑 0.0.0.0:8000);本机 dev 走 run-dev.sh
// export DEEPSEEK_BASE_URL=http://10.214.74.113:8000/v1 覆盖,连开发机 GB10。
const LOCAL_VLLM_BASE_URL: &str = "http://127.0.0.1:8000/v1";
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
        let this = Self {
            prefs,
            bundle,
            workspace: paths::user_home_dir(),
        };
        this.wire_max_output_tokens_env();
        // P0-2 + P-brand cleanup: 让 workspace=$HOME 不被底座
        // `load_project_context_with_parents` 当成项目自动 walk 出 500 行 $HOME
        // 目录树注入 prompt(含 .ssh/id_ed25519 等敏感路径名)。pinvou3 抢在底座
        // auto_generate 之前往 `~/.pinvou3/workspace_context.md` 写一份精简版
        // (配套底座 fork patch 把 PROJECT_CONTEXT_FILES 砍到只剩这一条 pinvou3
        // 自家路径)。同时清理早期 `~/.codewhale/instructions.md` 残留。
        if let Err(e) = this.write_pinvou3_workspace_context_if_needed() {
            eprintln!("[pinvou3-app] write pinvou3 workspace context failed: {e}");
        }
        // C 方案(P-no-disk): 清理 legacy `~/.pinvou3/sessions/<sid>/instructions.md`
        // 残留。新版 pinvou3 用 `InstructionSource::Inline` 注入,这些文件不再被读;
        // 留着只是用户 $HOME 里看着像配置文件实际作废。清掉减少混淆。
        this.cleanup_legacy_session_instructions();
        Ok(this)
    }

    /// 扫 `~/.pinvou3/sessions/*/instructions.md` 全删 — C 方案后这些文件没用。
    /// 不动 sessions 目录里其它文件(messages 历史/artifacts/workspace 等仍要保留)。
    fn cleanup_legacy_session_instructions(&self) {
        let Ok(entries) = std::fs::read_dir(paths::sessions_root()) else {
            return;
        };
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path().join("instructions.md");
            if path.is_file() && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            eprintln!(
                "[pinvou3-app] cleaned up {removed} legacy session instructions.md files \
                 (C-fork P-no-disk: instructions now Inline in memory)"
            );
        }
    }

    /// pinvou3 精简版 `project_context`,挡住底座 `auto_generate_context` 对 $HOME 的
    /// 500 行树状 dump。**只**写 `~/.codewhale/instructions.md`(底座 PROJECT_CONTEXT_FILES
    /// 顺序 `.codewhale > .deepseek`,前者存在则后者不再读)。
    fn write_pinvou3_workspace_context_if_needed(&self) -> std::io::Result<()> {
        write_pinvou3_workspace_context_at(&self.workspace)
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
        INSTRUCTIONS_MD
            .replace("{{PINVOU3_WORKSPACE}}", &ws.to_string_lossy())
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

    /// env > prefs.advanced > 65536。
    pub fn max_output_tokens(&self) -> u32 {
        if let Ok(v) = std::env::var("PINVOU3_MAX_OUTPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                return n;
            }
        }
        self.prefs.advanced.max_output_tokens.unwrap_or(65_536)
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
            cycle,
            capacity,
            todos,
            plan_state,
            max_spawn_depth,
            network_policy: _, // pinvou3 显式构造 (见下),不透传 default(None)
            lsp_config,
            runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
            snapshots_max_workspace_bytes,
            search_provider,
            search_api_key,
            // —— v0.8.47 上游新增字段,透传 default ——
            show_thinking,
            goal_state,
            tools_always_load,
            prefer_bwrap,
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
            // 2026-05-19: 工程层硬锁 single subagent at a time。
            // 多 subagent 并发在本地单 vLLM + 弱模型下不可控 (timeout 风险),
            // 用户场景是 context isolation 而非 fan-out,单 subagent 足够。
            // 第 2 个 agent_spawn 会被 SubAgentManager.max_agents 检查 reject,
            // LLM 拿到 "Sub-agent limit reached" 自然 fallback,不死磕。
            max_subagents: self.prefs.advanced.max_subagents.unwrap_or(1),
            snapshots_enabled: false,
            memory_enabled: false,
            memory_path: paths::memory_path(),
            locale_tag: self.locale_tag().to_string(),
            strict_tool_mode: false,
            // pinvou3 中文用户已经是中文语境，不走 /translate 路径
            translation_enabled: false,
            // Qwen3.6 实测支持视觉(2026-05-28 base64 image_url 识图通过),
            // image_analyze 工具复用同一 vllm 端点/模型/key,无需独立 vision 服务。
            vision_config: Some(deepseek_tui::config::VisionModelConfig {
                model: self.model(),
                api_key: Some(
                    std::env::var("DEEPSEEK_API_KEY")
                        .unwrap_or_else(|_| LOCAL_VLLM_API_KEY.to_string()),
                ),
                base_url: Some(
                    std::env::var("DEEPSEEK_BASE_URL")
                        .unwrap_or_else(|_| LOCAL_VLLM_BASE_URL.to_string()),
                ),
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
            // turn_loop:90 的 preflight should_compact 直接吃这两个参数:
            //  - token_threshold = 200K (256K × ~78%):should_compact 真正放行 LLM 摘要的线
            //  - auto_floor_tokens = 60K:should_compact 的"低于则拒绝"下限,**不是** prune
            //    启动线 (codex round 3/4 抓出:prune_tool_results 是 compact_messages_safe
            //    内部第一步,必须 should_compact 先放行,即 ≥200K 才跑)。60K 留着仅作为
            //    极短会话防误触发的下限保护
            // ⚠️ 上游默认 token_threshold=800K,对 256K 窗口永远撞不到,**必须显式 set**
            compaction: deepseek_tui::compaction::CompactionConfig {
                model: self.model(),
                token_threshold: 200_000,
                auto_floor_tokens: 60_000,
                ..compaction
            },
            // 关 cycle 子系统 (2026-05-19 codex adversarial-review round 3 发现):
            // cycle_manager:184 算 trigger_floor 时 saturating_sub
            // reserved_response_headroom_tokens(263168) 与 256K window,
            // 对小窗口模型 floor 永远变 0, threshold.min(0)=0 → 每轮触发
            // briefing + 归档 + 重置 messages。
            // pinvou3 用 compaction 路径管 context, 不需要 cycle 重复管理。
            cycle: deepseek_tui::cycle_manager::CycleConfig {
                enabled: false,
                ..cycle
            },
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
            search_provider,
            search_api_key,
            // v0.8.47 上游新增,透传 default
            show_thinking,
            goal_state,
            tools_always_load,
            prefer_bwrap,
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

    /// 构造 deepseek-tui 顶层 [`DtConfig`]：锁定本地 vLLM + Qwen3.6 +
    /// 注入敏感目录拦截 hook。
    /// 环境变量优先（兼容 run-dev.sh 里既有的 `DEEPSEEK_*` 设置）。
    pub fn build_dt_config(&self) -> DtConfig {
        let mut cfg = DtConfig::default();
        cfg.provider =
            Some(std::env::var("DEEPSEEK_PROVIDER").unwrap_or_else(|_| "vllm".to_string()));
        cfg.api_key = Some(
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_else(|_| LOCAL_VLLM_API_KEY.to_string()),
        );
        let base_url =
            std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| LOCAL_VLLM_BASE_URL.to_string());
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
    pub fn build_send_message_op(&self, content: String, mode: AppMode, phase: PlanPhase) -> Op {
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
        let full_content = match reminder_for(mode, phase) {
            Some(r) => format!(
                "<system-reminder>\n{}\n</system-reminder>\n\n{}",
                r, content
            ),
            None => content,
        };
        Op::SendMessage {
            content: full_content,
            mode,
            model: self.model(),
            goal_objective: None,
            reasoning_effort: Some("off".to_string()),
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell,
            trust_mode,
            auto_approve: true,
            approval_mode: ApprovalMode::Auto,
            translation_enabled: false,
            // v0.8.47 上游新增;pinvou3 reasoning_effort=off 故无实际影响,取默认。
            show_thinking: true,
        }
    }
}

/// 落地 P0-2: 在 `workspace/.pinvou3/workspace_context.md` 写 pinvou3 精简版,
/// 挡住底座 `auto_generate_context` 对 $HOME 的 500 行 dump。抽成 module-level
/// free fn 是为了测试能传 fake home 跑(避免动用户真实 `~/.pinvou3`)。
///
/// 路径迁移历史: 早期用 `.codewhale/instructions.md`(底座命名约定),后随
/// P-brand cleanup 改到 `.pinvou3/workspace_context.md` —— 配套底座 fork patch
/// 把 PROJECT_CONTEXT_FILES 砍到只剩这一条 pinvou3 自家路径,prompt 里不再
/// 暴露 codewhale 品牌路径名。同时清理 legacy `.codewhale/instructions.md` 和
/// `.deepseek/instructions.md`(底座 auto-gen 内容才清,用户自定义保留)。
///
/// 写入条件(保守):
/// 1. 不存在 → 写
/// 2. 存在且前 200 字节含底座 auto-gen marker `Project Structure (Auto-generated)`
///    → 覆盖(底座 auto-gen 文件用户不会真改)
/// 3. 否则保留(用户已定制 / 别的工具写的)
fn write_pinvou3_workspace_context_at(workspace: &std::path::Path) -> std::io::Result<()> {
    let target = workspace.join(".pinvou3").join("workspace_context.md");
    let should_write = match std::fs::read_to_string(&target) {
        Err(_) => true,
        Ok(existing) => {
            let head: String = existing.chars().take(200).collect();
            head.contains("Project Structure (Auto-generated)")
        }
    };
    if should_write {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, PINVOU3_WORKSPACE_CONTEXT_MD)?;
    }

    // 清理早期版本残留 / 底座 auto-gen 残留(仅自动生成的版本,用户改过的保留)。
    for legacy in [
        workspace.join(".codewhale").join("instructions.md"),
        workspace.join(".deepseek").join("instructions.md"),
    ] {
        if let Ok(existing) = std::fs::read_to_string(&legacy) {
            let head: String = existing.chars().take(200).collect();
            let is_auto_gen = head.contains("Project Structure (Auto-generated)");
            let is_old_pinvou3 = head.contains("pinvou3 workspace context");
            if is_auto_gen || is_old_pinvou3 {
                let _ = std::fs::remove_file(&legacy);
            }
        }
    }

    Ok(())
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
             用 `edit_file` / `apply_patch`,不要用 `append_file`。\
             可选再调 `checklist_write` 拆细。\n\
             3. **禁止**在 text 里描述方案/贴代码/写\"请点【就这么干】\"等按钮引导文字——\
             方案卡片由系统在你调 update_plan 后自动展示,你写引导是死锁。\n\
             4. **禁止**调 `write_file` / `append_file` / `edit_file` / `exec_shell` / `code_execution`——\
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
             用 `edit_file` / `apply_patch`,不要用 `append_file`。\n\
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
             用 `edit_file` / `apply_patch`,不要用 `append_file`;完成后读回关键片段验证。\
             **禁止**在 text 里贴完整代码代替工具调用——磁盘上不会有文件。",
        ),
        _ => None,
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
        // clean env 路径:helper 应 set 默认 65536
        std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
        std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS");
        fixture_bridge().wire_max_output_tokens_env();
        assert_eq!(
            std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS").as_deref(),
            Ok("65536"),
            "wire helper 必须 set DEEPSEEK_MAX_OUTPUT_TOKENS=65536, 让底座 \
             effective_max_output_tokens 走 pinvou3 显式 cap"
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
        assert_eq!(cfg.locale_tag, "zh-Hans", "默认中文 locale");
        assert_eq!(
            cfg.max_subagents, 1,
            "max_subagents 必须 1：工程层锁定 single subagent at a time \
             (multi-subagent 并发在本地单 vLLM + 弱模型下不可控)。\
             改这个值要先评估 multi-subagent 测试场景"
        );
        assert_eq!(
            cfg.subagent_api_timeout.as_secs(), 300,
            "subagent_api_timeout 必须 300s。上游默认 120s 是为 DeepSeek 云端 API 设计, \
             本地 Qwen3.6 vLLM 慢推理下单 step 30-90s 很常见,120s 频繁误杀子 agent。 \
             300s 与 elapsed cap 对齐,给复杂研究类任务留出完整单步窗口。"
        );
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
        let op = bridge.build_send_message_op("hi".into(), AppMode::Yolo, PlanPhase::None);
        let (_allow_shell, trust_mode) = extract_shell_trust(op);
        assert!(trust_mode, "Yolo 模式 trust_mode 必须 true");
    }

    /// L2-6: Plan 模式 → trust_mode=true（P1 修复回归，原本是 false 导致
    /// list_dir 跨 session workspace 边界报 PathEscape）。
    #[test]
    fn bridge_plan_mode_trust_mode_true_after_p1() {
        let bridge = fixture_bridge();
        let op =
            bridge.build_send_message_op("list dir".into(), AppMode::Plan, PlanPhase::Planning);
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
        let op = bridge.build_send_message_op("exec ls".into(), AppMode::Plan, PlanPhase::Planning);
        let (allow_shell, _trust_mode) = extract_shell_trust(op);
        assert!(
            allow_shell,
            "Plan 模式 allow_shell 必须 true (tool_setup.rs 依赖此字段路由工具集)"
        );
    }

    /// L2-8: build_session_system_prompt 必须把 `{{PINVOU3_WORKSPACE}}` 占位符
    /// 替换为 session-specific 路径，且替换后的 prompt 必须含 session_id 子串
    /// （session_workspace_dir 路径形如 `<root>/<session_id>/workspace`）。
    #[test]
    fn instructions_md_session_workspace_subst() {
        let bridge = fixture_bridge();
        let session_id = "test-l2-session-9f8a-2c1b";
        let prompt = bridge.build_session_system_prompt(session_id);
        assert!(
            !prompt.contains("{{PINVOU3_WORKSPACE}}"),
            "占位符必须被替换,残留=死锁(AI 看不到真实路径)"
        );
        assert!(
            prompt.contains(session_id),
            "替换后 prompt 必须含 session_id 子串, prompt 前 200 字: {}",
            &prompt.chars().take(200).collect::<String>()
        );
    }

    #[test]
    fn instructions_md_explains_append_file_tail_only() {
        let bridge = fixture_bridge();
        let prompt = bridge.build_session_system_prompt("append-contract-session");
        assert!(
            prompt.contains("append_file` 只能追加到文件尾"),
            "全局 instructions 必须说明 append_file 是尾追加,不能当中间插入"
        );
        assert!(
            prompt.contains("edit_file` / `apply_patch"),
            "全局 instructions 必须说明中间填充/占位替换应走 edit_file 或 apply_patch"
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
                reminder.contains("edit_file` / `apply_patch"),
                "reminder 必须给中间填充/占位替换的正确工具: mode={mode:?} phase={phase:?}"
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

    /// P0-2 fork-guard: pinvou3 boot 时必须给 `workspace=$HOME` 写一份
    /// 精简 `.codewhale/instructions.md`,挡住底座 `auto_generate_context`
    /// 在 $HOME 上生成 500 行树状 dump(暴露 ~/.ssh/id_ed25519 等敏感路径名)。
    ///
    /// 覆盖三个 case: (1) 不存在 → 写;(2) 存在但是底座 auto-gen → 覆盖;
    /// (3) 存在且用户自定义 → 保留不动。
    /// P0-2 + P-brand fork-guard: pinvou3 boot 时必须给 `workspace=$HOME` 写一份
    /// 精简 `.pinvou3/workspace_context.md`(P-brand cleanup 后路径,原 `.codewhale/
    /// instructions.md` 已迁移),挡住底座 `auto_generate_context` 在 $HOME 上生成
    /// 500 行树状 dump(暴露 ~/.ssh/id_ed25519 等敏感路径名)。
    ///
    /// 覆盖 4 个 case: (1) 不存在 → 写;(2) 存在但是底座 auto-gen → 覆盖;
    /// (3) 存在且用户自定义 → 保留不动;(4) 清理 legacy `.codewhale/instructions.md`
    /// auto-gen 残留(用户自定义版本保留)。
    #[test]
    fn forkguard_writes_pinvou3_workspace_context_to_codewhale_instructions() {
        // 用 PID + nanos 造唯一临时目录,免引 tempfile 依赖。
        fn unique_tempdir(tag: &str) -> std::path::PathBuf {
            use std::time::{SystemTime, UNIX_EPOCH};
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let pid = std::process::id();
            let dir = std::env::temp_dir().join(format!(
                "pinvou3-forkguard-p0_2-{tag}-{pid}-{nanos}"
            ));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        // case 1: 不存在 → 写
        let tmp = unique_tempdir("case1");
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        let target = tmp.join(".pinvou3/workspace_context.md");
        assert!(target.exists(), "case 1: 文件不存在时必须写");
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(
            content.contains("pinvou3 workspace context"),
            "case 1: 写的内容必须是 pinvou3 精简版"
        );
        assert!(
            !content.contains("Project Structure (Auto-generated)"),
            "case 1: 不能写底座 auto-gen 标识(否则下次启动认成可覆盖)"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // case 2: 存在 + 底座 auto-gen marker → 覆盖
        let tmp = unique_tempdir("case2");
        let target = tmp.join(".pinvou3/workspace_context.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(
            &target,
            "# Project Structure (Auto-generated)\n\n> Old 500-line tree dump...\n",
        )
        .unwrap();
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(
            content.contains("pinvou3 workspace context"),
            "case 2: 底座 auto-gen 版本必须被 pinvou3 精简版覆盖"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // case 3: 存在 + 用户自定义内容 → 不动
        let tmp = unique_tempdir("case3");
        let target = tmp.join(".pinvou3/workspace_context.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let user_custom = "# 我自己写的项目规则\n\n绝对不要碰 src/legacy/\n";
        std::fs::write(&target, user_custom).unwrap();
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            content, user_custom,
            "case 3: 用户自定义内容必须保留,不能被 pinvou3 覆盖"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // case 4: legacy `.codewhale/instructions.md` (auto-gen 或早期 pinvou3 自己写的)
        // 必须被清理;但用户自定义 `.codewhale/instructions.md` 保留。
        let tmp = unique_tempdir("case4");
        let legacy_codewhale = tmp.join(".codewhale/instructions.md");
        std::fs::create_dir_all(legacy_codewhale.parent().unwrap()).unwrap();
        // (4a) auto-gen 残留 → 删
        std::fs::write(
            &legacy_codewhale,
            "# Project Structure (Auto-generated)\n\n> Old dump...\n",
        )
        .unwrap();
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        assert!(
            !legacy_codewhale.exists(),
            "case 4a: legacy 底座 auto-gen `.codewhale/instructions.md` 必须被清理"
        );
        // (4b) pinvou3 早期写的版本 → 删
        std::fs::write(
            &legacy_codewhale,
            "# pinvou3 workspace context\n\n旧版 pinvou3 写在 .codewhale 路径\n",
        )
        .unwrap();
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        assert!(
            !legacy_codewhale.exists(),
            "case 4b: 早期 pinvou3 自己写到 `.codewhale/instructions.md` 的版本必须被清理"
        );
        // (4c) 用户自定义 `.codewhale/instructions.md` → 留(不动用户私有)
        std::fs::write(
            &legacy_codewhale,
            "# 我手动写在 .codewhale 的内容\n\n保留不动\n",
        )
        .unwrap();
        super::write_pinvou3_workspace_context_at(&tmp).unwrap();
        assert!(
            legacy_codewhale.exists(),
            "case 4c: 用户自定义 `.codewhale/instructions.md` 必须保留(不能删用户私有)"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
