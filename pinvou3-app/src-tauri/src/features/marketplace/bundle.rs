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
const BUILTIN_CLI_BUNDLES: &[(&str, &str)] = &[
    ("feishu", "飞书（Lark）"),
    ("wecom", "企业微信"),
    ("dingtalk", "钉钉"),
    ("tmeet", "腾讯会议"),
];

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

        // 1) MCP 源（含组合包；凭据项从 manifest config_fields/secret_env 收敛）
        for tool in self.mcp_manager.available_tools() {
            let companions = tool.companion_skills.clone();
            skill_claimed.extend(companions.iter().cloned());
            let credentials = tool_credentials(&tool);
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
                installed: self.mcp_manager.installed_ids().contains(&tool.id),
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
            out.push(BundleInfo {
                id: skill.id.clone(),
                name: skill.title.clone(),
                kind: BundleKind::Skill,
                mcp_servers: Vec::new(),
                skills: vec![skill.id.clone()],
                cli: Vec::new(),
                credentials: Vec::new(),
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
                installed: true,
                user_uploaded: true,
            });
        }

        // 4) CLI 连接器源（内置常量表；V2 后不含 ima）
        for (id, name) in BUILTIN_CLI_BUNDLES {
            out.push(BundleInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                kind: BundleKind::Cli,
                mcp_servers: Vec::new(),
                skills: Vec::new(),
                cli: vec![(*id).to_string()],
                credentials: Vec::new(),
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
    /// 注：与 marketplace/mod.rs 测试的 ENV_LOCK 不互斥（各自持锁），
    /// 并行跑存在环境变量竞争——CI rust-test 当前 skipped，修复环境后需共享锁。
    fn with_temp_home<F: FnOnce()>(f: F) {
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
    fn seed_fixture(home: &std::path::Path) {
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
            seed_fixture(std::path::Path::new(&home));
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
            // gongwen 组合包应携带 government-writing 技能
            let gongwen = bundles
                .iter()
                .find(|b| b.id == "gongwen")
                .expect("gongwen 应存在");
            assert!(
                gongwen.skills.contains(&"government-writing".to_string()),
                "gongwen 应携带 government-writing"
            );
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
            for (id, _) in BUILTIN_CLI_BUNDLES {
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
}
