//! pinvou3 内置 bundle：随 app 编译进去的 instructions.md / mcp.json / skills 模板，
//! 首次启动时解包到 `~/.pinvou3/bundle/`。
//!
//! 与 user/ 严格分离：bundle/ 每次升级被覆写，user/ 永远不动。
//! 解包用 `bundle/VERSION` 比对 [`BUNDLE_VERSION`]，相同则跳过。

use std::path::PathBuf;

use include_dir::{include_dir, Dir};

use crate::platform::paths;

/// 三省六部工作流：编译期内嵌整个目录树（roles/*.md + scripts/*.py + json）。
static SANSHENG_LIUBU_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/workflow/sansheng-liubu");

/// 飞书官方域技能（lark-*，MIT，sync 自 github.com/larksuite/cli `skills/`）：
/// 编译期内嵌整个 skills 目录树（各域 SKILL.md + references/*.md + NOTICE.md）。
/// 启动解包到 `bundle_skills_dir`，供引擎 `SkillRegistry` 发现、`load_skill` 渐进披露。
static LARK_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/skills");

/// 9 个 lark 域技能目录名(门控写/删共用)。skills_dir 下这些目录在不在
/// = 飞书技能对模型可见与否(引擎 `SkillRegistry` 扫目录)。
const LARK_SKILL_DIRS: [&str; 9] = [
    "lark-shared",
    "lark-calendar",
    "lark-doc",
    "lark-drive",
    "lark-sheets",
    "lark-im",
    "lark-task",
    "lark-wiki",
    "lark-base",
];

/// 企微官方域技能(wecomcli-*,MIT,来自 github.com/WecomTeam/wecom-cli `skills/`):
/// 编译期内嵌整个 wecom-skills 目录树。**单独放 `wecom-skills/`**(不进 `skills/`)——
/// `skills/` 整目录被 `LARK_SKILLS_DIR` 内嵌、随飞书门控解包,企微若混进去会被飞书
/// 连带控制,故隔离成独立 include_dir + 独立门控。
static WECOM_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/wecom-skills");

/// 钉钉官方 mono skill(dws,Apache-2.0,来自 dingtalk-workspace-cli `dws-skills.zip`)。
/// 独立放 `dingtalk-skills/`，按钉钉连接 / 停用状态单独门控。
static DINGTALK_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/dingtalk-skills");

/// 腾讯会议官方 CLI skill(tmeet-skill,来自 WorkBuddy 腾讯会议 CLI 连接器)。
/// 独立放 `tmeet-skills/`，按腾讯会议连接 / 停用状态单独门控。
static TMEET_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/tmeet-skills");

/// 7 个企微域技能目录名(门控写 / 删共用)。
const WECOM_SKILL_DIRS: [&str; 7] = [
    "wecomcli-msg",
    "wecomcli-doc",
    "wecomcli-meeting",
    "wecomcli-schedule",
    "wecomcli-todo",
    "wecomcli-contact",
    "wecomcli-smartsheet",
];

const DINGTALK_SKILL_DIRS: [&str; 1] = ["dws"];
const TMEET_SKILL_DIRS: [&str; 1] = ["tmeet-skill"];

/// Bundle 版本号：手动 base + 自动 instructions.md 内容 hash（build.rs 注入）。
/// 改 INSTRUCTIONS_MD 时不需要 bump base —— hash 自动变，ensure_extracted 自动覆写。
/// 改其他 bundle 资源（mcp.json 默认 / skills 模板等）才需要手动 bump base。
///
/// 0.4: 加 Pinvou Review 内置 skills(pinvou-review-plan / pinvou-review-final)
/// 0.5: 下线旧版演示工作流，phase 协议随之停渲
/// 0.6: 加 present_artifact 内置 MCP server(成品卡):mcp.json 注册 + server 脚本解包
/// 0.7: 下线 Pinvou Review v2(EXIT GATE 评审被推翻,等新方案):两个 review skill
///      不再解包,既有装机的残留目录启动时清理
/// 0.8: 上线三省六部卡片流工作流(sansheng-liubu):include_dir 内嵌 + 启动解包
/// 注:「视觉设计」内置 skill 在 VERSION gate 之前由 write_builtin_skills 每启动防御性写出,
///     不依赖版本号 bump(同 write_workflows / write_mcp_servers）。
/// 0.10: 接入飞书官方域技能(lark-shared + calendar/doc/drive/sheets/im/task/wiki/base):
///       include_dir 内嵌 + 启动解包到 bundle_skills_dir,供 SkillRegistry 发现
/// 0.11: Windows 敏感路径硬拦截新增 PowerShell hook,并补充 Credential Manager 相关拦截规则
/// 0.13: 接入企微官方域技能(wecomcli-*,MIT):独立 wecom-skills/ 内嵌 + 独立门控
/// 0.14: 接入钉钉官方 dws skill + Linux ARM64 内置 dws CLI
/// 0.15: 增加 exec_shell 登录终端环境过滤 hook(shell_env.sh)
/// 0.16: 接入腾讯会议官方 tmeet CLI skill
/// 0.17: 接入腾讯 ima OpenAPI Skill（原生受控工具）
/// 0.18: 连接器 CLI 统一为首次使用在线安装；原生 CLI 按平台 lock 校验后落用户目录
pub const BUNDLE_VERSION: &str = concat!(
    "0.18-",
    env!("BUNDLE_INSTRUCTIONS_HASH"),
    "-",
    env!("BUNDLE_WORKFLOW_HASH_SANSHENG"),
);

/// pinvou3 内置的 instructions 共享骨架（Qwen3.6 适配 prompt），编译时内嵌。
/// 骨架 = 身份/底线/工具与事实通用纪律/怎么干/红线/输出，两个模式层占位行：
/// `{{PINVOU3_MODE_ENV_SECTION}}`（§工作环境 位）与
/// `{{PINVOU3_MODE_ARTIFACT_RULE}}`（§工具与事实 的成品条位）。
/// 拆分说明：work 专属的 §工作环境(L10-13) 与 present_artifact 条(L18) 在原文中
/// 不连续，纯 concat 无法逐字节复原，故骨架留占位行、按模式替换拼装。
pub const INSTRUCTIONS_SHARED_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-shared.md");

/// work 模式层：§工作环境（产出物面板语义与 tmp/ 规则）+ §工具与事实 的
/// present_artifact 成品条。两段以空行分隔，供 [`work_layer_sections`] 切分。
pub const INSTRUCTIONS_WORK_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-work.md");

/// work 层两段：§工作环境 整节（无尾换行）与成品条（含尾换行）。
fn work_layer_sections() -> (&'static str, &'static str) {
    INSTRUCTIONS_WORK_MD
        .split_once("\n\n")
        .expect("instructions-work.md 必须是 §工作环境 段 + 空行 + 成品条段")
}

/// work 模式完整 instructions（共享骨架 + work 层占位替换）。
/// 与拆分前 instructions.md 逐字节相等（golden 测试 `work_instructions_render_byte_identical_to_legacy` 锁定）。
pub fn instructions_md() -> &'static str {
    static RENDERED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RENDERED.get_or_init(|| {
        let (env_section, artifact_rule) = work_layer_sections();
        INSTRUCTIONS_SHARED_MD
            // §工作环境 位：work 段无尾换行，补回占位行换行形成节间空行。
            .replace("{{PINVOU3_MODE_ENV_SECTION}}", &format!("{env_section}\n"))
            // 成品条位：占位行整体（含换行）替换为成品条原文 + 补回节间空行。
            .replace(
                "{{PINVOU3_MODE_ARTIFACT_RULE}}\n",
                &format!("{artifact_rule}\n"),
            )
    })
}

/// 代码模式层（品悟原生代码会话）：§工作环境（代码模式身份 + `{{PINVOU3_WORKSPACE_HINT}}`
/// 工作区占位）+ ## 代码场景纪律 增量段，两段以空行分隔。
/// 底座 `CORE_EXECUTION_PROFILE_PROMPT` 不复制进文件，由 [`instructions_code_md`]
/// 在渲染层原样拼接——上游更新自动跟随。
pub const INSTRUCTIONS_CODE_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-code.md");

/// 代码模式完整 instructions（共享骨架 + 代码层）：
/// 骨架的 §工作环境 位填代码版环境段（workspace_hint 已按绑定情况渲染），
/// 成品条位整行删除（代码会话无产出物/成品卡语义）；尾部依次拼接底座
/// core_execution 执行循环与 ## 代码场景纪律。
pub fn instructions_code_md(workspace_hint: &str) -> String {
    let (env_section, discipline) = INSTRUCTIONS_CODE_MD
        .split_once("\n\n")
        .expect("instructions-code.md 必须是 §工作环境 段 + 空行 + ## 代码场景纪律 段");
    let env_section = env_section.replace("{{PINVOU3_WORKSPACE_HINT}}", workspace_hint);
    let mut out = INSTRUCTIONS_SHARED_MD
        .replace("{{PINVOU3_MODE_ENV_SECTION}}", &format!("{env_section}\n"))
        .replace("{{PINVOU3_MODE_ARTIFACT_RULE}}\n", "");
    out.push_str("\n\n");
    out.push_str(deepseek_tui::prompts::CORE_EXECUTION_PROFILE_PROMPT.trim());
    out.push_str("\n\n");
    out.push_str(discipline.trim_end());
    out.push('\n');
    out
}

