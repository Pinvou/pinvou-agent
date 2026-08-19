//! 能力包统一模型 — 步骤 1/2：schema + 注册表 + kind 推导 + 就绪态（只读，不改安装/门禁/投影）。
//!
//! 设计依据：`docs/capability-governance.md` §3（能力包线）+ 实施修复方案（V1-V7）。
//! 一切外部能力统一建模为包：
//!
//! ```text
//! Bundle = { id, name, mcp_servers: [], skills: [], cli: [],
//!            credentials: [ { key, target: env|credential|bearer, required } ] }
//! ```
//!
//! - 包是唯一真相源；`mcp.json`、会话 skills 组合目录降级为投影（后续步骤实施）
//! - 包类型不做存储标签，`bundle_kind` 由内容现算（可信代码推导，防自报标签提权）
//! - 一个包 = 一个开关；包内技能可见性唯一跟随所属包
//! - **installed 与 ready 分离**：installed 是存储态（装没装，二态）；ready 是派生态
//!   （不进存储，查询时现算）——CLI 包按授权存在与否、凭据型按 credentials 必填项
//!   是否齐（查系统凭据）、本地免凭据包恒 ready。UI 统一消费 (installed, ready) 二元组。
//!
//! 注册表汇总四类源：MCP manifest（`bundle/mcp-servers/<id>/manifest.json`）、
//! 预置技能（编译内嵌）、CLI 连接器（内置常量表）、已上传技能
//! （`bundle/skills/` 带 `.installed-from=upload:` 标记）。

use serde::{Deserialize, Serialize};

use super::skill_marketplace::SkillMarketplaceManager;
use super::store;
use super::MarketplaceManager;

// 内置 CLI 连接器清单（修复方案 V2：ima 无 CLI 二进制，移出 CLI 包，归凭据型技能包）。
// 条目 = (id, 展示名, CLI 二进制名, 配套技能目录, 功能描述)。本表是「连接器 → CLI 二进制 /
// 配套技能」的单一真相源：能力包注册（下方 list_bundles）、companion 联动排除
// （MarketplaceManager::companion_skills）、execpolicy 硬拦截（engine_pool ruleset）
// 与技能解包门控（runtime_bundle apply_*_skills）全部从这里取数。
// 功能描述是功能事实（§3.1 下沉侧），取自前端 tsToolsData 既有文案；label/icon/
// color/welcomeQueries 等 i18n 展示资产仍留前端 overlay。
/// 9 个 lark 域技能目录名（飞书配套技能，门控写/删与组合目录排除共用）。
pub const LARK_SKILL_DIRS: &[&str] = &[
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
/// 7 个企微域技能目录名。
pub const WECOM_SKILL_DIRS: &[&str] = &[
    "wecomcli-msg",
    "wecomcli-doc",
    "wecomcli-meeting",
    "wecomcli-schedule",
    "wecomcli-todo",
    "wecomcli-contact",
    "wecomcli-smartsheet",
];
/// 钉钉 mono skill 目录名。
pub const DINGTALK_SKILL_DIRS: &[&str] = &["dws"];
/// 腾讯会议 mono skill 目录名。
pub const TMEET_SKILL_DIRS: &[&str] = &["tmeet-skill"];

const BUILTIN_CLI_BUNDLES: &[(&str, &str, &str, &[&str], &str)] = &[
    (
        "feishu",
        "飞书（Lark）",
        "lark-cli",
        LARK_SKILL_DIRS,
        "接入飞书官方 CLI + 官方域技能（MIT）：让 AI 以你本人身份读写云文档、查改日历、操作多维表格（Base）、收发消息、管理知识库与任务。点「连接飞书」浏览器一键授权，全程不填 key。数据经飞书云 OpenAPI（可选联网功能，opt-in）。",
    ),
    (
        "wecom",
        "企业微信",
        "wecom-cli",
        WECOM_SKILL_DIRS,
        "接入企业微信官方 CLI（@wecom/cli，MIT）+ 官方域技能：让 AI 以你本人身份收发消息、读写文档与智能表格、创建/查询会议与日程、管理待办、查询通讯录。点「连接」用企业微信 App 扫码授权，全程不填 key。数据经企业微信云（可选联网功能，opt-in）。",
    ),
    (
        "dingtalk",
        "钉钉",
        "dws",
        DINGTALK_SKILL_DIRS,
        "接入钉钉官方 DingTalk Workspace CLI（dws，Apache-2.0）+ 官方技能：让 AI 以你本人身份读写钉钉文档、查改日历、操作 AI 表格/在线表格、收发群聊消息、处理待办/审批/日志/邮箱等。点「连接」用钉钉 App 扫码授权，全程不填 key。",
    ),
    (
        "tmeet",
        "腾讯会议",
        "tmeet",
        TMEET_SKILL_DIRS,
        "接入腾讯会议官方 CLI（@tencentcloud/tmeet）+ 官方技能：让 AI 以你本人身份创建、查询、修改和取消腾讯会议，查询受邀人、参会报告、录制、转写与智能纪要，并支持会中呼叫成员入会。点「连接」打开腾讯会议授权页扫码登录，全程不填 key。",
    ),
];

/// 内置 CLI 连接器 id 列表（scope 默认全禁等门禁逻辑的覆盖来源）。
pub fn builtin_cli_bundle_ids() -> impl Iterator<Item = &'static str> {
    BUILTIN_CLI_BUNDLES.iter().map(|(id, ..)| *id)
}

/// CLI 连接器的配套技能目录名（非 CLI id 返回空切片）。
pub fn cli_bundle_skill_dirs(id: &str) -> &'static [&'static str] {
    BUILTIN_CLI_BUNDLES
        .iter()
        .find(|(cid, ..)| *cid == id)
        .map(|(.., dirs, _)| *dirs)
        .unwrap_or(&[])
}

/// CLI 连接器的二进制名（execpolicy 硬拦截按它构造 deny 规则）。
pub fn cli_bundle_bin(id: &str) -> Option<&'static str> {
    BUILTIN_CLI_BUNDLES
        .iter()
        .find(|(cid, ..)| *cid == id)
        .map(|(.., bin, _, _)| *bin)
}

