//! 技能市场管理器 — 管理 skill(SKILL.md 目录)的安装/卸载/上传导入。
//!
//! 与 MCP 工具市场([`super::marketplace`])刻意分开:MCP 工具是 server 进程(改
//! mcp.json),技能是磁盘上的 SKILL.md 目录。Phase 2 第十刀起按包聚合落盘:
//! 市场技能住 `~/.pinvou3/bundles/<pkg-id>/skills/<name>/`（§4 一个包 = 一个
//! 目录 = 一个属主，包 id 推导复用 `bundle::skill_owner_package`）；内置释放
//! 技能（visual-design 等，非市场包）仍住 `~/.pinvou3/bundle/skills/`。
//! `.installed-from` 标记已退役——来源与内容指纹进 BundleStore（bundles.json）。
//!
//! 预置技能(government-writing/pptx/visualizer/ima-skills)随 app 编译进二进制(`include_dir`),从嵌入资源复制到包目录——这是底座聊天**唯一加载**的 pinvou3 私有
//! skill 目录(fork patch #41 砍掉了其余扫描路径)。安装入口为 MCP 工具的配套技能
//! 联动(见 `marketplace::companion_skills`:装「公文写作」gongwen MCP 时一并装
//! government-writing、装「PPT 生成」pptx MCP 时一并装 pptx),已无独立「技能」市场页;用户上传 zip 技能包能力保留。
//!
//! 更新机制:无版本号。install/update 时把目录树内容指纹写入 BundleStore 记录
//! (`content_fingerprint`)；列出技能（打开插件中心）时按「记录指纹 vs 当前嵌入
//! 资源指纹（上游更新）∪ 磁盘指纹 vs 记录指纹（本地改动）」判定
//! `update_available`，前端显示"更新"按钮,用户确认后走 `install` 的原子覆盖重装(保留启用状态)。
//! 上传技能无嵌入对应物,不参与检测。
//!
//! 为何不复用底座 `skills::install`:那条通路对 monorepo / 带 plugin.json / 超
//! 5MiB 的仓库一律拒装,且选路逻辑私有硬编码。此处只做"已知来源的精确落盘",
//! 自带等价的路径穿越/symlink/大小安全防护(参照底座 install.rs 的判断)。

use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::platform::paths;

/// 预置技能资源:编译进二进制。每个子目录(pua/ nuwa/)是一个含 SKILL.md 的 skill。
static MARKETPLACE_DIR: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/skill-marketplace");

/// 单个 skill 子树未压缩大小上限(防御性,预置/上传都适用)。
/// `pub(crate)`:命令层 `import_skill_package_bytes` 复用同一上限。
pub(crate) const MAX_SKILL_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// 安装来源标记文件名。卸载时校验它存在,避免误删内置/手放的 skill。
const INSTALLED_FROM_MARKER: &str = ".installed-from";

/// 底座每次启动会清掉的已下线 skill 名;安装时拒绝撞名,免得装了被清。
pub(crate) const RETIRED_SKILL_NAMES: &[&str] = &[
    "legacy-ppt-workflow",
    "pinvou-review-plan",
    "pinvou-review-final",
];

// 预置技能清单 ----------------------------------------------------------------

#[derive(Debug, Clone)]
struct SkillManifest {
    /// 市场 key(前端/卸载用)
    id: &'static str,
    /// = SKILL.md frontmatter name = 落盘目录名
    skill_name: &'static str,
    /// MARKETPLACE_DIR 下的子目录名
    source_dir: &'static str,
    title: &'static str,
    subtitle: &'static str,
    description: &'static str,
    /// lucide 图标名(前端映射成组件)
    icon: &'static str,
    /// Tailwind 渐变 class
    color: &'static str,
}

fn preset_manifests() -> &'static [SkillManifest] {
    &[
        SkillManifest {
            id: "government-writing",
            skill_name: "government-writing",
            source_dir: "government-writing",
            title: "党政机关公文写作",
            subtitle: "通知/意见等法定文种，套话术、层级序号、自检",
            description: "撰写规范的党政机关公文（通知、意见…）：内置文种结构骨架、固定话术库、层级序号体系与立账核账自检，产出结构化公文内容。配合工具商店的「公文写作」工具即可直出 GB/T 9704 合规 .docx。",
            icon: "FileText",
            color: "bg-gradient-to-b from-red-500 to-rose-700",
        },
        SkillManifest {
            id: "pptx",
            skill_name: "pptx",
            source_dir: "pptx",
            title: "PPT 生成",
            subtitle: "本地直出可编辑 PowerPoint，套主题模板、真图表、带封面",
            description: "本地直出可编辑 PowerPoint（.pptx）：先列大纲确认，再按内容自动选主题（9 套）产结构化 deck，渲染器套主题模板生成真·可编辑图表、自带封面缩略图的演示文稿，数据不出机。配合工具商店的「PPT 生成」工具即可直出 .pptx。",
            icon: "Presentation",
            color: "bg-gradient-to-b from-orange-400 to-rose-500",
        },
        SkillManifest {
            id: "visualizer",
            skill_name: "visualizer",
            source_dir: "visualizer",
            title: "数据分析可视化",
            subtitle: "Chart.js 仪表盘 / 图表分析 / HTML 可视化",
            description: "将结构化数据、表格汇总和业务指标转成符合 Pinvou 宿主体验的 HTML 可视化仪表盘。默认使用 Chart.js、无障碍 canvas、自定义图例、扁平配色，并通过 .html 产物卡交付。",
            icon: "LineChart",
            color: "bg-gradient-to-b from-blue-500 to-cyan-600",
        },
        SkillManifest {
            id: "package-author",
            skill_name: "package-author",
            source_dir: "package-author",
            title: "插件包标准化",
            subtitle: "把技能/MCP/函数整理成可上传的标准插件包",
            description: "把散乱的技能（SKILL.md）、MCP 服务或它们的组合整理成 Pinvou 商店可导入的标准插件包：补 plugin.json、补 mcp/manifest.json、补 SKILL.md frontmatter、生成图标、校验命名与布局，最后产出目录或 zip。",
            icon: "Package",
            color: "bg-gradient-to-b from-emerald-500 to-teal-700",
        },
        SkillManifest {
            id: "skill-author",
            skill_name: "skill-author",
            source_dir: "skill-author",
            title: "技能创建",
            subtitle: "用户描述一句话，生成规范的 SKILL.md 技能",
            description: "把用户的一句话描述变成一个可用的技能（SKILL.md 目录）：生成 name/description/正文指令，校验命名与结构；需要交付成可上传插件包时，可继续按「插件包标准化」规则补 plugin.json、图标并导出标准包，最后询问用户是否安装。",
            icon: "Package",
            color: "bg-gradient-to-b from-violet-500 to-purple-700",
        },
        SkillManifest {
            id: "ima-skills",
            skill_name: "ima-skills",
            source_dir: "ima-skills",
            title: "腾讯 ima",
            subtitle: "IMA OpenAPI 笔记 / 知识库读取、写入、检索",
            description: "接入腾讯 ima OpenAPI，用本机凭据调用官方接口管理 IMA 笔记与知识库。凭据由 Pinvou 工具市场写入本机系统凭据，不需要在对话里粘贴 Token。",
            icon: "BookOpen",
            color: "bg-gradient-to-b from-sky-500 to-indigo-600",
        },
        SkillManifest {
            id: "tencent-docs-skill",
            skill_name: "tencent-docs",
            source_dir: "tencent-docs-skill",
            title: "腾讯文档",
            subtitle: "在线文档/表格/幻灯片/智能表格 创建、编辑、管理",
            description: "腾讯文档官方 MCP Skill（v1.0.41 适配版）：配合工具商店「腾讯文档 MCP」连接器使用，内置官方品类路由（智能文档/Word/Excel/PPT/思维导图/流程图/智能表格）与完整工具 API 参考。Token 由连接器写入本机系统凭据。",
            icon: "FileText",
            color: "bg-gradient-to-b from-blue-500 to-indigo-600",
        },
    ]
}

// 前端展示态 ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceSkillInfo {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub icon: String,
    pub color: String,
    pub installed: bool,
    /// true = 用户上传的(非预置),前端用默认图标渲染。
    pub user_uploaded: bool,
    /// true = 已安装预置技能的磁盘内容与当前嵌入资源不一致(App 升级带来新版,
    /// 或本地被改过),前端据此显示"更新"按钮;更新=覆盖重装,用户确认后执行。
    /// 无版本号概念:打开商店列表时做无状态目录树比对(见
    /// [`SkillMarketplaceManager::preset_update_available`])。未安装/上传技能恒 false。
    pub update_available: bool,
    /// 用户自定义展示名/说明覆盖的**原值**（仅上传技能；存于 bundles.json extra，
    /// 供前端编辑弹窗预填）。title/description 已是应用覆盖后的生效值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_description: Option<String>,
}

// 停用开关(按模式 scope 持久化)------------------------------------------------
//
// 技能停用按会话模式 scope 持久化到统一 `~/.pinvou3/disabled_bundles.json`
// (`{scopes: {<mode>: [...]}, initialized: [...]}`,scope 键即模式名,与连接器
// 开关同构),过滤职责移交
// **按会话拼的组合 skills_dir**(`features/assistant/skill_materialization.rs`):
// 组合目录内容 = 该会话 scope 的启用技能集,底座每轮重扫组合目录渲染
// `## Skills`。全局进程级 `DISABLED_SKILLS` 已退役(`set_disabled_skills(vec![])`,
// 见 lib.rs 启动段)——组合目录为空时整个块不渲染,路径泄露面随之封闭。
// companion 联动(禁用连接器 → 其配套技能一并隐藏)保留,改在组合目录计算时
// 按 scope 排除(见 `skill_materialization::disabled_skill_names_for`)。

// Manager ---------------------------------------------------------------------

pub struct SkillMarketplaceManager {
    /// 包目录根：市场技能落 `bundles/<pkg-id>/skills/<skill-name>/`（§4）
    packages_root: PathBuf,
    /// 旧扁平布局 `bundle/skills/`（内置释放技能仍住这里；迁移前市场技能残留
    /// 由读路径回退兼容，见 `find_skill_dir`）。
    legacy_skills_dir: PathBuf,
    /// bundles.json 写入口（Phase 2 起为安装态/来源/指纹的登记处；写失败不翻盘
    /// 主操作，fail loud 到日志）
    bundle_store: super::store::BundleStore,
}

impl Default for SkillMarketplaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillMarketplaceManager {
    pub fn new() -> Self {
        Self {
            packages_root: paths::bundles_root(),
            legacy_skills_dir: paths::bundle_skills_dir(),
            bundle_store: super::store::BundleStore::new(),
        }
    }

    /// 测试用：三套根都指到同一临时目录下，不碰真实 ~/.pinvou3。
    /// 认领（`skill_owner_package` → `bundle_installed`）经全局 env 读家目录，
    /// 须持 ENV_LOCK 与 env-mutating 测试串行——否则并行窗口内认领翻转，
    /// `package_skill_dir` 推导结果不稳定（测试间互相制造 flaky）。
    #[cfg(test)]
    fn with_roots(dir: PathBuf) -> LockedSkillManager {
        LockedSkillManager {
            _guard: crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            inner: Self {
                packages_root: dir.join("bundles"),
                legacy_skills_dir: dir.join("bundle/skills"),
                bundle_store: super::store::BundleStore::with_file(dir.join("bundles.json")),
            },
        }
    }

    /// 技能所属包目录：`bundles/<owner>/skills/<skill-name>`（属主推导复用
    /// `bundle::skill_owner_package`，与注册表同口径）。
    fn package_skill_dir(&self, skill_name: &str) -> PathBuf {
        self.packages_root
            .join(super::bundle::skill_owner_package(skill_name))
            .join("skills")
            .join(skill_name)
    }

    /// 已装技能目录定位：认领包目录 → 其余 `bundles/*/skills/<name>` 滞留副本
    /// （认领翻转/迁移失败留下的，see `package_candidate_dirs`）→ 旧扁平布局
    /// `bundle/skills/<name>` 回退读取——一次性迁移
    /// （`migrate_flat_skills_layout`）失败或目标已存在时保留旧位置，读路径在此
    /// 兜底，避免迁移失败/认领翻转后 is_installed / list / uninstall 与技能失联。
    pub(crate) fn find_skill_dir(&self, skill_name: &str) -> Option<PathBuf> {
        for cand in self.package_candidate_dirs(skill_name) {
            if cand.is_dir() {
                return Some(cand);
            }
        }
        let legacy_path = self.legacy_skills_dir.join(skill_name);
        if legacy_path.is_dir() {
            return Some(legacy_path);
        }
        None
    }

    /// 前端列表:预置技能(带 installed 状态) + 用户上传的技能（BundleStore 里
    /// `source=Upload` 的记录——`.installed-from` 标记已退役，登记即来源）。
    pub fn list_skills(&self) -> Vec<MarketplaceSkillInfo> {
        let presets = preset_manifests();
        let mut out: Vec<MarketplaceSkillInfo> = presets
            .iter()
            .map(|m| MarketplaceSkillInfo {
                id: m.id.to_string(),
                title: m.title.to_string(),
                subtitle: m.subtitle.to_string(),
                description: m.description.to_string(),
                icon: m.icon.to_string(),
                color: m.color.to_string(),
                installed: self.is_installed(m.skill_name),
                user_uploaded: false,
                update_available: self.preset_update_available(m),
                display_name: None,
                display_description: None,
            })
            .collect();

        // 上传技能：store 记录驱动（读失败回退为空列表 + warn，与 installed
        // 反转的回退纪律一致）
        match self.bundle_store.records() {
            Ok(records) => {
                for record in records {
                    if !matches!(record.source, super::store::BundleSource::Upload(_)) {
                        continue;
                    }
                    let Some(dir) = self.find_skill_dir(&record.id) else {
                        continue; // 登记在、目录不在（已卸载/迁移异常）→ 不列
                    };
                    if !dir.join("SKILL.md").is_file() {
                        continue;
                    }
                    // 用户自定义展示覆盖（bundles.json extra）优先；空/缺 key 回退
                    // 现状（title=记录 id、description=SKILL.md frontmatter）。
                    let display_name =
                        super::store::display_override(&record, super::store::EXTRA_DISPLAY_NAME);
                    let display_description = super::store::display_override(
                        &record,
                        super::store::EXTRA_DISPLAY_DESCRIPTION,
                    );
                    out.push(MarketplaceSkillInfo {
                        id: record.id.clone(),
                        title: display_name.clone().unwrap_or_else(|| record.id.clone()),
                        // 空 subtitle 让前端回退三语 localized 文案(上传技能无自有副标题)
                        subtitle: String::new(),
                        // 解析 SKILL.md frontmatter description 展示;缺失则空
                        description: display_description.clone().unwrap_or_else(|| {
                            read_skill_description(&dir.join("SKILL.md")).unwrap_or_default()
                        }),
                        icon: "Package".to_string(),
                        color: "bg-gradient-to-b from-slate-400 to-slate-600".to_string(),
                        installed: true,
                        user_uploaded: true,
                        // 上传技能无嵌入对应物,不参与更新检测
                        update_available: false,
                        display_name,
                        display_description,
                    });
                }
            }
            Err(e) => log::warn!("[skill-marketplace] BundleStore 读取失败，上传技能列表为空: {e}"),
        }
        out
    }

    fn is_installed(&self, skill_name: &str) -> bool {
        self.find_skill_dir(skill_name)
            .is_some_and(|d| d.join("SKILL.md").is_file())
    }

    /// 预置技能"可更新"检测（刀十起基于内容指纹）：
    /// - 记录指纹 ≠ 当前嵌入资源指纹 → 上游更新（App 升级带入新版）；
    /// - 磁盘指纹 ≠ 记录指纹 → 本地被改过（完整性视角，重装即复原）；
    /// - 无指纹记录（旧版安装/异常态）回退「磁盘 vs 嵌入」直比（与指纹同一豁免口径）。
    /// 未安装恒 false；磁盘遍历失败按 true 处理（提示可更新,重装即自愈）。
    fn preset_update_available(&self, m: &SkillManifest) -> bool {
        let Some(dir) = self.find_skill_dir(m.skill_name) else {
            return false;
        };
        if !dir.join("SKILL.md").is_file() {
            return false;
        }
        let Ok(disk_fp) = dir_fingerprint(&dir) else {
            return true;
        };
        let embedded_fp = embedded_skill_fingerprint(m);
        let record_fp = self
            .bundle_store
            .get(m.id)
            .ok()
            .flatten()
            .and_then(|r| r.content_fingerprint);
        match (record_fp, embedded_fp) {
            (Some(record_fp), Some(embedded_fp)) => {
                record_fp != embedded_fp || disk_fp != record_fp
            }
            (None, Some(embedded_fp)) => disk_fp != embedded_fp,
            _ => false,
        }
    }

    /// 已安装技能的市场 id（含预置与用户上传）。code scope 未初始化「默认全禁
    /// 已装技能」的兜底集合来源（见 `skill_materialization::load_disabled_skills_for`）。
    pub fn installed_skill_ids(&self) -> Vec<String> {
        self.list_skills()
            .into_iter()
            .filter(|s| s.installed)
            .map(|s| s.id)
            .collect()
    }