/// 内置「视觉设计」技能（设计系统直出 HTML）。编译期内嵌，解包到
/// `~/.pinvou3/bundle/skills/visual-design/SKILL.md`，进 SkillRegistry 的 `## Skills`
/// 目录。目录名与 frontmatter 均使用 v0.9 要求的安全命令名 `visual-design`；中文触发词
/// 继续由 description / 正文承载。
const VISUAL_DESIGN_SKILL_MD: &str =
    include_str!("../../../../resources/common/bundle/skills/visual-design/SKILL.md");

/// pinvou3 版 base prompt，编译期内嵌。通过底座 `prompts::set_base_prompt_override`
/// 注入，替换底座的上游 `BASE_PROMPT`。这样 pinvou3 的 prompt 定制活在 app,
/// CodeWhale submodule 的 base.md 回退上游原文（fork drift 归零）。
/// 注：base.md 已折叠为自述空壳，实际静态文案由本文件的 composer（见下）输出，
/// 工具纪律与语言要求分别由 instructions.md 与 `LOCALE_PREAMBLE_*` 承载。
pub const BASE_PROMPT_MD: &str = include_str!("../../../../resources/common/bundle/base.md");

/// pinvou3 版简体中文 locale 前导段（替换底座 `LOCALE_PREAMBLE_ZH_HANS`）。
/// 瘦身依据:底座原文的动机是防 thinking 漂英文(上游 #1118)——pinvou3 生产
/// `reasoning_effort=off` 无 thinking,该 failure mode 不存在;回复语言由
/// 用户消息驱动,这里只补"判断不了时的默认语言"。closer 同理。
pub const LOCALE_PREAMBLE_ZH_HANS: &str = "## 语言要求\n\n\
pinvou3 界面语言为简体中文。跟随用户消息的语言回复;无法判断时用简体中文。\
代码、路径、工具名、URL 保持原样。";

/// pinvou3 版简体中文 locale 收尾段（替换底座 `LOCALE_CLOSER_ZH_HANS` ~660B）。
pub const LOCALE_CLOSER_ZH_HANS: &str = "## 语言再提醒\n\n\
跟随用户最新消息的语言回复;无法判断时用简体中文。";

/// pinvou3 版日语 locale 前导段（替换底座 `LOCALE_PREAMBLE_JA` ~800B,瘦身
/// 依据同 `LOCALE_PREAMBLE_ZH_HANS`）。
pub const LOCALE_PREAMBLE_JA: &str = "## 言語要件\n\n\
pinvou3 の UI 言語は日本語です。ユーザーのメッセージの言語に従って\
返信し、判断できない場合は日本語を使用してください。コード、パス、\
ツール名、URL は元のまま。";

/// pinvou3 版日语 locale 收尾段（替换底座 `LOCALE_CLOSER_JA` ~660B）。
pub const LOCALE_CLOSER_JA: &str = "## 言語再確認\n\n\
ユーザーの最新メッセージの言語に従って返信してください。\
判断できない場合は日本語。";

/// pinvou3 版静态层 mode 块——Yolo（生产主路径,approval=Auto）。瘦身依据:
/// 行为引导大头已由 `bridge::reminder_for` 每 turn `<system-reminder>` 注入,
/// 静态块只立常驻事实;底座 YOLO_MODE/AUTO_APPROVAL/Session Longevity/
/// Efficient Approvals 的逐条教学全不保留。
///
/// (史料,防重蹈:句尾曾有「phase rules」尾巴,是 phase 时代残留;b891b2f 删它属正确清理。
/// 我一度误以为删它致 GUI 首请求采歪、还恢复过(8e20f16)——实为 **gongwen MCP 工具才是
/// 真因**(用户移除 gongwen 即不漂、删 phase 也不漂),phase 是被其开关混淆的红鲱鱼。
/// git 二分时 gongwen 开关状态不一致 → 误判。详见 memory。)
pub const MODE_EXECUTE_MD: &str = "\
## Mode: Execute

Tools run without per-call approval — the user has already authorized
execution. Produce files and run commands now; never end the turn with
a promise of future action. Then verify and report. Follow each
message's `<system-reminder>`.";

/// pinvou3 版静态层 composer：接管底座全部编译期静态文案
/// (taxonomy/base/personality/mode/approval/ContextMgmt/compact 模板)。
/// 从此底座升级新增的静态块只进 default 合成,不漏进 pinvou3 prompt。
/// 干掉:Personality(语气并入 base.md §Voice)、prompt-cache 教学、
/// Session Longevity、Efficient Approvals、Core Tool Taxonomy(instructions
/// 工具表已覆盖)、Compaction Relay 模板(实证死重:256K 自动压缩走
/// `canonical_prompt()` 代码拼装、手动压缩走 `create_summary()` 独立 LLM
/// 调用,二者均不按模板;`.codewhale/handoff.md` 在 pinvou3 无写入通路,
/// `load_handoff_block` 永远 None——模板既无生产者也无消费者)。
pub fn compose_static_layers(_ctx: &deepseek_tui::prompts::StaticPromptCtx<'_>) -> String {
    // 底座宪法层(CONSTITUTION/WORKING RULES)已折叠进 instructions.md —— instructions 是
    // 唯一 pinvou3 prompt 来源(单模型→多模型适配,2026-06-15 消融实测:base.md 对 Qwen3.6
    // 可测量价值仅 Voice 语气,核心权威顺序/防编造已并进 instructions §底线)。静态层只剩 Mode。
    //
    // 不再按 `ctx.mode` 选块:底座 v0.8.57 把 mode/approval 移到 per-turn,调 composer 钉死传
    // 常量 Yolo → 静态层恒为 Execute 块(dump 传 plan 实测亦出 `## Mode: Execute`)。Plan/Agent
    // 的 mode 真相全靠 per-turn reminder,不在静态层;原 Plan/Agent 块是选不中的死代码,已删。
    MODE_EXECUTE_MD.to_string()
}

/// Authority Recap（Final Reminder）清空——其内容(裁决顺序/防编造)已折叠进
/// instructions.md §底线,instructions 是唯一来源,不再单列末尾 recap。
pub const AUTHORITY_RECAP: &str = "";

/// 把 pinvou3 版 prompt 文案注入底座的 prompt 合成层。底座用 `OnceLock`,首次
/// set 生效、后续返回 Err(rejected) —— 幂等,可在每个 `Bridge::boot` 入口重复调用
/// (忽略后续 Err)。必须在任何 engine spawn 前调用(boot 早于 EnginePool 装配)。
/// 上游 v0.8.49 起 `set_*_override` 返回 `Result<(), String>`(首次 Ok,重复 Err)。
pub fn install_prompt_overrides() {
    let _ = deepseek_tui::prompts::set_base_prompt_override(BASE_PROMPT_MD.to_string());
    let _ = deepseek_tui::prompts::set_locale_preamble_zh_hans_override(
        LOCALE_PREAMBLE_ZH_HANS.to_string(),
    );
    let _ = deepseek_tui::prompts::set_locale_closer_zh_hans_override(
        LOCALE_CLOSER_ZH_HANS.to_string(),
    );
    let _ = deepseek_tui::prompts::set_locale_preamble_ja_override(LOCALE_PREAMBLE_JA.to_string());
    let _ = deepseek_tui::prompts::set_locale_closer_ja_override(LOCALE_CLOSER_JA.to_string());
    let _ = deepseek_tui::prompts::set_authority_recap_override(AUTHORITY_RECAP.to_string());
    // 静态层全量接管(fork patch: set_static_prompt_composer_override)。
    // 设置后底座的 Personality/Mode/Approval/ContextMgmt/COMPACT_TEMPLATE/
    // taxonomy 常量全部不进 prompt,由 compose_static_layers 输出替代;
    // base override 仍保留——composer 的 ctx.default_layers 引用它。
    let _ = deepseek_tui::prompts::set_static_prompt_composer_override(Box::new(|ctx| {
        compose_static_layers(ctx)
    }));
}

/// present_artifact MCP server 脚本(零依赖 python stdio),编译期内嵌,解包到
/// `~/.pinvou3/bundle/mcp-servers/`。底座按 mcp.json 用 `python3 <path>` 拉起它。
pub const PRESENT_ARTIFACT_SERVER_PY: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/present_artifact_server.py");

// --- 工具市场：内置 MCP server 资源(编译期内嵌) ---
const WEATHER_SERVER_PY: &str =
    include_str!("../../../../../resources/mcp-servers/weather/server.py");
const WEATHER_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/weather/manifest.json");
const IWENCAI_SERVER_PY: &str =
    include_str!("../../../../../resources/mcp-servers/iwencai/server.py");
const IWENCAI_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/iwencai/manifest.json");
const QCC_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/qcc/manifest.json");
const YUANDIAN_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/yuandian-mcp/manifest.json");
const CANVA_MCP_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/canva-mcp/manifest.json");
const PATSNAP_SEARCH_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/patsnap-search/manifest.json");
const OBSIDIAN_SERVER_PY: &str =
    include_str!("../../../../../resources/mcp-servers/obsidian/server.py");