/// 技能目录名 → CLI 连接器 id（内置清单反查；非 CLI companion 返回 None）。
pub(crate) fn cli_bundle_of_skill(skill_dir: &str) -> Option<&'static str> {
    BUILTIN_CLI_BUNDLES
        .iter()
        .find(|(.., dirs, _)| dirs.contains(&skill_dir))
        .map(|(id, ..)| *id)
}

/// 技能目录名 → 所属包 id（物理布局归属，§4「一个包 = 一个目录 = 一个属主」）。
///
/// **条件认领**（与 list_bundles 的 V5 决策一致）：ima 认领 ima-skills（同
/// list_bundles 的 skill_claimed 预置）→ CLI 内置清单 → MCP manifest
/// companion_skills（仅当所属 MCP 包当前已装才归 MCP；未装时技能保留独立
/// 纯技能包形态，owner = 技能名自身）→ 独立成包。迁移层
/// （`skill_marketplace::legacy_companion_owners`）按同一条件口径推导，
/// 两侧不得分叉（四轮评审 M-7）。
pub(crate) fn skill_owner_package(skill_name: &str) -> String {
    if skill_name == "ima-skills" {
        return "ima".to_string();
    }
    if let Some(cli) = cli_bundle_of_skill(skill_name) {
        return cli.to_string();
    }
    for tool in MarketplaceManager::new().available_tools() {
        if tool.companion_skills.iter().any(|s| s == skill_name) {
            // V5「随包」认领：包本体已装才把技能归属到包（与 list_bundles 的认领
            // 条件一致）；未装时技能保留独立纯技能包形态（owner = 技能名自身）。
            // 保证 save 归一与物化排除跟 UI 展示的包形态对齐（二轮评审：scope
            // save 归一与 V5 条件认领冲突）。
            if bundle_installed(&tool.id) {
                return tool.id;
            }
            break;
        }
    }
    skill_name.to_string()
}

/// 包是否已安装：BundleStore 记录优先；store 不可读时回退 installed.json——
/// 与 `list_bundles` 的 V5 认领判定同口径（Phase 2 过渡期 installed.json 仍权威）。
pub(crate) fn bundle_installed(id: &str) -> bool {
    match super::store::BundleStore::new().records() {
        Ok(records) => records.iter().any(|r| r.id == id && r.installed),
        Err(_) => MarketplaceManager::new()
            .installed_ids()
            .iter()
            .any(|installed| installed == id),
    }
}

/// 凭据目标：env（mcp.json 环境变量占位）、credential（系统凭据存储）、bearer（Authorization 头）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTarget {
    Env,
    Credential,
    Bearer,
}

/// CredentialTarget → keyring 存储的 target 字符串（env/header/credential）。
pub fn keyring_target(target: CredentialTarget) -> &'static str {
    match target {
        CredentialTarget::Env => "env",
        CredentialTarget::Bearer => "header",
        CredentialTarget::Credential => "credential",
    }
}

/// 包声明的凭据项（修复方案一：从 config_fields/secret_env/secret_headers 收敛）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialSpec {
    pub key: String,
    pub target: CredentialTarget,
    pub required: bool,
}

/// 配置弹窗字段的功能事实（修复方案 V4 下沉部分）。
/// label/placeholder/helpText 属 i18n 展示资产，留前端 overlay 按包 id 索引，不进后端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFieldSpec {
    pub key: String,
    pub required: bool,
    pub target: CredentialTarget,
    pub secret: bool,
}

/// 包形态（内容现算，不落存储）。优先级定死（修复方案 V2）：
/// cli 非空 → Cli > servers+skills 均非空 → Bundle > servers 非空 → Mcp > skills 非空 → Skill。
/// 注：旧 `Spanner` 变体已删除——脚本可执行能力并入 skill 包，通过 SKILL.md frontmatter
/// `tools[]` + `runtime` 段声明，由 skill_marketplace::install 后置 hook 注册。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    /// CLI 包：cli 非空（飞书/企微/钉钉/腾讯会议等内置连接器）
    Cli,
    /// 组合包：mcp_servers 与 skills 均非空（MCP 函数 + 使用引导一体）
    Bundle,
    /// 纯 MCP：mcp_servers 非空、skills/cli 空
    Mcp,
    /// 纯技能包：仅 skills（市场预置、用户上传；含凭据型技能包如 ima）
    Skill,
}

/// 空包错误：mcp_servers/skills/cli 全空（修复方案 V7，schema 层拦截，不默认归 Skill）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidBundle;

/// 就绪态（派生态，不进存储）。UI 消费 (installed, ready)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    /// 未就绪；reason 给前端提示（如缺凭据的 key 列表）
    NotReady(&'static str),
}

/// 能力包清单条目（前端消费；id 沿用现有命名空间：MCP 工具 id / 技能 id / CLI 连接器 id）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleInfo {
    pub id: String,
    pub name: String,
    pub kind: BundleKind,
    /// 包内 MCP server id（无则空）
    pub mcp_servers: Vec<String>,
    /// 包内技能 id（无则空）
    pub skills: Vec<String>,
    /// 包内 CLI 连接器 id（无则空）
    pub cli: Vec<String>,
    /// 包声明的凭据项（收敛自 config_fields/secret_env/secret_headers）
    pub credentials: Vec<CredentialSpec>,
    /// 功能事实（修复方案 V4 下沉；icon/color/todayImg/welcomeQueries/i18n 留前端 overlay）
    pub description: String,
    pub version: String,
    pub auth_required: bool,
    /// 配置弹窗字段功能事实（label/placeholder/helpText 留前端）
    pub config_fields: Vec<ConfigFieldSpec>,
    pub installed: bool,
    /// 用户上传（非预置），前端用默认图标渲染
    pub user_uploaded: bool,
    /// `Degraded` 异常态原因（§3.2：登记在、资源缺），从 BundleStore 记录透传，
    /// 前端据此提示修复动作（按来源重新获取）；None = 资源完整。只是资源完整性
    /// 标记，不参与 ready 判定（ready 仍为派生态，Readiness 枚举不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    /// 预置技能内容落后于嵌入资源（App 升级带入新版或本地被改过）→ 动作下发
    /// `update`。非预置技能恒 false（上传技能无嵌入对应物）。
    #[serde(default)]
    pub update_available: bool,
    /// 含远程 OAuth server（manifest `servers` 非空）→ 未安装时动作下发
    /// `connect`（flow=oauth）而非 `install`/`configure`。
    #[serde(default)]
    pub oauth: bool,
    /// 业务分类（功能事实：docs/collab/life/office…，技能为 "skill"）——
    /// 前端业务分组取数；分类的展示名（i18n label）仍留前端 overlay。
    #[serde(default)]
    pub category: String,
    /// 图标相对包目录路径（`icon.svg`/`icon.png`；缺省 None → 前端用默认图标）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// 注册表：从现有源汇总包清单。只读；安装/门禁/投影迁移见后续步骤。
