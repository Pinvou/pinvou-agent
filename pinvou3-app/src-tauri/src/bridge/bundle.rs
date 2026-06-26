//! pinvou3 内置 bundle：随 app 编译进去的 instructions.md / mcp.json / skills 模板，
//! 首次启动时解包到 `~/.pinvou3/bundle/`。
//!
//! 与 user/ 严格分离：bundle/ 每次升级被覆写，user/ 永远不动。
//! 解包用 `bundle/VERSION` 比对 [`BUNDLE_VERSION`]，相同则跳过。

use std::path::PathBuf;

use include_dir::{include_dir, Dir};

use super::paths;

/// 三省六部工作流：编译期内嵌整个目录树（roles/*.md + scripts/*.py + json）。
static SANSHENG_LIUBU_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/bundle/workflow/sansheng-liubu");

/// Bundle 版本号：手动 base + 自动 instructions.md 内容 hash（build.rs 注入）。
/// 改 INSTRUCTIONS_MD 时不需要 bump base —— hash 自动变，ensure_extracted 自动覆写。
/// 改其他 bundle 资源（mcp.json 默认 / skills 模板等）才需要手动 bump base。
///
/// 0.4: 加 Pinvou Review 内置 skills(pinvou-review-plan / pinvou-review-final)
/// 0.5: 下线 h3c-ppt workflow skill(workflow 功能转"开发中"),phase 协议随之停渲
/// 0.6: 加 present_artifact 内置 MCP server(成品卡):mcp.json 注册 + server 脚本解包
/// 0.7: 下线 Pinvou Review v2(EXIT GATE 评审被推翻,等新方案):两个 review skill
///      不再解包,既有装机的残留目录启动时清理
/// 0.8: 上线三省六部卡片流工作流(sansheng-liubu):include_dir 内嵌 + 启动解包
/// 注:「视觉设计」内置 skill 在 VERSION gate 之前由 write_builtin_skills 每启动防御性写出,
///     不依赖版本号 bump(同 write_workflows / write_mcp_servers）。
pub const BUNDLE_VERSION: &str = concat!(
    "0.9-",
    env!("BUNDLE_INSTRUCTIONS_HASH"),
    "-",
    env!("BUNDLE_WORKFLOW_HASH_SANSHENG"),
);

/// pinvou3 内置的 instructions.md（Qwen3.6 适配 prompt），编译时内嵌。
pub const INSTRUCTIONS_MD: &str = include_str!("../../resources/bundle/instructions.md");

/// 内置「视觉设计」技能（设计系统直出 HTML）。编译期内嵌，解包到
/// `~/.pinvou3/bundle/skills/visual-design/SKILL.md`，进 SkillRegistry 的 `## Skills`
/// 目录。ascii 目录名避开中文路径在 include_str! 的坑；frontmatter `name: 视觉设计`
/// 才是模型 load_skill 用的 id。
const VISUAL_DESIGN_SKILL_MD: &str =
    include_str!("../../resources/bundle/skills/visual-design/SKILL.md");

/// pinvou3 版 base prompt（Constitution / 工具纪律 / embedder-aware / 删 RLM·Toolbox·V4），
/// 编译期内嵌。通过底座 `prompts::set_base_prompt_override` 注入，替换底座的上游
/// `BASE_PROMPT`。这样 pinvou3 的 prompt 定制活在 app,DeepSeek-TUI submodule 的
/// base.md 回退上游原文(fork drift 归零)。见 docs/base-prompt-override-阶段2.md。
pub const BASE_PROMPT_MD: &str = include_str!("../../resources/bundle/base.md");

/// pinvou3 版简体中文 locale 前导段（替换底座 `LOCALE_PREAMBLE_ZH_HANS`）。
/// 瘦身依据:底座原文的动机是防 thinking 漂英文(上游 #1118)——pinvou3 生产
/// `reasoning_effort=off` 无 thinking,该 failure mode 不存在;回复语言已由
/// base.md §Language("match the latest user message")管,这里只补
/// "判断不了时的默认语言"。closer 同理。
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
/// ⚠️ **句尾「phase rules」别删**:b891b2f 当「过时幽灵」删掉这 2 个 token,GUI 真机
/// 首请求即把 `write_file` 调用采歪成 `<write_file>` 裸文本(磁盘无文件、用户拿不到成品)。
/// git 二分实锤就这处——删 → 必歪,恢复 → 不歪。机制:GUI 真实 mtp 投机解码 +
/// chunked-prefill 对这块结尾的 token 序列敏感(curl/headless 单请求复现不了,11次0漂)。
/// 语义过时无妨,token 序列本身 load-bearing。**改此 const 务必 GUI 真机回归
/// 「首轮直接写文件」场景(如"做一个贪吃蛇")**,别只信单测/curl。
pub const MODE_EXECUTE_MD: &str = "\
## Mode: Execute

