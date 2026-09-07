//! pinvou3 内置 bundle：随 app 编译进去的 instructions.md / mcp.json / skills 模板，
//! 首次启动时解包到 `~/.pinvou3/bundle/`。
//!
//! 与 user/ 严格分离：bundle/ 每次升级被覆写，user/ 永远不动。
//! 解包用 `bundle/VERSION` 比对 [`BUNDLE_VERSION`]，相同则跳过。

use std::path::PathBuf;

use include_dir::{Dir, include_dir};

use crate::platform::paths;

mod extraction;
pub use extraction::Pinvou3Bundle;

/// 飞书官方域技能（lark-*，MIT，sync 自 github.com/larksuite/cli `skills/`）：
/// 编译期内嵌整个 skills 目录树（各域 SKILL.md + references/*.md + NOTICE.md）。
/// 启动解包到 `bundle_skills_dir`，供引擎 `SkillRegistry` 发现、`load_skill` 渐进披露。
static LARK_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/lark-skills");

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

// 企微技能目录表（14 新名 + 0.1.9 legacy 名）已下沉到
// `crate::platform::connector_skills` 作为单一真相源：marketplace 注册表
// （wecom 卡 skills 列表）、扁平布局迁移、`cli_bundle_of_skill` 反查与
// `legacy_cli_records` 首启登记与本模块的门控写/删共用（五轮评审必修 3：
// marketplace 侧曾按 0.1.9 旧表写死 7 技能，与本表分叉）。
// 此处 re-export 保持本模块及 extraction 的既有引用不变。
pub(crate) use crate::platform::connector_skills::{WECOM_LEGACY_SKILL_DIRS, WECOM_SKILL_DIRS};

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
/// 0.23: wecom-cli 升 1.1.0：技能树按上游服务模型重排为 14 个（msg→message、
///       schedule→calendar，新增 disk/doc-manage/email/media/shared/sheet/smartpage），
///       旧目录（wecomcli-msg/wecomcli-schedule）启动门控时清理。
/// 0.25: wecom sheet/smartsheet/smartpage routing-rule unification and
///       fixes, plus calendar/meeting/email/message/media doc-audit
///       fixes (15 findings; all registered in NOTICE-wecom.md). Takes
///       the slot reserved for #366 by the 0.28 entry; the
///       extracted-VERSION gate is inequality-only, so landing order
///       and skipped numbers stay safe. Skill trees are excluded from
///       the content hash, so the semantic bump is required for
///       connected users to refresh at startup.
/// 0.26: tmeet-skill doc-audit fixes (seventh review round), registered
///       as entries 7-12 in NOTICE-tmeet.md: contact downstream command
///       list, underscore command names, crossed cause/solution columns
///       in the error table, hard-constraint item reference,
///       playback-address wording, and a digits-only meeting-code
///       reminder. Takes the slot reserved for #362 by the 0.28 entry;
///       the extracted-VERSION gate is inequality-only, so landing
///       order and skipped numbers stay safe. Skill trees are excluded
///       from the content hash, so the semantic bump is required for
///       connected users to refresh at startup.
/// 0.27: dws doc-audit fixes, 11 findings (PR #359, registered in
///       NOTICE-dingtalk.md). The dws skill tree is excluded from the
///       instructions content hash like the other connector trees, so
///       connected installs skip re-extraction unless the semantic base
///       changes. Lands after 0.28 (lark-skills #365), the 0.26
///       re-take (tmeet #362), and the 0.25 re-take (wecom #366) are
///       already on main; the extracted-VERSION gate is inequality-only,
///       so 0.25 -> 0.27 still changes the build VERSION and triggers
///       startup re-extraction.
/// 0.28: lark-skills doc audit fixes, 31 findings across eight packs
///       (registered in lark-skills/NOTICE.md, including review fixes
///       aligned with CLI v1.0.87 ground truth). Version takes the next
///       free slot after 0.24 (weibo #333), 0.25 (wecom #366),
///       0.26 (tmeet #362), and 0.27 (dws #359); 0.27 was already
///       claimed by #359, and a duplicate number would leave the
///       upgraded build VERSION unchanged between the two PRs and skip
///       startup re-extraction. Skill trees are excluded from the
///       content hash, so the semantic version must be bumped for
///       connected users to refresh at startup (otherwise the refresh
///       waits for the post-first-frame refresh_connector_auth_gates
///       backfill).
pub const BUNDLE_VERSION: &str = concat!("0.27-", env!("BUNDLE_INSTRUCTIONS_HASH"));

/// pinvou3 内置的 instructions 共享骨架（Qwen3.6 适配 prompt），编译时内嵌。
/// 骨架 = 身份/底线/工具与事实通用纪律/怎么干/红线/输出，两个模式层占位行：
/// `{{PINVOU3_MODE_ENV_SECTION}}`（§工作环境 位）与
/// `{{PINVOU3_MODE_ARTIFACT_RULE}}`（§工具与事实 的成品条位）。
/// 拆分说明：work 专属的 §工作环境(L10-13) 与 present_artifact 条(L18) 在原文中
/// 不连续，纯 concat 无法逐字节复原，故骨架留占位行、按模式替换拼装。
pub const INSTRUCTIONS_SHARED_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-shared.md");

/// Work-mode layer: the `## 工作环境` section for artifact-panel and tmp/ semantics, the
/// `## Browser capabilities` section as the model's static discovery entry point for the
/// embedded headed browser, and the present_artifact rule from `## 工具与事实`. Blank lines
/// separate the three sections for [`work_layer_sections`].
pub const INSTRUCTIONS_WORK_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-work.md");

/// Returns the three Work-layer sections: the complete `## 工作环境` section and complete
/// `## Browser capabilities` section without trailing newlines, plus the artifact rule with
/// its trailing newline.
// The input is a compile-time embedded resource whose section structure is
// fixed by the build artifact, and tests lock the rendered output byte for
// byte; if the resource is accidentally modified, the panic surfaces at first
// startup, and no runtime input exists that could make this branch reachable.
#[allow(clippy::expect_used)]
fn work_layer_sections() -> (&'static str, &'static str, &'static str) {
    let (env_section, rest) = INSTRUCTIONS_WORK_MD
        .split_once("\n\n")
        .expect("instructions-work.md must contain the '## 工作环境' section, a blank line, and the '## Browser capabilities' section");
    let (browser_section, artifact_rule) = rest.split_once("\n\n").expect(
        "instructions-work.md must contain '## 工作环境', '## Browser capabilities', and the artifact rule separated by blank lines",
    );
    (env_section, browser_section, artifact_rule)
}