pub struct BundleRegistry {
    mcp_manager: MarketplaceManager,
    skill_manager: SkillMarketplaceManager,
}

impl Default for BundleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BundleRegistry {
    pub fn new() -> Self {
        Self {
            mcp_manager: MarketplaceManager::new(),
            skill_manager: SkillMarketplaceManager::new(),
        }
    }

    /// 全部能力包（含未安装）。组合规则：
    /// - MCP manifest 的 `companion_skills` 声明 → 技能归入该 MCP 包（组合包）
    /// - 未被任何 MCP 引用的预置技能 → 独立纯技能包
    /// - 用户上传技能 → 独立纯技能包
    /// - 内置 CLI 连接器 → CLI 包（修复方案 V2：ima 移出，归凭据型技能包）
    /// - ima（凭据型）：OpenAPI 凭据 + companion 技能 ima-skills → 纯技能包（凭据型）
    pub fn list_bundles(&self) -> Vec<BundleInfo> {
        let mut out: Vec<BundleInfo> = Vec::new();
        // 已被 MCP/凭据型包认领的技能 id（ima-skills 须在预置扫描前声明，避免先独立成包）
        let mut skill_claimed: Vec<String> = vec!["ima-skills".to_string()];
        let installed_mcp_ids = self.mcp_manager.installed_ids();

        // installed 真相源反转（§3.2，Phase 2 第三刀）：bundles.json 是唯一可写
        // 存储，installed / degraded 从 BundleStore 记录读；无记录 = 未安装。
        // 过渡期防御：store 读失败（如损坏 JSON）回退旧的文件推导并 log::warn ——
        // Phase 3 切换后删除回退分支（届时读失败应直接报错）。
        let store_records = match store::BundleStore::new().records() {
            Ok(records) => Some(records),
            Err(e) => {
                log::warn!("[marketplace] BundleStore 读取失败，installed 回退旧文件推导: {e}");
                None
            }
        };
        // 返回 Some((installed, degraded))；store 不可用 → None（调用方走回退值）。
        let store_state = |id: &str| -> Option<(bool, Option<String>)> {
            store_records.as_ref().map(|records| {
                records
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| (r.installed, r.degraded.clone()))
                    .unwrap_or((false, None))
            })
        };

        // 1) MCP 源（含组合包；凭据项从 manifest config_fields/secret_env 收敛）
        for tool in self.mcp_manager.available_tools() {
            let (installed, degraded) = store_state(&tool.id)
                .unwrap_or_else(|| (installed_mcp_ids.contains(&tool.id), None));
            // 修复方案 V5：companion 认领是「随包」语义——包本体已装才认领技能；
            // 存量已单装技能的包未装时，技能保留独立纯技能包形态（不强制认领，
            // 只影响新装路径），避免用户既有开关/技能消失。
            let companions: Vec<String> = if installed {
                tool.companion_skills.clone()
            } else {
                Vec::new()
            };
            skill_claimed.extend(companions.iter().cloned());
            let credentials = tool_credentials(&tool);
            let config_fields = tool_config_fields(&tool);
            // auth_required 功能事实：有必填凭据或远程 server（OAuth）即需授权；
            // 本地免凭据工具（obsidian/pptx/gongwen）不需要。
            let auth_required = !config_fields.is_empty()
                || !tool.secret_env.is_empty()
                || !tool.secret_headers.is_empty()
                || !tool.servers.is_empty();
            out.push(BundleInfo {
                id: tool.id.clone(),
                name: tool.name.clone(),
                kind: if companions.is_empty() {
                    BundleKind::Mcp
                } else {
                    BundleKind::Bundle
                },
                mcp_servers: vec![tool.id.clone()],
                skills: companions,
                cli: Vec::new(),
                credentials,
                description: tool.description.clone(),
                version: tool.version.clone(),
                auth_required,
                config_fields,
                installed,
                user_uploaded: false,
                degraded,
                update_available: false,
                oauth: !tool.servers.is_empty(),
                category: tool.category.clone(),
                icon: bundle_icon_path(&tool.id),
            });
        }

        // 2) 预置技能源（未被认领的独立成包）
        for skill in self.skill_manager.list_skills() {
            if skill.user_uploaded {
                continue; // 上传技能单独处理
            }
            if skill_claimed.contains(&skill.id) {
                continue;
            }
            // id 唯一性：与既有包同名的技能不再独立成包。同名 companion 技能
            // （pptx 技能 ↔ pptx MCP）由同名 MCP 包全权代表——装后认领并 derive 为
            // Bundle 携带技能；未装时若再独立成包会产生两个同 id 包，破坏
            // 「一个包 = 一个开关」前提（前端同名技能装卸本就路由到该 MCP）。
            if out.iter().any(|b| b.id == skill.id) {
                continue;
            }
            let (installed, degraded) = store_state(&skill.id).unwrap_or((skill.installed, None));
            out.push(BundleInfo {
                id: skill.id.clone(),
                name: skill.title.clone(),
                kind: BundleKind::Skill,
                mcp_servers: Vec::new(),
                skills: vec![skill.id.clone()],
                cli: Vec::new(),
                credentials: Vec::new(),
                description: skill.description.clone(),
                version: String::new(),
                auth_required: false,
                config_fields: Vec::new(),
                installed,
                user_uploaded: false,
                degraded,
                update_available: skill.update_available,
                oauth: false,
                category: "skill".to_string(),
                icon: bundle_icon_path(&skill.id),
            });
        }