Tools run without per-call approval — the user has already authorized
execution. Produce files and run commands now; never end the turn with
a promise of future action. Then verify and report. Follow each
message's `<system-reminder>` phase rules.";

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
    let _ =
        deepseek_tui::prompts::set_locale_preamble_zh_hans_override(LOCALE_PREAMBLE_ZH_HANS.to_string());
    let _ =
        deepseek_tui::prompts::set_locale_closer_zh_hans_override(LOCALE_CLOSER_ZH_HANS.to_string());
    let _ = deepseek_tui::prompts::set_locale_preamble_ja_override(LOCALE_PREAMBLE_JA.to_string());
    let _ = deepseek_tui::prompts::set_locale_closer_ja_override(LOCALE_CLOSER_JA.to_string());
    let _ = deepseek_tui::prompts::set_authority_recap_override(AUTHORITY_RECAP.to_string());
    // 静态层全量接管(fork patch: set_static_prompt_composer_override)。
    // 设置后底座的 Personality/Mode/Approval/ContextMgmt/COMPACT_TEMPLATE/
    // taxonomy 常量全部不进 prompt,由 compose_static_layers 输出替代;
    // base override 仍保留——composer 的 ctx.default_layers 引用它。
    let _ = deepseek_tui::prompts::set_static_prompt_composer_override(Box::new(
        |ctx| compose_static_layers(ctx),
    ));
}

/// 内置 MCP 默认配置:注册 present_artifact server(成品卡)。`{{PINVOU3_PRESENT_SERVER}}`
/// 占位符在 `ensure_extracted` 写出时被替换成解包后的 server 脚本绝对路径(常量无法
/// 编译期拿到 `~/.pinvou3/bundle/` 运行时路径,同 INSTRUCTIONS_MD 的 `{{PINVOU3_WORKSPACE}}`)。
/// server key `pinvou` + tool `present_artifact` → 底座透传给前端的工具名是
/// `mcp_pinvou_present_artifact`(底座 `mcp.rs:all_tools` 格式 `mcp_{server}_{tool}`)。
/// instructions.md 的引导名与前端匹配都按这个全名;前端 `isPresentArtifactTool`
/// 用 `endsWith("present_artifact")` 命中,改 server 名也不破。
pub const DEFAULT_MCP_JSON: &str = "{\n  \"servers\": {\n    \"pinvou\": {\n      \"command\": \"python3\",\n      \"args\": [\"{{PINVOU3_PRESENT_SERVER}}\"]\n    }\n  }\n}\n";

/// present_artifact MCP server 脚本(零依赖 python stdio),编译期内嵌,解包到
/// `~/.pinvou3/bundle/mcp-servers/`。底座按 mcp.json 用 `python3 <path>` 拉起它。
pub const PRESENT_ARTIFACT_SERVER_PY: &str =
    include_str!("../../resources/bundle/mcp-servers/present_artifact_server.py");

// --- 工具市场：内置 MCP server 资源(编译期内嵌) ---
const WEATHER_SERVER_PY: &str =
    include_str!("../../../resources/mcp-servers/weather/server.py");
const WEATHER_MANIFEST_JSON: &str =
    include_str!("../../../resources/mcp-servers/weather/manifest.json");
const IWENCAI_SERVER_PY: &str =
    include_str!("../../../resources/mcp-servers/iwencai/server.py");
const IWENCAI_MANIFEST_JSON: &str =
    include_str!("../../../resources/mcp-servers/iwencai/manifest.json");
const QCC_MANIFEST_JSON: &str =
    include_str!("../../../resources/mcp-servers/qcc/manifest.json");