/// 绑定了真实工作目录的普通会话的环境段：仅 `## 工作环境` 一段（含
/// `{{PINVOU3_WORKSPACE_HINT}}` 工作区占位）。与 work 层默认环境段的差异：
/// 工作对象是用户选定的真实目录（改动实时可见），无产出物面板与 `tmp/` 语义。
pub const INSTRUCTIONS_WORK_BOUND_MD: &str =
    include_str!("../../../../resources/common/bundle/instructions-work-bound.md");

/// work 模式完整 instructions（共享骨架 + work 层占位替换）。
/// 与拆分前 instructions.md 逐字节相等。
pub fn instructions_md() -> &'static str {
    static RENDERED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RENDERED.get_or_init(|| {
        let (env_section, browser_section, artifact_rule) = work_layer_sections();
        INSTRUCTIONS_SHARED_MD
            // Replace the environment placeholder with both `## 工作环境` and
            // `## Browser capabilities`. Neither section has a trailing newline, so add
            // the inter-section blank line and one final newline before following content.
            .replace(
                "{{PINVOU3_MODE_ENV_SECTION}}",
                &format!("{env_section}\n\n{browser_section}\n"),
            )
            // 成品条位：占位行整体（含换行）替换为成品条原文 + 补回节间空行。
            .replace(
                "{{PINVOU3_MODE_ARTIFACT_RULE}}\n",
                &format!("{artifact_rule}\n"),
            )
    })
}

/// 绑定真实工作目录的普通会话完整 instructions（共享骨架 + 绑定环境段 +
/// work 层 Browser capabilities 段与成品条）：仅环境段换成绑定变体，浏览器
/// 与成品卡能力与普通会话一致。`instructions_md()` 的逐字节语义不受影响。
#[allow(clippy::expect_used)]
pub fn instructions_work_bound_md(workspace_hint: &str) -> String {
    let (_env_section, browser_section, artifact_rule) = work_layer_sections();
    let env_section = INSTRUCTIONS_WORK_BOUND_MD
        .trim_end()
        .replace("{{PINVOU3_WORKSPACE_HINT}}", workspace_hint);
    INSTRUCTIONS_SHARED_MD
        .replace(
            "{{PINVOU3_MODE_ENV_SECTION}}",
            &format!("{env_section}\n\n{browser_section}\n"),
        )
        .replace(
            "{{PINVOU3_MODE_ARTIFACT_RULE}}\n",
            &format!("{artifact_rule}\n"),
        )
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
// Same as work_layer_sections: the input is a compile-time embedded resource
// whose section structure is fixed by the build artifact; if the resource is
// accidentally modified, the panic surfaces at first startup, and no runtime
// path can reach this branch.
#[allow(clippy::expect_used)]
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
    include_str!("../../../../resources/common/bundle/builtin-skills/visual-design/SKILL.md");

/// Test-only view of the embedded builtin-skill resource tree: binds
/// `Pinvou3Bundle::BUILTIN_RELEASED_SKILL_DIRS` to the actual resources
/// (same pattern as `wecom_skill_dirs_match_embedded_resources`).
#[cfg(test)]
static BUILTIN_SKILLS_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/bundle/builtin-skills");

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
pub const MCP_PYTHON_DEPENDENCY_RUNNER_PY: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/python_dependency_runner.py");

/// Zero-dependency Node.js stdio wrapper for Browser MCP. It coordinates the app-owned
/// native WebView with Rust BrowserManager, then hosts vendored chrome-devtools-mcp through
/// `--browser-url`. Like present_artifact, it is embedded at compile time.
pub const BROWSER_WRAPPER_MJS: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/browser-wrapper.mjs");
pub const BROWSER_WRAPPER_PROTOCOL_MJS: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/browser-wrapper-protocol.mjs");
pub const BROWSER_CORE_PROTOCOL_MJS: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/browser-core-protocol.mjs");

// --- 工具市场：内置 MCP server 资源(编译期内嵌) ---

// --- 工具市场：内置 MCP server 资源 ---
// 编译期内嵌资源已收编到 `marketplace::mcp_catalog`（单一真相源：清单、释放、
// 校验同一份数据）；未安装包不再每启动全量释放到 bundle/mcp-servers/（§4）。

/// Embedded bundle hook scripts — used by the ToolCallBefore hook injected
/// via the bridge to correct skill-based connectors being mistakenly
/// introspected as MCP. The sensitive-path/dangerous-command/sudo hard-deny
/// segments the scripts used to carry have moved to the execpolicy rule
/// engine (features/assistant/safety_deny_rules).
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

/// Browser MCP Node.js runtime fallback: search PATH when bundled Node.js is unavailable,
/// which is expected in some development configurations.
fn find_system_node() -> Option<std::path::PathBuf> {
    let bin = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(bin))
            .find(|c| c.is_file())
    })
}

/// Returns whether the `browser` entry in mcp.json is historical residue owned by this app.
/// The current shape uses Node.js as command and puts the wrapper in args[0]. An early build
/// wrote the wrapper directly as the command in global mcp.json, leaving persistent residue
/// on development machines that ran it. User-defined servers with the same name, such as
/// playwright-mcp, match neither shape and must be preserved.
fn is_browser_wrapper_residue(entry: &serde_json::Value) -> bool {
    let cmd = entry.get("command").and_then(serde_json::Value::as_str);
    let wrapper_in_args = entry
        .get("args")
        .and_then(|a| a.get(0))
        .and_then(serde_json::Value::as_str)
        .map(|a0| a0.ends_with("browser-wrapper.mjs"))
        .unwrap_or(false);
    cmd.map(|c| c.ends_with("browser-wrapper.mjs") || wrapper_in_args)
        .unwrap_or(false)
}

/// Reserve the model-visible `browser` server name for Pinvou's task-owned
/// browser inside a work-session config. A user may already own that name in
/// the global config, so preserve their entry under a deterministic alias
/// instead of overwriting it or letting it masquerade as the built-in browser.
///
/// The first alias is `browser_user`; occupied aliases advance from
/// `browser_user_2`. The global config is never passed to this helper directly:
/// callers operate on the parsed per-session copy.
// Renaming is bounded by the number of user-defined servers, so a u64 suffix
// cannot overflow in practice; the expect documents that invariant instead of
// silently wrapping.
#[allow(clippy::expect_used)]
fn reserve_work_mode_browser_server_name(
    servers: &mut serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    servers.remove("browser").and_then(|existing| {
        if is_browser_wrapper_residue(&existing) {
            return None;
        }

        let mut suffix = 1_u64;
        let alias = loop {
            let candidate = if suffix == 1 {
                "browser_user".to_string()
            } else {
                format!("browser_user_{suffix}")
            };
            if !servers.contains_key(&candidate) {
                break candidate;
            }
            suffix = suffix
                .checked_add(1)
                .expect("MCP server alias namespace exhausted");
        };
        servers.insert(alias.clone(), existing);
        Some(alias)
    })
}