        // 3) 上传技能源（独立纯技能包；后续步骤改走统一安装管线）
        for skill in self.skill_manager.list_skills() {
            if !skill.user_uploaded {
                continue;
            }
            let (installed, degraded) = store_state(&skill.id).unwrap_or((true, None));
            out.push(BundleInfo {
                id: skill.id.clone(),
                name: skill.title.clone(),
                kind: BundleKind::Skill,
                mcp_servers: Vec::new(),
                skills: vec![skill.id.clone()],
                cli: Vec::new(),
                credentials: Vec::new(),
                description: skill.description.clone(),
                version: String::new(),
                auth_required: false,
                config_fields: Vec::new(),
                installed,
                user_uploaded: true,
                degraded,
                update_available: false,
                oauth: false,
                category: "skill".to_string(),
                icon: bundle_icon_path(&skill.id),
            });
        }

        // 4) CLI 连接器源（内置常量表；V2 后不含 ima。元数据已随 Phase 2 第七刀
        //    下沉：desc 取自常量表、version 取 lock 表钉住版本、auth_required 恒 true
        //    ——CLI 连接器都要授权；i18n 展示资产留前端 overlay）
        //    skills 登记配套官方技能目录（kind 仍由 cli 优先派生为 Cli）——注册表
        //    由此成为「连接器 → 配套技能」的单一真相源，供 companion 联动排除
        //    与技能解包门控取数。
        for (id, name, bin, skill_dirs, desc) in BUILTIN_CLI_BUNDLES {
            let (installed, degraded) =
                store_state(id).unwrap_or((self.cli_bundle_installed(id), None));
            // version 功能事实：lock 表钉住版本（tmeet 走 npm 无 lock 条目 → 空，
            // 前端 overlay 保留自报版本展示）
            let version = crate::platform::connector_lock::artifact_pin(bin)
                .map(|pin| pin.version)
                .unwrap_or_default();
            out.push(BundleInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                kind: BundleKind::Cli,
                mcp_servers: Vec::new(),
                skills: skill_dirs.iter().map(|s| (*s).to_string()).collect(),
                cli: vec![(*id).to_string()],
                credentials: Vec::new(),
                description: (*desc).to_string(),
                version,
                auth_required: true,
                config_fields: Vec::new(),
                installed,
                user_uploaded: false,
                degraded,
                update_available: false,
                oauth: false,
                category: "collab".to_string(),
                icon: bundle_icon_path(id),
            });
        }

        // 5) 凭据型技能包：ima（OpenAPI 凭据 + companion 技能 ima-skills；V2 归 Skill）。
        // 登记侧写入的记录 id 是 `ima-skills`（技能包 install 的登记口径），卡 id 是
        // `ima`——两个 id 任一有记录都算已装，保证「一个包 = 一张卡 = 一个开关」。
        // 注意区分「store 不可读」（回退推导）与「记录不存在」（再查别名 id）：
        // 通用 store_state 对缺记录也返回 Some((false, None))，直接 .or_else 会让
        // ima-skills 兜底永不触发（三轮评审死代码）。
        let ima_skills = ["ima-skills"];
        let (ima_installed, ima_degraded) = match &store_records {
            Some(records) => ["ima", "ima-skills"]
                .iter()
                .find_map(|id| records.iter().find(|r| r.id == *id))
                .map(|r| (r.installed, r.degraded.clone()))
                .unwrap_or((false, None)),
            None => (self.cli_bundle_installed("ima"), None),
        };
        out.push(BundleInfo {
            id: "ima".to_string(),
            name: "腾讯 ima".to_string(),
            kind: BundleKind::Skill,
            mcp_servers: Vec::new(),
            skills: ima_skills.iter().map(|s| s.to_string()).collect(),
            cli: Vec::new(),
            credentials: vec![
                CredentialSpec {
                    key: "IMA_CLIENT_ID".to_string(),
                    target: CredentialTarget::Credential,
                    required: true,
                },
                CredentialSpec {
                    key: "IMA_API_KEY".to_string(),
                    target: CredentialTarget::Credential,
                    required: true,
                },
            ],
            description: "接入腾讯 ima OpenAPI Skill：通过 Pinvou 内置的受控工具调用 ima.qq.com 官方 OpenAPI，支持笔记搜索/读取/创建/追加，以及知识库搜索、浏览、网页导入和内容添加。需要填写你自己的 Client ID 和 API Key，凭据只写入本机系统凭据，不进入对话、环境变量、仓库或 mcp.json。".to_string(),
            // 预置技能无版本概念（无版本号/无自动更新机制），version 留空，
            // 前端 overlay 保留自报版本展示
            version: String::new(),
            auth_required: true,
            config_fields: vec![
                ConfigFieldSpec {
                    key: "IMA_CLIENT_ID".to_string(),
                    required: true,
                    target: CredentialTarget::Credential,
                    secret: true,
                },
                ConfigFieldSpec {
                    key: "IMA_API_KEY".to_string(),
                    required: true,
                    target: CredentialTarget::Credential,
                    secret: true,
                },
            ],
            installed: ima_installed,
            user_uploaded: false,
            degraded: ima_degraded,
            update_available: false,
            oauth: false,
            category: "docs".to_string(),
            icon: bundle_icon_path("ima"),
        });

        out
    }

    pub fn bundle(&self, id: &str) -> Option<BundleInfo> {
        self.list_bundles().into_iter().find(|b| b.id == id)
    }

    /// CLI 包安装态的**回退推导**（仅在 BundleStore 读失败时启用，见 list_bundles
    /// 的反转注释）：飞书/企微/钉钉/腾讯会议走 CLI 认证状态，ima 走凭据配置态。
    /// 现有判定散落在前端连接态/后端 status 命令，此处先给保守默认（未安装），
    /// 安装态状态机后续步骤统一定义。
    fn cli_bundle_installed(&self, _id: &str) -> bool {
        false
    }
}