const OBSIDIAN_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/obsidian/manifest.json");
const PPTX_SERVER_PY: &str = include_str!("../../../../../resources/mcp-servers/pptx/server.py");
const PPTX_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/pptx/manifest.json");
const GONGWEN_SERVER_PY: &str =
    include_str!("../../../../../resources/mcp-servers/gongwen/server.py");
const GONGWEN_MANIFEST_JSON: &str =
    include_str!("../../../../../resources/mcp-servers/gongwen/manifest.json");
const GONGWEN_STYLES_PY: &str =
    include_str!("../../../../../resources/mcp-servers/gongwen/gbt9704_styles.py");

/// 内嵌的敏感目录拦截 shell 脚本——配合 bridge 注入的 hook 在 ToolCallBefore
/// 时阻止 LLM 触碰 ~/.ssh/ ~/.gnupg/ 等。
pub const DENY_SENSITIVE_PATHS_SH: &str =
    include_str!("../../../../resources/common/bundle/deny_sensitive_paths.sh");
pub const DENY_SENSITIVE_PATHS_PS1: &str =
    include_str!("../../../../resources/common/bundle/deny_sensitive_paths.ps1");

/// 内嵌的 exec_shell CLI 兼容环境 hook：读取登录 shell 环境并过滤凭证。
pub const SHELL_ENV_SH: &str = include_str!("../../../../resources/common/bundle/shell_env.sh");
#[derive(Debug, Clone)]
pub struct Pinvou3Bundle {
    pub root: PathBuf,
    pub instructions_md: PathBuf,
    pub skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub mcp_json: PathBuf,
    pub deny_sensitive_sh: PathBuf,
    pub deny_sensitive_ps1: PathBuf,
    pub shell_env_sh: PathBuf,
}

impl Pinvou3Bundle {
    pub fn paths() -> Self {
        Self {
            root: paths::bundle_root(),
            instructions_md: paths::bundle_instructions(),
            skills_dir: paths::bundle_skills_dir(),
            user_skills_dir: paths::user_skills_dir(),
            mcp_json: paths::bundle_mcp_json(),
            deny_sensitive_sh: paths::bundle_root().join("deny_sensitive_paths.sh"),
            deny_sensitive_ps1: paths::bundle_root().join("deny_sensitive_paths.ps1"),
            shell_env_sh: paths::bundle_root().join("shell_env.sh"),
        }
    }

