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
use super::MarketplaceManager;

// 内置 CLI 连接器清单（修复方案 V2：ima 无 CLI 二进制，移出 CLI 包，归凭据型技能包）。
// 条目 = (id, 展示名, CLI 二进制名, 配套技能目录)。本表是「连接器 → CLI 二进制 /
// 配套技能」的单一真相源：能力包注册（下方 list_bundles）、companion 联动排除
// （MarketplaceManager::companion_skills）、execpolicy 硬拦截（engine_pool ruleset）
// 与技能解包门控（runtime_bundle apply_*_skills）全部从这里取数。
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

const BUILTIN_CLI_BUNDLES: &[(&str, &str, &str, &[&str])] = &[
    ("feishu", "飞书（Lark）", "lark-cli", LARK_SKILL_DIRS),
    ("wecom", "企业微信", "wecom-cli", WECOM_SKILL_DIRS),
    ("dingtalk", "钉钉", "dws", DINGTALK_SKILL_DIRS),
    ("tmeet", "腾讯会议", "tmeet", TMEET_SKILL_DIRS),
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
        .map(|(.., dirs)| *dirs)
        .unwrap_or(&[])
}

/// CLI 连接器的二进制名（execpolicy 硬拦截按它构造 deny 规则）。
pub fn cli_bundle_bin(id: &str) -> Option<&'static str> {
    BUILTIN_CLI_BUNDLES
        .iter()
        .find(|(cid, ..)| *cid == id)
        .map(|(.., bin, _)| *bin)
}

/// 凭据目标：env（mcp.json 环境变量占位）、credential（系统凭据存储）、bearer（Authorization 头）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTarget {
    Env,
    Credential,
    Bearer,
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

        // 1) MCP 源（含组合包；凭据项从 manifest config_fields/secret_env 收敛）
        for tool in self.mcp_manager.available_tools() {
            // 修复方案 V5：companion 认领是「随包」语义——包本体已装才认领技能；
            // 存量已单装技能的包未装时，技能保留独立纯技能包形态（不强制认领，
            // 只影响新装路径），避免用户既有开关/技能消失。
            let companions: Vec<String> = if installed_mcp_ids.contains(&tool.id) {
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
                installed: installed_mcp_ids.contains(&tool.id),
                user_uploaded: false,
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
                installed: skill.installed,
                user_uploaded: false,
            });
        }

        // 3) 上传技能源（独立纯技能包；后续步骤改走统一安装管线）
        for skill in self.skill_manager.list_skills() {
            if !skill.user_uploaded {
                continue;
            }
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
                installed: true,
                user_uploaded: true,
            });
        }

        // 4) CLI 连接器源（内置常量表；V2 后不含 ima。V4 硬约束：desc/version/
        //    auth_required 内容迁移随步骤 6（前端数据源切换）同 PR 完成，此处结构占位）
        //    skills 登记配套官方技能目录（kind 仍由 cli 优先派生为 Cli）——注册表
        //    由此成为「连接器 → 配套技能」的单一真相源，供 companion 联动排除
        //    与技能解包门控取数。
        for (id, name, _bin, skill_dirs) in BUILTIN_CLI_BUNDLES {
            out.push(BundleInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                kind: BundleKind::Cli,
                mcp_servers: Vec::new(),
                skills: skill_dirs.iter().map(|s| (*s).to_string()).collect(),
                cli: vec![(*id).to_string()],
                credentials: Vec::new(),
                description: String::new(),
                version: String::new(),
                auth_required: true,
                config_fields: Vec::new(),
                installed: self.cli_bundle_installed(id),
                user_uploaded: false,
            });
        }

        // 5) 凭据型技能包：ima（OpenAPI 凭据 + companion 技能 ima-skills；V2 归 Skill）
        let ima_skills = ["ima-skills"];
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
            description: String::new(),
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
            installed: self.cli_bundle_installed("ima"),
            user_uploaded: false,
        });

        out
    }

    pub fn bundle(&self, id: &str) -> Option<BundleInfo> {
        self.list_bundles().into_iter().find(|b| b.id == id)
    }

    /// CLI 包安装态：飞书/企微/钉钉/腾讯会议走 CLI 认证状态，ima 走凭据配置态。
    /// 现有判定散落在前端连接态/后端 status 命令，此处先给保守默认（未安装），
    /// 安装态状态机后续步骤统一定义。
    fn cli_bundle_installed(&self, _id: &str) -> bool {
        false
    }
}

/// 纯函数：由内容推导包形态（修复方案 V2 优先级定死 + V7 空包报错）。
/// 优先级：cli 非空 → Cli > servers+skills 均非空 → Bundle > servers 非空 → Mcp
/// > skills 非空 → Skill；全空 → Err（空包在 schema 层拦截，不默认归 Skill）。
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

/// 从 MCP ToolManifest 收敛凭据声明（修复方案一）：config_fields → credentials，
/// secret_env/secret_headers 按 target 映射（env/bearer），required 语义保留。
fn tool_credentials(tool: &super::ToolManifest) -> Vec<CredentialSpec> {
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
    fn seed_fixture(home: &std::path::Path, install_gongwen: bool) {
        let write = |rel: &str, content: &str| {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        };
        // 组合包 gongwen（companion 声明 government-writing）
        write(
            "bundle/mcp-servers/gongwen/manifest.json",
            r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["government-writing"]}"#,
        );
        // 纯 MCP 包 weather
        write(
            "bundle/mcp-servers/weather/manifest.json",
            r#"{"id":"weather","name":"高德天气","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"","args":[]}"#,
        );
        // 预置技能（被认领 + 独立）
        for (name, marker) in [
            (
                "government-writing",
                "pinvou3-marketplace:government-writing",
            ),
            ("visualizer", "pinvou3-marketplace:visualizer"),
        ] {
            write(
                &format!("bundle/skills/{name}/SKILL.md"),
                "---\nname: {name}\n---\n# hi",
            );
            write(&format!("bundle/skills/{name}/.installed-from"), marker);
        }
        // 上传技能
        write(
            "bundle/skills/my-upload/SKILL.md",
            "---\nname: my-upload\n---\n# hi",
        );
        write("bundle/skills/my-upload/.installed-from", "upload:pkg.zip");
        // 安装态（V5 认领条件）
        if install_gongwen {
            write("marketplace/installed.json", r#"["gongwen"]"#);
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

            // 装后：认领同名技能 → 组合包
            write("marketplace/installed.json", r#"["pptx"]"#);
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
}