/// 纯函数：由内容推导包形态（修复方案 V2 优先级定死 + V7 空包报错）。
/// 优先级：cli 非空 → Cli > mcp+skills 组合 → Bundle > mcp 非空 → Mcp
/// > skills 非空 → Skill；全空 → Err（空包 schema 层拦截）。
///
/// 注：旧 spanners 参数已删除——脚本可执行能力通过 skill 包的 SKILL.md frontmatter
/// `tools[]` 段声明，不影响 kind 推导。
pub fn derive_bundle_kind(
    mcp_servers: &[String],
    skills: &[String],
    cli: &[String],
) -> Result<BundleKind, InvalidBundle> {
    if !cli.is_empty() {
        Ok(BundleKind::Cli)
    } else if !mcp_servers.is_empty() && !skills.is_empty() {
        Ok(BundleKind::Bundle)
    } else if !mcp_servers.is_empty() {
        Ok(BundleKind::Mcp)
    } else if !skills.is_empty() {
        Ok(BundleKind::Skill)
    } else {
        Err(InvalidBundle)
    }
}

/// 探测包目录下的图标文件（`icon.svg`/`icon.png`），返回相对包目录的路径。
/// 已装工具图标与工具同目录（plugin-protocol §15.6）；无图标返回 None → 前端默认图标。
pub fn bundle_icon_path(id: &str) -> Option<String> {
    let pkg = crate::platform::paths::bundles_root().join(id);
    for name in ["icon.svg", "icon.png"] {
        if pkg.join(name).is_file() {
            return Some(name.to_string());
        }
    }
    None
}

/// 从 MCP ToolManifest 收敛凭据声明（修复方案一）：config_fields → credentials，
/// secret_env/secret_headers 按 target 映射（env/bearer），required 语义保留。
/// `pub(crate)`：存储层 `store::legacy_mcp_records` 复用同一推导取凭据 key。
pub(crate) fn tool_credentials(tool: &super::ToolManifest) -> Vec<CredentialSpec> {
    let mut out: Vec<CredentialSpec> = Vec::new();
    for f in &tool.config_fields {
        let target = match f.target.as_str() {
            "bearer" => CredentialTarget::Bearer,
            "credential" => CredentialTarget::Credential,
            _ => CredentialTarget::Env,
        };
        out.push(CredentialSpec {
            key: f.key.clone(),
            target,
            required: f.required,
        });
    }
    for s in &tool.secret_env {
        out.push(CredentialSpec {
            key: s.key.clone(),
            target: CredentialTarget::Env,
            required: s.required,
        });
    }
    for s in &tool.secret_headers {
        out.push(CredentialSpec {
            key: s.source_key.clone(),
            target: CredentialTarget::Bearer,
            required: s.required,
        });
    }
    out
}

/// 配置弹窗字段功能事实（V4 下沉；label/placeholder/helpText 属 i18n 展示资产留前端）。
fn tool_config_fields(tool: &super::ToolManifest) -> Vec<ConfigFieldSpec> {
    let mut out: Vec<ConfigFieldSpec> = Vec::new();
    for f in &tool.config_fields {
        out.push(ConfigFieldSpec {
            key: f.key.clone(),
            required: f.required,
            target: match f.target.as_str() {
                "bearer" => CredentialTarget::Bearer,
                "credential" => CredentialTarget::Credential,
                _ => CredentialTarget::Env,
            },
            secret: f.secret,
        });
    }
    for s in &tool.secret_env {
        out.push(ConfigFieldSpec {
            key: s.key.clone(),
            required: s.required,
            target: CredentialTarget::Env,
            secret: true,
        });
    }
    for s in &tool.secret_headers {
        out.push(ConfigFieldSpec {
            key: s.source_key.clone(),
            required: s.required,
            target: CredentialTarget::Bearer,
            secret: true,
        });
    }
    out
}

