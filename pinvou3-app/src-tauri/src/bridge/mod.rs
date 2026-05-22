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
        Ok(this)
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
    /// 独立 workspace 目录。bridge 在切换 session 时调用,通过
    /// `Op::SyncSession { system_prompt }` 让 engine 拿到 session 专属的产出引导。
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

    /// 切换 session 前调用：**重写 disk 上的 `bundle/instructions.md`** 为
    /// session-specific workspace 路径。
    ///
    /// 为什么必须重写 disk:engine 的 `rehydrate_latest_canonical_state()` 会从
    /// `EngineConfig.instructions` (disk 文件路径) 重读并覆盖 session.system_prompt,
    /// 把我们通过 Op::SyncSession 传的 system_prompt 顶掉。要让 AI 看到 session-
    /// specific PINVOU3_WORKSPACE 必须改 disk 内容本身。
    ///
    /// pinvou3 是单用户单进程,disk 文件 race 不是问题。
    pub fn rewrite_instructions_for_session(&self, session_id: &str) -> std::io::Result<()> {
        let rendered = self.build_session_system_prompt(session_id);
        std::fs::write(&self.bundle.instructions_md, rendered)
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
            translation_enabled: _,
            vision_config: _,
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
            snapshots_max_workspace_bytes,
            search_provider,
            search_api_key,
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
            // Qwen3.6-35B-A3B-FP8 不是 vision 模型
            vision_config: None,
            // 上游 default 透传
            features,
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
            network_policy,
            lsp_config,
            runtime_services,
            subagent_model_overrides,
            goal_objective,
            workshop,
            snapshots_max_workspace_bytes,
            search_provider,
            search_api_key,
        }
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
             plan 必须拆成:先写骨架 → 分块 append_file 填内容 → read_file/命令验证;禁止写成\"一次编写完整文件\"。\
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
             先 `write_file` 写小骨架/占位,再用多个 `append_file` 或小范围 `edit_file` 分块填充(每块约 3-5 页/200 行以内)。\n\
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
             必须先 `write_file` 写骨架,再多次 `append_file` 分块追加,中间用 `read_file` 验证。\
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
}