const OBSIDIAN_SERVER_PY: &str =
    include_str!("../../../resources/mcp-servers/obsidian/server.py");
const OBSIDIAN_MANIFEST_JSON: &str =
    include_str!("../../../resources/mcp-servers/obsidian/manifest.json");
const PPTX_SERVER_PY: &str =
    include_str!("../../../resources/mcp-servers/pptx/server.py");
const PPTX_MANIFEST_JSON: &str =
    include_str!("../../../resources/mcp-servers/pptx/manifest.json");

/// 内嵌的敏感目录拦截 shell 脚本——配合 bridge 注入的 hook 在 ToolCallBefore
/// 时阻止 LLM 触碰 ~/.ssh/ ~/.gnupg/ 等。
pub const DENY_SENSITIVE_PATHS_SH: &str =
    include_str!("../../resources/bundle/deny_sensitive_paths.sh");

#[derive(Debug, Clone)]
pub struct Pinvou3Bundle {
    pub root: PathBuf,
    pub instructions_md: PathBuf,
    pub skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub mcp_json: PathBuf,
    pub deny_sensitive_sh: PathBuf,
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

        // 已下线 skills 每次启动都清理(防御性):既有装机的残留目录若不清,
        // SkillRegistry 仍会从 disk 发现它们、重新触发对应协议 prompt。
        self.cleanup_retired_skills()?;
        // 工作流目录同 skills:immutable bundle 资源,每次启动防御性重写
        // (防 "VERSION 对得上但目录缺失"),无副作用。
        self.write_workflows()?;
        // 内置 skill 同 workflow:immutable bundle 资源,每次启动防御性重写。
        self.write_builtin_skills()?;
        // MCP server 脚本同理。
        self.write_mcp_servers()?;
        // mcp.json merge:每次启动 upsert 内置 pinvou server,保留 marketplace 条目。
        // 不受 VERSION gate 限制——marketplace 安装可能在任何时候发生。
        self.ensure_builtin_mcp_servers()?;
        // 启动自愈:刷新 mcp.json 里陈旧的本地 python server command(安装时写死的裸
        // "python" → 重解析成可用路径)。必须在引擎 spawn 前跑(引擎从 mcp.json 拉起 server)。
        self.refresh_mcp_python_commands()?;