/// 就绪态判定（派生态，现算不进存储）。
/// - CLI 包：授权存在与否——由命令层经 `bundle_readiness` 分派到各 status 查询注入
///   （注册表不直连 CLI 运行时，注入闭包保持依赖方向 app → features）
/// - 凭据型：credentials 必填项在系统凭据存储中齐不齐（现算）
/// - 本地免凭据：恒 Ready
pub fn readiness_for(bundle: &BundleInfo, credential_has: impl Fn(&str) -> bool) -> Readiness {
    match bundle.kind {
        // CLI 包授权态由调用方（命令层）注入；此处按 installed 保守返回——
        // 命令层 `bundle_readiness` 覆盖 CLI 分派后，此分支不达。
        BundleKind::Cli => {
            if bundle.installed {
                Readiness::Ready
            } else {
                Readiness::NotReady("cli_not_installed")
            }
        }
        BundleKind::Mcp | BundleKind::Bundle => {
            // 本地免凭据（无必填凭据）恒 Ready；有必填凭据则查系统凭据
            let missing: Vec<&str> = bundle
                .credentials
                .iter()
                .filter(|c| c.required && !credential_has(&c.key))
                .map(|c| c.key.as_str())
                .collect();
            if missing.is_empty() {
                Readiness::Ready
            } else {
                Readiness::NotReady("missing_credentials")
            }
        }
        BundleKind::Skill => {
            let missing: Vec<&str> = bundle
                .credentials
                .iter()
                .filter(|c| c.required && !credential_has(&c.key))
                .map(|c| c.key.as_str())
                .collect();
            if missing.is_empty() {
                Readiness::Ready
            } else {
                Readiness::NotReady("missing_credentials")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包,跑完恢复并清理。
    /// 与 marketplace/mod.rs / paths 测试共享 ENV_LOCK（V6：CI rust-test 已启用，
    /// 并行跑会互相覆盖 PINVOU3_HOME，必须串行）。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("pinvou3-bundle-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// fixture：临时 home 下构造 mcp-servers manifest + 技能目录 + 上传标记。
    /// `install_gongwen=true` 时把 gongwen 写入 installed.json（V5 认领条件）。
    /// 布局按包聚合新布局：MCP manifest 落 `bundles/<id>/mcp/`，技能落
    /// `bundles/<owner>/skills/<name>/`（旧扁平 `bundle/skills/` 已退役）。
    fn seed_fixture(home: &std::path::Path, install_gongwen: bool) {
        let write = |rel: &str, content: &str| {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        };
        // 组合包 gongwen（companion 声明 government-writing）
        write(
            "bundles/gongwen/mcp/manifest.json",
            r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["government-writing"]}"#,
        );
        // 纯 MCP 包 weather
        write(
            "bundles/weather/mcp/manifest.json",
            r#"{"id":"weather","name":"高德天气","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"","args":[]}"#,
        );
        // 预置技能（被认领 + 独立），按包聚合新布局落位：
        // government-writing 属主 = gongwen（companion），visualizer 独立成包。
        for (owner, name) in [
            ("gongwen", "government-writing"),
            ("visualizer", "visualizer"),
        ] {
            write(
                &format!("bundles/{owner}/skills/{name}/SKILL.md"),
                "---\nname: {name}\n---\n# hi",
            );
        }
        // 上传技能（独立纯技能包）
        write(
            "bundles/my-upload/skills/my-upload/SKILL.md",
            "---\nname: my-upload\n---\n# hi",
        );
        // 上传技能的枚举自刀十起是 BundleStore 记录驱动（`.installed-from` 标记已退役）
        store::BundleStore::new()
            .upsert(store::BundleRecord::installed_now(
                "my-upload",
                store::BundleSource::Upload("pkg.zip".to_string()),
            ))
            .unwrap();
        // 旧布局安装态（V5 认领的回退推导来源；真相源反转后由 store_install 覆盖）
        if install_gongwen {
            write("marketplace/installed.json", r#"["gongwen"]"#);
        }
    }

    /// 往 BundleStore 写安装记录（真相源反转后 installed 的权威来源，§3.2）。
    fn store_install(ids: &[&str]) {
        let store = store::BundleStore::new();
        for id in ids {
            store
                .upsert(store::BundleRecord::installed_now(
                    *id,
                    store::BundleSource::Preset,
                ))
                .unwrap();
        }
    }

    #[test]
    fn derives_bundle_kind_by_content() {
        let mcp = |id: &str| id.to_string();
        // V7：空包报错，不默认归 Skill
        assert_eq!(derive_bundle_kind(&[], &[], &[]), Err(InvalidBundle));
        // V2 优先级：cli 恒赢 > Bundle > Mcp > Skill
        assert_eq!(
            derive_bundle_kind(&[mcp("a")], &[], &[]),
            Ok(BundleKind::Mcp)
        );
        assert_eq!(
            derive_bundle_kind(&[mcp("a")], &[mcp("s")], &[]),
            Ok(BundleKind::Bundle)
        );
        assert_eq!(
            derive_bundle_kind(&[], &[mcp("s")], &[]),
            Ok(BundleKind::Skill)
        );
        assert_eq!(
            derive_bundle_kind(&[], &[], &[mcp("feishu")]),
            Ok(BundleKind::Cli)
        );
        // 即使有 servers+skills，cli 非空仍归 Cli
        assert_eq!(
            derive_bundle_kind(&[mcp("a")], &[mcp("s")], &[mcp("feishu")]),
            Ok(BundleKind::Cli)
        );
    }

    #[test]
    fn readiness_rules() {
        let b = |kind: BundleKind, creds: Vec<CredentialSpec>| BundleInfo {
            id: "x".into(),
            name: "x".into(),
            kind,
            mcp_servers: vec![],
            skills: vec![],
            cli: vec![],
            credentials: creds,
            description: String::new(),
            version: String::new(),
            auth_required: false,
            config_fields: vec![],
            installed: true,
            user_uploaded: false,
            degraded: None,
            update_available: false,
            oauth: false,
            category: String::new(),
            icon: None,
        };
        // 本地免凭据 → 恒 Ready
        assert_eq!(
            readiness_for(&b(BundleKind::Mcp, vec![]), |_| false),
            Readiness::Ready
        );
        // 必填凭据缺失 → NotReady
        let creds = vec![CredentialSpec {
            key: "AMAP_KEY".into(),
            target: CredentialTarget::Env,
            required: true,
        }];
        assert_eq!(
            readiness_for(&b(BundleKind::Mcp, creds.clone()), |k| k != "AMAP_KEY"),
            Readiness::NotReady("missing_credentials")
        );
        assert_eq!(
            readiness_for(&b(BundleKind::Mcp, creds), |k| k == "AMAP_KEY"),
            Readiness::Ready
        );
        // 非必填凭据缺失不影响 ready
        let opt = vec![CredentialSpec {
            key: "OPT".into(),
            target: CredentialTarget::Credential,
            required: false,
        }];
        assert_eq!(
            readiness_for(&b(BundleKind::Skill, opt), |_| false),
            Readiness::Ready
        );
        // CLI 未安装 → NotReady（命令层注入真实授权态后此分支不达）
        let mut cli_b = b(BundleKind::Cli, vec![]);
        cli_b.installed = false;
        assert_eq!(
            readiness_for(&cli_b, |_| false),
            Readiness::NotReady("cli_not_installed")
        );
    }

    #[test]
    fn keyring_target_matches_install_storage() {
        assert_eq!(keyring_target(CredentialTarget::Env), "env");
        assert_eq!(keyring_target(CredentialTarget::Bearer), "header");
        assert_eq!(keyring_target(CredentialTarget::Credential), "credential");
    }

    #[test]
    fn collects_tool_credentials() {
        let tool = super::super::ToolManifest {
            id: "t".into(),
            name: "t".into(),
            description: String::new(),
            version: String::new(),
            icon: String::new(),
            category: String::new(),
            mcp_tools: vec![],
            command: String::new(),
            args: vec![],
            env: Default::default(),
            secret_env: vec![super::super::SecretEnv {
                key: "SEC".into(),
                provider: String::new(),
                required: true,
            }],
            secret_headers: vec![],
            validate_on_install: false,
            config_fields: vec![super::super::ConfigField {
                key: "KEY".into(),
                label: String::new(),
                required: true,
                target: "env".into(),
                secret: false,
            }],
            routing_rules: vec![],
            tool_table_entries: vec![],
            pip_dependencies: vec![],
            servers: vec![],
            companion_skills: vec![],
        };
        let creds = tool_credentials(&tool);
        assert_eq!(creds.len(), 2);
        assert_eq!(creds[0].key, "KEY");
        assert_eq!(creds[0].target, CredentialTarget::Env);
        assert!(creds[0].required);
        assert_eq!(creds[1].key, "SEC");
    }

    #[test]
    fn registry_lists_all_source_kinds() {
        with_temp_home(|| {
            let home = std::env::var("PINVOU3_HOME").unwrap();
            seed_fixture(std::path::Path::new(&home), true);
            // 真相源反转后 installed 以 BundleStore 为准（V5 认领条件同源）
            store_install(&["gongwen"]);
            let reg = BundleRegistry::new();
            let bundles = reg.list_bundles();
            // 四类源都存在
            assert!(
                bundles.iter().any(|b| b.kind == BundleKind::Mcp),
                "应含纯 MCP 包"
            );
            assert!(
                bundles.iter().any(|b| b.kind == BundleKind::Bundle),
                "应含组合包"
            );
            assert!(
                bundles.iter().any(|b| b.kind == BundleKind::Skill),
                "应含纯技能包"
            );
            assert!(
                bundles.iter().any(|b| b.kind == BundleKind::Cli),
                "应含 CLI 包"
            );
            // gongwen 已装 → 组合包，携带 government-writing
            let gongwen = bundles
                .iter()
                .find(|b| b.id == "gongwen")
                .expect("gongwen 应存在");
            assert_eq!(gongwen.kind, BundleKind::Bundle, "gongwen 已装应为组合包");
            assert!(
                gongwen.skills.contains(&"government-writing".to_string()),
                "gongwen 应携带 government-writing"
            );
            assert!(gongwen.installed);
            // government-writing 不应再以独立技能包出现（已被认领）
            assert!(
                !bundles
                    .iter()
                    .any(|b| b.id == "government-writing" && b.kind == BundleKind::Skill),
                "被认领技能不得独立成包"
            );
            // 上传技能 = 独立纯技能包 + user_uploaded
            let upload = bundles
                .iter()
                .find(|b| b.id == "my-upload")
                .expect("上传技能包应存在");
            assert_eq!(upload.kind, BundleKind::Skill);
            assert!(upload.user_uploaded);
            // CLI 包 id 覆盖内置清单（V2：ima 不在 CLI 包）
            for (id, ..) in BUILTIN_CLI_BUNDLES {
                assert!(
                    bundles
                        .iter()
                        .any(|b| b.id == *id && b.kind == BundleKind::Cli),
                    "CLI 包 {id} 应存在"
                );
            }
            // V2：ima 归凭据型技能包（Skill），且携带 ima-skills + 凭据声明
            let ima = bundles
                .iter()
                .find(|b| b.id == "ima")
                .expect("ima 包应存在");
            assert_eq!(ima.kind, BundleKind::Skill, "ima 应归 Skill");
            assert!(ima.skills.contains(&"ima-skills".to_string()));
            assert!(
                ima.credentials
                    .iter()
                    .any(|c| c.key == "IMA_API_KEY" && c.required),
                "ima 应声明必填凭据"
            );
            // ima-skills 不得再独立成包（被 ima 认领）
            assert!(
                !bundles.iter().any(|b| b.id == "ima-skills"),
                "ima-skills 不得独立成包"
            );
            // id 唯一（一个包 = 一个开关的前提）
            let mut ids: Vec<&str> = bundles.iter().map(|b| b.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), bundles.len(), "包 id 必须唯一");
        });
    }

    /// V5：包本体未装时 companion 技能保留独立纯技能包形态（存量单装兼容）。
    #[test]
    fn uninstalled_bundle_keeps_companion_skill_independent() {
        with_temp_home(|| {
            let home = std::env::var("PINVOU3_HOME").unwrap();
            seed_fixture(std::path::Path::new(&home), false);
            // 存量单装：installed 以 BundleStore 记录为准
            store_install(&["government-writing"]);
            let reg = BundleRegistry::new();
            let bundles = reg.list_bundles();
            // gongwen 未装 → 纯 MCP 包（不认领）
            let gongwen = bundles
                .iter()
                .find(|b| b.id == "gongwen")
                .expect("gongwen 应存在");
            assert_eq!(gongwen.kind, BundleKind::Mcp, "gongwen 未装应为纯 MCP 包");
            assert!(gongwen.skills.is_empty(), "未装包不得认领技能");
            assert!(!gongwen.installed);
            // government-writing 保留独立技能包（存量单装可继续开关）
            let skill = bundles
                .iter()
                .find(|b| b.id == "government-writing")
                .expect("government-writing 应独立成包");
            assert_eq!(skill.kind, BundleKind::Skill);
            assert!(skill.installed, "存量单装技能保持已装态");
        });
    }

    /// pptx 组合包化 × V5：companion 技能与 MCP 同名（pptx↔pptx）时——
    /// 未装 MCP：纯 MCP 包，同名技能**不**独立成包（否则两个包同 id，破坏唯一性）；
    /// 已装 MCP：认领同名技能，derive 为 Bundle 携带技能。
    #[test]
    fn pptx_same_id_companion_claim_follows_install_state() {
        with_temp_home(|| {
            let home = std::env::var("PINVOU3_HOME").unwrap();
            let home = std::path::Path::new(&home).to_path_buf();
            seed_fixture(&home, false);
            let write = |rel: &str, content: &str| {
                let p = home.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, content).unwrap();
            };
            // pptx MCP（manifest 声明同名 companion 技能）+ 预置 pptx 技能
            write(
                "bundle/mcp-servers/pptx/manifest.json",
                r#"{"id":"pptx","name":"PPT 生成","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["pptx"]}"#,
            );
            write("bundle/skills/pptx/SKILL.md", "---\nname: pptx\n---\n# hi");
            write(
                "bundle/skills/pptx/.installed-from",
                "pinvou3-marketplace:pptx",
            );

            // 未装：纯 MCP 包；同名技能不独立成包（包 id 唯一）
            let bundles = BundleRegistry::new().list_bundles();
            let pptx: Vec<_> = bundles.iter().filter(|b| b.id == "pptx").collect();
            assert_eq!(pptx.len(), 1, "同名技能不得再独立成包（包 id 唯一）");
            assert_eq!(pptx[0].kind, BundleKind::Mcp, "未装应为纯 MCP 包");
            assert!(pptx[0].skills.is_empty(), "未装包不得认领技能");
            assert!(!pptx[0].installed);

            // 装后：认领同名技能 → 组合包（installed 真相源 = BundleStore）
            store_install(&["pptx"]);
            let bundles = BundleRegistry::new().list_bundles();
            let pptx: Vec<_> = bundles.iter().filter(|b| b.id == "pptx").collect();
            assert_eq!(pptx.len(), 1);
            assert_eq!(pptx[0].kind, BundleKind::Bundle, "装后应为组合包");
            assert!(
                pptx[0].skills.contains(&"pptx".to_string()),
                "装后应携带同名 companion 技能"
            );
            assert!(pptx[0].installed);
        });
    }

    /// installed 真相源反转（§3.2）：BundleStore 是唯一权威 —— 装了→true、
    /// 卸载→false、degraded 透传；store 读失败（损坏 JSON）回退旧文件推导。
    #[test]
    fn installed_reads_bundle_store_with_legacy_fallback() {
        with_temp_home(|| {
            let home = std::env::var("PINVOU3_HOME").unwrap();
            seed_fixture(std::path::Path::new(&home), true); // installed.json 含 gongwen

            // 1) store 无记录 → 未安装（即使 installed.json 说装了：store 是真相源）
            let gongwen = BundleRegistry::new().bundle("gongwen").unwrap();
            assert!(!gongwen.installed, "store 无记录应为未安装");

            // 2) store 登记 → 已安装；degraded 透传给前端
            let store = store::BundleStore::new();
            let mut record =
                store::BundleRecord::installed_now("gongwen", store::BundleSource::Preset);
            record.degraded = Some("资源缺失".to_string());
            store.upsert(record).unwrap();
            let gongwen = BundleRegistry::new().bundle("gongwen").unwrap();
            assert!(gongwen.installed, "store 登记应为已安装");
            assert_eq!(gongwen.degraded.as_deref(), Some("资源缺失"));

            // 3) store 删记录 → 未安装
            store.remove("gongwen").unwrap();
            assert!(!BundleRegistry::new().bundle("gongwen").unwrap().installed);

            // 4) store 损坏 → 回退旧文件推导（installed.json 含 gongwen → 已安装）
            std::fs::write(store.file_path(), "corrupt{{{").unwrap();
            let gongwen = BundleRegistry::new().bundle("gongwen").unwrap();
            assert!(gongwen.installed, "store 读失败应回退 installed.json 推导");
            assert_eq!(gongwen.degraded, None, "回退推导无 degraded 信息");
        });
    }

    /// Phase 2 第七刀：CLI/ima 元数据下沉——desc/version/category/auth_required
    /// 是功能事实而非结构占位；version 与 lock 表钉住版本一致。
    #[test]
    fn cli_and_ima_bundles_carry_functional_metadata() {
        with_temp_home(|| {
            let bundles = BundleRegistry::new().list_bundles();
            for (id, _, bin, _, desc) in BUILTIN_CLI_BUNDLES {
                let b = bundles.iter().find(|b| b.id == *id).expect("CLI 包应存在");
                assert_eq!(b.description, *desc, "{id} desc 应取自常量表");
                assert!(!b.description.is_empty());
                assert!(b.auth_required, "{id} 应恒需授权");
                assert_eq!(b.category, "collab", "{id} 业务分类");
                match crate::platform::connector_lock::artifact_pin(bin) {
                    Some(pin) => assert_eq!(
                        b.version, pin.version,
                        "{id} version 应与 lock 表钉住版本一致"
                    ),
                    // tmeet（npm，无 lock 条目）/不支持的平台 → version 留空
                    None => assert!(b.version.is_empty(), "{id} 无 lock 条目 version 应空"),
                }
            }
            let ima = bundles
                .iter()
                .find(|b| b.id == "ima")
                .expect("ima 包应存在");
            assert!(!ima.description.is_empty(), "ima desc 应下沉");
            assert!(ima.auth_required);
            assert_eq!(ima.category, "docs");
            assert!(ima.version.is_empty(), "ima 无版本概念（overlay 保留展示）");
            // config_fields 与 credentials 同 key 同源（弹窗功能事实 ↔ 凭据声明）
            let cfg_keys: Vec<&str> = ima.config_fields.iter().map(|f| f.key.as_str()).collect();
            let cred_keys: Vec<&str> = ima.credentials.iter().map(|c| c.key.as_str()).collect();
            assert_eq!(cfg_keys, cred_keys, "ima 配置字段与凭据声明应同口径");
        });
    }

    /// 回归（三轮评审）：store 仅含 `ima-skills` 记录（技能包 install 的登记口径）时，
    /// ima 卡必须 installed=true —— 此前通用 store_state 对缺记录返回 Some((false,None))，
    /// `.or_else(|| store_state("ima-skills"))` 永不触发（死代码），ima 卡恒未安装。
    #[test]
    fn ima_card_installed_from_ima_skills_record() {
        with_temp_home(|| {
            // 无记录 → 未安装
            assert!(
                !BundleRegistry::new().bundle("ima").unwrap().installed,
                "无记录时 ima 应为未安装"
            );
            // 仅 ima-skills 记录 → ima 卡已安装
            store_install(&["ima-skills"]);
            let ima = BundleRegistry::new().bundle("ima").unwrap();
            assert!(
                ima.installed,
                "store 仅含 ima-skills 记录时 ima 卡应为已安装"
            );
            // ima 记录优先于 ima-skills 记录（含 degraded 透传）
            let store = store::BundleStore::new();
            let mut record = store::BundleRecord::installed_now("ima", store::BundleSource::Preset);
            record.installed = false;
            record.degraded = Some("资源缺失".to_string());
            store.upsert(record).unwrap();
            let ima = BundleRegistry::new().bundle("ima").unwrap();
            assert!(!ima.installed, "ima 记录存在时以其为准");
            assert_eq!(ima.degraded.as_deref(), Some("资源缺失"));
        });
    }
}
