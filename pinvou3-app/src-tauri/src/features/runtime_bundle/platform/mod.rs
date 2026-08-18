//! pinvou3 内置 bundle：随 app 编译进去的 instructions.md / mcp.json / skills 模板，
//! 首次启动时解包到 `~/.pinvou3/bundle/`。
//!
//! 与 user/ 严格分离：bundle/ 每次升级被覆写，user/ 永远不动。
//! 解包用 `bundle/VERSION` 比对 [`BUNDLE_VERSION`]，相同则跳过。

use std::path::PathBuf;

use include_dir::{include_dir, Dir};

use crate::platform::paths;

mod extraction;
pub use extraction::Pinvou3Bundle;

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

/// 腾讯会议官方 CLI skill(tmeet-skill,MIT,同步自 github.com/TencentCloud/
/// tencentmeeting-cli 对应 tag 的 `skills/tmeet-skill/`,见 NOTICE-tmeet.md)。
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
/// 注:「视觉设计」内置 skill 在 VERSION gate 之前由 write_builtin_skills 每启动防御性写出,
///     不依赖版本号 bump(同 write_mcp_servers）。
/// 0.10: 接入飞书官方域技能(lark-shared + calendar/doc/drive/sheets/im/task/wiki/base):
///       include_dir 内嵌 + 启动解包到 bundle_skills_dir,供 SkillRegistry 发现
/// 0.11: Windows 敏感路径硬拦截新增 PowerShell hook,并补充 Credential Manager 相关拦截规则
/// 0.13: 接入企微官方域技能(wecomcli-*,MIT):独立 wecom-skills/ 内嵌 + 独立门控
/// 0.14: 接入钉钉官方 dws skill + Linux ARM64 内置 dws CLI
/// 0.15: 增加 exec_shell 登录终端环境过滤 hook(shell_env.sh)
/// 0.16: 接入腾讯会议官方 tmeet CLI skill
/// 0.17: 接入腾讯 ima OpenAPI Skill（原生受控工具）
/// 0.18: 连接器 CLI 统一为首次使用在线安装；原生 CLI 按平台 lock 校验后落用户目录
/// 0.19: 增加多智能体深度护栏 hook，防止模型用单次 `max_depth` 覆盖会话上限
/// 0.20: 多智能体深度上限调整为两层，正数覆盖仍拦截
/// 0.21: 完整移除三省六部及专家团队
/// 0.22: 连接器技能全量同步（lark 1.0.87 / dws 1.0.58 / tmeet 1.0.15 / wecom 适配）。
///       四棵技能树不参与内容哈希，须 bump 语义版本让已连接用户启动即同步刷新
///       （否则要等首帧后 refresh_connector_auth_gates 补刷）
pub const BUNDLE_VERSION: &str = concat!("0.22-", env!("BUNDLE_INSTRUCTIONS_HASH"));

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
/// 与拆分前 instructions.md 逐字节相等。
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
/// 行为引导大头已由 `build_send_message_op`(Plan 段经 `SessionPolicy::plan_reminder`)
/// 每 turn `<system-reminder>` 注入,静态块只立常驻事实;底座 YOLO_MODE/AUTO_APPROVAL/
/// Session Longevity/Efficient Approvals 的逐条教学全不保留。
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

/// 多智能体会话的两层深度护栏。仅在多智能体 EngineConfig 中挂载，
/// 普通对话不使用。会话上限允许一级代理再派生一层；hook 拦截主会话显式
/// 传入的正数深度覆盖字段（嵌套子代理的调用不经过 ToolCallBefore，由
/// 继承上限与准入/并发额度兜底）；Workflow 的
/// `source_path` 要求改用 inline script/plan，避免 hook 无法检查文件内参数。
pub const MULTIAGENT_DEPTH_GUARD_SH: &str =
    include_str!("../../../../resources/common/bundle/multiagent_depth_guard.sh");
pub const MULTIAGENT_DEPTH_GUARD_PS1: &str =
    include_str!("../../../../resources/common/bundle/multiagent_depth_guard.ps1");