        if current.trim() == BUNDLE_VERSION {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.skills_dir)?;
        let workspace_abs = paths::workspace_dir();
        std::fs::create_dir_all(&workspace_abs)?;
        // 首次解包按当前 sudoers 状态填 PINVOU3_SUDO_INSTRUCTION,避免占位符原文
        // 漏到 LLM 看到的 system prompt(engine boot 时是从 disk 读的)。
        // 用户切换开关时 set_super_permission 会 sync_session 重写。
        let rendered = INSTRUCTIONS_MD
            .replace("{{PINVOU3_WORKSPACE}}", &workspace_abs.to_string_lossy())
            .replace(
                "{{PINVOU3_SUDO_INSTRUCTION}}",
                crate::super_permission::instruction_block(),
            )
            // 落盘副本无 per-session locale,默认填中文兜底(LLM 实际走 mod.rs 的 inline 渲染,
            // 那里按 locale 填);此处仅防 {{PINVOU3_TITLE_LANG}} 占位符原文残留在 disk 文件。
            .replace("{{PINVOU3_TITLE_LANG}}", "简体中文");
        std::fs::write(&self.instructions_md, rendered)?;
        // 敏感目录拦截脚本：写入 + 加可执行位
        std::fs::write(&self.deny_sensitive_sh, DENY_SENSITIVE_PATHS_SH)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&self.deny_sensitive_sh)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&self.deny_sensitive_sh, perm)?;
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
    /// - h3c-ppt:0.5 下线(workflow 功能转"开发中")
    /// - pinvou-review-plan / pinvou-review-final:0.7 下线(EXIT GATE 评审被推翻)
    fn cleanup_retired_skills(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.skills_dir)?;
        for retired in ["h3c-ppt", "pinvou-review-plan", "pinvou-review-final"] {
            let _ = std::fs::remove_dir_all(self.skills_dir.join(retired));
        }
        Ok(())
    }

    /// 解包内嵌的内置 skills。**落位到 `~/.agents/skills/`**——引擎 fork patch #41 让
    /// `load_skill` 工具只扫这个目录(`agents_global_skills_dir`);bundle/skills 只进
    /// system-prompt catalogue 的 union、`load_skill` 不认。落错目录会"列得出、load 不到"。
    /// 每次启动防御性重写(immutable 内置资源)。当前:视觉设计。
    fn write_builtin_skills(&self) -> std::io::Result<()> {
        let Some(agents) = deepseek_tui::skills::agents_global_skills_dir() else {
            return Ok(()); // 拿不到 home 目录就跳过,不致命
        };
        let dir = agents.join("visual-design");
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
            let existing = std::fs::read_to_string(&self.mcp_json)
                .unwrap_or_else(|_| "{}".to_string());
            serde_json::from_str(&existing)
                .unwrap_or_else(|_| serde_json::json!({"servers": {}}))
        } else {
            serde_json::json!({"servers": {}})
        };
        if mcp.get("servers").and_then(|s| s.as_object()).is_none() {
            mcp.as_object_mut()
                .unwrap()
                .insert("servers".into(), serde_json::json!({}));
        }
        let servers = mcp["servers"].as_object_mut().unwrap();
        // Windows 用内置 pythonw(无窗口 + 自带依赖);其他平台系统 python3。见 paths::python_command。
        let python_cmd = paths::python_command();
        servers.insert(
            "pinvou".to_string(),
            serde_json::json!({
                "command": python_cmd,
                "args": [present_server.to_string_lossy()]
            }),
        );
        let json = serde_json::to_string_pretty(&mcp)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
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
            let json = serde_json::to_string_pretty(&mcp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;

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
        // 已下线 skills(h3c-ppt / pinvou-review-*)不应再被写出。
        for retired in ["h3c-ppt", "pinvou-review-plan", "pinvou-review-final"] {
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

    /// forkguard(composer): 静态层 composer 接管后,底座的 Personality/
    /// Session Longevity/Efficient Approvals/taxonomy 不得再进 prompt,
    /// pinvou3 自有的 mode 块 + 瘦身 compact 模板必须在。上游 sync 后此测试
    /// 失败 = set_static_prompt_composer_override fork patch 被合丢。
    #[test]
    fn forkguard_static_composer_takes_over_static_layers() {
        use deepseek_tui::models::SystemPrompt;

        install_prompt_overrides(); // OnceLock 幂等,谁先调都一样

        // v0.8.57:上游删了 `system_prompt_for_mode(AppMode)`(prompt 改 mode-independent)。
        // 改用 mode-independent 入口;pinvou3 composer 以常量 Yolo 构造 ctx → 静态层 = base.md
        // + MODE_EXECUTE_MD(生产单 Yolo-Auto)。原 Plan-mode 断言移除——static prompt 不再分模式
        // (Plan 前端入口已下线;若恢复,mode 走 per-turn <runtime_prompt> tag,非静态前缀)。
        let tmp = tempdir();
        std::fs::create_dir_all(&tmp).unwrap();
        let SystemPrompt::Text(yolo) =
            deepseek_tui::prompts::system_prompt_for_mode_with_context_and_skills(
                std::path::Path::new(&tmp),
                None,
                None,
                None,
                None,
            )
        else {
            panic!("unexpected SystemPrompt variant")
        };
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
            "Tier ", // 九层已删,不许残留悬空 tier 引用
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
        use deepseek_tui::models::SystemPrompt;

        install_prompt_overrides();

        let tmp = tempdir();
        std::fs::create_dir_all(&tmp).unwrap();
        // v0.8.57:上游把 system prompt 改 mode-independent,该函数签名去掉首个 AppMode 参数。
        let SystemPrompt::Text(text) =
            deepseek_tui::prompts::system_prompt_for_mode_with_context_and_skills(
                std::path::Path::new(&tmp),
                None,
                None,
                None,
                None,
            )
        else {
            panic!("unexpected SystemPrompt variant")
        };
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



    fn tempdir() -> String {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("/tmp/pinvou3-bundle-test-{}", id)
    }

    fn cleanup(dir: &str) {
        std::env::remove_var("PINVOU3_HOME");
        let _ = std::fs::remove_dir_all(dir);
    }
}