    fn preset(&self, id: &str) -> Option<&'static SkillManifest> {
        preset_manifests().iter().find(|m| m.id == id)
    }

    /// 市场 id → 落盘 skill 名(= SKILL.md frontmatter `name` = 底座 `Skill.name`)。
    /// 预置查清单(id 可与 skill_name 不同);上传技能的 id 即目录名,直通。底座按此名过滤。
    pub fn model_skill_names(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .map(|id| {
                self.preset(id)
                    .map(|m| m.skill_name.to_string())
                    .unwrap_or_else(|| id.clone())
            })
            .collect()
    }

    /// 安装预置技能:从嵌入资源复制到 `bundles/<owner>/skills/<name>/`
    /// （原子:.tmp → rename；owner 由 `bundle::skill_owner_package` 推导）。
    /// `.installed-from` 标记已退役——来源与内容指纹写入 BundleStore 记录。
    pub fn install(&self, skill_id: &str) -> Result<(), String> {
        let m = self
            .preset(skill_id)
            .ok_or_else(|| format!("未知预置技能 '{skill_id}'"))?;
        if RETIRED_SKILL_NAMES.contains(&m.skill_name) {
            return Err(format!("技能名 '{}' 与已下线内置冲突", m.skill_name));
        }
        let src = MARKETPLACE_DIR
            .get_dir(m.source_dir)
            .ok_or_else(|| format!("嵌入资源缺失: {}", m.source_dir))?;

        let dest = self.package_skill_dir(m.skill_name);
        // bundles/<owner>/skills/<name> is joined from the root, so it always
        // has a parent; still fall back to an error return.
        let Some(parent) = dest.parent() else {
            return Err(format!(
                "skill package dir has no parent: {}",
                dest.display()
            ));
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("创建包 skills 目录: {e}"))?;
        let staged = parent.join(format!("{}.tmp", m.skill_name));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(|e| format!("创建暂存目录: {e}"))?;

        let result = (|| -> Result<String, String> {
            extract_embedded_subdir(src, m.source_dir, &staged)
                .map_err(|e| format!("解包嵌入资源: {e}"))?;
            // 校验 SKILL.md 存在 + name 与预期一致
            let name =
                read_skill_name(&staged.join("SKILL.md")).ok_or("解包后 SKILL.md 缺 name 字段")?;
            if name != m.skill_name {
                return Err(format!(
                    "SKILL.md name '{name}' 与预期 '{}' 不符",
                    m.skill_name
                ));
            }
            dir_fingerprint(&staged).map_err(|e| format!("计算内容指纹: {e}"))
        })();
        let fingerprint = match result {
            Ok(fp) => fp,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staged);
                return Err(e);
            }
        };

        // 与 mcp_catalog::release_package 同一纪律：删旧目录失败即中止（吞错误
        // 会让"删一半"的残缺目录静默残留，rename 也必然失败）。
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| {
                let _ = std::fs::remove_dir_all(&staged);
                format!("清理旧技能目录失败（已中止，原目录可能部分删除）: {e}")
            })?;
        }
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;

        // 一个技能只留一份市场副本：清扫认领翻转滞留/双份安装的其余物理副本
        // （F2：独立安装后又装 companion MCP；F3：迁移 kept 留下的双份）。
        self.sweep_duplicate_skill_dirs(m.skill_name, &dest);

        // 登记（预置 source=Preset + 内容指纹；更新走同一 install 管线，
        // upsert_preserving 保留首次安装时间）。失败只记日志，目录落盘仍是权威。
        let mut record =
            super::store::BundleRecord::installed_now(m.id, super::store::BundleSource::Preset);
        record.content_fingerprint = Some(fingerprint);
        if let Err(e) = self.bundle_store.upsert_preserving(record) {
            log::warn!(
                "[skill-marketplace] bundles.json 镜像写入失败（install {}）: {e}",
                m.id
            );
        }
        Ok(())
    }

    /// `bundles/*/skills/<name>` 的全部候选（认领目录 + 其余包目录下的滞留
    /// 副本）。认领（`skill_owner_package`）随安装态时变，认领翻转/迁移失败/
    /// 双份安装都会留下与现算认领不一致的物理副本（F1/F2）；按包聚合布局本身
    /// 即市场属主契约，扫描只用于定位/删除市场副本，不碰用户目录。
    fn package_candidate_dirs(&self, skill_name: &str) -> Vec<PathBuf> {
        let mut out = vec![self.package_skill_dir(skill_name)];
        if let Ok(entries) = std::fs::read_dir(&self.packages_root) {
            for entry in entries.flatten() {
                let cand = entry.path().join("skills").join(skill_name);
                if cand.is_dir() && !out.contains(&cand) {
                    out.push(cand);
                }
            }
        }
        out
    }

    /// 落盘后清扫同名技能的其余物理副本，保证一个技能只有一份市场副本：
    /// 其余 `bundles/*/skills/<name>`（滞留/双份）与带市场标记的旧扁平目录
    /// 一律删除。Unmarked legacy flat dirs are not touched here — but note
    /// `bundle/skills/` is a builtin-managed area, not protected user storage:
    /// its unmarked residue is converged by self-heal step 4, and hand-placed
    /// user skills belong in `~/.pinvou3/user/skills/`.
    fn sweep_duplicate_skill_dirs(&self, skill_name: &str, keep: &Path) {
        for dir in self.package_candidate_dirs(skill_name) {
            if dir == *keep || !dir.is_dir() {
                continue;
            }
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                log::warn!(
                    "[skill-marketplace] 清扫重复技能副本失败（{}）: {e}",
                    dir.display()
                );
                continue;
            }
            if let Some(skills_parent) = dir.parent() {
                let _ = std::fs::remove_dir(skills_parent); // 仅空目录能删掉
                if let Some(pkg_dir) = skills_parent.parent() {
                    let _ = std::fs::remove_dir(pkg_dir);
                }
            }
        }
        let legacy = self.legacy_skills_dir.join(skill_name);
        if legacy != *keep && legacy.is_dir() && legacy.join(INSTALLED_FROM_MARKER).is_file() {
            if let Err(e) = std::fs::remove_dir_all(&legacy) {
                log::warn!(
                    "[skill-marketplace] 清扫旧布局技能副本失败（{}）: {e}",
                    legacy.display()
                );
            }
        }
    }

    /// 自愈对账（名称无关，按**归属证明**判定残旧；各发行版构建自动适配——
    /// 判定只依赖「本构建内嵌什么 / 登记在册什么 / CLI 门控静态所有什么」，
    /// 不使用任何硬编码技能名单）。启动路径在技能布局迁移之后、CLI gates 之前
    /// 调用。返回报告供启动标记观测。
    ///
    /// 1. **认领错位归位**：`bundles/<pkg>/skills/<name>` 的活认领（
    ///    `skill_owner_package`，随安装态时变）≠ pkg → 正确位置已有副本则去重
    ///    删除（按包聚合布局 = 市场属主契约），无则移动归位（保留用户安装；
    ///    rename 失败保留，下个启动周期重试）。
    /// 2. **孤儿副本清理**：`bundles/` 下技能目录无 BundleStore 记录、非预置
    ///    技能（可重释放）、非 CLI 门控静态所有 → 残旧删除。store 不可读 →
    ///    整段跳过（fail-closed：登记丢失时不得误删用户安装）。
    /// 3. **瘫记录清理**：Upload 记录但任何候选位置都找不到目录 → 内容不可
    ///    再生，记录即死配置，删除（Preset/Builtin 保留：可重释放/再装）。
    /// 4. **Builtin-release convergence**: unmarked, unrecorded dirs under
    ///    `bundle/skills/` that are not in this build's embedded builtin set
    ///    `builtin_released` are deleted as stale residue. Another edition
    ///    embedding a same-named skill (e.g. the group edition's eip) keeps it
    ///    via its own builtin set — the essential difference from a name-list
    ///    cleanup. Marked dirs are left to the layout migration.
    ///    `bundle/skills/` is a builtin-managed area, not user storage:
    ///    hand-placed user skills belong in `~/.pinvou3/user/skills/`.
    ///
    /// **Package-layout ownership guard** (applies to steps 1-3): a package dir
    /// carrying `plugin.json` (always landed by `plugin_import`) or an `mcp/`
    /// sibling owns its whole `skills/` subtree — plugin packages may contain
    /// multiple skills whose names differ from the single registered package
    /// id, and pure-MCP uploads have no skill dir at all. Name-based
    /// rehome/orphan/stale-record heuristics must never touch package-owned
    /// content, otherwise user uploads are destroyed within two launches.
    pub fn self_heal_skills(&self, builtin_released: &[&str]) -> SkillSelfHealReport {
        let mut report = SkillSelfHealReport::default();
        // Fail-closed when the store is unreadable OR missing: a missing
        // bundles.json yields an empty record set that is indistinguishable
        // from "everything is an orphan" — with user content on disk, steps
        // 2-4 would mass-delete it. (Startup runs import_legacy before
        // self-heal, so a missing file means the registry was lost, not that
        // nothing was ever installed.)
        let records = if self.bundle_store.file_path().is_file() {
            match self.bundle_store.records() {
                Ok(recs) => Some(recs),
                Err(_) => {
                    // fail-closed：登记读不出时只做不依赖登记的错位归位
                    log::warn!("[skill-marketplace] BundleStore 不可读，自愈仅执行认领错位归位");
                    None
                }
            }
        } else {
            log::warn!(
                "[skill-marketplace] bundles.json 缺失，自愈仅执行认领错位归位（fail-closed）"
            );
            None
        };

        // 1 + 2：扫描 bundles/<pkg>/skills/<name>。本轮归位落入的目标目录跳过
        // 当轮孤儿判定（read_dir 顺序不定，归位目标可能在本轮稍后被扫到；
        // 无记录的归位副本留给下轮对账，不在同轮边搬边删）。
        let mut rehomed_this_run: Vec<PathBuf> = Vec::new();
        if let Ok(pkgs) = std::fs::read_dir(&self.packages_root) {
            for pkg_entry in pkgs.flatten() {
                let pkg = pkg_entry.file_name().to_string_lossy().into_owned();
                let pkg_path = pkg_entry.path();
                // Package-layout ownership guard: plugin-import packages
                // (plugin.json) and MCP-bearing packages (mcp/ sibling) own
                // their skills/ subtree regardless of skill dir names —
                // multi-skill packages and plugin.json id != skill name layouts
                // register only one record (the package id), so the name-based
                // heuristics below would rehome/delete user content.
                if pkg_path.join("plugin.json").is_file() || pkg_path.join("mcp").is_dir() {
                    continue;
                }
                let skills_dir = pkg_path.join("skills");
                let Ok(rd) = std::fs::read_dir(&skills_dir) else {
                    continue;
                };
                for entry in rd.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !is_safe_skill_name(&name) {
                        continue;
                    }
                    let owner = super::bundle::skill_owner_package(&name);
                    if owner != pkg {
                        // 1. 错位归位/去重
                        let dest = self.packages_root.join(&owner).join("skills").join(&name);
                        if dest.is_dir() {
                            if std::fs::remove_dir_all(&dir).is_ok() {
                                report.deduped.push(format!("{pkg}/{name}"));
                            }
                        } else {
                            let staged_ok = dest
                                .parent()
                                .map(|p| std::fs::create_dir_all(p).is_ok())
                                .unwrap_or(false);
                            match staged_ok.then(|| std::fs::rename(&dir, &dest)) {
                                Some(Ok(())) => {
                                    rehomed_this_run.push(dest);
                                    report
                                        .rehomed
                                        .push(format!("{pkg}/{name} → {owner}/{name}"));
                                }
                                _ => log::warn!(
                                    "[skill-marketplace] 错位技能归位失败（{} → {}），下个启动周期重试",
                                    dir.display(),
                                    dest.display()
                                ),
                            }
                        }
                        continue;
                    }
                    // 2. 孤儿（无记录 + 非预置 + 非 CLI 静态所有 + 非本轮归位）
                    if let Some(recs) = &records {
                        if !self.has_install_record(recs, &name)
                            && self.preset_by_skill_name(&name).is_none()
                            && super::bundle::cli_bundle_of_skill(&name).is_none()
                            && !rehomed_this_run.iter().any(|d| d == &dir)
                            && std::fs::remove_dir_all(&dir).is_ok()
                        {
                            report.removed_orphan_dirs.push(format!("{pkg}/{name}"));
                        }
                    }
                }
                // 清理腾空的 skills/ 与包目录（仅空目录能删掉）
                let _ = std::fs::remove_dir(&skills_dir);
                let _ = std::fs::remove_dir(pkg_entry.path());
            }
        }

        if let Some(recs) = &records {
            // 3. 瘫记录（Upload 且无目录）
            for record in recs {
                if !matches!(record.source, super::store::BundleSource::Upload(_)) {
                    continue;
                }
                // Package-layout ownership guard: plugin_import registers one
                // record per package whose id is the package id — not
                // necessarily a skill dir name (multi-skill packages,
                // plugin.json id != skill names, pure-MCP uploads with no
                // skills/ at all). The existing package dir is the content
                // proof; only a record with neither skill dir nor package dir
                // is stale.
                if self.find_skill_dir(&record.id).is_none()
                    && !self.packages_root.join(&record.id).is_dir()
                    && self.bundle_store.remove(&record.id).is_ok()
                {
                    report.removed_stale_records.push(record.id.clone());
                }
            }

            // 4. 内置释放目录收敛
            if let Ok(rd) = std::fs::read_dir(&self.legacy_skills_dir) {
                for entry in rd.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !is_safe_skill_name(&name)
                        || builtin_released.contains(&name.as_str())
                        || dir.join(INSTALLED_FROM_MARKER).is_file()
                        || self.has_install_record(recs, &name)
                    {
                        continue;
                    }
                    if std::fs::remove_dir_all(&dir).is_ok() {
                        report.converged_builtin_dirs.push(name);
                    }
                }
            }
        }

        report
    }

    /// 技能是否有安装登记：记录 id = 目录名（上传/同名预置），或预置 id 与
    /// 技能名异名（如 tencent-docs-skill ↔ tencent-docs）时按预置 id 命中。
    fn has_install_record(&self, records: &[super::store::BundleRecord], skill_name: &str) -> bool {
        records.iter().any(|r| r.id == skill_name)
            || self
                .preset_by_skill_name(skill_name)
                .is_some_and(|m| records.iter().any(|r| r.id == m.id))
    }

    fn preset_by_skill_name(&self, skill_name: &str) -> Option<&'static SkillManifest> {
        preset_manifests()
            .iter()
            .find(|m| m.skill_name == skill_name)
    }

    /// 卸载:按**实际物理位置**删除（不按认领现算——认领随安装态时变，现算
    /// 会在认领翻转后算错目录、把滞留副本判成"非市场安装"，F1/F3）：
    /// - `bundles/*/skills/<name>` 全部候选副本（按包聚合布局 = 市场属主证明）；
    /// - legacy flat-layout residue is deleted only when it carries the
    ///   `.installed-from` marker. Unmarked `bundle/skills/` dirs are refused
    ///   here — that area is builtin-managed (unmarked residue there is
    ///   converged by self-heal step 4), and hand-placed user skills belong
    ///   in `~/.pinvou3/user/skills/`;
    /// - 目录已不存在（外部删除/安装中途失败）时仍删 BundleStore 记录——记录
    ///   即安装态登记，删记录不以目录存在为前提，否则卡成永远"已安装"的瘫记录。
    pub fn uninstall(&self, skill_id: &str) -> Result<(), String> {
        // 预置 id(pua/nuwa) → skill_name;上传技能 id 即目录名本身。
        let dir_name = self
            .preset(skill_id)
            .map(|m| m.skill_name.to_string())
            .unwrap_or_else(|| skill_id.to_string());
        if !is_safe_skill_name(&dir_name) {
            return Err(format!("非法技能名 '{dir_name}'"));
        }
        let legacy = self.legacy_skills_dir.join(&dir_name);
        let mut deleted_any = false;
        for dir in self.package_candidate_dirs(&dir_name) {
            if !dir.is_dir() {
                continue;
            }
            std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败: {e}"))?;
            deleted_any = true;
            // 清理腾空的父目录（skills/ 空 → 删；独立包的 bundles/<id>/ 空 → 删；
            // companion 的包目录还装着 MCP 资源，非空自然保留）
            if let Some(skills_parent) = dir.parent() {
                let _ = std::fs::remove_dir(skills_parent); // 仅空目录能删掉
                if let Some(pkg_dir) = skills_parent.parent() {
                    let _ = std::fs::remove_dir(pkg_dir);
                }
            }
        }
        if legacy.is_dir() && legacy.join(INSTALLED_FROM_MARKER).is_file() {
            std::fs::remove_dir_all(&legacy).map_err(|e| format!("删除失败: {e}"))?;
            deleted_any = true;
        }
        if !deleted_any && legacy.is_dir() {
            // 只剩无标记旧扁平目录：内置/手放保护对象，维持拒绝语义且不动记录。
            return Err(format!("技能 '{dir_name}' 非市场安装(不在包目录),拒绝删除"));
        }
        // 镜像删除（预置 id 即记录 id；上传技能 id = 目录名，与 install 的登记口径一致）。
        if let Err(e) = self.bundle_store.remove(skill_id) {
            log::warn!(
                "[skill-marketplace] failed to delete bundles.json mirror entry (uninstall {skill_id}): {e}"
            );
        }
        Ok(())
    }

    /// 导入用户上传的 zip 技能包:解压找 SKILL.md → 安全校验 → 落盘到
    /// `bundle/skills/<name>/`。穿越/symlink/大小防护对齐底座 install.rs。
    /// 返回落盘技能名(frontmatter name),供命令层同步 scope 禁用集。
    pub fn import_package(&self, zip_path: &str) -> Result<String, String> {
        let fname = Path::new(zip_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "package.zip".to_string());
        self.import_package_named(zip_path, &fname)
    }

    /// `display_name` 仅写入 `.installed-from=upload:<display_name>` 标记
    /// (保留用户原始 zip 名,便于卸载提示),其余行为与 `import_package` 一致。
    /// 拖放字节通道落临时文件导入时,zip 名已丢,由命令层传入净化后的展示名。
    pub fn import_package_named(
        &self,
        zip_path: &str,
        display_name: &str,
    ) -> Result<String, String> {
        let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 zip: {e}"))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip: {e}"))?;

        // pass1:逐 entry 安全校验 + 累计头部声明大小（真实解压字节由 pass2 兜底
        // 计量，声明可被伪造）+ 找最优 SKILL.md(定 skill_root)。
        let mut best: Option<(usize, String)> = None; // (rank, skill_root)
        let mut total: u64 = 0;
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| format!("zip 条目 #{i}: {e}"))?;
            // 路径穿越:enclosed_name 为 None 即不安全(.. / 绝对路径)。
            let Some(enclosed) = entry.enclosed_name() else {
                return Err("zip 含不安全路径(穿越),拒绝".to_string());
            };
            // symlink/hardlink 拒绝
            if let Some(mode) = entry.unix_mode() {
                if mode & 0o170000 == 0o120000 {
                    return Err("zip 含 symlink,拒绝".to_string());
                }
            }
            total = total.saturating_add(entry.size());
            if total > MAX_SKILL_SIZE_BYTES {
                return Err(format!(
                    "技能包解压超过 {} MiB 上限",
                    MAX_SKILL_SIZE_BYTES / 1024 / 1024
                ));
            }
            if entry.is_dir() {
                continue;
            }
            let path_str = enclosed.to_string_lossy().replace('\\', "/");
            if let Some(rank) = skill_md_rank(&path_str) {
                let root = skill_root_of(&path_str);
                if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                    best = Some((rank, root));
                }
            }
        }
        let (_, skill_root) = best.ok_or("zip 里没找到 SKILL.md")?;

        // 读 SKILL.md 拿 frontmatter name
        let md_rel = if skill_root.is_empty() {
            "SKILL.md".to_string()
        } else {
            format!("{skill_root}/SKILL.md")
        };
        let name = {
            let mut md = archive
                .by_name(&md_rel)
                .map_err(|e| format!("读 SKILL.md: {e}"))?;
            // 有界预读（六轮评审 R2）：伪造 size=0 头部的 SKILL.md 在不受限
            // read_to_string 下会整流读入内存；take(声明+1) 超即拒，与 pass2 /
            // plugin_import 同一收口。
            let declared = md.size();
            let buf = super::plugin_import::read_zip_entry_bounded(&mut md, declared, "SKILL.md")?;
            let text = String::from_utf8(buf).map_err(|e| format!("读 SKILL.md: {e}"))?;
            read_skill_name_from_str(&text).ok_or("SKILL.md 缺 name 字段")?
        };
        if !is_safe_skill_name(&name) {
            return Err(format!("非法技能名 '{name}'"));
        }
        if RETIRED_SKILL_NAMES.contains(&name.as_str()) {
            return Err(format!("技能名 '{name}' 与已下线内置冲突,拒绝"));
        }
        // Preset/companion name collisions are rejected up front: the market
        // lifecycle (post-install sweep, claim-driven rehome in self-heal)
        // owns these names, so an uploaded copy would be shadowed or swept
        // away (review: collision must not escalate from shadowed to swept).
        if is_preset_skill_name(&name) {
            return Err(format!("技能名 '{name}' 与市场预置技能冲突，请改名后重试"));
        }
        let owner = super::bundle::skill_owner_package(&name);
        if owner != name {
            return Err(format!(
                "技能名 '{name}' 已被包 '{owner}' 的配套技能占用，请改名后重试"
            ));
        }
        // A copy under another package dir would be swept by the post-install
        // dedupe below — destroying that package's component. Reject instead.
        if let Some(other) = foreign_skill_copies_under(&self.packages_root, &name, &name)
            .into_iter()
            .next()
        {
            return Err(format!(
                "技能 '{name}' 已存在于包 '{other}'，请先卸载该包或改名后重试"
            ));
        }

        // pass2:写出 skill_root 子树到 staged（上传技能独立成包：bundles/<name>/skills/）
        let dest = self.packages_root.join(&name).join("skills").join(&name);
        // Same as above: joined from the root, so a parent always exists;
        // still fall back to an error return.
        let Some(parent) = dest.parent() else {
            return Err(format!(
                "skill package dir has no parent: {}",
                dest.display()
            ));
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("创建包 skills 目录: {e}"))?;
        let staged = parent.join(format!("{name}.tmp"));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(|e| format!("暂存目录: {e}"))?;
        let prefix = if skill_root.is_empty() {
            String::new()
        } else {
            format!("{skill_root}/")
        };

        let result = (|| -> Result<String, String> {
            // 实际写盘字节计量：pass1 只累计 zip 头部声明 size，可被伪造（声明很小、
            // 真实解压很大的 zip bomb）。读取经 read_zip_entry_bounded 有界收口
            // （take(声明+1)，伪造 size=0 头部的单条 zip bomb 先被拒，不会完整读入
            // 内存，与新管线 plugin_import 对齐，五轮评审 M-5），再按真实字节累计
            // 兜底（四轮评审 M-4）。超限由外层清 staged 拒收。
            let mut actual_total: u64 = 0;
            for i in 0..archive.len() {
                let mut entry = archive
                    .by_index(i)
                    .map_err(|e| format!("zip 条目 #{i}: {e}"))?;
                if entry.is_dir() {
                    continue;
                }
                let Some(enclosed) = entry.enclosed_name() else {
                    continue;
                };
                let path_str = enclosed.to_string_lossy().replace('\\', "/");
                // 只取 skill_root 子树
                let rel = if prefix.is_empty() {
                    path_str.clone()
                } else {
                    match path_str.strip_prefix(&prefix) {
                        Some(r) => r.to_string(),
                        None => continue,
                    }
                };
                if rel.is_empty() {
                    continue;
                }
                // 跳过隐藏/版本控制目录(.git/.github 等)
                if rel.split('/').any(|c| c.starts_with('.')) {
                    continue;
                }
                let target = staged.join(&rel);
                if !target.starts_with(&staged) {
                    return Err("路径穿越,拒绝".to_string());
                }
                if let Some(p) = target.parent() {
                    std::fs::create_dir_all(p).map_err(|e| format!("建目录: {e}"))?;
                }
                let declared = entry.size();
                let buf =
                    super::plugin_import::read_zip_entry_bounded(&mut entry, declared, "条目")?;
                actual_total = actual_total.saturating_add(buf.len() as u64);
                if actual_total > MAX_SKILL_SIZE_BYTES {
                    return Err(format!(
                        "技能包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                        MAX_SKILL_SIZE_BYTES / 1024 / 1024
                    ));
                }
                std::fs::write(&target, buf).map_err(|e| format!("写文件: {e}"))?;
            }
            dir_fingerprint(&staged).map_err(|e| format!("计算内容指纹: {e}"))
        })();
        let fingerprint = match result {
            Ok(fp) => fp,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staged);
                return Err(e);
            }
        };

        // 与 preset install 同一纪律：删旧目录失败即中止，不留"删一半"残缺目录。
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| {
                let _ = std::fs::remove_dir_all(&staged);
                format!("清理旧技能目录失败（已中止，原目录可能部分删除）: {e}")
            })?;
        }
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;
        // 一个技能只留一份市场副本：清扫认领翻转滞留/双份安装的其余物理副本。
        self.sweep_duplicate_skill_dirs(&name, &dest);
        // 登记（上传 source=Upload(zip 展示名) + 内容指纹，记录 id = 落盘技能名，
        // 与 import_legacy 的反推口径一致；`.installed-from` 标记已退役）。失败只记日志。
        let mut record = super::store::BundleRecord::installed_now(
            name.clone(),
            super::store::BundleSource::Upload(display_name.to_string()),
        );
        record.content_fingerprint = Some(fingerprint);
        if let Err(e) = self.bundle_store.upsert_preserving(record) {
            log::warn!("[skill-marketplace] bundles.json 镜像写入失败（import {name}）: {e}");
        }
        Ok(name)
    }

    /// 上传包展示名/说明覆盖的统一编排（`update_bundle_display_meta` 命令的
    /// 特性层入口，存在/来源门禁与长度/字符校验都在这里，命令层只做搬运）。
    ///
    /// 顺序契约（「报错但包内容已变」中间态的最小化）：
    /// 1. 门禁（存在 + Upload）与校验（[`validate_display_meta`]）先于一切落盘；
    /// 2. 展示说明非空 → 单技能包回写 SKILL.md（含互洽校验、原值备份、指纹
    ///    重算）；空（清覆盖）→ 单技能包从备份恢复原值。非单技能包两向都跳过；
    /// 3. 最后 `set_display_meta` 写 extra（与门禁同口径兜底）。
    /// 步骤 2 与 3 之间仍存在窄窗口（并发卸载时 SKILL.md 已改而命令 Err），
    /// 展示回退链读 SKILL.md 所以状态自洽；可接受，注释如实记录。
    pub fn update_display_meta(
        &self,
        bundle_id: &str,
        display_name: Option<&str>,
        display_description: Option<&str>,
    ) -> Result<(), String> {
        use super::store::BundleSource;
        // 门禁必须在回写之前：先挡住预置/内置包与未登记 id，避免改写非上传包
        // 的技能内容；set_display_meta 内同口径兜底（防 TOCTOU）。
        let record = self
            .bundle_store
            .get(bundle_id)?
            .ok_or_else(|| format!("包 '{bundle_id}' 未登记，无法设置展示名/说明"))?;
        if !matches!(record.source, BundleSource::Upload(_)) {
            return Err(format!(
                "包 '{bundle_id}' 非用户上传来源，预置/内置包不允许覆盖展示名/说明"
            ));
        }
        super::store::validate_display_meta(display_name, display_description)?;
        if let Some(desc) = display_description.map(str::trim) {
            if desc.is_empty() {
                self.sync_display_description(bundle_id, SyncDesc::Restore)?;
            } else {
                self.sync_display_description(bundle_id, SyncDesc::Set(desc))?;
            }
        }
        self.bundle_store
            .set_display_meta(bundle_id, display_name, display_description)
    }

    /// 单技能上传包的 SKILL.md description 双向同步（设覆盖时回写新值、清覆盖时
    /// 恢复原值；模型侧看到的技能描述与展示侧一致，展示层仍由 extra 覆盖优先）。
    ///
    /// 仅当 `bundles/<id>/skills/` 下恰有一个技能目录且内含 SKILL.md 时动文件；
    /// 多技能包、纯 MCP 包、目录缺失一律跳过（返回 Ok(false)，不报错）。改文件后
    /// 重算**整包目录**内容指纹并经 upsert_preserving 补写登记（保留
    /// extra/来源/首装时间——display_* 覆盖与说明备份都在 extra，一并保留）。
    fn sync_display_description(&self, bundle_id: &str, dir: SyncDesc) -> Result<bool, String> {
        let skills_dir = self.packages_root.join(bundle_id).join("skills");
        let Ok(rd) = std::fs::read_dir(&skills_dir) else {
            return Ok(false); // 非按包布局（纯 MCP 包/旧扁平残留）→ 跳过
        };
        let dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        if dirs.len() != 1 {
            return Ok(false); // 多技能包/空包 → 跳过
        }
        let md_path = dirs[0].join("SKILL.md");
        if !md_path.is_file() {
            return Ok(false);
        }
        match dir {
            SyncDesc::Set(desc) => {
                let content = std::fs::read_to_string(&md_path)
                    .map_err(|e| format!("读取 {} 失败: {e}", md_path.display()))?;
                // 首次回写前备份引擎口径原值（含「原本没有」空串哨兵）——
                // 清覆盖时据此恢复，frontmatter 默认值不因覆盖过一次而丢失。
                // 互斥（同锁 RMW）+ 时机（首写前）保证不会备份到自己的回写值。
                if self.bundle_store.skill_desc_backup(bundle_id)?.is_none() {
                    let original = read_skill_description_from_str(&content);
                    // 空串哨兵 = 原本没有 description（Some("")），不是 Option None
                    self.bundle_store.set_skill_desc_backup(
                        bundle_id,
                        Some(original.as_deref().unwrap_or("")),
                    )?;
                }
                let new_content = rewrite_frontmatter_description(&content, desc)?;
                self.write_skill_md_and_fingerprint(bundle_id, &md_path, content, new_content)?;
                Ok(true)
            }
            SyncDesc::Restore => {
                // 无备份（从未回写过，或非单技能包形态）→ 不动文件。
                let Some(backup) = self.bundle_store.skill_desc_backup(bundle_id)? else {
                    return Ok(false);
                };
                let content = std::fs::read_to_string(&md_path)
                    .map_err(|e| format!("读取 {} 失败: {e}", md_path.display()))?;
                // 当前 frontmatter 值已是原值（用户手动改回/已恢复）→ 不重写，
                // 只清备份 key。空备份哨兵 = 原本没有 description → 删行恢复。
                let current = read_skill_description_from_str(&content);
                if current.as_deref() != Some(backup.as_str()) {
                    let new_content = if backup.is_empty() {
                        remove_frontmatter_description(&content)?
                    } else {
                        rewrite_frontmatter_description(&content, &backup)?
                    };
                    self.write_skill_md_and_fingerprint(bundle_id, &md_path, content, new_content)?;
                }
                // 恢复完成（或本就是原值）→ 清备份 key，回到「从未回写过」状态。
                self.bundle_store.set_skill_desc_backup(bundle_id, None)?;
                Ok(true)
            }
        }
    }

    /// SKILL.md 变更落盘 + 整包指纹重算补写登记（回写/恢复共用）。
    /// 内容没变（no-op 保存）则不动指纹，避免无谓 churn。
    fn write_skill_md_and_fingerprint(
        &self,
        bundle_id: &str,
        md_path: &Path,
        old_content: String,
        new_content: String,
    ) -> Result<(), String> {
        if new_content == old_content {
            return Ok(());
        }
        deepseek_tui::utils::write_atomic(md_path, new_content.as_bytes())
            .map_err(|e| format!("写入 {} 失败: {e}", md_path.display()))?;
        self.refresh_package_fingerprint(bundle_id)
    }

    /// 重算 `bundles/<id>/` 整包目录内容指纹并经 upsert_preserving 补写登记
    /// （**整包口径**，与 plugin_import 的登记基线一致——算技能子目录会把上传
    /// 包的整包指纹静默缩窄，未来任何按整包口径重算比对的完整性校验都会对
    /// 编辑过的包永久误报）；登记已不在（并发卸载）则跳过。
    fn refresh_package_fingerprint(&self, bundle_id: &str) -> Result<(), String> {
        let fingerprint = dir_fingerprint(&self.packages_root.join(bundle_id))?;
        if let Some(mut record) = self.bundle_store.get(bundle_id)? {
            record.content_fingerprint = Some(fingerprint);
            self.bundle_store
                .upsert_preserving(record)
                .map_err(|e| format!("更新 {bundle_id} 内容指纹失败: {e}"))?;
        }
        Ok(())
    }

    /// 扁平技能布局（`bundle/skills/<name>/`）→ 按包聚合（`bundles/<pkg>/skills/
    /// <name>/`）的一次性迁移（§9.1），由启动路径（runtime_bundle ensure_extracted，
    /// import_legacy 之后）调用。幂等：旧位置不在即 no-op。
    ///
    /// - 市场技能（带 `.installed-from` 标记）→ 移动到所属包目录；预置技能迁移后
    ///   把目录指纹补写进既有 BundleStore 记录（update_available 的比对基准）；
    /// - 企微 0.1.9 退役目录（msg/schedule，无标记）→ 删除而非搬移：搬进
    ///   `bundles/wecom/skills/` 后门控的 legacy 清理（只扫旧扁平目录）够不到，
    ///   退役技能会永久残留并被物化进会话（五轮评审必修 3）；
    /// - CLI companion（无标记，内置清单目录名）：连接器当前可见才移动；不可见 =
    ///   断开后的残留，按门控语义删除（immutable 资源，重连重解包，无用户数据）；
    /// - other unmarked dirs (builtin-released skills like visual-design) →
    ///   untouched here; stale unmarked residue in `bundle/skills/` is
    ///   converged by self-heal step 4. `bundle/skills/` is not user storage —
    ///   hand-placed user skills belong in `~/.pinvou3/user/skills/`;
    /// - 目标已存在或 rename 失败 → 保留旧位置并 warn（读路径 `find_skill_dir`
    ///   对旧位置有回退，迁移下个启动周期自愈）。
    pub fn migrate_flat_skills_layout(&self) -> SkillsMigrationReport {
        let mut report = SkillsMigrationReport::default();
        let Ok(rd) = std::fs::read_dir(&self.legacy_skills_dir) else {
            return report;
        };
        // 迁移期 companion 归属兜底：extraction 顺序为 import_legacy →
        // migrate_custom_mcp_layout → 技能迁移（M-7），正常路径下自定义 MCP 已搬到
        // 新布局，`available_tools` 能读到其 manifest 的 companion_skills 声明，
        // `skill_owner_package` 直接条件认领；仅当自定义 MCP 迁移被门控跳过（明文
        // 密钥迁移失败）时旧布局仍在，companion 声明需从旧目录现算。映射本身是
        // 条件认领（MCP 已装才归 MCP，未装则技能独立成包），与查询层
        // `skill_owner_package` 同口径（M-7）。
        let legacy_companions = legacy_companion_owners();
        for entry in rd.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_safe_skill_name(&name) {
                continue;
            }
            let target = self.migration_skill_dir(&name, &legacy_companions);
            let marker = std::fs::read_to_string(dir.join(INSTALLED_FROM_MARKER))
                .unwrap_or_default()
                .trim()
                .to_string();
            if marker.is_empty() {
                // 企微 0.1.9 退役目录（msg/schedule）：不搬移、直接删除——它们已
                // 不在内置清单（cli_bundle_of_skill 反查不命中），且无论连接器
                // 可见与否都已退役；门控的 legacy 清理只扫旧扁平目录，搬进新布局
                // 会永久残留（五轮评审必修 3）。
                if crate::platform::connector_skills::WECOM_LEGACY_SKILL_DIRS
                    .contains(&name.as_str())
                {
                    let _ = std::fs::remove_dir_all(&dir);
                    report.removed_stale.push(name);
                    continue;
                }
                if let Some(cli) = super::bundle::cli_bundle_of_skill(&name) {
                    if crate::platform::connector_state::skills_visible_for(cli) {
                        self.move_skill_dir(&dir, &name, &target, &mut report);
                    } else {
                        let _ = std::fs::remove_dir_all(&dir);
                        report.removed_stale.push(name);
                    }
                } else {
                    report.kept.push(name);
                }
                continue;
            }
            let is_preset = marker.starts_with("pinvou3-marketplace:");
            if self.move_skill_dir(&dir, &name, &target, &mut report) && is_preset {
                // 补写指纹到既有记录（import_legacy 先跑，记录应已存在；
                // 不存在则不擅自新建——异常态留给下一周期）
                if let Ok(fp) = dir_fingerprint(&target) {
                    match self.bundle_store.get(&name) {
                        Ok(Some(mut record)) => {
                            record.content_fingerprint = Some(fp);
                            if let Err(e) = self.bundle_store.upsert_preserving(record) {
                                log::warn!("[skill-marketplace] 迁移补写指纹失败（{name}）: {e}");
                            }
                        }
                        Ok(None) => {
                            log::warn!("[skill-marketplace] 迁移补写指纹跳过：{name} 无 store 记录")
                        }
                        Err(e) => {
                            log::warn!("[skill-marketplace] 迁移补写指纹读取失败（{name}）: {e}")
                        }
                    }
                }
            }
        }
        report
    }

    /// 迁移目标目录：`bundles/<owner>/skills/<skill-name>`。属主推导复用
    /// `bundle::skill_owner_package`（条件认领）；推导结果为「独立成包」（owner =
    /// 技能名自身）时回退查迁移期的旧布局 companion 映射（同样条件口径：所属
    /// MCP 已装才归 MCP；仅自定义 MCP 迁移被门控跳过时才用得到，见
    /// `migrate_flat_skills_layout` 注释）。
    fn migration_skill_dir(
        &self,
        skill_name: &str,
        legacy_companions: &std::collections::HashMap<String, String>,
    ) -> PathBuf {
        let mut owner = super::bundle::skill_owner_package(skill_name);
        if owner == skill_name {
            if let Some(legacy_owner) = legacy_companions.get(skill_name) {
                owner = legacy_owner.clone();
            }
        }
        self.packages_root
            .join(owner)
            .join("skills")
            .join(skill_name)
    }

    /// 移动单个技能目录到所属包目录。返回是否完成移动。
    fn move_skill_dir(
        &self,
        dir: &Path,
        name: &str,
        target: &Path,
        report: &mut SkillsMigrationReport,
    ) -> bool {
        if target.exists() {
            log::warn!(
                "[skill-marketplace] 迁移跳过 {name}：目标已存在（{}），保留旧位置 {}",
                target.display(),
                dir.display()
            );
            report.kept.push(name.to_string());
            return false;
        }
        // The migration target is joined from the root, so it always has a
        // parent; treat the anomalous shape as "migration failed, keep the
        // old location".
        let Some(parent) = target.parent() else {
            log::warn!(
                "[skill-marketplace] failed to migrate {name}: target dir has no parent ({}), keeping the old location",
                target.display()
            );
            report.kept.push(name.to_string());
            return false;
        };
        let result = std::fs::create_dir_all(parent).and_then(|()| std::fs::rename(dir, target));
        match result {
            Ok(()) => {
                report.moved.push(name.to_string());
                true
            }
            Err(e) => {
                log::warn!(
                    "[skill-marketplace] 迁移 {name} 失败（{} → {}）: {e}，保留旧位置",
                    dir.display(),
                    target.display()
                );
                report.kept.push(name.to_string());
                false
            }
        }
    }
}