/// 内嵌的 exec_shell CLI 兼容环境 hook：读取登录 shell 环境并过滤凭证。
pub const SHELL_ENV_SH: &str = include_str!("../../../../resources/common/bundle/shell_env.sh");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;

    #[test]
    fn work_instructions_use_only_canonical_model_visible_tools() {
        let rendered = instructions_md();
        for retired in [
            "read_file",
            "write_file",
            "list_dir",
            "file_search",
            "exec_shell",
            "checklist_write",
        ] {
            assert!(
                !rendered.contains(retired),
                "retired tool leaked: {retired}"
            );
        }
        for canonical in ["File(action=", "Bash(action=", "todo_write"] {
            assert!(
                rendered.contains(canonical),
                "canonical guidance missing: {canonical}"
            );
        }
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

    fn run_depth_guard(bundle: &Pinvou3Bundle, tool: &str, args: &str) -> std::process::Output {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("powershell.exe");
            command
                .arg("-NoProfile")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&bundle.multiagent_depth_guard_ps1);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = std::process::Command::new("bash");
            command.arg(&bundle.multiagent_depth_guard_sh);
            command
        };
        command
            .env("DEEPSEEK_TOOL_NAME", tool)
            .env("DEEPSEEK_TOOL_ARGS", args)
            .output()
            .expect("run multi-agent depth guard")
    }

    /// 测试 bundle 解包的两个场景：首次解包成功 + VERSION 匹配时不覆写。
    /// 借 crate 级唯一 bridge::paths::tests::ENV_LOCK 跟其他 mutate PINVOU3_HOME 的测试串行化，
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
        assert!(bundle.multiagent_depth_guard_sh.is_file());
        assert!(bundle.multiagent_depth_guard_ps1.is_file());
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
    fn multiagent_depth_guard_enforces_two_level_inputs() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();
        bundle.ensure_extracted().unwrap();

        let positive = run_depth_guard(&bundle, "agent", r#"{"prompt":"inspect","max_depth":2}"#);
        assert_eq!(positive.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&positive.stderr).contains("at most two child levels"),
            "拒绝原因必须能指导模型重试: {positive:?}"
        );

        let inherited = run_depth_guard(&bundle, "agent", r#"{"prompt":"inspect"}"#);
        assert!(
            inherited.status.success(),
            "复杂一级委派省略深度参数时应继承会话两层上限: {inherited:?}"
        );

        let leaf = run_depth_guard(&bundle, "agent", r#"{"prompt":"inspect","max_depth":0}"#);
        assert!(leaf.status.success(), "叶子委派应通过: {leaf:?}");

        let positive_one =
            run_depth_guard(&bundle, "agent", r#"{"prompt":"inspect","max_depth":1}"#);
        assert_eq!(
            positive_one.status.code(),
            Some(2),
            "正数覆盖会让嵌套代理逐层扩大上限，必须拒绝"
        );

        let alias = run_depth_guard(
            &bundle,
            "agent",
            r#"{"prompt":"inspect","max_spawn_depth":2}"#,
        );
        assert_eq!(alias.status.code(), Some(2));

        let quoted_example = run_depth_guard(
            &bundle,
            "agent",
            r#"{"prompt":"review this JSON: {\"max_depth\":2}"}"#,
        );
        assert!(
            quoted_example.status.success(),
            "任务正文里的转义示例不得误伤: {quoted_example:?}"
        );

        let workflow = run_depth_guard(
            &bundle,
            "workflow",
            r#"{"script":"return task({ description: 'x', maxDepth: 1 });"}"#,
        );
        assert_eq!(workflow.status.code(), Some(2));

        let opaque_workflow = run_depth_guard(
            &bundle,
            "workflow",
            r#"{"source_path":"workflows/review.workflow.js"}"#,
        );
        assert_eq!(opaque_workflow.status.code(), Some(2));

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

    /// include_dir! 内嵌树中的 `__pycache__/` 与 `*.pyc` 不得物化到用户 bundle
    /// (在仓库里直接运行技能脚本会产生这些编译缓存,详见 extract_dir 文档注释)。
    /// 覆盖实际走 extract_dir 的两棵树(lark skills 与 dws);构建机存在 pycache 时
    /// 验证排除逻辑生效,不存在时断言"零物化 pyc"作为回归基线。
    #[test]
    fn extract_dir_skips_python_compilation_caches() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let bundle = Pinvou3Bundle::paths();

        bundle.apply_feishu_skills(true).unwrap();
        bundle.apply_dingtalk_skills(true).unwrap();

        let mut caches: Vec<std::path::PathBuf> = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        if p.file_name().is_some_and(|n| n == "__pycache__") {
                            out.push(p);
                        } else {
                            walk(&p, out);
                        }
                    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pyc")) {
                        out.push(p);
                    }
                }
            }
        }
        walk(&bundle.skills_dir, &mut caches);
        assert!(caches.is_empty(), "物化出了 Python 编译缓存: {caches:?}");
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
