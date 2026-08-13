//! 能力包统一模型 — 步骤 1：schema + 注册表 + kind 推导（只读，不改安装/门禁/投影）。
//!
//! 设计依据：`docs/capability-governance.md` §3（能力包线）。一切外部能力统一建模为包：
//!
//! ```text
//! Bundle = { id, name, mcp_servers: [], skills: [], cli: [] }
//! ```
//!
//! - 包是唯一真相源；`mcp.json`、会话 skills 组合目录降级为投影（后续步骤实施）
//! - 包类型不做存储标签，`bundle_kind` 由内容现算（可信代码推导，防自报标签提权）
//! - 一个包 = 一个开关；包内技能可见性唯一跟随所属包
//!
//! 注册表汇总四类源：MCP manifest（`bundle/mcp-servers/<id>/manifest.json`）、
//! 预置技能（编译内嵌）、CLI 连接器（内置常量表，UI 元数据从 tool-common.jsx 迁移）、
//! 已上传技能（`bundle/skills/` 带 `.installed-from=upload:` 标记）。

use serde::{Deserialize, Serialize};

use super::skill_marketplace::SkillMarketplaceManager;
use super::MarketplaceManager;

// 内置 CLI 连接器清单（UI 元数据从 tool-common.jsx 的 feishuCli/wecomCli/dingtalkCli/
// tmeetCli/imaOpenapi 条目迁移；安装态 = CLI 安装 + 扫码/凭据配置，状态机后续步骤定义）。
const BUILTIN_CLI_BUNDLES: &[(&str, &str)] = &[
    ("feishu", "飞书（Lark）"),
    ("wecom", "企业微信"),
    ("dingtalk", "钉钉"),
    ("tmeet", "腾讯会议"),
    ("ima", "腾讯 ima"),
];

/// 包形态（内容现算，不落存储）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    /// 纯 MCP：mcp_servers 非空、其余空
    Mcp,
    /// 组合包：mcp_servers 与 skills 均非空（MCP 函数 + 使用引导一体）
    Bundle,
    /// CLI 包：cli 非空（飞书/企微/钉钉/tmeet/ima 等内置连接器）
    Cli,
    /// 纯技能包：仅 skills（市场预置、用户上传）
    Skill,
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
    /// - 内置 CLI 连接器 → CLI 包（ima 无 CLI 二进制，走 OpenAPI 凭据，kind 仍按内容推导）
    pub fn list_bundles(&self) -> Vec<BundleInfo> {
        let mut out: Vec<BundleInfo> = Vec::new();
        let mut skill_claimed: Vec<String> = Vec::new(); // 已被 MCP 包认领的技能 id

        // 1) MCP 源（含组合包）
        for tool in self.mcp_manager.available_tools() {
            let companions = tool.companion_skills.clone();
            skill_claimed.extend(companions.iter().cloned());
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
                installed: true,
                user_uploaded: true,
            });
        }

        // 4) CLI 连接器源（内置常量表；ima 为 OpenAPI 凭据型，仍归 CLI 线）
        for (id, name) in BUILTIN_CLI_BUNDLES {
            out.push(BundleInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                kind: BundleKind::Cli,
                mcp_servers: Vec::new(),
                skills: Vec::new(),
                cli: vec![(*id).to_string()],
                installed: self.cli_bundle_installed(id),
                user_uploaded: false,
            });
        }

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

/// 纯函数：由内容推导包形态（与 [`BundleInfo::kind`] 同一逻辑，供规则查表复用）。
pub fn derive_bundle_kind(mcp_servers: &[String], skills: &[String], cli: &[String]) -> BundleKind {
    if !cli.is_empty() {
        BundleKind::Cli
    } else if !mcp_servers.is_empty() && !skills.is_empty() {
        BundleKind::Bundle
    } else if !mcp_servers.is_empty() {
        BundleKind::Mcp
    } else {
        BundleKind::Skill
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
            ("government-writing", "pinvou3-marketplace:government-writing"),
            ("visualizer", "pinvou3-marketplace:visualizer"),
        ] {
            write(&format!("bundle/skills/{name}/SKILL.md"), "---\nname: {name}\n---\n# hi");
            write(
                &format!("bundle/skills/{name}/.installed-from"),
                marker,
            );
        }
        // 上传技能
        write("bundle/skills/my-upload/SKILL.md", "---\nname: my-upload\n---\n# hi");
        write("bundle/skills/my-upload/.installed-from", "upload:pkg.zip");
    }

    #[test]
    fn derives_bundle_kind_by_content() {
        let mcp = |id: &str| id.to_string();
        assert_eq!(derive_bundle_kind(&[], &[], &[]), BundleKind::Skill);
        assert_eq!(derive_bundle_kind(&[mcp("a")], &[], &[]), BundleKind::Mcp);
        assert_eq!(
            derive_bundle_kind(&[mcp("a")], &[mcp("s")], &[]),
            BundleKind::Bundle
        );
        assert_eq!(derive_bundle_kind(&[], &[], &[mcp("feishu")]), BundleKind::Cli);
        assert_eq!(
            derive_bundle_kind(&[mcp("a")], &[], &[mcp("feishu")]),
            BundleKind::Cli
        );
    }

    #[test]
    fn registry_lists_all_source_kinds() {
        with_temp_home(|| {
            let home = std::env::var("PINVOU3_HOME").unwrap();
            seed_fixture(std::path::Path::new(&home));
            let reg = BundleRegistry::new();
            let bundles = reg.list_bundles();
            // 四类源都存在
            assert!(bundles.iter().any(|b| b.kind == BundleKind::Mcp), "应含纯 MCP 包");
            assert!(bundles.iter().any(|b| b.kind == BundleKind::Bundle), "应含组合包");
            assert!(bundles.iter().any(|b| b.kind == BundleKind::Skill), "应含纯技能包");
            assert!(bundles.iter().any(|b| b.kind == BundleKind::Cli), "应含 CLI 包");
            // gongwen 组合包应携带 government-writing 技能
            let gongwen = bundles.iter().find(|b| b.id == "gongwen").expect("gongwen 应存在");
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
            let upload = bundles.iter().find(|b| b.id == "my-upload").expect("上传技能包应存在");
            assert_eq!(upload.kind, BundleKind::Skill);
            assert!(upload.user_uploaded);
            // CLI 包 id 覆盖内置清单
            for (id, _) in BUILTIN_CLI_BUNDLES {
                assert!(
                    bundles.iter().any(|b| b.id == *id && b.kind == BundleKind::Cli),
                    "CLI 包 {id} 应存在"
                );
            }
            // id 唯一（一个包 = 一个开关的前提）
            let mut ids: Vec<&str> = bundles.iter().map(|b| b.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), bundles.len(), "包 id 必须唯一");
        });
    }
}