fn install_work_mode_browser_server(
    servers: &mut serde_json::Map<String, serde_json::Value>,
    browser_entry: serde_json::Value,
) -> Option<String> {
    let migrated_name = reserve_work_mode_browser_server_name(servers);

    servers.insert("browser".to_string(), browser_entry);
    migrated_name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;
    use crate::platform::credential_store::{
        CredentialError, CredentialReference, CredentialStore,
    };

    #[derive(Clone, Default)]
    struct RejectingCredentialStore;

    impl CredentialStore for RejectingCredentialStore {
        fn get(&self, _reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
            Err(CredentialError::new("test credential store unavailable"))
        }

        fn set(
            &self,
            _reference: &CredentialReference,
            _value: &str,
        ) -> Result<(), CredentialError> {
            Err(CredentialError::new("test credential store unavailable"))
        }

        fn delete(&self, _reference: &CredentialReference) -> Result<(), CredentialError> {
            Err(CredentialError::new("test credential store unavailable"))
        }
    }

    #[test]
    fn work_mode_browser_server_reserves_name_and_preserves_user_conflicts() {
        let builtin = serde_json::json!({ "command": "pinvou-wrapper" });
        let user_browser = serde_json::json!({
            "command": "playwright-mcp",
            "env": { "USER_SETTING": "kept" }
        });
        let existing_alias = serde_json::json!({ "command": "existing-user-browser" });
        let existing_numbered_alias = serde_json::json!({ "command": "existing-numbered-browser" });
        let mut servers = serde_json::json!({
            "browser": user_browser,
            "browser_user": existing_alias,
            "browser_user_2": existing_numbered_alias,
            "weather": { "command": "weather-mcp" }
        })
        .as_object()
        .unwrap()
        .clone();

        let migrated = install_work_mode_browser_server(&mut servers, builtin.clone());

        assert_eq!(migrated.as_deref(), Some("browser_user_3"));
        assert_eq!(servers["browser"], builtin);
        assert_eq!(servers["browser_user_3"], user_browser);
        assert_eq!(servers["browser_user"], existing_alias);
        assert_eq!(servers["browser_user_2"], existing_numbered_alias);
        assert_eq!(servers["weather"]["command"], "weather-mcp");
    }

    #[test]
    fn work_mode_browser_server_refreshes_its_own_residue_without_aliasing_it() {
        let builtin = serde_json::json!({ "command": "fresh-pinvou-wrapper" });
        let mut servers = serde_json::json!({
            "browser": {
                "command": "node",
                "args": ["/old/browser-wrapper.mjs"]
            },
            "browser_user": { "command": "user-browser" }
        })
        .as_object()
        .unwrap()
        .clone();

        let migrated = install_work_mode_browser_server(&mut servers, builtin.clone());

        assert_eq!(migrated, None);
        assert_eq!(servers["browser"], builtin);
        assert_eq!(servers["browser_user"]["command"], "user-browser");
        assert!(!servers.contains_key("browser_user_2"));
    }

    #[test]
    fn disabled_builtin_still_reserves_browser_name_in_session_copy() {
        let user_browser = serde_json::json!({ "command": "playwright-mcp" });
        let mut servers = serde_json::json!({
            "browser": user_browser,
            "weather": { "command": "weather-mcp" }
        })
        .as_object()
        .unwrap()
        .clone();

        let migrated = reserve_work_mode_browser_server_name(&mut servers);

        assert_eq!(migrated.as_deref(), Some("browser_user"));
        assert!(!servers.contains_key("browser"));
        assert_eq!(servers["browser_user"], user_browser);
        assert_eq!(servers["weather"]["command"], "weather-mcp");
    }

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
    fn work_instructions_include_browser_capability_section() {
        // The model's static discovery entry point, `## Browser capabilities`, must render
        // between `## 工作环境` and `## 工具与事实`. It must name the model-visible tools and
        // override registry-first guidance because built-in Browser MCP is not in the registry.
        let rendered = instructions_md();
        let env_at = rendered
            .find("## 工作环境")
            .expect("the '## 工作环境' section must exist");
        let browser_at = rendered
            .find("## Browser capabilities")
            .expect("the Browser capabilities section must exist");
        let tools_at = rendered
            .find("## 工具与事实")
            .expect("the '## 工具与事实' section must exist");
        assert!(
            env_at < browser_at && browser_at < tools_at,
            "Browser capabilities must appear between the Work environment and Tools and facts sections"
        );
        assert!(
            rendered.contains("mcp_browser_"),
            "the instructions must name model-visible mcp_browser_* tools"
        );
        assert!(
            rendered.contains("registry_sync"),
            "the instructions must override registry-first guidance for the built-in browser"
        );
        assert!(
            rendered.contains("Never describe a successful screenshot as having seen or visually confirmed the page"),
            "the instructions must state that screenshot results are not model-readable image input"
        );
        assert!(
            !rendered.contains("take_screenshot` to inspect visual details"),
            "the instructions must not advertise screenshot output as model visual capability"
        );
    }

    #[test]
    fn browser_unavailability_reason_reports_missing_runtime() {
        // An empty temporary home lacks the platform automation backend and/or wrapper. The
        // static probe must return a model-readable, injectable reason instead of silently
        // hiding browser capability and leaving the model unable to discover it.
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_CDMCP_BIN") };
        let bundle = Pinvou3Bundle::paths();
        let msg = bundle
            .browser_unavailability_reason()
            .expect("missing prerequisites must return an unavailable reason");
        if !crate::platform::capabilities::browser_product_enabled() {
            assert!(
                msg.contains("not enabled in this product build"),
                "a closed release gate must report that the product build does not enable browser tools: {msg}"
            );
        }
        #[cfg(windows)]
        assert!(
            msg.contains("chrome-devtools-mcp"),
            "the message must name the missing runtime: {msg}"
        );
        #[cfg(target_os = "linux")]
        assert!(
            msg.contains("WebKitWebDriver") || msg.contains("browser wrapper"),
            "the message must name the missing Linux WebDriver or wrapper: {msg}"
        );
        #[cfg(target_os = "macos")]
        if crate::platform::capabilities::browser_product_enabled() {
            assert!(
                msg.contains("browser wrapper"),
                "preview or released builds must name the missing macOS wrapper: {msg}"
            );
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        assert!(
            msg.contains("not enabled in this product build"),
            "the message must state that this platform has no released automation backend: {msg}"
        );
        assert!(
            msg.contains("mcp_browser_"),
            "the message must identify browser tools as unavailable: {msg}"
        );
        cleanup(&tmp);
    }

    #[test]
    fn browser_last_error_accepts_only_fresh_whitelisted_codes() {
        let now = 2_000_000_u64;
        let mappings = super::extraction::BROWSER_LAST_ERROR_HINTS;
        assert_eq!(
            mappings.len(),
            8,
            "the stable persistence contract has eight codes"
        );
        for &(expected_code, expected_hint) in mappings {
            let known = serde_json::json!({
                "code": expected_code,
                "at": now - 10,
                "reason": r#"C:\Users\private\workspace"#,
            })
            .to_string();
            let actual = super::extraction::browser_last_error_hint(&known, now)
                .expect("every whitelisted code should have a static hint");
            assert_eq!(actual, (expected_code, expected_hint));
            assert!(!actual.1.contains("Users"));
            assert!(!actual.1.contains("workspace"));
        }

        let cancelled = format!(r#"{{"code":"browser/request-cancelled","at":{}}}"#, now - 1);
        assert_eq!(
            super::extraction::browser_last_error_hint(&cancelled, now),
            None,
            "ordinary cancellation is not a persistent availability failure"
        );

        let legacy_with_path = format!(
            r#"{{"reason":"transport closed at /Users/private/project","at":{}}}"#,
            now - 1
        );
        assert_eq!(
            super::extraction::browser_last_error_hint(&legacy_with_path, now),
            None,
            "legacy raw reasons must never be copied into model-visible instructions"
        );

        let expired = format!(
            r#"{{"code":"browser/host-backend-unavailable","at":{}}}"#,
            now - 24 * 60 * 60 - 1
        );
        assert_eq!(
            super::extraction::browser_last_error_hint(&expired, now),
            None,
            "stale availability failures must expire"
        );

        let future = format!(
            r#"{{"code":"browser/host-backend-unavailable","at":{}}}"#,
            now + 301
        );
        assert_eq!(
            super::extraction::browser_last_error_hint(&future, now),
            None,
            "crafted future timestamps must not stay fresh indefinitely"
        );
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

    #[test]
    fn work_bound_instructions_render_workspace_hint_without_panel_tmp_semantics() {
        let rendered = instructions_work_bound_md(
            "你正在用户选择的工作目录 `/repo/demo` 中工作,相对路径即相对该目录;",
        );
        // 工作区占位渲染正确。
        assert!(rendered.contains("你正在用户选择的工作目录 `/repo/demo` 中工作"));
        assert!(!rendered.contains("{{PINVOU3_WORKSPACE_HINT}}"));
        // 产出物面板/tmp 纪律不出现（环境段否定式提及"没有产出物面板与 tmp/ 语义"
        // 是刻意保留的行为指引）。
        assert!(!rendered.contains("自动落到本会话专属工作目录"));
        assert!(!rendered.contains("只有**最终成品**"));
        assert!(!rendered.contains("产出用**相对路径**写"));
        // 浏览器段与成品条保留（普通会话能力不变）。
        assert!(rendered.contains("## Browser capabilities"));
        assert!(rendered.contains("mcp_pinvou3_present_artifact"));
        // 占位行无残留；骨架结构保持：绑定环境段位于 §底线 与 §工具与事实 之间，
        // Browser capabilities 段位于环境段之后。
        assert!(!rendered.contains("{{PINVOU3_MODE_ENV_SECTION}}"));
        assert!(!rendered.contains("{{PINVOU3_MODE_ARTIFACT_RULE}}"));
        let bottom = rendered.find("## 底线").unwrap();
        let env = rendered.find("## 工作环境").unwrap();
        let browser = rendered.find("## Browser capabilities").unwrap();
        let tools = rendered.find("## 工具与事实").unwrap();
        assert!(bottom < env && env < browser && browser < tools);
    }

    #[test]
    fn work_instructions_unbound_unaffected_by_bound_variant() {
        // 绑定变体的存在不影响未绑定渲染：默认 work instructions 保持原语义。
        let rendered = instructions_md();
        assert!(rendered.contains("自动落到本会话专属工作目录"));
        assert!(!rendered.contains("用户选择的工作目录"));
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

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
        assert!(
            paths::bundle_mcp_python_runner().is_file(),
            "Python MCP 依赖启动器应被解包"
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
        // 市场 MCP 包按「能力包」模型按需释放（§4）：启动不再全量释放，未安装的
        // canva-mcp 在旧布局（bundle/mcp-servers/）与新布局（bundles/<id>/mcp/）
        // 都不应占盘。
        assert!(
            !paths::bundle_mcp_servers_dir().join("canva-mcp").exists(),
            "未安装包不应释放到旧布局"
        );
        let canva_pkg_mcp = crate::features::marketplace::mcp_catalog::package_mcp_dir("canva-mcp");
        assert!(!canva_pkg_mcp.exists(), "未安装包不应释放到新布局");
        // 安装后释放到新布局：bundles/<id>/mcp/manifest.json。
        crate::features::marketplace::MarketplaceManager::new()
            .install("canva-mcp", &std::collections::HashMap::new())
            .unwrap();
        let canva_manifest = canva_pkg_mcp.join("manifest.json");
        assert!(
            canva_manifest.is_file(),
            "Canva 可画 manifest 应释放到包目录"
        );
        let canva_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&canva_manifest).unwrap()).unwrap();
        assert_eq!(canva_json["id"], "canva-mcp");
        assert_eq!(canva_json["servers"][0]["name"], "canva_mcp");
        assert!(
            !canva_pkg_mcp.join("server.py").exists(),
            "Canva 远程 MCP 不应解包本地 server.py"
        );
        // 腾讯文档 manifest 应解包,且四个官方远程 server + 无 scheme Authorization 声明完整。
        // 未安装包在新架构下只住 mcp_catalog 内嵌目录（不落盘）；先登记安装再校验释放。
        let store = crate::features::marketplace::store::BundleStore::new();
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "tencent-docs",
                    crate::features::marketplace::store::BundleSource::Preset,
                ),
            )
            .unwrap();
        crate::features::marketplace::mcp_catalog::ensure_package_released("tencent-docs")
            .expect("腾讯文档包应释放");
        let tdoc_dir = crate::features::marketplace::mcp_catalog::package_mcp_dir("tencent-docs");
        let tdoc_manifest = tdoc_dir.join("manifest.json");
        assert!(tdoc_manifest.is_file(), "腾讯文档 manifest 应被解包");
        let tdoc_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tdoc_manifest).unwrap()).unwrap();
        assert_eq!(tdoc_json["id"], "tencent-docs");
        let server_names: Vec<&str> = tdoc_json["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            server_names,
            vec!["tencent-docs", "tdoc-slide", "tdoc-doc", "tdoc-sheet"]
        );
        assert_eq!(
            tdoc_json["secret_headers"][0]["header"], "Authorization",
            "Token 走 Authorization 头"
        );
        assert_eq!(
            tdoc_json["secret_headers"][0]["scheme"], "",
            "scheme 必须为空——官方端点要求原始 Token,不能加 Bearer 前缀"
        );
        assert!(
            !tdoc_dir.join("server.py").exists(),
            "腾讯文档远程 MCP 不应解包本地 server.py"
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
    fn secret_migration_failure_still_releases_runner_and_preserves_retryable_legacy_python_tool() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();
        let server_root = paths::bundle_mcp_servers_dir();

        let weather_dir = server_root.join("weather");
        std::fs::create_dir_all(&weather_dir).unwrap();
        let legacy_weather = br#"{"id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"],"env":{"AMAP_KEY":"legacy-fixture-value"}}"#;
        std::fs::write(weather_dir.join("manifest.json"), legacy_weather).unwrap();

        let gongwen_dir = server_root.join("gongwen");
        std::fs::create_dir_all(&gongwen_dir).unwrap();
        std::fs::write(gongwen_dir.join("server.py"), "print('legacy')\n").unwrap();
        std::fs::write(
            gongwen_dir.join("manifest.json"),
            r#"{"id":"gongwen","name":"Gongwen","description":"d","version":"0","icon":"x","category":"c","mcp_tools":["mcp_gongwen_make_gongwen"],"command":"python","args":["server.py"],"companion_skills":["government-writing"]}"#,
        )
        .unwrap();
        let marketplace_dir = paths::pinvou3_home().join("marketplace");
        std::fs::create_dir_all(&marketplace_dir).unwrap();
        std::fs::write(marketplace_dir.join("installed.json"), r#"["gongwen"]"#).unwrap();
        let expected_gongwen = serde_json::json!({
            "command": "python",
            "args": [gongwen_dir.join("server.py")],
            "env": {"KEEP": "configured"},
            "enabled": false,
            "timeout_ms": 12345,
            "future_field": {"preserve": true}
        });
        std::fs::write(
            paths::mcp_config_path(),
            serde_json::to_vec(&serde_json::json!({
                "servers": {
                    "gongwen": expected_gongwen.clone()
                }
            }))
            .unwrap(),
        )
        .unwrap();
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .install("government-writing")
            .unwrap();

        let manager =
            crate::features::marketplace::MarketplaceManager::with_store(RejectingCredentialStore);
        bundle
            .ensure_extracted_with_marketplace(&manager, |manager| {
                let installed_before =
                    std::fs::read(marketplace_dir.join("installed.json")).unwrap();
                let mcp_before: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(paths::mcp_config_path()).unwrap())
                        .unwrap();
                let gongwen_before = mcp_before["servers"]["gongwen"].clone();
                let failpoint =
                    crate::features::marketplace::fail_next_managed_dependency_install_for_test();
                let errors = manager.repair_installed_python_tools()?;
                assert!(
                    !crate::features::marketplace::
                        managed_dependency_install_failure_pending_for_test(),
                    "the high-level transient dependency failure must be consumed"
                );
                drop(failpoint);
                assert_eq!(
                    errors,
                    vec![
                        concat!(
                            "tool 'gongwen' dependency repair will retry: ",
                            "test-injected transient managed dependency install failure"
                        )
                        .to_string()
                    ]
                );
                assert_eq!(
                    std::fs::read(marketplace_dir.join("installed.json")).unwrap(),
                    installed_before
                );
                let mcp_after: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(paths::mcp_config_path()).unwrap())
                        .unwrap();
                assert_eq!(mcp_after["servers"]["gongwen"], gongwen_before);
                assert!(!marketplace_dir.join("state-transaction.json").exists());
                Ok(errors)
            })
            .unwrap();

        assert!(paths::bundle_mcp_python_runner().is_file());
        let released_manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                crate::features::marketplace::mcp_catalog::package_mcp_dir("gongwen")
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(released_manifest.get("python_dependencies").is_some());
        assert_eq!(
            std::fs::read(weather_dir.join("manifest.json")).unwrap(),
            legacy_weather.to_vec(),
            "credential failure must preserve the legacy plaintext source"
        );
        let installed: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(marketplace_dir.join("installed.json")).unwrap(),
        )
        .unwrap();
        assert!(installed.contains(&"gongwen".to_string()));
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(paths::mcp_config_path()).unwrap())
                .unwrap();
        let gongwen = &mcp["servers"]["gongwen"];
        assert!(gongwen.is_object());
        assert_eq!(
            gongwen["command"],
            serde_json::Value::String(paths::python_command())
        );
        for field in ["args", "env", "enabled", "timeout_ms", "future_field"] {
            assert_eq!(gongwen[field], expected_gongwen[field], "field: {field}");
        }
        assert!(!marketplace_dir.join("state-transaction.json").exists());
        assert!(
            paths::bundles_root()
                .join("gongwen")
                .join("skills")
                .join("government-writing")
                .exists()
        );

        cleanup(&tmp);
    }

    #[test]
    fn multiagent_depth_guard_enforces_two_level_inputs() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();
        // 连接器技能现按包聚合落盘：bundles/<pkg>/skills/<dir>（§4 新布局）。
        let pkg_skills = |id: &str| paths::bundles_root().join(id).join("skills");
        std::fs::create_dir_all(pkg_skills("feishu")).unwrap();

        assert!(!bundle.cached_feishu_skills_visible());
        assert!(!bundle.cached_wecom_skills_visible());
        assert!(!bundle.cached_dingtalk_skills_visible());
        assert!(!bundle.cached_tmeet_skills_visible());

        for dir in LARK_SKILL_DIRS {
            let path = pkg_skills("feishu").join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in WECOM_SKILL_DIRS {
            let path = pkg_skills("wecom").join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in DINGTALK_SKILL_DIRS {
            let path = pkg_skills("dingtalk").join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("SKILL.md"), "test").unwrap();
        }
        for dir in TMEET_SKILL_DIRS {
            let path = pkg_skills("tmeet").join(dir);
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

        std::fs::remove_file(
            pkg_skills("feishu")
                .join(LARK_SKILL_DIRS[0])
                .join("SKILL.md"),
        )
        .unwrap();
        std::fs::remove_file(
            pkg_skills("wecom")
                .join(WECOM_SKILL_DIRS[0])
                .join("SKILL.md"),
        )
        .unwrap();
        std::fs::remove_file(
            pkg_skills("dingtalk")
                .join(DINGTALK_SKILL_DIRS[0])
                .join("SKILL.md"),
        )
        .unwrap();
        std::fs::remove_file(
            pkg_skills("tmeet")
                .join(TMEET_SKILL_DIRS[0])
                .join("SKILL.md"),
        )
        .unwrap();
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

        // This test neither holds ENV_LOCK nor sets PINVOU3_HOME, so it must
        // not use cleanup() — its contract requires the caller to hold the
        // lock, and it unconditionally removes the variable. A lock-free
        // removal here would punch through a parallel test's lock-holding
        // window, and the dingtalk/wecom skill-gate tests then fail
        // intermittently with assertions reading the wrong root.
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 已下架预置技能的清理:市场标记的删、无标记裸残留的删、用户上传(upload:)的保。
    #[test]
    fn cleanup_removed_marketplace_skills_respects_upload_marker() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();

        bundle.write_builtin_skills().unwrap();

        let skill_path = bundle.skills_dir.join("visual-design").join("SKILL.md");
        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert!(content.contains("name: visual-design"));
        assert!(skill_path.starts_with(&bundle.skills_dir));
        cleanup(&tmp);
    }

    /// `BUILTIN_RELEASED_SKILL_DIRS` must mirror the embedded builtin-skills
    /// resource tree (same pattern as `wecom_skill_dirs_match_embedded_resources`):
    /// a builtin skill added without extending the constant would be written by
    /// every launch and deleted by self-heal step 4 in the same run — silently
    /// invisible forever.
    #[test]
    fn builtin_released_skill_dirs_match_embedded_resources() {
        let mut embedded: Vec<&str> = BUILTIN_SKILLS_DIR
            .dirs()
            .map(|d| d.path().to_str().expect("目录名应为 UTF-8"))
            .collect();
        embedded.sort_unstable();
        let mut listed = Pinvou3Bundle::BUILTIN_RELEASED_SKILL_DIRS.to_vec();
        listed.sort_unstable();
        assert_eq!(
            embedded, listed,
            "BUILTIN_RELEASED_SKILL_DIRS 与 bundle/builtin-skills 实际目录不一致"
        );
        for d in listed {
            assert!(
                BUILTIN_SKILLS_DIR
                    .get_file(format!("{d}/SKILL.md"))
                    .is_some(),
                "builtin-skills/{d} 缺 SKILL.md"
            );
        }
    }

    /// The other divergence direction: `write_builtin_skills` must release
    /// exactly the dirs listed in `BUILTIN_RELEASED_SKILL_DIRS` — a skill added
    /// to the write path without extending the constant would be converged
    /// away by self-heal step 4 on the same launch.
    #[test]
    fn write_builtin_skills_releases_exactly_the_listed_dirs() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();

        bundle.write_builtin_skills().unwrap();

        let mut written: Vec<String> = std::fs::read_dir(&bundle.skills_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        written.sort_unstable();
        let mut listed: Vec<String> = Pinvou3Bundle::BUILTIN_RELEASED_SKILL_DIRS
            .iter()
            .map(|s| s.to_string())
            .collect();
        listed.sort_unstable();
        assert_eq!(
            written, listed,
            "write_builtin_skills 的释放面与 BUILTIN_RELEASED_SKILL_DIRS 不一致"
        );
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
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

    /// 坏 mcp.json(parse 失败)必须自愈:重建骨架并恢复内置 pinvou3 server 条目——
    /// 早期返回"留给后续启动"会让坏文件永远修不好(present_artifact 永久失效)。
    #[test]
    fn ensure_builtin_mcp_servers_repairs_broken_mcp_json() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();
        std::fs::write(&bundle.mcp_json, "{ not valid json !!!").unwrap();

        bundle.ensure_builtin_mcp_servers().unwrap();

        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap())
                .expect("自愈后 mcp.json 必须是合法 json");
        let servers = mcp["servers"].as_object().unwrap();
        assert!(
            servers.contains_key("pinvou3"),
            "坏 mcp.json 自愈后应恢复内置 pinvou3 条目,实际={:?}",
            servers.keys().collect::<Vec<_>>()
        );
        assert!(!servers.contains_key("pinvou"), "旧 pinvou 不残留");
        cleanup(&tmp);
    }

    /// write_if_changed:内容逐字节一致时跳过写盘(返回 false 且不动文件);
    /// 内容不同才实际写入(返回 true)。
    #[test]
    fn write_if_changed_skips_rewrite_when_content_identical() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();
        let target = std::path::Path::new(&tmp).join("nested").join("out.txt");

        // 首次:文件不存在 → 实际写入(父目录自动创建)
        assert!(bundle.write_if_changed(&target, "hello").unwrap());
        let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();
        // 内容一致 → 跳过写盘,mtime 不变
        assert!(!bundle.write_if_changed(&target, "hello").unwrap());
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            mtime,
            "内容一致时不应重写文件"
        );
        // 内容变化 → 重写
        assert!(bundle.write_if_changed(&target, "world").unwrap());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "world");
        cleanup(&tmp);
    }

    /// 已下架工具无任何残留时走快速路径:不实例化 MarketplaceManager 跑 uninstall。
    /// 可观测证据:uninstall 必写 marketplace/installed.json,快速路径下它不得出现。
    #[test]
    fn cleanup_removed_marketplace_tools_fast_path_skips_uninstall() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();

        bundle.cleanup_removed_marketplace_tools().unwrap();

        assert!(
            !paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json")
                .exists(),
            "无残留时不应触发 uninstall(其必写 installed.json)"
        );
        cleanup(&tmp);
    }

    /// G8a Upload protection must be fail-closed like the uninstall path's
    /// `source_may_be_upload`: an unreadable BundleStore means "may be an
    /// upload" → skip the whole retired-tool cleanup (its last step deletes
    /// `bundles/<id>` wholesale, which is user content for an upload).
    #[test]
    fn cleanup_removed_marketplace_tools_fail_closed_on_unreadable_store() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();

        // Corrupt (unparseable) bundles.json.
        let store_file = paths::pinvou3_home()
            .join("marketplace")
            .join("bundles.json");
        std::fs::create_dir_all(store_file.parent().unwrap()).unwrap();
        std::fs::write(&store_file, "{ not valid json !!!").unwrap();
        // Residue that would otherwise be cleaned (new-layout package dir).
        let pkg_dir = paths::bundles_root().join("data_analysis");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("plugin.json"), "{}").unwrap();

        bundle.cleanup_removed_marketplace_tools().unwrap();

        assert!(
            pkg_dir.join("plugin.json").is_file(),
            "store 不可读时退役清理必须 fail-closed（跳过），不得删包目录"
        );
        cleanup(&tmp);
    }
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
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();

        bundle.apply_dingtalk_skills(true).unwrap();
        // 新布局：钉钉 mono skill 解包到包目录 bundles/dingtalk/skills/。
        let pkg_skills = paths::bundles_root().join("dingtalk").join("skills");
        let skill = pkg_skills.join("dws");
        assert!(skill.join("SKILL.md").is_file());
        assert!(
            skill
                .join("references")
                .join("global-reference.md")
                .is_file()
        );
        assert!(pkg_skills.join("NOTICE-dingtalk.md").is_file());

        bundle.apply_dingtalk_skills(false).unwrap();
        assert!(!skill.exists());
        assert!(!pkg_skills.join("NOTICE-dingtalk.md").exists());

        cleanup(&tmp);
    }

    /// `WECOM_SKILL_DIRS` 清单必须与内嵌 wecom-skills 资源树的实际目录一致:
    /// 漏列会让技能静默不可见(cached_wecom_skills_visible 的 all() 不查多余目录),
    /// 多列会让门控删除/缓存判定指向不存在的目录——两侧都对齐才有报警。
    #[test]
    fn wecom_skill_dirs_match_embedded_resources() {
        let mut embedded: Vec<&str> = WECOM_SKILLS_DIR
            .dirs()
            .map(|d| d.path().to_str().expect("目录名应为 UTF-8"))
            .collect();
        embedded.sort_unstable();
        let mut listed = WECOM_SKILL_DIRS.to_vec();
        listed.sort_unstable();
        assert_eq!(
            embedded, listed,
            "WECOM_SKILL_DIRS 与 bundle/wecom-skills 实际目录不一致"
        );
        // 每个技能目录都必须带 SKILL.md(门控可见性按它判定)
        for d in WECOM_SKILL_DIRS {
            assert!(
                WECOM_SKILLS_DIR.get_file(format!("{d}/SKILL.md")).is_some(),
                "wecom-skills/{d} 缺 SKILL.md"
            );
        }
    }

    /// 0.1.9 时代的 legacy 目录(wecomcli-msg/schedule)无论显示与否都要被清掉,
    /// 防残留技能教已死的命令(`msg`/`schedule` 服务)。
    #[test]
    fn wecom_skill_gate_cleans_legacy_dirs() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(&bundle.skills_dir).unwrap();

        for d in WECOM_LEGACY_SKILL_DIRS {
            let legacy = bundle.skills_dir.join(d);
            std::fs::create_dir_all(&legacy).unwrap();
            std::fs::write(legacy.join("SKILL.md"), "legacy").unwrap();
        }

        // 隐藏门控:清 legacy + 删当前技能
        bundle.apply_wecom_skills(false).unwrap();
        for d in WECOM_LEGACY_SKILL_DIRS {
            assert!(!bundle.skills_dir.join(d).exists(), "{d} 应被清理");
        }

        // 显示门控:同样先清 legacy(开头无条件清理),再解包 14 个当前技能
        for d in WECOM_LEGACY_SKILL_DIRS {
            std::fs::create_dir_all(bundle.skills_dir.join(d)).unwrap();
        }
        bundle.apply_wecom_skills(true).unwrap();
        for d in WECOM_LEGACY_SKILL_DIRS {
            assert!(!bundle.skills_dir.join(d).exists(), "{d} 应被清理");
        }
        // 门控解包目标在按包聚合布局 bundles/wecom/skills/（旧扁平 skills_dir 已退役）
        let wecom_pkg_skills = crate::platform::paths::bundles_root()
            .join("wecom")
            .join("skills");
        for d in WECOM_SKILL_DIRS {
            assert!(
                wecom_pkg_skills.join(d).join("SKILL.md").is_file(),
                "{d} 应被解包"
            );
        }

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
        // `std::env::temp_dir` already honors the platform's temporary-dir
        // convention. A literal `/tmp` is not the same location on Windows
        // after `PINVOU3_HOME` passes through `platform_compat_path`, which can
        // split this fixture across two drives.
        std::env::temp_dir()
            .join(format!("pinvou3-bundle-test-{id}"))
            .to_string_lossy()
            .into_owned()
    }

    /// Removes a test fixture directory and unsets `PINVOU3_HOME` in-process.
    ///
    /// Contract: the caller must hold `bridge::paths::tests::ENV_LOCK` for the
    /// whole call — only that lock serializes the unconditional `remove_var`
    /// against tests that set the variable under it. A test that never sets
    /// `PINVOU3_HOME` must not borrow this helper; remove its directory
    /// directly instead.
    fn cleanup(dir: &str) {
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::remove_var("PINVOU3_HOME") };
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Verifies the Browser MCP gate defined by the ADR:
    /// 1. global mcp.json never registers a browser entry and historical residue is removed;
    /// 2. `work_mode_mcp_config_path` falls back to the global path when prerequisites fail;
    /// 3. when prerequisites pass, it creates a session-specific global-plus-Pinvou-browser
    ///    file and deterministically renames any user-owned browser server only in that copy.
    #[test]
    fn browser_mcp_entry_is_gated_to_work_mode_config() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_CDMCP_BIN") };
        paths::ensure_dirs().unwrap();
        let bundle = Pinvou3Bundle::paths();
        std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();

        // 1) Preserve a user-defined browser server in global mcp.json byte-for-byte. Remove
        //    only historical app-owned residue whose command targets browser-wrapper.mjs.
        std::fs::write(
            &bundle.mcp_json,
            r#"{"servers":{"browser":{"command":"/old/browser"},"weather":{"command":"python3","args":["/x/w.py"]}}}"#,
        )
        .unwrap();
        bundle.ensure_builtin_mcp_servers().unwrap();
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
        assert!(
            mcp["servers"].as_object().unwrap().contains_key("browser"),
            "a user-defined browser server must not be deleted"
        );
        assert_eq!(
            mcp["servers"]["browser"]["command"], "/old/browser",
            "a user-defined browser server must remain unchanged"
        );

        if bundle.browser_mcp_entry().is_none() {
            // Even when the built-in backend is unavailable, do not expose a user server
            // under the reserved name in Work mode: the Agent would mistake `mcp_browser_*`
            // for the same-page embedded browser. Rename only in the session copy and leave
            // global configuration unchanged. Default macOS builds take this branch; preview
            // builds with complete prerequisites use the built-in conflict case below.
            let reserved_path = bundle.work_mode_mcp_config_path_for_session("work/gated");
            assert_ne!(reserved_path, bundle.mcp_json);
            let reserved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&reserved_path).unwrap()).unwrap();
            assert!(
                !reserved["servers"]
                    .as_object()
                    .unwrap()
                    .contains_key("browser")
            );
            assert_eq!(
                reserved["servers"]["browser_user"]["command"],
                "/old/browser"
            );
            let global_after_reservation: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
            assert_eq!(global_after_reservation, mcp);
        }

        // 1b) Remove historical app-owned wrapper residue; global configuration never
        //     registers Browser MCP.
        std::fs::write(
            &bundle.mcp_json,
            r#"{"servers":{"browser":{"command":"/x/browser-wrapper.mjs"},"weather":{"command":"python3","args":["/x/w.py"]}}}"#,
        )
        .unwrap();
        bundle.ensure_builtin_mcp_servers().unwrap();
        let mcp2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
        assert!(
            !mcp2["servers"].as_object().unwrap().contains_key("browser"),
            "historical app-owned browser residue must be removed"
        );

        // 2) Missing prerequisites (no vendor entry point) fall back to global mcp.json
        //    without writing a session-specific file.
        assert_eq!(
            bundle.work_mode_mcp_config_path(),
            bundle.mcp_json,
            "missing prerequisites must return global mcp.json"
        );

        // 3) Complete prerequisites (Windows vendor, Linux WebKitWebDriver, or macOS
        //    WKWebView BrowserCore, plus wrapper and Node.js) generate a session-specific
        //    file. Unreleased platforms must still fail closed.
        let fake_bin = std::path::PathBuf::from(&tmp).join("fake-cdmcp-entry.js");
        std::fs::write(&fake_bin, "#!/usr/bin/env node\n").unwrap();
        #[cfg(windows)]
        {
            let fake_bin_for_env = std::path::PathBuf::from(format!(r"\\?\{}", fake_bin.display()));
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            unsafe { std::env::set_var("PINVOU3_CDMCP_BIN", &fake_bin_for_env) };
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let fake_driver = std::path::PathBuf::from(&tmp).join("WebKitWebDriver");
            std::fs::write(&fake_driver, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&fake_driver, std::fs::Permissions::from_mode(0o755)).unwrap();
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            unsafe { std::env::set_var("PINVOU3_WEBKIT_WEBDRIVER_BIN", fake_driver) };
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe {
            std::env::set_var("PINVOU3_CDMCP_BIN", &fake_bin)
        };
        let wrapper = paths::bundle_browser_wrapper();
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "#!/usr/bin/env node\n").unwrap();
        let work_path = bundle.work_mode_mcp_config_path();
        if !crate::platform::capabilities::browser_product_enabled() {
            assert_eq!(
                work_path, bundle.mcp_json,
                "an unreleased product build must not register Browser MCP from environment residue"
            );
            assert_eq!(
                bundle.work_mode_mcp_config_path_for_session("work/session-A"),
                bundle.mcp_json,
                "an unreleased build without a reserved-name conflict needs no session copy"
            );
            assert!(
                bundle
                    .browser_unavailability_reason()
                    .is_some_and(|reason| reason.contains("not enabled in this product build"))
            );
        } else {
            assert_ne!(
                work_path, bundle.mcp_json,
                "complete prerequisites must return a session-specific MCP file"
            );
            let work: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&work_path).unwrap()).unwrap();
            let servers = work["servers"].as_object().unwrap();
            assert!(
                servers.contains_key("browser"),
                "the session-specific file must contain a browser entry"
            );
            assert!(
                servers.contains_key("weather"),
                "the marketplace weather entry must be preserved"
            );
            let args = servers["browser"]["args"].as_array().unwrap();
            assert_eq!(
                args.len(),
                3,
                "the browser wrapper must receive only the wrapper, platform backend entry, and host-state file"
            );
            assert!(
                args[0].as_str().unwrap().ends_with("browser-wrapper.mjs"),
                "the browser entry must use the wrapper as its entry point"
            );
            assert!(
                !args[0].as_str().unwrap().starts_with(r"\\?\"),
                "the wrapper entry passed to Node.js must not retain a Windows verbatim prefix"
            );
            #[cfg(windows)]
            assert!(
                !args[1].as_str().unwrap().starts_with(r"\\?\"),
                "the MCP entry passed to Node.js must not retain a Windows verbatim prefix"
            );
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            assert_eq!(
                args[1], "@pinvou/browser-core",
                "Linux and macOS must select in-app BrowserCore instead of registering Chrome MCP"
            );

            // 4) Each Work-mode session receives an independent configuration and stable
            //    token. The wrapper environment retains the original session_id for host
            //    requests, while paths and labels use only the safe token.
            let session_path = bundle.work_mode_mcp_config_path_for_session("work/session-A");
            assert_ne!(session_path, work_path);
            assert_eq!(
                session_path.file_name().and_then(|value| value.to_str()),
                Some("f1731564024f1ec5.json")
            );
            let session: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&session_path).unwrap()).unwrap();
            assert_eq!(
                session["servers"]["browser"]["env"]["PINVOU3_BROWSER_SESSION_ID"],
                "work/session-A"
            );
            assert_eq!(
                session["servers"]["browser"]["env"]["PINVOU3_BROWSER_SESSION_TOKEN"],
                crate::platform::paths::browser_session_token("work/session-A")
            );

            // 5) When a user occupies the reserved browser name globally, leave the global
            //    file unchanged. Work mode always points browser to the Pinvou wrapper and
            //    preserves the user entry after existing deterministic aliases.
            let global_with_conflict = serde_json::json!({
                "servers": {
                    "browser": {
                        "command": "playwright-mcp",
                        "env": { "USER_SETTING": "kept" }
                    },
                    "browser_user": { "command": "existing-user-browser" },
                    "browser_user_2": { "command": "existing-numbered-browser" },
                    "weather": { "command": "weather-mcp" }
                }
            });
            std::fs::write(
                &bundle.mcp_json,
                serde_json::to_string_pretty(&global_with_conflict).unwrap(),
            )
            .unwrap();

            let conflict_path =
                bundle.work_mode_mcp_config_path_for_session("work/session-conflict");
            assert_ne!(conflict_path, bundle.mcp_json);
            let conflict: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&conflict_path).unwrap()).unwrap();
            assert!(
                conflict["servers"]["browser"]["args"][0]
                    .as_str()
                    .unwrap()
                    .ends_with("browser-wrapper.mjs"),
                "the session-reserved browser name must always target the Pinvou wrapper"
            );
            assert_eq!(
                conflict["servers"]["browser_user_3"], global_with_conflict["servers"]["browser"],
                "the conflicting user server must move to the first deterministic free alias"
            );
            assert_eq!(
                conflict["servers"]["browser_user"],
                global_with_conflict["servers"]["browser_user"]
            );
            assert_eq!(
                conflict["servers"]["browser_user_2"],
                global_with_conflict["servers"]["browser_user_2"]
            );
            let global_after: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
            assert_eq!(
                global_after, global_with_conflict,
                "generating a session copy must not rewrite the user's global mcp.json"
            );
            assert_eq!(
                bundle.browser_unavailability_reason(),
                None,
                "a globally user-owned browser name must not degrade the embedded browser or inject an error"
            );
        }

        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_CDMCP_BIN") };
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::remove_var("PINVOU3_WEBKIT_WEBDRIVER_BIN") };
        cleanup(&tmp);
    }
}