    /// 比对 `bundle/VERSION` 与 [`BUNDLE_VERSION`]：相同跳过；
    /// 不同则覆写 bundle 内文件并更新 VERSION。**不动 user/ 和 settings.json**。
    ///
    /// 解包时对 `INSTRUCTIONS_MD` 做模板替换，把 `{{PINVOU3_WORKSPACE}}` 占位符
    /// 替换成 `~/.pinvou3/workspace/` 的实际绝对路径——让 AI 直接拿到完整路径
    /// 给 write_file 用，避免先 exec_shell 探一遍 env var。
    pub fn ensure_extracted(&self) -> std::io::Result<()> {
        paths::ensure_dirs()?;
        let version_file = paths::bundle_version_file();
        let current = std::fs::read_to_string(&version_file).unwrap_or_default();
        let bundle_changed = current.trim() != BUNDLE_VERSION;

        // 已下线 skills 每次启动都清理(防御性):既有装机的残留目录若不清,
        // SkillRegistry 仍会从 disk 发现它们、重新触发对应协议 prompt。
        crate::platform::startup::mark("bundle_extract:cleanup_retired:start");
        self.cleanup_retired_skills()?;
        // 已从技能市场下架的预置技能(pua/女娲/头脑风暴):它们曾走 marketplace 装、带
        // `pinvou3-marketplace:` 标记,故按标记内容精确删,只跳过用户上传的同名目录。
        self.cleanup_removed_marketplace_skills()?;
        // 已从工具市场下架的预置 MCP 工具也要清理运行态残留;否则旧 manifest 仍会被
        // MarketplaceManager 扫到,在 composer「已接入工具」里继续出现。
        self.cleanup_removed_marketplace_tools()?;
        crate::platform::startup::mark("bundle_extract:cleanup_retired:done");
        // 工作流目录同 skills:immutable bundle 资源,每次启动防御性重写
        // (防 "VERSION 对得上但目录缺失"),无副作用。
        crate::platform::startup::mark("bundle_extract:write_workflows:start");
        self.write_workflows()?;
        crate::platform::startup::mark("bundle_extract:write_workflows:done");
        // PR #132 早期构建曾把 CLI 解包进 immutable bundle；统一在线安装后清掉该
        // app 自有旧目录，避免旧二进制掩盖按需安装与 hash 校验。
        let _ = std::fs::remove_dir_all(paths::bundle_root().join("connectors"));
        // Migrate plaintext MCP secrets before bundled manifests are rewritten. If migration
        // fails, keep the old files as a recoverable source instead of overwriting the only
        // remaining plaintext copy.
        crate::platform::startup::mark("bundle_extract:migrate_mcp_secrets:start");
        let mcp_secret_migration_ok = match crate::features::marketplace::MarketplaceManager::new()
            .migrate_mcp_plaintext_secrets()
        {
            Ok(_) => true,
            Err(err) => {
                eprintln!("[pinvou3-app] MCP secret migration skipped: {err}");
                false
            }
        };
        crate::platform::startup::mark("bundle_extract:migrate_mcp_secrets:done");
        // Built-in skills and workflow resources are immutable bundle assets.
        crate::platform::startup::mark("bundle_extract:write_builtin_skills:start");
        self.write_builtin_skills()?;
        crate::platform::startup::mark("bundle_extract:write_builtin_skills:done");
        // 飞书 / 企微 / 钉钉鉴权 CLI 不得阻塞 Tauri setup。启动阶段只沿用上次落盘的完整
        // 技能目录作为缓存；React 首屏提交后调用 refresh_connector_auth_gates 并行
        // 实时探测，再按真实状态修正目录。bundle 升级时仅刷新当前可见的缓存目录。
        crate::platform::startup::mark("bundle_extract:apply_skill_gates:start");
        let feishu_show = self.cached_feishu_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:feishu_cached_gate",
            &format!("show={feishu_show}"),
        );
        if bundle_changed || !feishu_show {
            self.apply_feishu_skills(feishu_show)?;
        }
        let wecom_show = self.cached_wecom_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:wecom_cached_gate",
            &format!("show={wecom_show}"),
        );
        if bundle_changed || !wecom_show {
            self.apply_wecom_skills(wecom_show)?;
        }
        let dingtalk_show = self.cached_dingtalk_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:dingtalk_cached_gate",
            &format!("show={dingtalk_show}"),
        );
        if bundle_changed || !dingtalk_show {
            self.apply_dingtalk_skills(dingtalk_show)?;
        }
        let tmeet_show = self.cached_tmeet_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:tmeet_cached_gate",
            &format!("show={tmeet_show}"),
        );
        if bundle_changed || !tmeet_show {
            self.apply_tmeet_skills(tmeet_show)?;
        }
        crate::platform::startup::mark("bundle_extract:apply_skill_gates:done");
        // MCP server scripts are immutable as well, but wait for secret migration to avoid
        // deleting legacy plaintext before it has been copied into the credential store.
        crate::platform::startup::mark("bundle_extract:write_mcp_servers:start");
        if mcp_secret_migration_ok {
            self.write_mcp_servers()?;
        }
        // mcp.json merge:每次启动 upsert 内置 pinvou server,保留 marketplace 条目。
        // 不受 VERSION gate 限制——marketplace 安装可能在任何时候发生。
        self.ensure_builtin_mcp_servers()?;
        // 启动自愈:刷新 mcp.json 里陈旧的本地 python server command(安装时写死的裸
        // "python" → 重解析成可用路径)。必须在引擎 spawn 前跑(引擎从 mcp.json 拉起 server)。
        self.refresh_mcp_python_commands()?;
        crate::platform::startup::mark("bundle_extract:write_mcp_servers:done");

        if !bundle_changed {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.skills_dir)?;
        let workspace_abs = paths::workspace_dir();
        std::fs::create_dir_all(&workspace_abs)?;
        // 首次解包按当前 sudoers 状态填 PINVOU3_SUDO_INSTRUCTION,避免占位符原文
        // 漏到 LLM 看到的 system prompt(engine boot 时是从 disk 读的)。
        // 用户切换开关时 set_super_permission 会 sync_session 重写。
        let rendered = instructions_md()
            .replace("{{PINVOU3_WORKSPACE}}", &workspace_abs.to_string_lossy())
            .replace(
                "{{PINVOU3_SUDO_INSTRUCTION}}",
                crate::platform::super_permission::instruction_block(),
            )
            // 落盘副本无 per-session locale,默认填中文兜底(LLM 实际走 mod.rs 的 inline 渲染,
            // 那里按 locale 填);此处仅防 {{PINVOU3_TITLE_LANG}} 占位符原文残留在 disk 文件。
            .replace("{{PINVOU3_TITLE_LANG}}", "简体中文");
        std::fs::write(&self.instructions_md, rendered)?;
        // PINVOU 自有 hooks：写入 + 加可执行位
        std::fs::write(&self.deny_sensitive_sh, DENY_SENSITIVE_PATHS_SH)?;
        std::fs::write(&self.deny_sensitive_ps1, DENY_SENSITIVE_PATHS_PS1)?;
        std::fs::write(&self.shell_env_sh, SHELL_ENV_SH)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [&self.deny_sensitive_sh, &self.shell_env_sh] {
                let mut perm = std::fs::metadata(script)?.permissions();
                perm.set_mode(0o755);
                std::fs::set_permissions(script, perm)?;
            }
        }
        std::fs::write(&version_file, BUNDLE_VERSION)?;
        eprintln!(
            "[pinvou3-app] bundle extracted to {} (version {})",
            self.root.display(),
            BUNDLE_VERSION
        );
        Ok(())
    }

    /// 清理已下线内置 skills 的残留目录(被 ensure_extracted 在 VERSION check 前
    /// 调用,每次启动都跑):
    /// - legacy-ppt-workflow:0.5 下线(workflow 功能转"开发中")
    /// - pinvou-review-plan / pinvou-review-final:0.7 下线(EXIT GATE 评审被推翻)
    ///
    /// 技能市场([`super::skill_marketplace`])装的技能带 `.installed-from` 标记、
    /// 落在同一 `bundle/skills/` 目录。清理时显式跳过带标记的目录——这是保护契约,
    /// 任何未来对 `skills_dir` 的全量重写也必须遵守,否则会误删用户装的技能。
    fn cleanup_retired_skills(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.skills_dir)?;
        for retired in [
            "legacy-ppt-workflow",
            "pinvou-review-plan",
            "pinvou-review-final",
        ] {
            let dir = self.skills_dir.join(retired);
            if dir.join(".installed-from").is_file() {
                continue; // marketplace 技能占用了该名,保护不删
            }
            let _ = std::fs::remove_dir_all(dir);
        }
        Ok(())
    }

    /// 清理已从技能市场移除的预置技能残留(dir 名 → 原市场 id)。
    ///
    /// 与 [`Self::cleanup_retired_skills`] 相反:这些**曾是** marketplace 技能、装时写了
    /// `pinvou3-marketplace:<id>` 标记,所以不能沿用"带标记即跳过"的保护——否则永远删不掉。
    /// 改为**按标记内容精确匹配**:只删标记恰为本技能市场安装(或无标记的裸残留),
    /// 显式跳过 `upload:` 开头的用户上传目录,避免误删用户自己传的同名技能。
    fn cleanup_removed_marketplace_skills(&self) -> std::io::Result<()> {
        for (dir_name, market_id) in [
            ("pua", "pua"),
            ("huashu-nuwa", "nuwa"),
            ("brainstorming", "brainstorming"),
        ] {
            let dir = self.skills_dir.join(dir_name);
            if !dir.exists() {
                continue;
            }
            let marker = std::fs::read_to_string(dir.join(".installed-from")).unwrap_or_default();
            let marker = marker.trim();
            if marker.starts_with("upload:") {
                continue; // 用户上传的同名技能,保护不删
            }
            if marker.is_empty() || marker == format!("pinvou3-marketplace:{market_id}") {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        Ok(())
    }

    /// 清理已从工具市场移除的预置 MCP 工具残留。
    ///
    /// 不能删除所有未知目录:未来/本地可能有自定义 MCP 工具。这里只精确处理曾经内置、
    /// 现在源码资源已经移除的 marketplace 工具。
    fn cleanup_removed_marketplace_tools(&self) -> std::io::Result<()> {
        {
            let tool_id = "data_analysis";
            let _ = crate::features::marketplace::MarketplaceManager::new().uninstall(tool_id);

            let mut disabled = crate::features::marketplace::load_disabled_connectors();
            let before = disabled.len();
            disabled.retain(|id| id != tool_id);
            if disabled.len() != before {
                crate::features::marketplace::save_disabled_connectors(&disabled);
            }

            let _ = std::fs::remove_dir_all(paths::bundle_mcp_servers_dir().join(tool_id));
        }
        Ok(())
    }

    /// 解包内嵌的内置 skills 到 pinvou3 单一来源 `bundle/skills`。v0.9 clean re-fork
    /// 后 catalogue 与 `load_skill` 都只扫描此目录，不再写 `~/.agents/skills`。
    /// 每次启动防御性重写(immutable 内置资源)。当前:视觉设计。
    fn write_builtin_skills(&self) -> std::io::Result<()> {
        let dir = self.skills_dir.join("visual-design");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), VISUAL_DESIGN_SKILL_MD)?;
        Ok(())
    }

    /// 解包内嵌的工作流目录到 `~/.pinvou3/bundle/workflow/`。
    /// 每次启动防御性重写（immutable bundle 资源）。
    fn write_workflows(&self) -> std::io::Result<()> {
        let workflow_root = paths::bundle_workflow_dir();
        // sansheng-liubu
        let dest = workflow_root.join("sansheng-liubu");
        Self::extract_dir(&SANSHENG_LIUBU_DIR, &dest)?;
        Ok(())
    }

    /// 解包内嵌的飞书官方域技能(lark-*)到 `~/.pinvou3/bundle/skills/`。
    /// 每次启动防御性重写（immutable bundle 资源）。`LARK_SKILLS_DIR` 的根对应
    /// `bundle/skills/`,内含 `lark-<域>/SKILL.md` + `references/`,直接铺到
    /// `skills_dir`——引擎 `SkillRegistry` 扫该目录的每个含 `SKILL.md` 的子目录。
    /// (顶层散落的 NOTICE.md 不含 SKILL.md,会被注册表忽略。)
    /// 飞书技能门控:`show` → 解包 9 个 lark 技能到 `skills_dir`;否则**删掉**它们(+ NOTICE.md)。
    /// 幂等(删不存在的目录不报错)。可见性 = 目录在不在,引擎重刷系统提示时重扫即生效。
    pub fn apply_feishu_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&LARK_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in LARK_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE.md"));
        }
        Ok(())
    }

    /// 启动缓存只在 9 个飞书域技能全部完整落盘时判 visible，避免上次异常中断留下
    /// 半套目录却被 SkillRegistry 当成已连接。实时真相在首屏后的 CLI 探测中刷新。
    fn cached_feishu_skills_visible(&self) -> bool {
        crate::platform::connector_state::feishu_skills_visible()
            && LARK_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 企微域技能门控:`show` → 解包 7 个 wecomcli 技能到 `skills_dir`;否则**删掉**它们。
    /// 幂等。与飞书门控正交(各自的连接 / 停用状态独立)。
    /// 注:`WECOM_SKILLS_DIR` 根 = `wecom-skills/`,内含 `wecomcli-<域>/SKILL.md`(+ NOTICE.md);
    /// 直接铺到 `skills_dir`,引擎 `SkillRegistry` 扫每个含 `SKILL.md` 的子目录。
    /// 出处声明用 `NOTICE-wecom.md`(避开飞书的 `NOTICE.md`,两者解包到同一 skills_dir
    /// 不会互相覆盖)。隐藏时一并删掉。
    pub fn apply_wecom_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&WECOM_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in WECOM_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-wecom.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的企微技能目录作为启动缓存。
    fn cached_wecom_skills_visible(&self) -> bool {
        crate::platform::connector_state::wecom_skills_visible()
            && WECOM_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 钉钉 mono skill 门控:`show` → 解包 `dws` 到 `skills_dir`;否则删除。
    /// 出处声明用 `NOTICE-dingtalk.md`,避免覆盖飞书 / 企微的 NOTICE。
    pub fn apply_dingtalk_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&DINGTALK_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in DINGTALK_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-dingtalk.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的钉钉技能目录作为启动缓存。
    fn cached_dingtalk_skills_visible(&self) -> bool {
        crate::platform::connector_state::dingtalk_skills_visible()
            && DINGTALK_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 腾讯会议 mono skill 门控:`show` → 解包 `tmeet-skill` 到 `skills_dir`;否则删除。
    /// 出处声明用 `NOTICE-tmeet.md`,避免覆盖其他 CLI 连接器 NOTICE。
    pub fn apply_tmeet_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&TMEET_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in TMEET_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-tmeet.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的腾讯会议技能目录作为启动缓存。
    fn cached_tmeet_skills_visible(&self) -> bool {
        crate::platform::connector_state::tmeet_skills_visible()
            && TMEET_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }
    /// 递归解包 `include_dir::Dir` 到磁盘目标路径。
    /// `root` 是磁盘目标根(对应 include_dir 的顶层),`dir` 可以是任意层级子目录。
    /// `Dir::files()` 返回的 `path()` 是相对于 **include_dir 根** 的完整路径
    /// (如 "roles/taizi.md"),所以一律用 `root.join(file.path())` 定位。
    fn extract_dir(dir: &Dir<'_>, root: &std::path::Path) -> std::io::Result<()> {
        for file in dir.files() {
            let path = root.join(file.path());
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, file.contents())?;
        }
        for sub in dir.dirs() {
            Self::extract_dir(sub, root)?;
        }
        Ok(())
    }

    /// mcp.json merge：upsert 内置 pinvou server，保留 marketplace 已安装的条目。
    /// 每次启动都调用（不受 VERSION gate 限制）。
    fn ensure_builtin_mcp_servers(&self) -> std::io::Result<()> {
        let present_server = paths::bundle_present_artifact_server();
        let mut mcp: serde_json::Value = if self.mcp_json.is_file() {
            let existing =
                std::fs::read_to_string(&self.mcp_json).unwrap_or_else(|_| "{}".to_string());
            serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({"servers": {}}))
        } else {
            serde_json::json!({"servers": {}})
        };
        if mcp.get("servers").and_then(|s| s.as_object()).is_none() {
            mcp.as_object_mut()
                .unwrap()
                .insert("servers".into(), serde_json::json!({}));
        }
        let servers = mcp["servers"].as_object_mut().unwrap();
        // 迁移:旧版 server key 是 `pinvou`(与产品名 `pinvou3` 差一个 3,模型采样必漂成
        // pinvou3 → `Failed to find MCP server: pinvou3`)。改用 `pinvou3` 对齐产品名,并删掉
        // 旧 `pinvou` 条目——upsert 不会自动删旧名,不删会留两个指向同一脚本的 server。
        servers.remove("pinvou");
        // Windows 用内置 pythonw(无窗口 + 自带依赖);其他平台系统 python3。见 paths::python_command。
        let python_cmd = paths::python_command();
        servers.insert(
            "pinvou3".to_string(),
            serde_json::json!({
                "command": python_cmd,
                "args": [present_server.to_string_lossy()]
            }),
        );
        let json = serde_json::to_string_pretty(&mcp).map_err(std::io::Error::other)?;
        std::fs::write(&self.mcp_json, json)
    }

    /// 启动自愈:`mcp.json` 里本地 python server 的 `command` 是**安装时写死**的,老条目
    /// 常是裸 `"python"`/`"python3"` —— 在没把 python 加进 PATH 的机器(或只有 python3 的
    /// Linux)上永远拉不起来(高德天气等 marketplace 工具静默失效)。每次启动重解析:凡
    /// command 是裸 python 家族名、或指向不存在的 python 路径,统一替换成当前
    /// `paths::python_command()`。`url` 型远程 server / 非 python command 一律不动。
    fn refresh_mcp_python_commands(&self) -> std::io::Result<()> {
        if !self.mcp_json.is_file() {
            return Ok(());
        }
        let existing = std::fs::read_to_string(&self.mcp_json).unwrap_or_default();
        let mut mcp: serde_json::Value = match serde_json::from_str(&existing) {
            Ok(v) => v,
            Err(_) => return Ok(()), // 坏 json 不碰
        };
        let resolved = paths::python_command();
        let mut changed = false;
        if let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) {
            for (_name, entry) in servers.iter_mut() {
                let Some(obj) = entry.as_object_mut() else {
                    continue;
                };
                let Some(cmd) = obj.get("command").and_then(|c| c.as_str()) else {
                    continue; // url 型远程 server 无 command 字段
                };
                if cmd != resolved && Self::is_stale_python_command(cmd) {
                    obj.insert(
                        "command".to_string(),
                        serde_json::Value::String(resolved.clone()),
                    );
                    changed = true;
                }
            }
        }
        if changed {
            let json = serde_json::to_string_pretty(&mcp).map_err(std::io::Error::other)?;
            std::fs::write(&self.mcp_json, json)?;
        }
        Ok(())
    }

    /// command 是否是"需要重解析"的 python:裸解释器名(python/python3/pythonw[.exe]),
    /// 或指向一个已不存在的 python 路径。非 python command 一律 false,绝不误伤别的工具。
    fn is_stale_python_command(cmd: &str) -> bool {
        let lower = cmd.to_ascii_lowercase();
        let bare = !cmd.contains('/') && !cmd.contains('\\');
        if bare {
            return matches!(
                lower.as_str(),
                "python" | "python3" | "pythonw" | "python.exe" | "pythonw.exe" | "python3.exe"
            );
        }
        // 带路径但文件不存在、且看起来是 python → 重解析(指向已删/搬走的解释器)
        lower.contains("python") && !std::path::Path::new(cmd).exists()
    }

    /// 写出内置 MCP server 脚本到 `~/.pinvou3/bundle/mcp-servers/` + 加可执行位。
    /// 每次启动防御性重写(immutable bundle 资源,无副作用)。底座按 mcp.json
    /// 用 `python <path>` 拉起,不依赖可执行位,但 chmod +x 无害。
    fn write_mcp_servers(&self) -> std::io::Result<()> {
        let dir = paths::bundle_mcp_servers_dir();
        std::fs::create_dir_all(&dir)?;
        // pinvou 内置 present_artifact server
        let server = paths::bundle_present_artifact_server();
        std::fs::write(&server, PRESENT_ARTIFACT_SERVER_PY)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&server)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&server, perm)?;
        }
        // 工具市场：天气 MCP server
        let weather_dir = dir.join("weather");
        std::fs::create_dir_all(&weather_dir)?;
        std::fs::write(weather_dir.join("server.py"), WEATHER_SERVER_PY)?;
        std::fs::write(weather_dir.join("manifest.json"), WEATHER_MANIFEST_JSON)?;
        // 工具市场：同花顺问财 MCP server
        let iwencai_dir = dir.join("iwencai");
        std::fs::create_dir_all(&iwencai_dir)?;
        std::fs::write(iwencai_dir.join("server.py"), IWENCAI_SERVER_PY)?;
        std::fs::write(iwencai_dir.join("manifest.json"), IWENCAI_MANIFEST_JSON)?;
        // 工具市场：企查查（远程 MCP，只有 manifest.json，无 server.py）
        let qcc_dir = dir.join("qcc");
        std::fs::create_dir_all(&qcc_dir)?;
        std::fs::write(qcc_dir.join("manifest.json"), QCC_MANIFEST_JSON)?;
        // 工具市场：华宇元典（远程 MCP + OAuth，只有 manifest.json，无 server.py）
        let yuandian_dir = dir.join("yuandian-mcp");
        std::fs::create_dir_all(&yuandian_dir)?;
        std::fs::write(yuandian_dir.join("manifest.json"), YUANDIAN_MANIFEST_JSON)?;
        // 工具市场：Canva 可画（远程 MCP + OAuth，只有 manifest.json，无 server.py）
        let canva_dir = dir.join("canva-mcp");
        std::fs::create_dir_all(&canva_dir)?;
        std::fs::write(canva_dir.join("manifest.json"), CANVA_MCP_MANIFEST_JSON)?;
        // 工具市场：智慧芽专利&文献融合检索（远程 MCP，API Key 通过请求头占位符注入）
        let patsnap_dir = dir.join("patsnap-search");
        std::fs::create_dir_all(&patsnap_dir)?;
        std::fs::write(
            patsnap_dir.join("manifest.json"),
            PATSNAP_SEARCH_MANIFEST_JSON,
        )?;
        // 工具市场：Obsidian 知识库 MCP server（本地 stdio，检索本机 vault）
        let obsidian_dir = dir.join("obsidian");
        std::fs::create_dir_all(&obsidian_dir)?;
        std::fs::write(obsidian_dir.join("server.py"), OBSIDIAN_SERVER_PY)?;
        std::fs::write(obsidian_dir.join("manifest.json"), OBSIDIAN_MANIFEST_JSON)?;
        // 工具市场：PPT 生成 MCP server（本地 stdio，python-pptx 直出 .pptx；非零依赖，装时自动 pip install）
        let pptx_dir = dir.join("pptx");
        std::fs::create_dir_all(&pptx_dir)?;
        std::fs::write(pptx_dir.join("server.py"), PPTX_SERVER_PY)?;
        std::fs::write(pptx_dir.join("manifest.json"), PPTX_MANIFEST_JSON)?;
        // 工具市场：公文写作 MCP server（本地 stdio，python-docx 直出 GB/T 9704 .docx；
        // 比别的多一个 gbt9704_styles.py 渲染模块，server.py 同目录 import 它）
        let gongwen_dir = dir.join("gongwen");
        std::fs::create_dir_all(&gongwen_dir)?;
        std::fs::write(gongwen_dir.join("server.py"), GONGWEN_SERVER_PY)?;
        std::fs::write(gongwen_dir.join("manifest.json"), GONGWEN_MANIFEST_JSON)?;
        std::fs::write(gongwen_dir.join("gbt9704_styles.py"), GONGWEN_STYLES_PY)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    /// 拆分前 instructions.md 原文快照（2026-08，33 行）。
    /// golden：work 渲染必须与原文逐字节相等——骨架占位行替换的任何手滑
    /// （换行/空行/节序/分段锚错位）都会让本测试变红。
    const LEGACY_INSTRUCTIONS_MD: &str = "# pinvou3 运行守则\n\n> 你是 {{PINVOU3_MODEL}},运行在 pinvou3(本地桌面 GUI 助手)中。运行时态(Plan / Yolo 模式、超级权限开关)走每轮 `<system-reminder>`,以那里为准。禁 `read_file` `.pinvou3/bundle/` 下任何文件。\n\n## 底线\n- **真相优先**:不编造工具结果 / 路径 / 数字;声称做完前先验证(跑检查 / 读回关键处)或如实说明为何没验。\n- **权威顺序**:用户当前指令 > 既定规则 > 你的记忆;实时工具输出与文件内容 > 你的记忆,冲突时重读、信工具。\n- 语气平实,少感叹号与最高级。\n\n## 工作环境\n- workspace = `$HOME`,但**这不是项目目录** —— 你是桌面 GUI 助手。产出用**相对路径**写(如 `write_file(\"report.html\", …)`),自动落到本会话专属工作目录;别用 `~` 或绝对路径。\n- **工作目录根 = 用户看到的「产出物」面板**:只有**最终成品**才直接写到根。所有**中间 / 临时文件**(命令行入参、API 响应、分步数据等)一律写到 `tmp/` 子目录(相对路径,如 `tmp/params.json`)—— 子目录里的文件不进产出物列表,免得一堆过程文件污染面板。能用 stdin / 内存不落文件就别落。\n- 用户文件常在 `~/Documents` `~/Desktop` `~/Downloads` `~/桌面` `~/下载` `~/文档`;找文件用 `file_search`,别 `list_dir ~/` 或 `find ~/` 扫整个家目录。\n\n## 工具与事实\n- **只调你工具列表里实际出现的工具**;没出现的就是没有,别编工具名(算术 / 跑脚本用 `exec_shell python3 -c '...'`,git log 用 `exec_shell git log`)。\n- **不知道的当前信息必须调工具、禁止凭记忆编**:算术 / 精确当前时间 / 系统状态 / 库最新版本 / 文件内容与行数。\n- 给客户看的**单文件成品**(html / markdown / 图)写完,立刻调 `mcp_pinvou3_present_artifact`(绝对 `path` + 一眼看懂的 `title`,**title 用{{PINVOU3_TITLE_LANG}}、与你的回复同语种**);迭代重写后再调一次。\n\n## 怎么干\n- **说做就做,别光宣布**:一旦说要用工具做某事(写文件 / 搜索 / 读取 / 展示成品),就在**同一条回复里立刻发出该工具调用**;严禁只回「我来写 / 现在开始 / 让我…」之类就停下、把回合交还用户。前言一两句够了,别长篇铺垫策略。\n- 缺信息当轮就用工具补,别硬编;读多文件 / 多关键词搜索**并行**发起。\n- 遇到**影响结果走向的岔路**(选哪个方案 / 往哪个方向做),主动用 `request_user_input` 给 2-3 个选项让用户拍板,别自己闷头猜;纯执行细节有合理默认就直接做。不复述过程,做完给结果 + 关键决策。\n- 复杂任务(≥5 步)先 `checklist_write` 列清单再做(列完同轮就动手);**及时收手**,信息够答好就交付,别拿到 80% 还抠细节。\n\n## 红线(任何模式、任何请求都不破)\n- **密钥凭证禁读禁写**:`~/.ssh`、含 `id_rsa` / `credentials` / `.env` / `token` 的路径、`/etc/shadow`;被要求时给终端替代方案。\n- 不 `rm -rf` 用户文件 / 目录、不 `git reset --hard` / 批量清理,**除非用户精确点名**那个操作。\n- 写 `/etc` `/usr` `/var` 需超级权限:关闭态禁写,引导用户去【设置 → 系统权限】。\n\n## 输出\nGUI 富文本,代码块 / 列表 / 表格随便用。\n{{PINVOU3_SUDO_INSTRUCTION}}\n";

    #[test]
    fn work_instructions_render_byte_identical_to_legacy() {
        assert_eq!(instructions_md(), LEGACY_INSTRUCTIONS_MD);
    }

    #[test]
    fn shared_skeleton_keeps_mode_placeholder_rows_in_place() {
        // 骨架占位行必须仍在原位：§底线 与 §工具与事实 之间、§工具与事实 与 §怎么干 之间。
        let env_at = INSTRUCTIONS_SHARED_MD
            .find("{{PINVOU3_MODE_ENV_SECTION}}")
            .expect("env placeholder");
        let artifact_at = INSTRUCTIONS_SHARED_MD
            .find("{{PINVOU3_MODE_ARTIFACT_RULE}}")
            .expect("artifact placeholder");
        let tools_at = INSTRUCTIONS_SHARED_MD.find("## 工具与事实").unwrap();
        let how_at = INSTRUCTIONS_SHARED_MD.find("## 怎么干").unwrap();
        assert!(env_at < tools_at && tools_at < artifact_at && artifact_at < how_at);
    }

    #[test]
    fn code_instructions_render_project_hint_and_drop_artifact_semantics() {
        let rendered =
            instructions_code_md("你正在用户的项目目录 `/repo/demo` 中工作,相对路径即相对项目根;");
        // 工作区占位渲染正确。
        assert!(rendered.contains("你正在用户的项目目录 `/repo/demo` 中工作"));
        assert!(!rendered.contains("{{PINVOU3_WORKSPACE_HINT}}"));
        // 底座执行循环原样引用（上游常量内容的一个稳定锚点）。
        let core = deepseek_tui::prompts::CORE_EXECUTION_PROFILE_PROMPT.trim();
        let anchor = core.lines().next().expect("core_execution 非空");
        assert!(rendered.contains(anchor));
        assert!(rendered.contains(core));
        // 无产出物/成品卡语义：work 层的面板与落盘规则不出现（增量段的否定式
        // 提及“没有产出物面板/不建 tmp/”是刻意保留的行为指引）。
        assert!(!rendered.contains("mcp_pinvou3_present_artifact"));
        assert!(!rendered.contains("只有**最终成品**"));
        assert!(!rendered.contains("自动落到本会话专属工作目录"));
        assert!(!rendered.contains("产出用**相对路径**写"));
        // 代码场景纪律与共享红线都在。
        assert!(rendered.contains("## 代码场景纪律"));
        assert!(rendered.contains("不主动执行 git 写操作"));
        assert!(rendered.contains("## 红线"));
        // 占位行无残留。
        assert!(!rendered.contains("{{PINVOU3_MODE_ENV_SECTION}}"));
        assert!(!rendered.contains("{{PINVOU3_MODE_ARTIFACT_RULE}}"));
        // 骨架结构保持：代码环境段位于 §底线 与 §工具与事实 之间。
        let bottom = rendered.find("## 底线").unwrap();
        let env = rendered.find("## 工作环境").unwrap();
        let tools = rendered.find("## 工具与事实").unwrap();
        assert!(bottom < env && env < tools);
    }

    #[test]
    fn code_instructions_render_temporary_hint() {
        let rendered = instructions_code_md("你在本会话专属工作目录中工作,相对路径即相对该目录;");
        assert!(rendered.contains("你在本会话专属工作目录中工作"));
        assert!(!rendered.contains("项目目录"));
    }

    /// 测试 bundle 解包的两个场景：首次解包成功 + VERSION 匹配时不覆写。
    /// 借 paths::tests::ENV_LOCK 跟其他 mutate PINVOU3_HOME 的测试串行化，
    /// 不靠唯一 nanos 路径躲 race（仍会读 env var）。
    #[test]
    fn ensure_extracted_behavior() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);

        // 1) 首次解包：文件被写入 + VERSION 记录
        let bundle = Pinvou3Bundle::paths();
        bundle.ensure_extracted().unwrap();
        assert!(bundle.instructions_md.is_file());
        assert!(bundle.mcp_json.is_file());
        assert!(bundle.deny_sensitive_sh.is_file());
        assert!(bundle.deny_sensitive_ps1.is_file());
        assert!(bundle.shell_env_sh.is_file());
        assert!(paths::bundle_version_file().is_file());
        // present_artifact MCP server 应解包,mcp.json 注册且占位符替换成绝对路径
        assert!(
            paths::bundle_present_artifact_server().is_file(),
            "present_artifact server 脚本应被解包"
        );
        let mcp = std::fs::read_to_string(&bundle.mcp_json).unwrap();
        assert!(
            mcp.contains("present_artifact_server.py"),
            "mcp.json 应注册 present server 的绝对路径"
        );
        assert!(
            !mcp.contains("{{PINVOU3_PRESENT_SERVER}}"),
            "mcp.json 的 server 路径占位符应被替换"
        );
        // present server key 必须是 pinvou3(对齐产品名,消除模型把 pinvou 漂成 pinvou3 的撞脸);
        // 旧 pinvou 名不残留。
        let mcp_keys: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        let server_keys = mcp_keys["servers"].as_object().unwrap();
        assert!(
            server_keys.contains_key("pinvou3") && !server_keys.contains_key("pinvou"),
            "present server key 应为 pinvou3、旧 pinvou 不残留,实际={:?}",
            server_keys.keys().collect::<Vec<_>>()
        );
        let canva_dir = paths::bundle_mcp_servers_dir().join("canva-mcp");
        let canva_manifest = canva_dir.join("manifest.json");
        assert!(canva_manifest.is_file(), "Canva 可画 manifest 应被解包");
        let canva_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&canva_manifest).unwrap()).unwrap();
        assert_eq!(canva_json["id"], "canva-mcp");
        assert_eq!(canva_json["servers"][0]["name"], "canva_mcp");
        assert!(
            !canva_dir.join("server.py").exists(),
            "Canva 远程 MCP 不应解包本地 server.py"
        );
        // 已下线 skills(legacy-ppt-workflow / pinvou-review-*)不应再被写出。
        for retired in [
            "legacy-ppt-workflow",
            "pinvou-review-plan",
            "pinvou-review-final",
        ] {
            assert!(
                !bundle.skills_dir.join(retired).exists(),
                "{retired} 已下线,不应再解包"
            );
        }
        // 三省六部工作流应解包到 bundle/workflow/sansheng-liubu/
        let wf_dir = paths::bundle_workflow_dir().join("sansheng-liubu");
        assert!(
            wf_dir.join("workflow.json").is_file(),
            "sansheng-liubu/workflow.json 应被解包"
        );
        assert!(
            wf_dir.join("roles").join("taizi.md").is_file(),
            "sansheng-liubu/roles/taizi.md 应被解包"
        );
        assert!(
            wf_dir.join("scripts").join("scheduler.py").is_file(),
            "sansheng-liubu/scripts/scheduler.py 应被解包"
        );
        let v = std::fs::read_to_string(paths::bundle_version_file()).unwrap();
        assert_eq!(v.trim(), BUNDLE_VERSION);

        // 2) VERSION 匹配则跳过：故意改 instructions.md，再 ensure，不应覆写
        std::fs::write(&bundle.instructions_md, "USER TOUCHED").unwrap();
        bundle.ensure_extracted().unwrap();
        let content = std::fs::read_to_string(&bundle.instructions_md).unwrap();
        assert_eq!(
            content, "USER TOUCHED",
            "VERSION 匹配时不应覆写已存在的 bundle 文件"
        );

        cleanup(&tmp);
    }

    #[test]
    fn connector_skill_cache_requires_complete_domain_sets() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(&bundle.skills_dir).unwrap();

        assert!(!bundle.cached_feishu_skills_visible());
        assert!(!bundle.cached_wecom_skills_visible());
        assert!(!bundle.cached_dingtalk_skills_visible());
        assert!(!bundle.cached_tmeet_skills_visible());

        for dir in LARK_SKILL_DIRS {
            let path = bundle.skills_dir.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in WECOM_SKILL_DIRS {
            let path = bundle.skills_dir.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in DINGTALK_SKILL_DIRS {
            let path = bundle.skills_dir.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in TMEET_SKILL_DIRS {
            let path = bundle.skills_dir.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        assert!(bundle.cached_feishu_skills_visible());
        assert!(bundle.cached_wecom_skills_visible());
        assert!(bundle.cached_dingtalk_skills_visible());
        assert!(bundle.cached_tmeet_skills_visible());

        std::fs::write(paths::pinvou3_home().join("feishu_disabled"), "1").unwrap();
        std::fs::write(paths::pinvou3_home().join("wecom_disabled"), "1").unwrap();
        std::fs::write(paths::pinvou3_home().join("dingtalk_disabled"), "1").unwrap();
        std::fs::write(paths::pinvou3_home().join("tmeet_disabled"), "1").unwrap();
        assert!(!bundle.cached_feishu_skills_visible());
        assert!(!bundle.cached_wecom_skills_visible());
        assert!(!bundle.cached_dingtalk_skills_visible());
        assert!(!bundle.cached_tmeet_skills_visible());
        std::fs::remove_file(paths::pinvou3_home().join("feishu_disabled")).unwrap();
        std::fs::remove_file(paths::pinvou3_home().join("wecom_disabled")).unwrap();
        std::fs::remove_file(paths::pinvou3_home().join("dingtalk_disabled")).unwrap();
        std::fs::remove_file(paths::pinvou3_home().join("tmeet_disabled")).unwrap();

        std::fs::remove_file(bundle.skills_dir.join(LARK_SKILL_DIRS[0]).join("SKILL.md")).unwrap();
        std::fs::remove_file(bundle.skills_dir.join(WECOM_SKILL_DIRS[0]).join("SKILL.md")).unwrap();
        std::fs::remove_file(
            bundle
                .skills_dir
                .join(DINGTALK_SKILL_DIRS[0])
                .join("SKILL.md"),
        )
        .unwrap();
        std::fs::remove_file(bundle.skills_dir.join(TMEET_SKILL_DIRS[0]).join("SKILL.md")).unwrap();
        assert!(!bundle.cached_feishu_skills_visible());
        assert!(!bundle.cached_wecom_skills_visible());
        assert!(!bundle.cached_dingtalk_skills_visible());
        assert!(!bundle.cached_tmeet_skills_visible());
        cleanup(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn shell_env_hook_keeps_cli_context_and_filters_credentials() {
        use std::collections::HashMap;
        use std::process::Command;

        let tmp = tempdir();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            std::path::Path::new(&tmp).join(".bash_profile"),
            "printf 'PROFILE_STDOUT=must-not-inject'\n\
             export PATH=/opt/custom-cli/bin:/usr/bin:/bin\n\
             export CUSTOM_FROM_PROFILE=loaded\n",
        )
        .unwrap();
        let script = std::path::Path::new(&tmp).join("shell_env.sh");
        std::fs::write(&script, SHELL_ENV_SH).unwrap();
        let output = Command::new("bash")
            .arg(&script)
            .env_clear()
            .env("HOME", &tmp)
            .env("USER", "pinvou-test")
            .env("SHELL", "/bin/bash")
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_RUNTIME_DIR", "/run/user/1000")
            .env("CUSTOM_SDK_HOME", "/opt/custom-sdk")
            .env("HTTPS_PROXY", "http://127.0.0.1:7890")
            .env("OPENAI_API_KEY", "must-not-leak")
            .env("PINVOU3_MCP_SECRET_AMAP_KEY", "must-not-leak")
            .env("SSH_AUTH_SOCK", "/run/user/1000/ssh-agent")
            .env("NODE_OPTIONS", "--require=/tmp/inject.js")
            .env(
                "PRIVATE_INDEX",
                "https://user:password@example.invalid/simple",
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "shell_env hook failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let vars = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>();

        assert_eq!(
            vars.get("PATH").map(String::as_str),
            Some("/opt/custom-cli/bin:/usr/bin:/bin")
        );
        assert_eq!(
            vars.get("CUSTOM_FROM_PROFILE").map(String::as_str),
            Some("loaded")
        );
        assert_eq!(
            vars.get("XDG_RUNTIME_DIR").map(String::as_str),
            Some("/run/user/1000")
        );
        assert_eq!(
            vars.get("CUSTOM_SDK_HOME").map(String::as_str),
            Some("/opt/custom-sdk")
        );
        assert_eq!(
            vars.get("HTTPS_PROXY").map(String::as_str),
            Some("http://127.0.0.1:7890")
        );
        assert!(
            !vars.contains_key("PROFILE_STDOUT"),
            "登录 profile 在 marker 前的 KEY=VALUE 输出不得被注入"
        );
        for key in [
            "OPENAI_API_KEY",
            "PINVOU3_MCP_SECRET_AMAP_KEY",
            "SSH_AUTH_SOCK",
            "NODE_OPTIONS",
            "PRIVATE_INDEX",
        ] {
            assert!(
                !vars.contains_key(key),
                "敏感变量 {key} 不得进入 exec_shell"
            );
        }

        cleanup(&tmp);
    }

    /// 已下架预置技能的清理:市场标记的删、无标记裸残留的删、用户上传(upload:)的保。
    #[test]
    fn cleanup_removed_marketplace_skills_respects_upload_marker() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(&bundle.skills_dir).unwrap();

        // pua:本市场装的(标记匹配)→ 应删
        let pua = bundle.skills_dir.join("pua");
        std::fs::create_dir_all(&pua).unwrap();
        std::fs::write(pua.join(".installed-from"), "pinvou3-marketplace:pua").unwrap();
        // huashu-nuwa:无标记裸残留 → 应删
        let nuwa = bundle.skills_dir.join("huashu-nuwa");
        std::fs::create_dir_all(&nuwa).unwrap();
        // brainstorming:用户上传的同名 → 应保
        let brainstorm = bundle.skills_dir.join("brainstorming");
        std::fs::create_dir_all(&brainstorm).unwrap();
        std::fs::write(brainstorm.join(".installed-from"), "upload:my.zip").unwrap();

        bundle.cleanup_removed_marketplace_skills().unwrap();

        assert!(!pua.exists(), "市场标记的 pua 应被删");
        assert!(!nuwa.exists(), "无标记的 huashu-nuwa 残留应被删");
        assert!(brainstorm.exists(), "用户上传(upload:)的同名目录应保留");

        cleanup(&tmp);
    }

    #[test]
    fn forkguard_builtin_visual_skill_uses_bundle_root_and_safe_name() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();

        bundle.write_builtin_skills().unwrap();

        let skill_path = bundle.skills_dir.join("visual-design").join("SKILL.md");
        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert!(content.contains("name: visual-design"));
        assert!(skill_path.starts_with(&bundle.skills_dir));
        cleanup(&tmp);
    }

    /// 已下架预置 MCP 工具的清理:目录、installed.json、mcp.json、禁用列表都不应残留。
    #[test]
    fn cleanup_removed_marketplace_tools_removes_data_analysis() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();
        let data_dir = paths::bundle_mcp_servers_dir().join("data_analysis");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("manifest.json"),
            r#"{
                "id":"data_analysis",
                "name":"数据分析与可视化",
                "description":"removed",
                "version":"1",
                "icon":"bar-chart-3",
                "category":"办公",
                "mcp_tools":["mcp_data_analysis_build_dashboard"],
                "command":"python",
                "args":["server.py"]
            }"#,
        )
        .unwrap();
        let marketplace_dir = paths::pinvou3_home().join("marketplace");
        std::fs::create_dir_all(&marketplace_dir).unwrap();
        std::fs::write(
            marketplace_dir.join("installed.json"),
            r#"["weather","data_analysis"]"#,
        )
        .unwrap();
        std::fs::write(
            paths::mcp_config_path(),
            r#"{"servers":{"data_analysis":{"command":"python","args":["server.py"]},"weather":{"command":"python","args":["server.py"]}}}"#,
        )
        .unwrap();
        crate::features::marketplace::save_disabled_connectors(&[
            "data_analysis".to_string(),
            "weather".to_string(),
        ]);

        bundle.cleanup_removed_marketplace_tools().unwrap();

        assert!(!data_dir.exists(), "data_analysis 运行目录应被删");
        let installed = std::fs::read_to_string(marketplace_dir.join("installed.json")).unwrap();
        assert!(
            !installed.contains("data_analysis"),
            "installed.json 不应残留 data_analysis"
        );
        let mcp = std::fs::read_to_string(paths::mcp_config_path()).unwrap();
        assert!(
            !mcp.contains("data_analysis"),
            "mcp.json 不应残留 data_analysis server"
        );
        let disabled = crate::features::marketplace::load_disabled_connectors();
        assert!(
            !disabled.contains(&"data_analysis".to_string()),
            "disabled_connectors 不应残留 data_analysis"
        );

        cleanup(&tmp);
    }

    /// 旧版 mcp.json 的 present server key 是 `pinvou`(与产品名差一个 3,模型采样必漂成
    /// pinvou3 → `Failed to find MCP server: pinvou3`)。升级时 ensure_builtin_mcp_servers
    /// 必须迁成 `pinvou3`、删干净旧 `pinvou`,且不碰 marketplace 已装条目。
    #[test]
    fn migrates_legacy_pinvou_server_key_to_pinvou3() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();
        // 模拟旧版本写下的 mcp.json:present server 仍叫 pinvou + 一个 marketplace 条目(weather)。
        std::fs::write(
            &bundle.mcp_json,
            r#"{"servers":{"pinvou":{"command":"python3","args":["/old/present.py"]},"weather":{"command":"python3","args":["/x/w.py"]}}}"#,
        )
        .unwrap();
        bundle.ensure_builtin_mcp_servers().unwrap();
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
        let servers = mcp["servers"].as_object().unwrap();
        assert!(
            servers.contains_key("pinvou3"),
            "应迁到 pinvou3,实际={:?}",
            servers.keys().collect::<Vec<_>>()
        );
        assert!(!servers.contains_key("pinvou"), "旧 pinvou 应删除,不留残");
        assert!(
            servers.contains_key("weather"),
            "marketplace 条目 weather 不应被迁移误删"
        );
        cleanup(&tmp);
    }

    /// forkguard(composer): 静态层 composer 接管后,底座的 Personality/
    /// Session Longevity/Efficient Approvals/taxonomy 不得再进 prompt,
    /// pinvou3 自有的 mode 块 + 瘦身 compact 模板必须在。上游 sync 后此测试
    /// 失败 = set_static_prompt_composer_override fork patch 被合丢。
    #[test]
    fn forkguard_static_composer_takes_over_static_layers() {
        install_prompt_overrides(); // OnceLock 幂等,谁先调都一样

        // v0.8.57:上游删了 `system_prompt_for_mode(AppMode)`(prompt 改 mode-independent)。
        // 改用 mode-independent 入口;pinvou3 composer 以常量 Yolo 构造 ctx → 静态层 = base.md
        // + MODE_EXECUTE_MD(生产单 Yolo-Auto)。原 Plan-mode 断言移除——static prompt 不再分模式
        // (Plan 前端入口已下线;若恢复,mode 走 per-turn <runtime_prompt> tag,非静态前缀)。
        let tmp = tempdir();
        std::fs::create_dir_all(&tmp).unwrap();
        let prompt = deepseek_tui::prompts::system_prompt_for_mode_with_context_and_skills(
            std::path::Path::new(&tmp),
            None,
            None,
            None,
            None,
        );
        let yolo = deepseek_tui::prompts::system_prompt_flat_text(&prompt);
        // 干掉的底座块(composer 密封 + gate:Compaction 模板/Sub-agents/Thinking budget/
        // Tier 体系/全模式 Runtime Policy Reference 实证死重)
        for gone in [
            "Personality: Calm",
            "## Session Longevity",
            "## Efficient Approvals",
            "## Core Tool Taxonomy",
            "Compaction Relay Template",
            "Sub-agents",
            "Thinking budget",
            "Tier ",                       // 九层已删,不许残留悬空 tier 引用
            "## Runtime Policy Reference", // v0.8.57 上游新增全模式块,composer gate 抑制
        ] {
            assert!(!yolo.contains(gone), "底座静态块应被 composer 干掉: {gone}");
        }
        // composer 静态层现在只剩 Mode —— 宪法/裁决/Voice 已折叠进 instructions.md §底线
        // (单一来源,2026-06-15 第四轮),不再出现在静态层。
        assert!(
            yolo.contains("## Mode: Execute"),
            "composer 静态层应含 Mode 块"
        );
        for folded in [
            "CONSTITUTION OF PINVOU3",
            "### When directives conflict",
            "### Voice",
        ] {
            assert!(
                !yolo.contains(folded),
                "宪法层应已折叠出静态层(并入 instructions): {folded}"
            );
        }
    }

    /// forkguard(composer): 完整合成路径上,底座在 compose 之外追加的
    /// Context Management(含 prompt-cache 教学)与 COMPACT_TEMPLATE 也要被
    /// composer 抑制(prompts.rs 的 static_prompt_composer().is_none() gate)。
    #[test]
    fn forkguard_static_composer_suppresses_context_mgmt_appends() {
        install_prompt_overrides();

        let tmp = tempdir();
        std::fs::create_dir_all(&tmp).unwrap();
        // v0.8.57:上游把 system prompt 改 mode-independent,该函数签名去掉首个 AppMode 参数。
        let prompt = deepseek_tui::prompts::system_prompt_for_mode_with_context_and_skills(
            std::path::Path::new(&tmp),
            None,
            None,
            None,
            None,
        );
        let text = deepseek_tui::prompts::system_prompt_flat_text(&prompt);
        assert!(
            !text.contains("## Context Management"),
            "Context Management 应被 composer 抑制"
        );
        assert!(
            !text.contains("## Runtime Policy Reference"),
            "Runtime Policy Reference 应被 composer gate 抑制(v0.8.57 新增全模式块)"
        );
        assert!(
            !text.contains("Prompt-cache awareness"),
            "prompt-cache 教学应被 composer 抑制"
        );
        // Compaction 模板全删(第二轮瘦身):真实压缩走 canonical_prompt/
        // create_summary,模板无生产者无消费者。底座原版也不许回流。
        assert!(
            !text.contains("Compaction Relay"),
            "Compaction 模板不应出现(pinvou3 已删,底座版也不许回流)"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// forkguard(composer): per-turn `<runtime_prompt>` tag 也受 composer gate
    /// (v0.8.57 上游新增,turn_loop 每请求注入 transient user 消息)。pinvou3 单
    /// Yolo-Auto 下 tag 恒定零信息,且其解释文档(Runtime Policy Reference)已被
    /// composer 抑制——无解释 internal tag 会诱发模型复述。本测试断言 composer
    /// 安装后 `static_prompt_composer_installed()` 为真(turn_loop gate 的读数);
    /// gate 行本身由 fork-guard 指纹守。
    #[test]
    fn forkguard_static_composer_gates_runtime_prompt_tag() {
        install_prompt_overrides();
        assert!(
            deepseek_tui::prompts::static_prompt_composer_installed(),
            "composer 安装后 installed() 应为 true → turn_loop 不再注入 <runtime_prompt> tag"
        );
    }

    #[test]
    fn dingtalk_skill_gate_extracts_and_removes_official_mono_skill() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();

        bundle.apply_dingtalk_skills(true).unwrap();
        let skill = bundle.skills_dir.join("dws");
        assert!(skill.join("SKILL.md").is_file());
        assert!(skill
            .join("references")
            .join("global-reference.md")
            .is_file());
        assert!(bundle.skills_dir.join("NOTICE-dingtalk.md").is_file());

        bundle.apply_dingtalk_skills(false).unwrap();
        assert!(!skill.exists());
        assert!(!bundle.skills_dir.join("NOTICE-dingtalk.md").exists());

        cleanup(&tmp);
    }

    fn tempdir() -> String {
        // 叠加 pid + 进程内原子计数器:pid 保证跨进程唯一(双终端 cargo test),
        // unique_suffix 保证进程内唯一(纯纳秒会碰撞;曾因两个 bundle 测试落同纳秒,
        // 临时目录同名 → 互相删文件)。与 scheduled/tasks.rs::temp_home 一致。
        let id = format!(
            "{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        );
        std::env::var_os("TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(format!("pinvou3-bundle-test-{id}"))
            .to_string_lossy()
            .into_owned()
    }

    fn cleanup(dir: &str) {
        std::env::remove_var("PINVOU3_HOME");
        let _ = std::fs::remove_dir_all(dir);
    }
}