/// 技能布局迁移报告（启动标记/日志观测用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillsMigrationReport {
    /// 成功移动到包目录的技能名
    pub moved: Vec<String>,
    /// 连接器不可见而按门控语义删除的 CLI companion 残留
    pub removed_stale: Vec<String>,
    /// Untouched: builtin-released skills / migration-failed dirs kept at the
    /// old location. (`bundle/skills/` unmarked residue is converged by
    /// self-heal step 4; hand-placed user skills belong in
    /// `~/.pinvou3/user/skills/`.)
    pub kept: Vec<String>,
}

/// 自愈对账报告（启动标记/日志观测用），见
/// [`SkillMarketplaceManager::self_heal_skills`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillSelfHealReport {
    /// 认领错位后移动归位的 `<旧pkg>/<name> → <认领pkg>/<name>`
    pub rehomed: Vec<String>,
    /// 认领错位且正确位置已有副本，去重删除的 `<pkg>/<name>`
    pub deduped: Vec<String>,
    /// 无记录、非预置、非 CLI 静态所有的孤儿副本 `<pkg>/<name>`
    pub removed_orphan_dirs: Vec<String>,
    /// 任何位置都找不到目录的 Upload 瘫记录 id
    pub removed_stale_records: Vec<String>,
    /// 内置释放目录收敛删除的残旧目录名
    pub converged_builtin_dirs: Vec<String>,
}

// 辅助 ------------------------------------------------------------------------

/// 迁移期 companion 归属映射：扫描旧布局 `bundle/mcp-servers/<id>/manifest.json`
/// 的 `companion_skills` 声明，产出 技能名 → 所属 MCP 包 id。仅供
/// `migrate_flat_skills_layout` 使用——正常路径下自定义 MCP 已先于技能迁移搬到
/// 新布局（extraction：import_legacy → migrate_custom_mcp_layout → 技能迁移），
/// `available_tools` 可直接读到 companion 声明；本函数只兜底自定义 MCP 迁移被
/// 门控跳过（明文密钥迁移失败，旧布局仍在）的异常路径。manifest 缺失/损坏的目录
/// 跳过（其技能按独立包迁移，下个启动周期自愈）。
///
/// **条件认领**（四轮评审 M-7，与查询层 `skill_owner_package` 的 V5 口径一致）：
/// 所属 MCP 包当前已装才归 MCP；未装时技能按独立包迁移（owner = 技能名自身）。
/// 否则 companion 单装（MCP 未装）会被迁进未装包的目录，UI 恒显未装、卸载失联。
/// 安装态读 BundleStore——启动序列里 import_legacy 先于技能迁移跑完，安装态可读。
fn legacy_companion_owners() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(rd) = std::fs::read_dir(paths::bundle_mcp_servers_dir()) else {
        return map;
    };
    for entry in rd.flatten() {
        let manifest_path = entry.path().join("manifest.json");
        let Ok(content) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<super::types::ToolManifest>(&content) else {
            continue;
        };
        if !super::bundle::bundle_installed(&manifest.id) {
            continue;
        }
        for skill in &manifest.companion_skills {
            map.insert(skill.clone(), manifest.id.clone());
        }
    }
    map
}

/// `with_roots` 的返回包装：持有 ENV_LOCK 的 manager，经 Deref 透明调用方法，
/// guard 随测试绑定结束自动释放。
#[cfg(test)]
struct LockedSkillManager {
    _guard: std::sync::MutexGuard<'static, ()>,
    inner: SkillMarketplaceManager,
}

#[cfg(test)]
impl std::ops::Deref for LockedSkillManager {
    type Target = SkillMarketplaceManager;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// 收集嵌入资源子树的 `(相对路径, 内容)` 列表,供更新检测比对。
/// 口径与 [`extract_embedded_subdir`] 一致:strip `source_dir` 前缀、跳过 SOURCE.md、
/// 跳过 `__pycache__`/`*.pyc`——否则构建期混入的 pyc 会让内嵌指纹永远 ≠ 落盘
/// 指纹，`update_available` 幽灵常亮（G6）。
fn collect_embedded_files(dir: &Dir<'_>, source_dir: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let prefix = format!("{source_dir}/");
    for file in dir.files() {
        let p = file.path().to_string_lossy();
        let rel = p.strip_prefix(&prefix).unwrap_or(&p);
        if Path::new(rel).file_name().and_then(|s| s.to_str()) == Some("SOURCE.md") {
            continue;
        }
        if is_python_cache_path(Path::new(rel)) {
            continue;
        }
        out.push((rel.to_string(), file.contents().to_vec()));
    }
    for sub in dir.dirs() {
        collect_embedded_files(sub, source_dir, out);
    }
}

/// 递归收集磁盘技能目录的 `(相对路径, 内容)` 列表,供更新检测比对。
/// 跳过 `.installed-from` 安装标记(非技能内容);相对路径统一用 `/` 分隔,
/// 与嵌入侧口径一致。遍历失败返回 Err(调用方按"可更新"处理,重装自愈)。
fn collect_disk_files(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
    collect_disk_files_under(dir, dir, out)
}

fn collect_disk_files_under(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_disk_files_under(root, &path, out)?;
            continue;
        }
        if entry.file_name().to_string_lossy() == INSTALLED_FROM_MARKER {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, std::fs::read(&path)?));
    }
    Ok(())
}

/// 递归写出 `include_dir` 子目录到 `dest`,strip 掉 `source_dir` 前缀
/// (`file.path()` 是相对最外层 include_dir 根的完整路径,如 "pua/SKILL.md")。
/// 跳过 vendored 来源标注文件 SOURCE.md(非 skill 运行内容)。
/// 同样排除 `__pycache__/` 与 `*.pyc`:include_dir! 按文件系统内嵌(不受
/// .gitignore 约束),在仓库里直接运行技能脚本产生的 Python 编译缓存若不
/// 排除,会在用户安装预置技能时物化到运行时目录(跨平台 cpython 版本耦合)。
/// 与 runtime_bundle::platform 的 extract_dir 排除规则保持一致。
fn extract_embedded_subdir(dir: &Dir<'_>, source_dir: &str, dest: &Path) -> std::io::Result<()> {
    let prefix = format!("{source_dir}/");
    for file in dir.files() {
        let p = file.path();
        if is_python_cache_path(p) {
            continue;
        }
        let p = p.to_string_lossy();
        let rel = p.strip_prefix(&prefix).unwrap_or(&p);
        if Path::new(rel).file_name().and_then(|s| s.to_str()) == Some("SOURCE.md") {
            continue;
        }
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, file.contents())?;
    }
    for sub in dir.dirs() {
        if sub
            .path()
            .components()
            .any(|c| c.as_os_str() == "__pycache__")
        {
            continue;
        }
        extract_embedded_subdir(sub, source_dir, dest)?;
    }
    Ok(())
}

/// 目录树内容指纹：SHA-256（排序后的 相对路径+字节 流），路径统一 '/' 分隔。
/// 豁免口径与既有比对一致：磁盘侧跳过 `.installed-from`（旧布局标记），
/// 嵌入侧跳过 SOURCE.md（见 collect_* 两个收集器）。
/// `pub(crate)`：MCP 包目录指纹（mcp_catalog 释放/校验）复用同一实现。
pub(crate) fn dir_fingerprint(dir: &Path) -> Result<String, String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_disk_files(dir, &mut files).map_err(|e| format!("遍历 {} 失败: {e}", dir.display()))?;
    Ok(fingerprint_of(&mut files))
}

/// 嵌入资源（预置技能当前版本）的内容指纹；嵌入目录缺失 → None。
fn embedded_skill_fingerprint(m: &SkillManifest) -> Option<String> {
    let dir = MARKETPLACE_DIR.get_dir(m.source_dir)?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_embedded_files(dir, m.source_dir, &mut files);
    Some(fingerprint_of(&mut files))
}

fn fingerprint_of(files: &mut [(String, Vec<u8>)]) -> String {
    files.sort();
    let mut digest = Sha256::new();
    for (rel, bytes) in files.iter() {
        digest.update(rel.as_bytes());
        digest.update(b"\0");
        digest.update(bytes);
        digest.update(b"\0");
    }
    crate::platform::encoding::hex_lower(&digest.finalize())
}

/// 相对 include_dir 根的路径是否属于 Python 编译缓存(`__pycache__/` 子树内
/// 或任意层级的 `.pyc`,大小写不敏感)。纯函数便于单测。
fn is_python_cache_path(rel: &std::path::Path) -> bool {
    rel.components().any(|c| c.as_os_str() == "__pycache__")
        || rel
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pyc"))
}

fn read_skill_name(md_path: &Path) -> Option<String> {
    read_skill_name_from_str(&std::fs::read_to_string(md_path).ok()?)
}

/// 解析 SKILL.md frontmatter 的 `name:`(前两个 `---` 之间的第一个顶层 name 行)。
/// `pub(crate)`：统一插件导入（plugin_import）的裸技能回退复用同一解析口径。
pub(crate) fn read_skill_name_from_str(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(rest) = t.strip_prefix("name:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn read_skill_description(md_path: &Path) -> Option<String> {
    let raw = read_skill_description_from_str(&std::fs::read_to_string(md_path).ok()?)?;
    // 展示口径截断（与 MAX_DISPLAY_DESCRIPTION_CHARS 对齐）；备份/互洽校验用
    // `read_skill_description_from_str` 的原值，不截断。
    Some(raw.chars().take(240).collect())
}

/// 解析 SKILL.md frontmatter 的 `description:` **原值**（不截断；展示读取与
/// SKILL.md 回写互洽校验/原值备份共用）。语义**镜像 CodeWhale 平铺 frontmatter
/// 解析器**（`CodeWhale/crates/tui/src/skills/mod.rs` 的 `parse_skill`）：
///
/// - 任意缩进的 `description:` 行都算（平摊解析不区分嵌套），重复键 last-wins，
///   后出现的空值覆盖先出现的非空值；
/// - 支持六种块状标记（`|` `>` 及 `|-` `|+` `>-` `>+` chomping）：续行 = 缩进
///   大于键行基准缩进的行与空行，按首个非空行缩进剥一层；literal 按 `\n` 连接，
///   folded 非空行以空格折叠、空行成段间换行；
/// - 单行值剥**成对**单/双引号；`#` 开头的整行注释跳过（行内注释不算）；
/// - 首 `---` 前容忍空白；BOM / 无 frontmatter / 缺结束 `---` → 引擎走
///   `# Heading` 降级路径（description 为空）→ None。
///
/// 为何镜像而不复用：引擎解析器是 `pub(crate)`，应用侧不可调用。两端口径一致
/// 由本镜像 + `skill_description_mirrors_engine_flat_parser` 测试钉住；改这里
/// 前先对照引擎实现。
pub(crate) fn read_skill_description_from_str(content: &str) -> Option<String> {
    if !content.trim_start().starts_with("---") {
        return None;
    }
    let start = content.find("---")?;
    let rest = &content[start + 3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut description: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            i += 1;
            continue;
        };
        if key.trim().to_ascii_lowercase() != "description" {
            i += 1;
            continue;
        }
        let value = value.trim();
        if matches!(value, ">" | "|" | ">-" | ">+" | "|-" | "|+") {
            let is_folded = value.starts_with('>');
            let chomp_strip = value.ends_with('-');
            let chomp_keep = value.ends_with('+');
            let base_indent = raw.len() - raw.trim_start().len();
            let mut block: Vec<&str> = Vec::new();
            let mut content_indent: Option<usize> = None;
            i += 1;
            while i < lines.len() {
                let raw_line = lines[i];
                if raw_line.trim().is_empty() {
                    block.push("");
                    i += 1;
                    continue;
                }
                let indent = raw_line.len() - raw_line.trim_start().len();
                if indent > base_indent {
                    if content_indent.is_none() {
                        content_indent = Some(indent);
                    }
                    block.push(raw_line);
                    i += 1;
                } else {
                    break;
                }
            }
            let content_indent = content_indent.unwrap_or(base_indent);
            let block: Vec<&str> = block
                .into_iter()
                .map(|l| {
                    if l.is_empty() {
                        ""
                    } else {
                        let indent = l.len() - l.trim_start().len();
                        &l[indent.min(content_indent)..]
                    }
                })
                .collect();
            // chomping 作用于尾部空行：strip 全删 / clip 至多留一行 / keep 全留
            let block = if chomp_strip {
                let mut b = block;
                while b.last().is_some_and(|s| s.is_empty()) {
                    b.pop();
                }
                b
            } else if !chomp_keep {
                let mut b = block;
                while b.len() >= 2 && b[b.len() - 1].is_empty() && b[b.len() - 2].is_empty() {
                    b.pop();
                }
                b
            } else {
                block
            };
            let joined = if is_folded {
                // 折叠：非空行以空格连接，空行成段间换行
                let mut result = String::new();
                let mut pending_space = false;
                for l in &block {
                    if l.is_empty() {
                        result.push('\n');
                        pending_space = false;
                    } else {
                        if pending_space {
                            result.push(' ');
                        }
                        result.push_str(l);
                        pending_space = true;
                    }
                }
                result
            } else {
                block.join("\n")
            };
            description = if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            };
        } else {
            let unquoted = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            description = if unquoted.is_empty() {
                None
            } else {
                Some(unquoted.to_string())
            };
            i += 1;
        }
    }
    description
}

/// 把 SKILL.md frontmatter 的 description 重写为单行值（`sync_display_description`
/// 用），写口径与 [`read_skill_description_from_str`]（CodeWhale 引擎平铺解析器
/// 镜像）互洽：
///
/// - 已有**顶层** description（含 `|`/`>` 块状及其 `|-`/`>-`/`|+`/`>+` chomping
///   变体——续行按引擎消费口径一并删除）→ 原地替换（大小写不敏感，统一写小写
///   规范形）；嵌套 map 的缩进 `description:` 行**不在原位替换**（会破坏嵌套
///   结构），改为在 frontmatter 末尾插入顶层行——引擎平铺解析重复键 last-wins，
///   插在嵌套行之前会被盖回旧值；
/// - 没有 description → 插在 name 行之后（无 name 行则插在 opening `---` 后）；
/// - 统一写单行。读端只做「剥成对双/单引号」、不做反转义，因此值含 `"` 或 `\` 时
///   双引号包裹转义会与读值不符（不互洽），含换行也无法单行表达——这两类值连同
///   首尾成对引号（读端会剥掉丢字符）一律 Err 拒绝，不猜；
/// - 值在 YAML plain scalar 下不安全（前导指示符、内嵌 `: ` / ` #`、块状标记、
///   关键字/数字形态）时整体双引号包裹——此时值内已无 `"` 和 `\`，包裹后读端剥
///   引号即得原值；
/// - frontmatter 边界取**前两个 `---`**（与引擎一致）。若该区间内存在第二个
///   `description:`（含块状续行里的裸 `---` 造成的边界内缩进副本等结构性多重
///   定义），原位替换会漏改其一、last-wins 读法仍得旧值 → 返回
///   `Err("...frontmatter 内存在多个 description 定义")` 结构性拒绝，不盲改。
fn rewrite_frontmatter_description(content: &str, description: &str) -> Result<String, String> {
    let v = description.trim();
    if v.is_empty() {
        return Err("回写说明为空（清空覆盖走 extra 删 key，不回写 SKILL.md）".to_string());
    }
    if v.chars().any(|c| c.is_control()) {
        return Err("说明含控制字符/换行，无法写入单行 frontmatter".to_string());
    }
    if v.contains('"') || v.contains('\\') {
        return Err(
            "说明含双引号或反斜杠，frontmatter 单行写法无法与读口径互洽，拒绝回写".to_string(),
        );
    }
    if v.starts_with('\'') || v.ends_with('\'') {
        return Err("说明首尾的单引号会被读取端剥除，拒绝回写".to_string());
    }
    let new_line = if yaml_plain_needs_quotes(v) {
        format!("description: \"{v}\"")
    } else {
        format!("description: {v}")
    };

    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing_newline = content.ends_with('\n');
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("SKILL.md 缺 frontmatter（首行非 ---），拒绝回写".to_string());
    }
    // 前两个 --- 区间为 frontmatter；缺结束标记 = 畸形 frontmatter，拒绝改。
    let Some(fm_end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|p| p + 1)
    else {
        return Err("SKILL.md frontmatter 缺结束 ---，拒绝回写".to_string());
    };

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 1);
    out.push(lines[0].to_string());
    let mut name_line_at: Option<usize> = None;
    let mut replaced = false;
    // 嵌套 map 里的 description（缩进行，任意大小写）：不动它，但插入点须在其后。
    let mut nested_desc_seen = false;
    let mut i = 1;
    while i < fm_end {
        let raw = lines[i];
        let t = raw.trim();
        let indent = raw.len() - raw.trim_start().len();
        // 键匹配只认顶层（indent == 0）：缩进行的 `description:`/`name:` 属嵌套
        // map 字段，原位改写会破坏嵌套结构（trim 后前缀匹配会误命中）。
        if indent == 0 {
            if let Some(colon) = t.find(':') {
                let key = &t[..colon];
                let rest = t[colon + 1..].trim();
                if key.eq_ignore_ascii_case("description") {
                    out.push(new_line.clone());
                    replaced = true;
                    i += 1;
                    // 块状：续行 = 空行或缩进行（与读端消费口径一致），遇顶层字段
                    // 结束。起始 `|`/`>` 一律算（含 |- >- |+ >+ chomping 变体与
                    // 缩进指示符；平摊解析器只认六个变体，但顶层标量后跟缩进内容
                    // 在真实 YAML 里本就只能是块，一并消费使两种读法都得到新值）。
                    if rest.starts_with('|') || rest.starts_with('>') {
                        while i < fm_end {
                            let l = lines[i];
                            if !l.trim().is_empty() && l.len() == l.trim_start().len() {
                                break;
                            }
                            i += 1;
                        }
                    }
                    continue;
                }
                if key.eq_ignore_ascii_case("name") {
                    name_line_at = Some(out.len());
                }
            }
        } else if t
            .split(':')
            .next()
            .is_some_and(|k| k.trim().eq_ignore_ascii_case("description"))
        {
            nested_desc_seen = true;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    if !replaced {
        let at = if nested_desc_seen {
            // frontmatter 末尾（closing --- 之前）：CodeWhale 平摊解析重复键
            // last-wins，插在嵌套 description 之前会被盖回旧值（模型侧不同步）。
            out.len()
        } else {
            name_line_at.map_or(1, |x| x + 1)
        };
        out.insert(at, new_line);
    }
    for l in &lines[fm_end..] {
        out.push((*l).to_string());
    }
    let mut s = out.join(newline);
    if had_trailing_newline {
        s.push_str(newline);
    }
    // 结构守卫：改写后行视图（第一个整行 --- 边界内）不得残留多个顶层
    // description。输入侧显式重复键（都替换成新值仍留两行）、或块状续行里的
    // 裸 `---` 让部分旧 description 落在写入器行边界外未被替换——这两种畸形
    // 输入虽经引擎子串边界读回新值（下方互洽校验会过），但行视图留下重复
    // 顶层键，任何按行/真实 YAML 的读者都会歧义。畸形输入不值得猜，拒绝。
    if count_top_level_description_lines(&s) > 1 {
        return Err("SKILL.md frontmatter 内存在多个 description 定义，拒绝回写".to_string());
    }
    // 互洽校验（生产路径常开，非 debug_assert）：改写结果经镜像读取器必须读回
    // 新值。嵌套 description 走「frontmatter 末尾插入」，last-wins 语义下该断言
    // 同时验证插入位置正确。失败说明写口径与引擎镜像读口径出现了分歧——这正是
    // 本函数存在的意义，直接 Err 而不是落盘一个引擎读不回的值。
    if read_skill_description_from_str(&s).as_deref() != Some(v) {
        return Err("SKILL.md 回写结果与读取口径互洽校验失败（内部不一致），拒绝落盘".to_string());
    }
    Ok(s)
}

/// [`SkillMarketplaceManager::sync_display_description`] 的方向参数：
/// `Set` = 回写新说明值；`Restore` = 清覆盖时从备份恢复 frontmatter 原值。
enum SyncDesc<'a> {
    Set(&'a str),
    Restore,
}

/// 删除 SKILL.md frontmatter 里的顶层 description 行（含块状续行）——恢复
/// 「原本没有 description」的空备份哨兵用。与 [`rewrite_frontmatter_description`]
/// 同一套结构守卫（frontmatter 边界、多重定义拒绝、块状续行消费）；嵌套缩进的
/// description 不动（属嵌套 map 字段）。删完经镜像读取器校验读回 None。
fn remove_frontmatter_description(content: &str) -> Result<String, String> {
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing_newline = content.ends_with('\n');
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("SKILL.md 缺 frontmatter（首行非 ---），拒绝回写".to_string());
    }
    let Some(fm_end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|p| p + 1)
    else {
        return Err("SKILL.md frontmatter 缺结束 ---，拒绝回写".to_string());
    };

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.push(lines[0].to_string());
    let mut i = 1;
    while i < fm_end {
        let raw = lines[i];
        let t = raw.trim();
        let indent = raw.len() - raw.trim_start().len();
        if indent == 0 {
            if let Some(colon) = t.find(':') {
                let key = &t[..colon];
                let rest = t[colon + 1..].trim();
                if key.eq_ignore_ascii_case("description") {
                    i += 1;
                    // 块状续行一并删除（口径同 rewrite_frontmatter_description）
                    if rest.starts_with('|') || rest.starts_with('>') {
                        while i < fm_end {
                            let l = lines[i];
                            if !l.trim().is_empty() && l.len() == l.trim_start().len() {
                                break;
                            }
                            i += 1;
                        }
                    }
                    continue;
                }
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    for l in &lines[fm_end..] {
        out.push((*l).to_string());
    }
    let mut s = out.join(newline);
    if had_trailing_newline {
        s.push_str(newline);
    }
    // 结构守卫：删行后行视图不得残留任何顶层 description（显式重复键只删一个
    // 会留一个——畸形输入拒绝，口径同 rewrite_frontmatter_description）。
    if count_top_level_description_lines(&s) > 0 {
        return Err("SKILL.md frontmatter 内存在多个 description 定义，拒绝回写".to_string());
    }
    if read_skill_description_from_str(&s).is_some() {
        return Err("SKILL.md 删行结果与读取口径互洽校验失败（内部不一致），拒绝落盘".to_string());
    }
    Ok(s)
}

/// 行视图（第一个整行 `---` 边界内）的顶层 `description:` 行数——写端口径的
/// 结构守卫用（引擎按子串边界解析，与行边界不同；见
/// [`rewrite_frontmatter_description`] 的结构守卫注释）。
fn count_top_level_description_lines(content: &str) -> usize {
    let lines: Vec<&str> = content.lines().collect();
    let Some(fm_end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|p| p + 1)
    else {
        return 0;
    };
    lines[1..fm_end]
        .iter()
        .filter(|l| {
            let lt = l.trim();
            lt.len() == l.len()
                && lt
                    .split(':')
                    .next()
                    .is_some_and(|k| k.eq_ignore_ascii_case("description"))
        })
        .count()
}

/// 值按 YAML plain scalar 写是否不安全（需双引号包裹）。保守口径：宁可多引。
/// 调用方已保证值内无 `"` / `\`，包裹后两种解析口径（本模块剥引号读法与真实
/// YAML 解析器）得到同一字符串。
fn yaml_plain_needs_quotes(v: &str) -> bool {
    let Some(first) = v.chars().next() else {
        return false;
    };
    // 起始指示符（含读端的块状标记 | >）；- ? : 保守一律算（后随空白/结尾才是
    // 真指示符，但包裹无害）。
    if matches!(
        first,
        '!' | '&'
            | '*'
            | '?'
            | '|'
            | '>'
            | '%'
            | '@'
            | '`'
            | '#'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '-'
            | ':'
    ) {
        return true;
    }
    // 内嵌 ": " / " #"、以 ':' 结尾
    if v.contains(": ") || v.contains(" #") || v.ends_with(':') {
        return true;
    }
    // YAML 非字符串形态（真实 YAML 解析器会变类型）
    if matches!(
        v.to_ascii_lowercase().as_str(),
        "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~"
    ) || v.parse::<f64>().is_ok()
    {
        return true;
    }
    false
}

/// SKILL.md 布局优先级(越小越优先):根 SKILL.md(0) > `*/skills/<n>/SKILL.md`(1)
/// > `<n>/SKILL.md`(2) > 更深嵌套(3)。仿底座 scan_tarball 的 rank。
/// `pub(crate)`：统一插件导入的裸技能回退复用同一优先级。
pub(crate) fn skill_md_rank(path: &str) -> Option<usize> {
    if !path.eq_ignore_ascii_case("SKILL.md") && !path.to_ascii_lowercase().ends_with("/skill.md") {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        1 => Some(0),
        2 => Some(2),
        n if parts[n - 3].eq_ignore_ascii_case("skills") => Some(1),
        _ => Some(3),
    }
}

/// 含 SKILL.md 的目录(skill_root);根级 SKILL.md → 空串。
/// `pub(crate)`：统一插件导入的裸技能/裸 MCP 回退复用同一父目录推导。
pub(crate) fn skill_root_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// 技能名安全校验（`[a-zA-Z0-9_-]{1,64}`，禁 `.`/`..`）。`pub(crate)`：
/// 统一插件导入的裸技能回退复用同一口径。
pub(crate) fn is_safe_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `skill_name` collides with a preset skill (embedded marketplace
/// manifest, by frontmatter/dir name). Upload channels must reject such names
/// up front: the preset install pipeline owns the name and its sweep/rehome
/// lifecycle would destroy or absorb the uploaded copy.
/// `pub(crate)`: the unified plugin import pipeline applies the same check.
pub(crate) fn is_preset_skill_name(skill_name: &str) -> bool {
    preset_manifests()
        .iter()
        .any(|m| m.skill_name == skill_name)
}

/// On-disk copies of `skill_name` under `<packages_root>/*/skills/` whose
/// package dir name differs from `own_pkg` (staging dirs like `<id>.tmp` /
/// `<id>.old` are excluded by the component-id check). Upload channels use
/// this to reject cross-package name collisions up front: another package's
/// copy must never be silently shadowed or swept away.
/// `pub(crate)`: the unified plugin import pipeline applies the same check.
pub(crate) fn foreign_skill_copies_under(
    packages_root: &Path,
    skill_name: &str,
    own_pkg: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(packages_root) {
        for entry in entries.flatten() {
            let pkg = entry.file_name().to_string_lossy().into_owned();
            if pkg == own_pkg || !super::plugin_import::is_safe_component_id(&pkg) {
                continue;
            }
            if entry.path().join("skills").join(skill_name).is_dir() {
                out.push(pkg);
            }
        }
    }
    out
}

/// 把任意字符串（文件名 stem 等）净化为合法技能名：非 `[a-zA-Z0-9_-]` 字符 → `-`，
/// 掐头去尾的 `-` 去掉、截 64；空结果兜底 "skill"。`pub(crate)`：单 .md 导入的
/// 文件名兜底命名用。
pub(crate) fn sanitize_skill_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extract_embedded_subdir` 的 Python 编译缓存排除与 runtime_bundle 的
    /// `extract_dir` 同规则:仓库内跑技能脚本产生的 `__pycache__/`/`*.pyc`
    /// 会被 include_dir! 内嵌,不得在用户安装预置技能时物化到运行时目录。
    #[test]
    fn python_cache_paths_are_excluded_from_extraction() {
        assert!(is_python_cache_path(std::path::Path::new(
            "visualizer/scripts/__pycache__/validate.cpython-311.pyc"
        )));
        assert!(is_python_cache_path(std::path::Path::new(
            "visualizer/scripts/validate.PYC"
        )));
        assert!(!is_python_cache_path(std::path::Path::new(
            "visualizer/scripts/validate_visualizer_html.py"
        )));
        assert!(!is_python_cache_path(std::path::Path::new("pua/SKILL.md")));
    }

    #[test]
    fn parses_frontmatter_name() {
        let md = "---\nname: pua\ndescription: x\n---\n# h";
        assert_eq!(read_skill_name_from_str(md).as_deref(), Some("pua"));
        let multiline = "---\nname: huashu-nuwa\ndescription: |\n  多行\n  描述\n---\n";
        assert_eq!(
            read_skill_name_from_str(multiline).as_deref(),
            Some("huashu-nuwa")
        );
        assert!(read_skill_name_from_str("no frontmatter").is_none());
    }

    #[test]
    fn parses_frontmatter_description() {
        // 单行
        assert_eq!(
            read_skill_description_from_str("---\nname: x\ndescription: 整理会议纪要\n---\n")
                .as_deref(),
            Some("整理会议纪要")
        );
        // 引号剥离
        assert_eq!(
            read_skill_description_from_str("---\ndescription: \"带 引号\"\n---\n").as_deref(),
            Some("带 引号")
        );
        // | 块状（literal）：换行连接，空行保留（clip 至多一段尾换行）
        assert_eq!(
            read_skill_description_from_str(
                "---\ndescription: |\n  第一行\n  第二行\nname: x\n---\n"
            )
            .as_deref(),
            Some("第一行\n第二行")
        );
        // > 块状（folded）：非空行空格折叠
        assert_eq!(
            read_skill_description_from_str("---\ndescription: >\n  fold\n  ed\n---\n").as_deref(),
            Some("fold ed")
        );
        // 缺失 / 空 / 无 frontmatter
        assert!(read_skill_description_from_str("---\nname: x\n---\n").is_none());
        assert!(read_skill_description_from_str("---\ndescription: ''\n---\n").is_none());
        assert!(read_skill_description_from_str("no frontmatter").is_none());
    }

    /// 读取器是 CodeWhale 引擎平铺解析器（`parse_skill`）的镜像：本测试把引擎
    /// 语义逐条钉住——嵌套缩进 description 也算（平摊不区分嵌套）、重复键
    /// last-wins、chomping 变体、成对引号剥离、`#` 注释行跳过、首 --- 前容忍
    /// 空白、BOM 降级 None、空值覆盖。改读取器前先对照引擎实现再同步本测试。
    #[test]
    fn skill_description_mirrors_engine_flat_parser() {
        // 嵌套（缩进）description 也算——引擎平摊解析不区分嵌套 map
        assert_eq!(
            read_skill_description_from_str(
                "---\nname: x\nmetadata:\n  description: 嵌套说明\n---\n"
            )
            .as_deref(),
            Some("嵌套说明"),
            "引擎平摊读取嵌套 description，镜像不得只认顶层"
        );
        // 重复键 last-wins；后出现的空值覆盖先出现的非空值
        assert_eq!(
            read_skill_description_from_str("---\ndescription: 第一\ndescription: 第二\n---\n")
                .as_deref(),
            Some("第二")
        );
        assert_eq!(
            read_skill_description_from_str(
                "---\ndescription: 第一\nname: x\ndescription: ''\n---\n"
            )
            .as_deref(),
            None,
            "last-wins 下后出现的空值应覆盖"
        );
        // chomping 变体：|- strip / |+ keep / >- folded+strip
        assert_eq!(
            read_skill_description_from_str("---\ndescription: |-\n  a\n  b\n---\n").as_deref(),
            Some("a\nb")
        );
        assert_eq!(
            read_skill_description_from_str("---\ndescription: >-\n  fold\n  ed\n---\n").as_deref(),
            Some("fold ed")
        );
        // key 大小写不敏感（引擎统一小写）
        assert_eq!(
            read_skill_description_from_str("---\nDescription: 大写键\n---\n").as_deref(),
            Some("大写键")
        );
        // `#` 开头整行注释跳过（非注释行内的 # 不特殊处理）
        assert_eq!(
            read_skill_description_from_str("---\n# 注释\ndescription: 值\n---\n").as_deref(),
            Some("值")
        );
        // 首 --- 前容忍空白（引擎 trim_start 后判 starts_with）
        assert_eq!(
            read_skill_description_from_str("\n---\ndescription: 前置空白\n---\n").as_deref(),
            Some("前置空白")
        );
        // BOM：Rust trim 不剥 U+FEFF，引擎 starts_with("---") 不成立 → 降级路径
        assert!(read_skill_description_from_str("\u{feff}---\ndescription: x\n---\n").is_none());
        // 展示读取截断在 read_skill_description（文件版）做，from_str 返回原值
        let long = format!("---\ndescription: {}\n---\n", "字".repeat(300));
        assert_eq!(
            read_skill_description_from_str(&long)
                .unwrap()
                .chars()
                .count(),
            300
        );
    }

    #[test]
    fn ranks_skill_md_layouts() {
        assert_eq!(skill_md_rank("SKILL.md"), Some(0));
        assert_eq!(skill_md_rank("my-skill/SKILL.md"), Some(2));
        assert_eq!(skill_md_rank("repo/skills/foo/SKILL.md"), Some(1));
        assert_eq!(skill_md_rank("a/b/c/SKILL.md"), Some(3));
        assert_eq!(skill_md_rank("README.md"), None);
    }

    #[test]
    fn rejects_unsafe_names() {
        assert!(is_safe_skill_name("pua"));
        assert!(is_safe_skill_name("huashu-nuwa"));
        assert!(!is_safe_skill_name(""));
        assert!(!is_safe_skill_name(".."));
        assert!(!is_safe_skill_name("a/b"));
        assert!(!is_safe_skill_name("../etc"));
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pinvou3_skilltest_{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 预置 government-writing 从嵌入资源落盘（按包聚合布局）→ list 反映
    /// installed → 卸载删目录的全链路。技能目录经 `package_skill_dir` 取
    /// （owner 推导依赖 manifest 环境，测试不硬编码绝对布局）。
    #[test]
    fn install_then_uninstall_preset_roundtrip() {
        let tmp = fresh_dir("roundtrip");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());

        mgr.install("government-writing").unwrap();
        let skill_dir = mgr.package_skill_dir("government-writing");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert!(
            !skill_dir.join(".installed-from").exists(),
            "标记已退役，不应再写"
        );
        assert!(
            skill_dir.join("templates").is_dir(),
            "templates/ 应一并复制"
        );
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("government-writing")
        );
        // install 登记内容指纹（update_available 的比对基准）
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        assert!(
            store
                .get("government-writing")
                .unwrap()
                .expect("install should register the skill")
                .content_fingerprint
                .is_some()
        );
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "government-writing" && s.installed)
        );

        mgr.uninstall("government-writing").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "government-writing" && !s.installed)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// F1 回归：认领翻转后的滞留副本（物理在 `bundles/<其他包>/skills/<name>`，
    /// 与现算认领目录不一致）必须可按实际物理位置删除并清除记录——旧实现按
    /// 认领现算目录，判「非市场安装」让残留永驻且持续物化进会话。
    #[test]
    fn uninstall_removes_stranded_claim_mismatch_copy() {
        let tmp = fresh_dir("stranded");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let stranded = tmp.join("bundles/gongwen/skills/ghost-writer");
        std::fs::create_dir_all(&stranded).unwrap();
        std::fs::write(stranded.join("SKILL.md"), "---\nname: ghost-writer\n---\n").unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "ghost-writer".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("x.zip".to_string()),
                ),
            )
            .unwrap();
        // 现算认领目录（未知技能 owner=自身）并不存在——旧实现在此报错
        assert!(!mgr.package_skill_dir("ghost-writer").is_dir());
        // find_skill_dir 应能定位滞留副本（可见、可管）
        assert_eq!(mgr.find_skill_dir("ghost-writer"), Some(stranded.clone()));

        mgr.uninstall("ghost-writer").unwrap();
        assert!(!stranded.exists(), "滞留副本应被删除");
        assert!(
            !tmp.join("bundles/gongwen").exists(),
            "腾空的包目录应被清理"
        );
        assert!(
            store.get("ghost-writer").unwrap().is_none(),
            "记录应同步删除"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 瘫记录回归：目录已不存在（外部删除/安装中途失败）时卸载仍须删记录——
    /// 记录即安装态登记，删记录不以目录存在为前提，否则卡成永远「已安装」。
    #[test]
    fn uninstall_removes_record_when_dir_missing() {
        let tmp = fresh_dir("ghostrec");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "ghost-skill".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("x.zip".to_string()),
                ),
            )
            .unwrap();
        mgr.uninstall("ghost-skill").unwrap();
        assert!(store.get("ghost-skill").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 保护契约回归：只剩无标记旧扁平目录（内置/手放）时仍拒绝删除。
    #[test]
    fn uninstall_still_protects_unmarked_legacy_dir() {
        let tmp = fresh_dir("protect_unmarked");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let legacy = tmp.join("bundle/skills/hand-placed");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("SKILL.md"), "---\nname: hand-placed\n---\n").unwrap();
        assert!(mgr.uninstall("hand-placed").is_err(), "无标记旧扁平应拒绝");
        assert!(legacy.join("SKILL.md").is_file(), "保护对象不得被动");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// F2/F3 回归：install 落盘后清扫同名其余物理副本（滞留包目录 + 带标记
    /// 旧扁平），保证一个技能只有一份市场副本；无标记旧扁平（内置/手放）不动。
    #[test]
    fn install_sweeps_duplicate_copies() {
        let tmp = fresh_dir("sweep");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let stray = tmp.join("bundles/stray-pkg/skills/government-writing");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(
            stray.join("SKILL.md"),
            "---\nname: government-writing\n---\nstray",
        )
        .unwrap();
        let legacy = tmp.join("bundle/skills/government-writing");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("SKILL.md"),
            "---\nname: government-writing\n---\nlegacy",
        )
        .unwrap();
        std::fs::write(
            legacy.join(".installed-from"),
            "pinvou3-marketplace:government-writing",
        )
        .unwrap();

        mgr.install("government-writing").unwrap();
        let dest = mgr.package_skill_dir("government-writing");
        assert!(dest.join("SKILL.md").is_file());
        assert!(!stray.exists(), "滞留副本应被清扫");
        assert!(!legacy.exists(), "带标记旧扁平副本应被清扫");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 自愈对账回归：认领错位目录归位（无正确副本 → 移动保留用户安装；
    /// 有正确副本 → 去重删除）。判定名称无关，纯归属证明驱动。
    #[test]
    fn self_heal_rehomes_and_dedupes_claim_mismatch() {
        let tmp = fresh_dir("heal_rehome");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        // ghost-skill 活认领 = 自身（未知技能），物理在 wrong-pkg 下 = 错位
        let stray = tmp.join("bundles/wrong-pkg/skills/ghost-skill");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("SKILL.md"), "---\nname: ghost-skill\n---\n").unwrap();
        // dup-skill 同样错位，且正确位置已有副本
        let dup_stray = tmp.join("bundles/wrong-pkg/skills/dup-skill");
        std::fs::create_dir_all(&dup_stray).unwrap();
        std::fs::write(
            dup_stray.join("SKILL.md"),
            "---\nname: dup-skill\n---\nstray",
        )
        .unwrap();
        let dup_right = tmp.join("bundles/dup-skill/skills/dup-skill");
        std::fs::create_dir_all(&dup_right).unwrap();
        std::fs::write(
            dup_right.join("SKILL.md"),
            "---\nname: dup-skill\n---\nright",
        )
        .unwrap();
        // 正确位置副本需有登记（生产语义：已装技能必有记录），否则它自身即孤儿
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "dup-skill".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("x.zip".to_string()),
                ),
            )
            .unwrap();

        let report = mgr.self_heal_skills(&["visual-design"]);
        let rightful = tmp.join("bundles/ghost-skill/skills/ghost-skill");
        assert!(rightful.join("SKILL.md").is_file(), "错位副本应归位");
        assert!(!stray.exists(), "原错位目录应消失");
        assert_eq!(report.rehomed.len(), 1);
        assert!(!dup_stray.exists(), "有正确副本的错位副本应去重删除");
        assert_eq!(
            std::fs::read_to_string(dup_right.join("SKILL.md")).unwrap(),
            "---\nname: dup-skill\n---\nright",
            "正确位置副本内容应保持"
        );
        assert_eq!(report.deduped.len(), 1);
        assert!(!tmp.join("bundles/wrong-pkg").exists(), "腾空包目录应清理");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 自愈对账回归：孤儿副本（无记录/非预置/非 CLI 静态所有）删除；有记录、
    /// 预置名、CLI 静态所有的目录保留；Upload 瘫记录删除；内置释放目录收敛
    /// （无标记/无记录/不在内嵌集 → 删；内嵌集内与带标记 → 留）。
    #[test]
    fn self_heal_cleans_orphans_stale_records_and_builtin_residue() {
        let tmp = fresh_dir("heal_clean");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        let write_skill = |pkg: &str, name: &str| {
            let dir = tmp.join("bundles").join(pkg).join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
            dir
        };
        // 孤儿（无记录、非预置、非 CLI）→ 删
        let orphan = write_skill("orphan-skill", "orphan-skill");
        // 有记录 → 留
        let recorded = write_skill("recorded-skill", "recorded-skill");
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "recorded-skill".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("x.zip".to_string()),
                ),
            )
            .unwrap();
        // 预置名无记录（可重释放）→ 留
        let preset_dir = write_skill("visualizer", "visualizer");
        // CLI 静态所有（lark-doc 属 feishu 门控）→ 留
        let cli_dir = write_skill("feishu", "lark-doc");
        // Upload 瘫记录（无目录）→ 删记录
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "ghost-upload".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("y.zip".to_string()),
                ),
            )
            .unwrap();
        // 内置释放目录：残旧（无标记/无记录/非内嵌）→ 删；内嵌集内 → 留；带标记 → 留
        let stale_builtin = tmp.join("bundle/skills/old-thing");
        std::fs::create_dir_all(&stale_builtin).unwrap();
        std::fs::write(
            stale_builtin.join("SKILL.md"),
            "---\nname: old-thing\n---\n",
        )
        .unwrap();
        let embedded = tmp.join("bundle/skills/visual-design");
        std::fs::create_dir_all(&embedded).unwrap();
        std::fs::write(embedded.join("SKILL.md"), "---\nname: visual-design\n---\n").unwrap();
        let marked = tmp.join("bundle/skills/marked-thing");
        std::fs::create_dir_all(&marked).unwrap();
        std::fs::write(marked.join("SKILL.md"), "---\nname: marked-thing\n---\n").unwrap();
        std::fs::write(
            marked.join(".installed-from"),
            "pinvou3-marketplace:marked-thing",
        )
        .unwrap();

        let report = mgr.self_heal_skills(&["visual-design"]);
        assert!(!orphan.exists(), "孤儿副本应删除");
        assert!(
            report
                .removed_orphan_dirs
                .contains(&"orphan-skill/orphan-skill".to_string())
        );
        assert!(recorded.exists(), "有记录副本应保留");
        assert!(preset_dir.exists(), "预置名副本应保留（可重释放）");
        assert!(cli_dir.exists(), "CLI 静态所有副本应保留");
        assert!(
            store.get("ghost-upload").unwrap().is_none(),
            "Upload 瘫记录应删除"
        );
        assert!(
            store.get("recorded-skill").unwrap().is_some(),
            "有目录的 Upload 记录应保留"
        );
        assert!(!stale_builtin.exists(), "内置释放目录残旧应收敛删除");
        assert!(embedded.exists(), "内嵌集内目录应保留");
        assert!(marked.exists(), "带市场标记目录应留给布局迁移");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Plugin-package layouts (plugin_import): one BundleStore record per
    /// package whose id need not equal any skill dir name. self_heal must
    /// leave package-owned content untouched on every launch — no rehome, no
    /// orphan deletion, no stale-record removal — otherwise multi-skill
    /// packages, id != skill name packages, and pure-MCP uploads lose user
    /// content within two launches (review BLOCKER).
    #[test]
    fn self_heal_preserves_plugin_package_layouts() {
        let tmp = fresh_dir("heal_plugin_layout");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        let write_skill = |pkg: &str, name: &str| {
            let dir = tmp.join("bundles").join(pkg).join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
            dir
        };
        let upload_record = |id: &str| {
            store
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        id.to_string(),
                        crate::features::marketplace::store::BundleSource::Upload(
                            "pkg.zip".to_string(),
                        ),
                    ),
                )
                .unwrap();
        };
        // 1) Multi-skill package, record id = first skill name.
        let alpha = write_skill("alpha", "alpha");
        let beta = write_skill("alpha", "beta");
        std::fs::write(tmp.join("bundles/alpha/plugin.json"), "{}").unwrap();
        upload_record("alpha");
        // 2) plugin.json id != skill component names.
        let gamma = write_skill("combo", "gamma");
        let delta = write_skill("combo", "delta");
        std::fs::write(tmp.join("bundles/combo/plugin.json"), "{}").unwrap();
        upload_record("combo");
        // 3) Pure-MCP upload (no skills/ subtree at all).
        let mcp_manifest = tmp.join("bundles/puremcp/mcp/manifest.json");
        std::fs::create_dir_all(mcp_manifest.parent().unwrap()).unwrap();
        std::fs::write(&mcp_manifest, "{}").unwrap();
        std::fs::write(tmp.join("bundles/puremcp/plugin.json"), "{}").unwrap();
        upload_record("puremcp");

        for round in 1..=2 {
            let report = mgr.self_heal_skills(&["visual-design"]);
            assert!(alpha.join("SKILL.md").is_file(), "round {round}: alpha");
            assert!(beta.join("SKILL.md").is_file(), "round {round}: beta");
            assert!(gamma.join("SKILL.md").is_file(), "round {round}: gamma");
            assert!(delta.join("SKILL.md").is_file(), "round {round}: delta");
            assert!(mcp_manifest.is_file(), "round {round}: pure-MCP manifest");
            assert!(
                report.rehomed.is_empty()
                    && report.deduped.is_empty()
                    && report.removed_orphan_dirs.is_empty()
                    && report.removed_stale_records.is_empty(),
                "round {round}: package-owned content must be untouched: {report:?}"
            );
            for id in ["alpha", "combo", "puremcp"] {
                assert!(
                    store.get(id).unwrap().is_some(),
                    "round {round}: package record {id} must survive"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// MCP-bearing package without plugin.json (preset MCP release layout):
    /// its skills/ subtree is package-owned even when the claim no longer
    /// resolves (store record gone) — fail closed, leave content in place
    /// instead of rehoming it into a phantom standalone package.
    #[test]
    fn self_heal_keeps_skills_under_mcp_bearing_package() {
        let tmp = fresh_dir("heal_mcp_sibling");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        // Unrelated record so bundles.json exists (record-dependent steps run).
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "unrelated".to_string(),
                    crate::features::marketplace::store::BundleSource::Upload("x.zip".to_string()),
                ),
            )
            .unwrap();
        let skill = tmp.join("bundles/gongwen/skills/ghost-helper");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: ghost-helper\n---\n").unwrap();
        std::fs::create_dir_all(tmp.join("bundles/gongwen/mcp")).unwrap();
        std::fs::write(tmp.join("bundles/gongwen/mcp/manifest.json"), "{}").unwrap();

        for round in 1..=2 {
            let report = mgr.self_heal_skills(&["visual-design"]);
            assert!(
                skill.join("SKILL.md").is_file(),
                "round {round}: skill under mcp-bearing package must stay put"
            );
            assert!(
                !tmp.join("bundles/ghost-helper").exists(),
                "round {round}: must not rehome into a phantom standalone package"
            );
            assert!(
                report.rehomed.is_empty() && report.removed_orphan_dirs.is_empty(),
                "round {round}: {report:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A missing bundles.json must not be treated as an empty registry: with
    /// the store file absent, self-heal skips the record-dependent steps
    /// (orphan/stale/converge) instead of mass-deleting unregistered skills.
    #[test]
    fn self_heal_fail_closed_when_store_file_missing() {
        let tmp = fresh_dir("heal_no_store");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let orphan = tmp.join("bundles/orphan-skill/skills/orphan-skill");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("SKILL.md"), "---\nname: orphan-skill\n---\n").unwrap();
        let stale_builtin = tmp.join("bundle/skills/old-thing");
        std::fs::create_dir_all(&stale_builtin).unwrap();
        std::fs::write(
            stale_builtin.join("SKILL.md"),
            "---\nname: old-thing\n---\n",
        )
        .unwrap();

        let report = mgr.self_heal_skills(&["visual-design"]);
        assert!(orphan.exists(), "store 缺失时孤儿判定不得执行");
        assert!(stale_builtin.exists(), "store 缺失时内置收敛不得执行");
        assert!(report.removed_orphan_dirs.is_empty());
        assert!(report.removed_stale_records.is_empty());
        assert!(report.converged_builtin_dirs.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Upload channels reject preset/companion/cross-package name collisions
    /// up front instead of shadowing or sweeping package-owned copies
    /// (review MINOR: collision must not escalate from shadowed to swept).
    #[test]
    fn import_rejects_colliding_skill_names() {
        use std::io::Write;
        let tmp = fresh_dir("import_collision");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let write_zip = |file: &str, name: &str| {
            let zip_path = tmp.join(file);
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("s/SKILL.md", opts).unwrap();
            zw.write_all(format!("---\nname: {name}\n---\n# hi").as_bytes())
                .unwrap();
            zw.finish().unwrap();
            zip_path
        };

        // Preset skill name → reject.
        let zip_path = write_zip("preset.zip", "visualizer");
        let err = mgr.import_package(zip_path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("预置技能冲突"), "实际: {err}");

        // Skill already on disk under another package dir → reject (the
        // post-install sweep would otherwise destroy that package's copy).
        let foreign = tmp.join("bundles/other-pkg/skills/taken-skill");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "---\nname: taken-skill\n---\n").unwrap();
        let zip_path = write_zip("taken.zip", "taken-skill");
        let err = mgr.import_package(zip_path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("已存在于包 'other-pkg'"), "实际: {err}");
        assert!(foreign.join("SKILL.md").is_file(), "外来副本不得被动");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Companion-claimed name collision (claim depends on install state, so
    /// this test needs an env-isolated home with an installed MCP package
    /// whose manifest declares the skill as companion).
    #[test]
    fn import_rejects_companion_claimed_skill_name() {
        use std::io::Write;
        with_temp_home(|| {
            let home = paths::pinvou3_home();
            // Installed custom MCP package claiming "helper-skill" as companion.
            let manifest_dir = home.join("bundles/custom-mcp/mcp");
            std::fs::create_dir_all(&manifest_dir).unwrap();
            std::fs::write(
                manifest_dir.join("manifest.json"),
                r#"{"id":"custom-mcp","name":"x","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["helper-skill"]}"#,
            )
            .unwrap();
            crate::features::marketplace::store::BundleStore::new()
                .upsert(
                    crate::features::marketplace::store::BundleRecord::installed_now(
                        "custom-mcp".to_string(),
                        crate::features::marketplace::store::BundleSource::Upload(
                            "x.zip".to_string(),
                        ),
                    ),
                )
                .unwrap();
            let zip_path = home.join("companion.zip");
            {
                let f = std::fs::File::create(&zip_path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("s/SKILL.md", opts).unwrap();
                zw.write_all(b"---\nname: helper-skill\n---\n# hi").unwrap();
                zw.finish().unwrap();
            }
            let mgr = SkillMarketplaceManager::new();
            let err = mgr.import_package(zip_path.to_str().unwrap()).unwrap_err();
            assert!(
                err.contains("配套技能占用"),
                "companion 撞名应拒收，实际: {err}"
            );
        });
    }

    /// 预置 pptx 从嵌入资源落盘 → list 反映 installed → 卸载删目录的全链路。
    /// pptx 是「PPT 生成」MCP 的同名 companion 技能(manifest `companion_skills`)。
    #[test]
    fn install_then_uninstall_pptx_preset_roundtrip() {
        let tmp = fresh_dir("pptx");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());

        mgr.install("pptx").unwrap();
        let skill_dir = mgr.package_skill_dir("pptx");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert!(!skill_dir.join(".installed-from").exists(), "标记已退役");
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("pptx")
        );
        let skill_md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(
            skill_md.contains("mcp_pptx_make_pptx"),
            "应引导调用 pptx MCP 工具"
        );
        assert!(
            skill_md.contains("present_artifact(path, title)"),
            "应要求产物卡交付"
        );
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "pptx" && s.installed)
        );

        mgr.uninstall("pptx").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "pptx" && !s.installed)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Visualizer 预置技能带 references/ 子树,安装后必须可被 SkillRegistry 读取。
    #[test]
    fn install_visualizer_preset_with_references() {
        let tmp = fresh_dir("visualizer");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());

        mgr.install("visualizer").unwrap();
        // 独立技能包：owner = 自身 → bundles/visualizer/skills/visualizer
        let skill_dir = tmp.join("bundles/visualizer/skills/visualizer");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert!(
            skill_dir
                .join("references")
                .join("visualizer-design-system.md")
                .is_file(),
            "references/ 应一并复制"
        );
        assert!(
            skill_dir
                .join("scripts")
                .join("validate_visualizer_html.py")
                .is_file(),
            "scripts/ 校验器应一并复制"
        );
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("visualizer")
        );
        // install 登记的指纹应与嵌入资源指纹一致（刚装即一致 → 无更新提示）
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        let record = store
            .get("visualizer")
            .unwrap()
            .expect("install should register the skill");
        let embedded_fp = embedded_skill_fingerprint(mgr.preset("visualizer").unwrap());
        assert_eq!(record.content_fingerprint, embedded_fp);
        let skill_md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(
            skill_md.contains("https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js"),
            "Visualizer 应固定使用 cdnjs Chart.js UMD"
        );
        assert!(
            skill_md.contains("present_artifact(path, title)"),
            "Visualizer 应要求用 artifact 卡片交付"
        );
        assert!(
            skill_md.contains("role=\"img\""),
            "Visualizer 应要求 canvas 无障碍属性"
        );
        assert!(
            skill_md.contains("ECharts") && skill_md.contains("Plotly"),
            "Visualizer 应显式禁止默认回退到其他图库"
        );
        assert!(
            skill_md.contains("失败判定"),
            "Visualizer 应保留失败判定段，便于生成前自检"
        );
        let design_system = std::fs::read_to_string(
            skill_dir
                .join("references")
                .join("visualizer-design-system.md"),
        )
        .unwrap();
        assert!(
            design_system.contains("Chart.js UMD")
                && design_system.contains("present_artifact(path, title)")
                && design_system.contains("role=\"img\""),
            "Visualizer reference 应包含 Chart.js、artifact 和 canvas 无障碍规则"
        );
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "visualizer" && s.installed)
        );

        mgr.uninstall("visualizer").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn install_ima_preset_with_native_tool_instructions() {
        let tmp = fresh_dir("ima");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());

        mgr.install("ima-skills").unwrap();
        // ima 凭据包认领：bundles/ima/skills/ima-skills
        let skill_dir = tmp.join("bundles/ima/skills/ima-skills");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert!(
            !skill_dir.join("ima_api.cjs").exists(),
            "不得复制本地凭据 helper"
        );
        assert!(
            skill_dir.join("knowledge-base").join("SKILL.md").is_file(),
            "knowledge-base 子模块说明应一并复制"
        );
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("ima-skills")
        );
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "ima-skills" && s.installed)
        );

        mgr.uninstall("ima-skills").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 腾讯文档官方 skill(vendored):安装落盘到 frontmatter name(tencent-docs),
    /// 品类参考与 smartcanvas 模板随包;mcporter 依赖脚本(setup.sh/import_file.sh/
    /// ocr.js)不应存在,适配版 get_slide_info.sh 应在。
    /// setup.js(slidep 全局安装脚本)已移除:工作流不调用 slidep CLI,保留一条可被
    /// 诱导执行的无校验和全局安装路径没有收益。
    #[test]
    fn install_tencent_docs_preset_with_official_references() {
        let tmp = fresh_dir("tdoc");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());

        mgr.install("tencent-docs-skill").unwrap();
        // 预置 id 是市场名(tencent-docs-skill),落盘按 frontmatter name(tencent-docs)
        // 经 package_skill_dir 推导 owner——与 gongwen 等同名 companion 不同,勿硬编码。
        let skill_dir = mgr.package_skill_dir("tencent-docs");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("tencent-docs")
        );
        for reference in [
            "references/manage_references.md",
            "references/smartsheet_references.md",
            "references/docengine_references.md",
            "references/slideengine_references.md",
            "sheet/api/mcp-api.md",
            "smartcanvas/entry.md",
            "slide/entry.md",
        ] {
            assert!(
                skill_dir.join(reference).is_file(),
                "{reference} 应随包落盘"
            );
        }
        // mcporter 依赖脚本不应 vendored 进来
        for dropped in ["setup.sh", "import_file.sh", "ocr.js"] {
            assert!(
                !skill_dir.join(dropped).exists(),
                "{dropped} 依赖 mcporter,不应保留"
            );
        }
        assert!(
            !skill_dir
                .join("sidebar-pptx-generator/scripts/setup.js")
                .exists(),
            "setup.js 是无校验和的全局安装脚本,工作流不使用 slidep CLI,不应保留"
        );
        assert!(
            skill_dir
                .join("sidebar-pptx-generator/scripts/get_slide_info.sh")
                .is_file(),
            "适配版状态脚本应在"
        );
        let skmd = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(
            !skmd.contains("mcporter"),
            "SKILL.md 不应残留 mcporter 调用说明"
        );
        assert!(
            mgr.list_skills()
                .iter()
                .any(|s| s.id == "tencent-docs-skill" && s.installed)
        );

        mgr.uninstall("tencent-docs-skill").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 不在包目录的目录（旧布局内置/手放）拒绝卸载,防误删。
    #[test]
    fn uninstall_refuses_non_market_dir() {
        let tmp = fresh_dir("protect");
        let legacy = tmp.join("bundle/skills");
        std::fs::create_dir_all(legacy.join("pua")).unwrap();
        std::fs::write(legacy.join("pua").join("SKILL.md"), "---\nname: pua\n---").unwrap();
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        assert!(mgr.uninstall("pua").is_err(), "非市场目录应拒删");
        assert!(legacy.join("pua").exists(), "目录应保留");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 用户上传 zip:解压找 SKILL.md → 按 frontmatter name 落盘 → list 标 user_uploaded。
    #[test]
    fn import_zip_lands_subtree_by_frontmatter_name() {
        use std::io::Write;
        let tmp = fresh_dir("import");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            // 顶层目录包裹(rank 2)：my-skill/ 下含 SKILL.md + 辅助 + 应被跳过的 .git/
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: my-test-skill\ndescription: t\n---\n# hi")
                .unwrap();
            zw.start_file("my-skill/ref.md", opts).unwrap();
            zw.write_all(b"reference body").unwrap();
            zw.start_file("my-skill/.git/config", opts).unwrap();
            zw.write_all(b"[core]").unwrap();
            zw.finish().unwrap();
        }
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        mgr.import_package(zip_path.to_str().unwrap()).unwrap();

        // 上传技能独立成包：bundles/<name>/skills/<name>/
        let dest = tmp.join("bundles/my-test-skill/skills/my-test-skill");
        assert!(dest.join("SKILL.md").is_file(), "按 frontmatter name 落盘");
        assert!(dest.join("ref.md").is_file(), "辅助文件应带过来");
        assert!(!dest.join(".git").exists(), ".git 等隐藏目录应跳过");
        assert!(!dest.join(".installed-from").exists(), "标记已退役");
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        assert_eq!(
            store.get("my-test-skill").unwrap().unwrap().source,
            crate::features::marketplace::store::BundleSource::Upload("pkg.zip".to_string()),
            "来源应登记进 BundleStore"
        );
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "my-test-skill")
            .expect("list 应含上传技能");
        assert!(listed.user_uploaded && listed.installed);
        // frontmatter description 应被解析展示;subtitle 留空交前端回退
        assert_eq!(listed.description, "t");
        assert!(listed.subtitle.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `import_package_named` 用调用方给的 display_name 登记 Upload 来源
    /// (拖放字节通道的 zip 名经命令层净化后传入；标记文件已退役)。
    #[test]
    fn import_package_named_records_upload_source() {
        use std::io::Write;
        let tmp = fresh_dir("named");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: named-skill\ndescription: d\n---\n# hi")
                .unwrap();
            zw.finish().unwrap();
        }
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        mgr.import_package_named(zip_path.to_str().unwrap(), "my skill.zip")
            .unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        assert_eq!(
            store.get("named-skill").unwrap().unwrap().source,
            crate::features::marketplace::store::BundleSource::Upload("my skill.zip".to_string())
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 伪造 zip 头部（中央目录声明 size=0、压缩流真实解压非空）的条目必须被
    /// pass2 有界读取（read_zip_entry_bounded）响亮拒收，不能完整读入内存
    /// （五轮评审 M-5：旧管线与新管线 plugin_import 防护对齐）。
    #[test]
    fn import_package_named_rejects_forged_size_header() {
        use std::io::Write;
        let tmp = fresh_dir("forged_header");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("bomb-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: bomb-skill\ndescription: d\n---\n# hi")
                .unwrap();
            zw.start_file("bomb-skill/big.txt", opts).unwrap();
            zw.write_all(&[b'x'; 64]).unwrap();
            zw.finish().unwrap();
        }
        // 篡改中央目录：把 big.txt 条目的「未压缩大小」字段（条目内偏移 24）
        // 改为 0（伪造头部）；压缩流保持原样，真实解压仍非空。
        let mut bytes = std::fs::read(&zip_path).unwrap();
        let name = b"bomb-skill/big.txt";
        let sig = 0x02014b50u32.to_le_bytes();
        let pos = (0..bytes.len().saturating_sub(50 + name.len()))
            .find(|&p| bytes[p..p + 4] == sig && &bytes[p + 46..p + 46 + name.len()] == name)
            .expect("中央目录应含 big.txt 条目");
        bytes[pos + 24..pos + 28].copy_from_slice(&0u32.to_le_bytes());
        std::fs::write(&zip_path, &bytes).unwrap();

        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let err = mgr
            .import_package_named(zip_path.to_str().unwrap(), "pkg.zip")
            .unwrap_err();
        assert!(
            err.contains("超过 zip 头声明"),
            "伪造头部条目应被有界读取拒收，实际: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 伪造 SKILL.md 自身的头部（中央目录声明 size=0、压缩流真实解压非空）：
    /// frontmatter 预读也必须走有界读取（read_zip_entry_bounded）响亮拒收，
    /// 不能整流读入内存（六轮评审 R2：pass2 已收口，预读取名同一防护）。
    #[test]
    fn import_package_named_rejects_forged_skill_md_header() {
        use std::io::Write;
        let tmp = fresh_dir("forged_skill_md");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("bomb-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: bomb-skill\ndescription: d\n---\n# hi")
                .unwrap();
            zw.finish().unwrap();
        }
        // 篡改中央目录：把 SKILL.md 条目的「未压缩大小」字段（条目内偏移 24）
        // 改为 0（伪造头部）；压缩流保持原样，真实解压仍非空。
        let mut bytes = std::fs::read(&zip_path).unwrap();
        let name = b"bomb-skill/SKILL.md";
        let sig = 0x02014b50u32.to_le_bytes();
        let pos = (0..bytes.len().saturating_sub(50 + name.len()))
            .find(|&p| bytes[p..p + 4] == sig && &bytes[p + 46..p + 46 + name.len()] == name)
            .expect("中央目录应含 SKILL.md 条目");
        bytes[pos + 24..pos + 28].copy_from_slice(&0u32.to_le_bytes());
        std::fs::write(&zip_path, &bytes).unwrap();

        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let err = mgr
            .import_package_named(zip_path.to_str().unwrap(), "pkg.zip")
            .unwrap_err();
        assert!(
            err.contains("超过 zip 头声明"),
            "伪造 SKILL.md 头部应被有界预读拒收，实际: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn update_available_detects_disk_drift() {
        let tmp = fresh_dir("update");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let flagged = |mgr: &SkillMarketplaceManager| {
            mgr.list_skills()
                .into_iter()
                .find(|s| s.id == "visualizer")
                .unwrap()
                .update_available
        };

        assert!(!flagged(&mgr), "未安装应恒 false");

        mgr.install("visualizer").unwrap();
        assert!(!flagged(&mgr), "刚安装应与嵌入资源一致");

        // 篡改文件内容（磁盘 ≠ 记录）→ true
        let skill_dir = mgr.package_skill_dir("visualizer");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: visualizer\n---\n本地改过",
        )
        .unwrap();
        assert!(flagged(&mgr), "内容被改应检出");

        // 重装(=更新)后复原 → false
        mgr.install("visualizer").unwrap();
        assert!(!flagged(&mgr), "重装后应恢复一致");

        // 删除文件 → true
        std::fs::remove_file(
            skill_dir
                .join("references")
                .join("visualizer-design-system.md"),
        )
        .unwrap();
        assert!(flagged(&mgr), "缺文件应检出");

        // 复原后新增多余文件 → true
        mgr.install("visualizer").unwrap();
        std::fs::write(skill_dir.join("local-notes.md"), "my notes").unwrap();
        assert!(flagged(&mgr), "多文件应检出");

        // 模拟上游更新：磁盘与记录同步、但记录指纹滞后于嵌入资源 → true
        mgr.install("visualizer").unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        let mut record = store.get("visualizer").unwrap().unwrap();
        record.content_fingerprint = Some("stale-upstream-fingerprint".to_string());
        store.upsert(record).unwrap();
        assert!(flagged(&mgr), "上游更新（记录 ≠ 嵌入）应检出");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 上传技能无嵌入对应物,不参与更新检测(改内容也恒 false)。
    #[test]
    fn uploaded_skill_never_update_available() {
        use std::io::Write;
        let tmp = fresh_dir("upload_no_update");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("up-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: up-skill\ndescription: d\n---\n# hi")
                .unwrap();
            zw.finish().unwrap();
        }
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        mgr.import_package(zip_path.to_str().unwrap()).unwrap();
        std::fs::write(
            tmp.join("bundles/up-skill/skills/up-skill")
                .join("SKILL.md"),
            "---\nname: up-skill\n---\n改过",
        )
        .unwrap();
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "up-skill")
            .unwrap();
        assert!(listed.user_uploaded && !listed.update_available);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Phase 2 镜像：预置 install → bundles.json 登记 Preset 记录；uninstall → 删除。
    /// 镜像文件随 `with_roots` 落在同一临时目录，不碰真实家目录。
    #[test]
    fn install_and_uninstall_mirror_bundle_store_record() {
        let tmp = fresh_dir("mirror_preset");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));

        mgr.install("visualizer").unwrap();
        let record = store
            .get("visualizer")
            .unwrap()
            .expect("install 应镜像登记 bundles.json");
        assert_eq!(
            record.source,
            crate::features::marketplace::store::BundleSource::Preset
        );
        assert!(record.installed);

        mgr.uninstall("visualizer").unwrap();
        assert!(
            store.get("visualizer").unwrap().is_none(),
            "uninstall 应镜像删除记录"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Phase 2 镜像：上传导入登记 Upload(zip 名) 记录，id = 落盘技能名。
    #[test]
    fn import_package_mirrors_upload_record() {
        use std::io::Write;
        let tmp = fresh_dir("mirror_upload");
        let zip_path = tmp.join("pkg.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("up-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: up-skill\ndescription: d\n---\n# hi")
                .unwrap();
            zw.finish().unwrap();
        }
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        mgr.import_package(zip_path.to_str().unwrap()).unwrap();

        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        let record = store
            .get("up-skill")
            .unwrap()
            .expect("上传导入应镜像登记 bundles.json");
        assert_eq!(
            record.source,
            crate::features::marketplace::store::BundleSource::Upload("pkg.zip".to_string())
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包（迁移涉及 connector_state /
    /// manifest 扫描等 env 路径，必须 env 隔离 + ENV_LOCK 串行）。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-skillmigrate-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧扁平布局 → 按包聚合迁移：四个分支（纯技能 / MCP companion / CLI companion /
    /// 上传）+ 内置技能不动 + 预置指纹补写 + 幂等。companion 归属是条件认领（M-7）：
    /// 所属 MCP 已装（installed.json → import_legacy 登记）才归 MCP 包目录；未装的
    /// MCP companion 按独立包迁移。
    #[test]
    fn migrate_flat_skills_layout_covers_all_branches() {
        with_temp_home(|| {
            let home = paths::pinvou3_home();
            let legacy = paths::bundle_skills_dir();
            let seed = |rel: &str, content: &str| {
                let p = home.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, content).unwrap();
            };
            // gongwen 已装（installed.json → import_legacy 登记），manifest 声明
            // companion（companion 归 MCP 分支的归属依据）
            seed("marketplace/installed.json", r#"["gongwen"]"#);
            seed(
                "bundle/mcp-servers/gongwen/manifest.json",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["government-writing"]}"#,
            );
            // ghost 未装（仅旧布局 manifest 声明 companion）→ 条件认领不成立，
            // 其 companion 应按独立包迁移
            seed(
                "bundle/mcp-servers/ghost/manifest.json",
                r#"{"id":"ghost","name":"未装","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["ghost-helper"]}"#,
            );
            // 纯技能（无包认领）
            seed(
                "bundle/skills/visualizer/SKILL.md",
                "---\nname: visualizer\n---\n",
            );
            seed(
                "bundle/skills/visualizer/.installed-from",
                "pinvou3-marketplace:visualizer",
            );
            // MCP companion（所属包已装）
            seed(
                "bundle/skills/government-writing/SKILL.md",
                "---\nname: government-writing\n---\n",
            );
            seed(
                "bundle/skills/government-writing/.installed-from",
                "pinvou3-marketplace:government-writing",
            );
            // MCP companion（所属包未装 → 独立成包）
            seed(
                "bundle/skills/ghost-helper/SKILL.md",
                "---\nname: ghost-helper\n---\n",
            );
            seed(
                "bundle/skills/ghost-helper/.installed-from",
                "pinvou3-marketplace:ghost-helper",
            );
            // 上传技能
            seed(
                "bundle/skills/my-upload/SKILL.md",
                "---\nname: my-upload\n---\n",
            );
            seed("bundle/skills/my-upload/.installed-from", "upload:pkg.zip");
            // CLI companion（无标记；连接器可见 = 无 feishu_disabled 文件）
            seed(
                "bundle/skills/lark-shared/SKILL.md",
                "---\nname: lark-shared\n---\n",
            );
            // 内置释放技能（无标记、非 CLI 清单）→ 不动
            seed(
                "bundle/skills/visual-design/SKILL.md",
                "---\nname: visual-design\n---\n",
            );

            // 先跑 import（生产顺序：import 登记 → 迁移搬目录）
            let store = crate::features::marketplace::store::BundleStore::new();
            store.import_legacy().unwrap();

            let mgr = SkillMarketplaceManager::new();
            let report = mgr.migrate_flat_skills_layout();

            assert!(
                home.join("bundles/visualizer/skills/visualizer/SKILL.md")
                    .is_file(),
                "纯技能 → 独立包目录"
            );
            assert!(
                home.join("bundles/gongwen/skills/government-writing/SKILL.md")
                    .is_file(),
                "companion（MCP 已装）→ 所属 MCP 包目录"
            );
            assert!(
                home.join("bundles/ghost-helper/skills/ghost-helper/SKILL.md")
                    .is_file(),
                "companion（MCP 未装）→ 独立包目录（条件认领，M-7）"
            );
            assert!(
                !home.join("bundles/ghost").exists(),
                "未装 MCP 不应因 companion 迁移而空建包目录"
            );
            assert!(
                home.join("bundles/feishu/skills/lark-shared/SKILL.md")
                    .is_file(),
                "CLI companion → 连接器包目录"
            );
            assert!(
                home.join("bundles/my-upload/skills/my-upload/SKILL.md")
                    .is_file(),
                "上传技能 → 独立包目录"
            );
            assert!(
                legacy.join("visual-design/SKILL.md").is_file(),
                "内置技能不动"
            );
            assert!(!legacy.join("visualizer").exists(), "旧位置已搬空");
            assert!(report.moved.len() == 5, "应移动 5 个: {report:?}");
            assert!(report.kept.contains(&"visual-design".to_string()));

            // 预置指纹补写（update_available 比对基准）
            let fp = store
                .get("visualizer")
                .unwrap()
                .expect("visualizer 记录应在")
                .content_fingerprint;
            assert!(fp.is_some(), "迁移应补写预置技能指纹");

            // 幂等：二次迁移零移动
            let report2 = mgr.migrate_flat_skills_layout();
            assert!(report2.moved.is_empty(), "二次迁移应无移动");
            assert!(report2.removed_stale.is_empty());
        });
    }

    /// CLI companion 在连接器不可见（停用标记在）时是断开残留：按门控语义删除，
    /// 不迁移（immutable 资源，重连重解包）。
    #[test]
    fn migrate_removes_stale_cli_skills_when_connector_hidden() {
        with_temp_home(|| {
            let home = paths::pinvou3_home();
            let legacy = paths::bundle_skills_dir();
            let dir = legacy.join("lark-shared");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "---\nname: lark-shared\n---\n").unwrap();
            std::fs::write(home.join("feishu_disabled"), "1").unwrap();

            let report = SkillMarketplaceManager::new().migrate_flat_skills_layout();
            assert!(!dir.exists(), "不可见连接器的残留应删除");
            assert!(
                !home.join("bundles/feishu").exists(),
                "不应迁移出连接器包目录"
            );
            assert_eq!(report.removed_stale, vec!["lark-shared".to_string()]);
        });
    }

    /// 企微 0.1.9 退役目录（msg/schedule）迁移走删除而非搬移：搬进
    /// `bundles/wecom/skills/` 后门控的 legacy 清理（只扫旧扁平目录）够不到，
    /// 退役技能会永久残留并被物化进会话（五轮评审必修 3）。14 个新名正常搬移。
    #[test]
    fn migrate_deletes_retired_wecom_skills_instead_of_moving() {
        with_temp_home(|| {
            let home = paths::pinvou3_home();
            let legacy = paths::bundle_skills_dir();
            // 退役名 + 一个新名（连接器可见 = 无 wecom_disabled 文件）
            for name in ["wecomcli-msg", "wecomcli-schedule", "wecomcli-message"] {
                let dir = legacy.join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
            }

            let report = SkillMarketplaceManager::new().migrate_flat_skills_layout();

            for retired in ["wecomcli-msg", "wecomcli-schedule"] {
                assert!(!legacy.join(retired).exists(), "{retired} 旧位置应删除");
                assert!(
                    !home
                        .join(format!("bundles/wecom/skills/{retired}"))
                        .exists(),
                    "{retired} 不得搬入新布局"
                );
                assert!(
                    report.removed_stale.contains(&retired.to_string()),
                    "{retired} 应记入 removed_stale: {report:?}"
                );
            }
            assert!(
                home.join("bundles/wecom/skills/wecomcli-message/SKILL.md")
                    .is_file(),
                "新名应正常搬移到连接器包目录"
            );
            assert_eq!(report.moved, vec!["wecomcli-message".to_string()]);
        });
    }

    // ------------------------------------------------------------------
    // SKILL.md description 回写（编辑上传包展示说明时联动）
    // ------------------------------------------------------------------

    /// 单行替换 + 读口径互洽：改写后再经 read_skill_description_from_str 读回原值。
    #[test]
    fn rewrite_description_replaces_single_line() {
        let md = "---\nname: x\ndescription: 旧描述\n---\n# 正文\n";
        let out = rewrite_frontmatter_description(md, "新描述").unwrap();
        assert_eq!(out, "---\nname: x\ndescription: 新描述\n---\n# 正文\n");
        assert_eq!(
            read_skill_description_from_str(&out).as_deref(),
            Some("新描述"),
            "写完必须能读回同一值"
        );
    }

    /// 块状 `|` 替换：缩进/空行续行被一并清除（与读端消费口径一致），后续顶层字段保留。
    #[test]
    fn rewrite_description_replaces_block_and_drops_continuations() {
        let md = "---\nname: x\ndescription: |\n  第一行\n\n  第二行\nversion: 1\n---\n";
        let out = rewrite_frontmatter_description(md, "折叠后的新描述").unwrap();
        assert_eq!(
            out,
            "---\nname: x\ndescription: 折叠后的新描述\nversion: 1\n---\n"
        );
        assert_eq!(
            read_skill_description_from_str(&out).as_deref(),
            Some("折叠后的新描述")
        );
    }

    /// 块状 chomping 变体（|- >- |+ >+）同样替换并清除续行：模型侧解析器
    /// （CodeWhale parse_skill）支持四种变体，只认 `|`/`>` 会残留续行——
    /// 续行里再出现 `description:` 形态时 last-wins 会取回旧值，回写静默失效。
    #[test]
    fn rewrite_description_replaces_chomping_variants() {
        for marker in ["|-", ">-", "|+", ">+"] {
            let md = format!("---\nname: x\ndescription: {marker}\n  第一行\n  description: 旧文案\nversion: 1\n---\n");
            let out = rewrite_frontmatter_description(&md, "新描述").unwrap();
            assert_eq!(
                out, "---\nname: x\ndescription: 新描述\nversion: 1\n---\n",
                "chomping 变体 {marker} 的续行必须一并清除"
            );
            assert_eq!(
                read_skill_description_from_str(&out).as_deref(),
                Some("新描述")
            );
        }
    }

    /// 嵌套 map 的缩进 `description:` 行不在原位替换（会破坏嵌套结构）：
    /// 顶层新行插在 frontmatter 末尾——平摊解析（CodeWhale）重复键 last-wins，
    /// 插在嵌套行之前会被盖回旧值。嵌套行原样保留。
    #[test]
    fn rewrite_description_appends_top_level_when_nested_description_present() {
        let md = "---\nname: x\nmetadata:\n  description: 内部备注\n  author: 张三\n---\n";
        let out = rewrite_frontmatter_description(md, "新描述").unwrap();
        assert_eq!(
            out,
            "---\nname: x\nmetadata:\n  description: 内部备注\n  author: 张三\ndescription: 新描述\n---\n"
        );
        // 本模块读端取首个顶层 description → 新值
        assert_eq!(
            read_skill_description_from_str(&out).as_deref(),
            Some("新描述")
        );
    }

    /// 顶层 `Description:`（大写）原地替换为规范小写形——平摊解析键不区分大小写，
    /// 不识别会走插入分支，留下重复键且 last-wins 取旧值（模型侧不同步）。
    #[test]
    fn rewrite_description_matches_top_level_case_insensitively() {
        let md = "---\nname: x\nDescription: 旧描述\n---\n";
        let out = rewrite_frontmatter_description(md, "新描述").unwrap();
        assert_eq!(out, "---\nname: x\ndescription: 新描述\n---\n");
        assert_eq!(
            read_skill_description_from_str(&out).as_deref(),
            Some("新描述")
        );
    }

    /// 无 description → 插在 name 行之后；无 name 行 → 插在 opening --- 之后。
    #[test]
    fn rewrite_description_inserts_after_name_line() {
        let out = rewrite_frontmatter_description("---\nname: x\n---\n", "补充描述").unwrap();
        assert_eq!(out, "---\nname: x\ndescription: 补充描述\n---\n");
        let no_name =
            rewrite_frontmatter_description("---\nversion: 1\n---\n", "补充描述").unwrap();
        assert_eq!(no_name, "---\ndescription: 补充描述\nversion: 1\n---\n");
    }

    /// YAML plain scalar 不安全的值整体双引号包裹（值内无 "/\\ 时两种读法同值）。
    #[test]
    fn rewrite_description_quotes_yaml_unsafe_values() {
        for v in ["true", "123", "含: 冒号", "|开头"] {
            let out = rewrite_frontmatter_description("---\nname: x\n---\n", v).unwrap();
            assert!(
                out.contains(&format!("description: \"{v}\"")),
                "{v} 应被双引号包裹: {out}"
            );
            assert_eq!(read_skill_description_from_str(&out).as_deref(), Some(v));
        }
    }

    /// 不互洽的值拒绝回写：换行/控制字符、双引号、反斜杠、首尾单引号。
    #[test]
    fn rewrite_description_rejects_non_roundtrippable_values() {
        for v in ["含\n换行", "含\"双引号", "含\\反斜杠", "'首尾单引号'"] {
            assert!(
                rewrite_frontmatter_description("---\nname: x\n---\n", v).is_err(),
                "{v:?} 应拒绝回写"
            );
        }
        // 畸形 frontmatter（缺 opening/closing ---）也拒绝
        assert!(rewrite_frontmatter_description("no frontmatter", "x").is_err());
        assert!(rewrite_frontmatter_description("---\nname: x\n", "x").is_err());
    }

    /// 结构守卫：显式重复顶层 description（替换后行视图仍留两行）→ 拒绝；
    /// 块标量续行里的裸 `---`（写入器行边界=引擎子串边界都落在续行内，旧值行
    /// 在边界外 body 中）→ 插入行进引擎 frontmatter，读回新值，正确放行。
    #[test]
    fn rewrite_description_structural_guard_for_duplicate_keys() {
        // 显式重复键：两行都替换成新值，行视图仍留两行顶层 description → Err
        assert!(rewrite_frontmatter_description(
            "---\ndescription: 第一\ndescription: 第二\n---\n",
            "新"
        )
        .is_err());
        // remove 路径：循环删所有顶层行（不 break），重复键被全删 → 成功且无残留
        let out =
            remove_frontmatter_description("---\ndescription: 第一\ndescription: 第二\n---\n")
                .expect("remove 循环删所有顶层 description，重复键应全删成功");
        assert!(read_skill_description_from_str(&out).is_none());
        // 块内裸 ---：旧顶层 description 在边界外（引擎不读），插入新值在边界内
        let block_with_fence = "---\nname: x\nlicense: |\n  ---\n  MIT\ndescription: 旧值\n---\n";
        let out = rewrite_frontmatter_description(block_with_fence, "新")
            .expect("边界外旧值不算重复，应放行");
        assert_eq!(read_skill_description_from_str(&out).as_deref(), Some("新"));
    }

    /// 单技能上传包：编排入口设覆盖 → 回写落盘 + 原值备份 + 内容指纹重算补写
    /// 登记（extra 展示覆盖必须保留）。
    #[test]
    fn writeback_updates_skill_md_and_fingerprint_preserving_extra() {
        let tmp = fresh_dir("writeback_single");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let skill_dir = tmp.join("bundles/wb-skill/skills/wb-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: wb-skill\ndescription: 旧描述\n---\n",
        )
        .unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "wb-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();
        let fp_before = store.get("wb-skill").unwrap().unwrap().content_fingerprint;

        mgr.update_display_meta("wb-skill", Some("我的技能"), Some("新描述"))
            .unwrap();
        let md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(md.contains("description: 新描述"), "{md}");
        let rec = store.get("wb-skill").unwrap().unwrap();
        assert!(
            rec.content_fingerprint.is_some() && rec.content_fingerprint != fp_before,
            "回写后指纹应被重算补写"
        );
        // 指纹口径 = 整包目录（与 plugin_import 的登记基线一致），非技能子目录。
        assert_eq!(
            rec.content_fingerprint.as_deref(),
            dir_fingerprint(&tmp.join("bundles/wb-skill"))
                .ok()
                .as_deref(),
            "回写后指纹应为整包目录口径（plugin_import 同口径）"
        );
        assert_eq!(
            crate::features::marketplace::store::display_override(
                &rec,
                crate::features::marketplace::store::EXTRA_DISPLAY_NAME
            )
            .as_deref(),
            Some("我的技能"),
            "指纹补写不得丢 extra 展示覆盖"
        );
        // 首次回写备份了引擎口径原值（清覆盖时恢复用）
        assert_eq!(
            store.skill_desc_backup("wb-skill").unwrap().as_deref(),
            Some("旧描述"),
            "首次回写应备份 frontmatter 原值"
        );
        assert_eq!(
            rec.source,
            crate::features::marketplace::store::BundleSource::Upload("pkg.zip".to_string())
        );
    }

    /// 单技能包设覆盖后清覆盖：SKILL.md 恢复备份原值、备份 key 清除、
    /// 展示回退 frontmatter 现值；再次清（无备份）为 no-op 不动文件。
    #[test]
    fn restore_reverts_skill_md_description_from_backup_on_clear() {
        let tmp = fresh_dir("writeback_restore");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let skill_dir = tmp.join("bundles/rb-skill/skills/rb-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: rb-skill\ndescription: 原描述\n---\n",
        )
        .unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "rb-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();

        mgr.update_display_meta("rb-skill", None, Some("覆盖描述"))
            .unwrap();
        let md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(md.contains("description: 覆盖描述"), "{md}");

        // 清覆盖 → SKILL.md 恢复原值、备份 key 删除
        mgr.update_display_meta("rb-skill", None, Some("  "))
            .unwrap();
        let md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(
            md.contains("description: 原描述") && !md.contains("覆盖描述"),
            "清覆盖应恢复原值: {md}"
        );
        assert!(store.skill_desc_backup("rb-skill").unwrap().is_none());
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "rb-skill")
            .unwrap();
        assert_eq!(listed.description, "原描述");
        assert_eq!(listed.display_description, None);

        // 再清一次（无备份）→ no-op，文件不动
        let md_before = md.clone();
        mgr.update_display_meta("rb-skill", None, Some("")).unwrap();
        assert_eq!(
            std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            md_before,
            "无备份的清覆盖不得动文件"
        );
    }

    /// 原本没有 description 的单技能包：回写备份空串哨兵，清覆盖删行恢复缺失态。
    #[test]
    fn restore_removes_description_line_when_backup_is_empty_sentinel() {
        let tmp = fresh_dir("writeback_restore_missing");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let skill_dir = tmp.join("bundles/nb-skill/skills/nb-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: nb-skill\n---\n").unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "nb-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();

        mgr.update_display_meta("nb-skill", None, Some("临时描述"))
            .unwrap();
        assert_eq!(
            store.skill_desc_backup("nb-skill").unwrap().as_deref(),
            Some(""),
            "原缺失哨兵应为空串"
        );
        mgr.update_display_meta("nb-skill", None, Some("")).unwrap();
        let md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        assert!(
            !md.contains("description"),
            "清覆盖应删除 description 行: {md}"
        );
        assert_eq!(md, "---\nname: nb-skill\n---\n");
    }

    /// 编排门禁：未登记 / 预置来源拒绝；超长说明在回写**之前**被拒（SKILL.md
    /// 字节不变、指纹不变——「报错但包内容已变」中间态的命令级顺序契约）。
    #[test]
    fn update_display_meta_rejects_before_mutating_package() {
        let tmp = fresh_dir("display_meta_gate");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let skill_dir = tmp.join("bundles/g-skill/skills/g-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let original_md = "---\nname: g-skill\ndescription: 原描述\n---\n";
        std::fs::write(skill_dir.join("SKILL.md"), original_md).unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "g-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "preset-x",
                    crate::features::marketplace::store::BundleSource::Preset,
                ),
            )
            .unwrap();

        // 未登记 / 预置来源 → Err，且不动文件不写 extra
        assert!(mgr.update_display_meta("ghost", None, Some("x")).is_err());
        assert!(mgr
            .update_display_meta("preset-x", None, Some("x"))
            .is_err());
        assert!(store.skill_desc_backup("preset-x").unwrap().is_none());

        // 超长说明（>240）→ 校验先于回写：SKILL.md 字节不变、无备份、无 extra
        let long_desc = "x".repeat(241);
        assert!(mgr
            .update_display_meta("g-skill", None, Some(&long_desc))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            original_md,
            "超长拒绝不得动 SKILL.md"
        );
        assert!(store.skill_desc_backup("g-skill").unwrap().is_none());
        let rec = store.get("g-skill").unwrap().unwrap();
        assert!(
            rec.extra.is_empty(),
            "拒绝路径不得写 extra: {:?}",
            rec.extra
        );

        // 控制字符/换行说明 → 同样先于回写拒绝
        assert!(mgr
            .update_display_meta("g-skill", None, Some("含\n换行"))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            original_md,
            "控制字符拒绝不得动 SKILL.md"
        );
    }

    /// 多技能包 / 无 skills 目录（纯 MCP 包）→ 说明同步跳过，不报错、不动文件、
    /// 不留备份（说明覆盖仍写入 extra）。
    #[test]
    fn writeback_skips_multi_skill_and_non_skill_packages() {
        let tmp = fresh_dir("writeback_skip");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        // 多技能包
        for s in ["s1", "s2"] {
            let dir = tmp.join(format!("bundles/multi/skills/{s}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {s}\n---\n")).unwrap();
        }
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "multi",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();
        // 无 bundles/<id>/skills/ 目录（纯 MCP 包）
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "pure-mcp",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();

        mgr.update_display_meta("multi", None, Some("x")).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.join("bundles/multi/skills/s1/SKILL.md")).unwrap(),
            "---\nname: s1\n---\n",
            "多技能包不得改写"
        );
        assert!(store.skill_desc_backup("multi").unwrap().is_none());
        assert_eq!(
            store.skill_desc_backup("pure-mcp").unwrap().is_none(),
            true,
            "纯 MCP 包不留备份"
        );
        // 覆盖本身仍写入 extra
        assert_eq!(
            crate::features::marketplace::store::display_override(
                &store.get("multi").unwrap().unwrap(),
                crate::features::marketplace::store::EXTRA_DISPLAY_DESCRIPTION
            )
            .as_deref(),
            Some("x")
        );
    }

    /// 展示优先级：extra 覆盖优先于 record.id / SKILL.md frontmatter；清空后回退。
    #[test]
    fn list_skills_prefers_display_overrides_for_uploads() {
        let tmp = fresh_dir("display_override");
        let mgr = SkillMarketplaceManager::with_roots(tmp.clone());
        let skill_dir = tmp.join("bundles/ov-skill/skills/ov-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ov-skill\ndescription: frontmatter 描述\n---\n",
        )
        .unwrap();
        let store =
            crate::features::marketplace::store::BundleStore::with_file(tmp.join("bundles.json"));
        store
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "ov-skill",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "pkg.zip".to_string(),
                    ),
                ),
            )
            .unwrap();

        // 无覆盖：回退现状（title=id，description=frontmatter）
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "ov-skill")
            .expect("上传技能应列出");
        assert_eq!(listed.title, "ov-skill");
        assert_eq!(listed.description, "frontmatter 描述");
        assert_eq!(listed.display_name, None);
        assert_eq!(listed.display_description, None);

        // 覆盖优先
        store
            .set_display_meta("ov-skill", Some("我的天气"), Some("覆盖后的说明"))
            .unwrap();
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "ov-skill")
            .unwrap();
        assert_eq!(listed.title, "我的天气");
        assert_eq!(listed.description, "覆盖后的说明");
        assert_eq!(listed.display_name.as_deref(), Some("我的天气"));
        assert_eq!(listed.display_description.as_deref(), Some("覆盖后的说明"));

        // 清空（trim 空串删 key）→ 回退
        store
            .set_display_meta("ov-skill", Some(" "), Some(" "))
            .unwrap();
        let listed = mgr
            .list_skills()
            .into_iter()
            .find(|s| s.id == "ov-skill")
            .unwrap();
        assert_eq!(listed.title, "ov-skill");
        assert_eq!(listed.description, "frontmatter 描述");
    }
}
