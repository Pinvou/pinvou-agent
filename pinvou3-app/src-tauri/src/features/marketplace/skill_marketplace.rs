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
//! (`content_fingerprint`)；列出技能（打开工具商店）时按「记录指纹 vs 当前嵌入
//! 资源指纹（上游更新）∪ 磁盘指纹 vs 记录指纹（本地改动）」判定
//! `update_available`，前端显示"更新"按钮,用户确认后走 `install` 的原子覆盖重装(保留启用状态)。
//! 上传技能无嵌入对应物,不参与检测。
//!
//! 为何不复用底座 `skills::install`:那条通路对 monorepo / 带 plugin.json / 超
//! 5MiB 的仓库一律拒装,且选路逻辑私有硬编码。此处只做"已知来源的精确落盘",
//! 自带等价的路径穿越/symlink/大小安全防护(参照底座 install.rs 的判断)。

use std::io::Read;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::platform::paths;

/// 预置技能资源:编译进二进制。每个子目录(pua/ nuwa/)是一个含 SKILL.md 的 skill。
static MARKETPLACE_DIR: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/resources/common/skill-marketplace");

/// 单个 skill 子树未压缩大小上限(防御性,预置/上传都适用)。
/// `pub(crate)`:命令层 `import_skill_package_bytes` 复用同一上限。
pub(crate) const MAX_SKILL_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// 安装来源标记文件名。卸载时校验它存在,避免误删内置/手放的 skill。
const INSTALLED_FROM_MARKER: &str = ".installed-from";

/// 底座每次启动会清掉的已下线 skill 名;安装时拒绝撞名,免得装了被清。
const RETIRED_SKILL_NAMES: &[&str] = &[
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
            description: "把散乱的技能（SKILL.md）、MCP 服务、扳手插件（spanner）或它们的组合整理成 Pinvou 商店可导入的标准插件包：补 plugin.json、补 mcp/manifest.json、补 SKILL.md frontmatter、生成图标、校验命名与布局，最后产出目录或 zip。",
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
}

// 停用开关(按模式 scope 持久化)------------------------------------------------
//
// 技能停用按会话模式 scope 独立持久化到 `~/.pinvou3/disabled_skills.json`
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
    #[cfg(test)]
    fn with_roots(dir: PathBuf) -> Self {
        Self {
            packages_root: dir.join("bundles"),
            legacy_skills_dir: dir.join("bundle/skills"),
            bundle_store: super::store::BundleStore::with_file(dir.join("bundles.json")),
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

    /// 已装技能目录定位：仅新布局（按包聚合 `bundles/<pkg>/skills/<name>`）。
    /// 旧扁平布局 `bundle/skills/` 已退役，不再回退读取（强制迁移后删除）。
    pub(crate) fn find_skill_dir(&self, skill_name: &str) -> Option<PathBuf> {
        let new_path = self.package_skill_dir(skill_name);
        if new_path.is_dir() {
            Some(new_path)
        } else {
            None
        }
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
                    out.push(MarketplaceSkillInfo {
                        id: record.id.clone(),
                        title: record.id.clone(),
                        // 空 subtitle 让前端回退三语 localized 文案(上传技能无自有副标题)
                        subtitle: String::new(),
                        // 解析 SKILL.md frontmatter description 展示;缺失则空
                        description: read_skill_description(&dir.join("SKILL.md"))
                            .unwrap_or_default(),
                        icon: "Package".to_string(),
                        color: "bg-gradient-to-b from-slate-400 to-slate-600".to_string(),
                        installed: true,
                        user_uploaded: true,
                        // 上传技能无嵌入对应物,不参与更新检测
                        update_available: false,
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
        let parent = dest.parent().expect("包目录必有父级");
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

        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;

        // Super-skill 协议后置 hook：扫描 SKILL.md frontmatter 的 `runtime` + `tools` 段。
        // 若完整声明，把 `skills/<name>/scripts/**` 加进 priority_paths（execpolicy 通道
        // 硬兜底），让 skill-run wrapper 能从沙箱列表外进入。
        if let Err(e) = self.register_skill_exec_priority_paths(&dest, m.skill_name) {
            log::warn!(
                "[skill-marketplace] skill-run 注册失败（install {}）: {e}",
                m.id
            );
        }

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

    /// 把已装 skill 目录（`dest`）下的 SKILL.md frontmatter 中声明的可执行能力
    /// 注册到 priority_paths，并刷新 execpolicy 规则集（in-flight 引擎的双向兜底）。
    /// 内容-only skill（无 runtime + tools 段）→ 静默跳过。
    fn register_skill_exec_priority_paths(
        &self,
        skill_dir: &Path,
        skill_name: &str,
    ) -> Result<(), String> {
        let md = skill_dir.join("SKILL.md");
        if !md.is_file() {
            return Ok(()); // 不存在 SKILL.md 一定是裸技能，无 exec 段
        }
        let content = std::fs::read_to_string(&md).map_err(|e| format!("读 SKILL.md: {e}"))?;
        let exec = read_skill_exec_from_str(&content)?;
        if !exec.is_executable() {
            return Ok(()); // 内容-only skill
        }
        // 收集该 skill 下所有 entry 路径（基于 runtime.dir 与 tools[].entry）
        let runtime_dir = exec
            .runtime
            .as_ref()
            .and_then(|r| r.dir.as_deref())
            .map(|d| skill_dir.join(d));
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Some(rd) = runtime_dir.as_ref() {
            if rd.is_dir() {
                paths.push(rd.clone());
            }
        }
        for tool in &exec.tools {
            let entry = tool.entry.trim_start_matches("./");
            let entry_path = skill_dir.join(entry);
            if entry_path.exists() {
                if let Some(parent) = entry_path.parent() {
                    paths.push(parent.to_path_buf());
                }
            }
        }
        if paths.is_empty() {
            return Err(format!(
                "skill '{skill_name}' 声明 runtime+tools 但未找到 entry/runtime 目录"
            ));
        }
        // 把这些路径加进 priority_paths（execpolicy 已知可执行白名单，
        // 引擎 spawn 时绕过常规 deny 名单——本 skill 自身的能力面）。
        crate::platform::paths::add_skill_priority_paths(&paths);
        log::info!(
            "[skill-marketplace] skill '{skill_name}' 已注册 {} 个 priority path",
            paths.len()
        );
        Ok(())
    }

    /// 卸载:删包目录内的技能目录（包目录 = 市场属主证明，无需再验标记）；
    /// 旧扁平布局残留要求带 `.installed-from` 标记才删（保护内置/手放目录）。
    pub fn uninstall(&self, skill_id: &str) -> Result<(), String> {
        // 预置 id(pua/nuwa) → skill_name;上传技能 id 即目录名本身。
        let dir_name = self
            .preset(skill_id)
            .map(|m| m.skill_name.to_string())
            .unwrap_or_else(|| skill_id.to_string());
        if !is_safe_skill_name(&dir_name) {
            return Err(format!("非法技能名 '{dir_name}'"));
        }
        let dir = self.package_skill_dir(&dir_name);
        if !dir.is_dir() {
            // 旧布局残留（迁移未跑/失败）：沿用标记保护语义删除
            let legacy = self.legacy_skills_dir.join(&dir_name);
            if legacy.is_dir() && legacy.join(INSTALLED_FROM_MARKER).is_file() {
                std::fs::remove_dir_all(&legacy).map_err(|e| format!("删除失败: {e}"))?;
                if let Err(e) = self.bundle_store.remove(skill_id) {
                    log::warn!("[skill-marketplace] bundles.json 镜像删除失败（uninstall {skill_id}）: {e}");
                }
                return Ok(());
            }
            return Err(format!("技能 '{dir_name}' 非市场安装(不在包目录),拒绝删除"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败: {e}"))?;
        // 清理腾空的父目录（skills/ 空 → 删；独立包的 bundles/<id>/ 空 → 删；
        // companion 的包目录还装着 MCP 资源，非空自然保留）
        if let Some(skills_parent) = dir.parent() {
            let _ = std::fs::remove_dir(skills_parent); // 仅空目录能删掉
            if let Some(pkg_dir) = skills_parent.parent() {
                let _ = std::fs::remove_dir(pkg_dir);
            }
        }
        // 镜像删除（预置 id 即记录 id；上传技能 id = 目录名，与 install 的登记口径一致）。
        if let Err(e) = self.bundle_store.remove(skill_id) {
            log::warn!(
                "[skill-marketplace] bundles.json 镜像删除失败（uninstall {skill_id}）: {e}"
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

        // pass1:逐 entry 安全校验 + 累计大小 + 找最优 SKILL.md(定 skill_root)。
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
            let mut buf = String::new();
            md.read_to_string(&mut buf)
                .map_err(|e| format!("读 SKILL.md: {e}"))?;
            read_skill_name_from_str(&buf).ok_or("SKILL.md 缺 name 字段")?
        };
        if !is_safe_skill_name(&name) {
            return Err(format!("非法技能名 '{name}'"));
        }
        if RETIRED_SKILL_NAMES.contains(&name.as_str()) {
            return Err(format!("技能名 '{name}' 与已下线内置冲突,拒绝"));
        }

        // pass2:写出 skill_root 子树到 staged（上传技能独立成包：bundles/<name>/skills/）
        let dest = self.packages_root.join(&name).join("skills").join(&name);
        let parent = dest.parent().expect("包目录必有父级");
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
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("读条目: {e}"))?;
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

        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;
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

    /// 扁平技能布局（`bundle/skills/<name>/`）→ 按包聚合（`bundles/<pkg>/skills/
    /// <name>/`）的一次性迁移（§9.1），由启动路径（runtime_bundle ensure_extracted，
    /// import_legacy 之后）调用。幂等：旧位置不在即 no-op。
    ///
    /// - 市场技能（带 `.installed-from` 标记）→ 移动到所属包目录；预置技能迁移后
    ///   把目录指纹补写进既有 BundleStore 记录（update_available 的比对基准）；
    /// - CLI companion（无标记，内置清单目录名）：连接器当前可见才移动；不可见 =
    ///   断开后的残留，按门控语义删除（immutable 资源，重连重解包，无用户数据）；
    /// - 无标记的其它目录（内置释放技能 visual-design、手放目录）→ 不动；
    /// - 目标已存在或 rename 失败 → 保留旧位置并 warn（读路径 `find_skill_dir`
    ///   对旧位置有回退，迁移下个启动周期自愈）。
    pub fn migrate_flat_skills_layout(&self) -> SkillsMigrationReport {
        let mut report = SkillsMigrationReport::default();
        let Ok(rd) = std::fs::read_dir(&self.legacy_skills_dir) else {
            return report;
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
            let marker = std::fs::read_to_string(dir.join(INSTALLED_FROM_MARKER))
                .unwrap_or_default()
                .trim()
                .to_string();
            if marker.is_empty() {
                if let Some(cli) = super::bundle::cli_bundle_of_skill(&name) {
                    if crate::platform::connector_state::skills_visible_for(cli) {
                        self.move_skill_dir(&dir, &name, &mut report);
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
            if self.move_skill_dir(&dir, &name, &mut report) && is_preset {
                // 补写指纹到既有记录（import_legacy 先跑，记录应已存在；
                // 不存在则不擅自新建——异常态留给下一周期）
                if let Ok(fp) = dir_fingerprint(&self.package_skill_dir(&name)) {
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

    /// 移动单个技能目录到所属包目录。返回是否完成移动。
    fn move_skill_dir(&self, dir: &Path, name: &str, report: &mut SkillsMigrationReport) -> bool {
        let target = self.package_skill_dir(name);
        if target.exists() {
            log::warn!(
                "[skill-marketplace] 迁移跳过 {name}：目标已存在（{}），保留旧位置 {}",
                target.display(),
                dir.display()
            );
            report.kept.push(name.to_string());
            return false;
        }
        let parent = target.parent().expect("包目录必有父级");
        let result = std::fs::create_dir_all(parent).and_then(|()| std::fs::rename(dir, &target));
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
    /// 未动：内置释放技能 / 手放目录 / 迁移失败保留旧位置
    pub kept: Vec<String>,
}

// 辅助 ------------------------------------------------------------------------

/// 收集嵌入资源子树的 `(相对路径, 内容)` 列表,供更新检测比对。
/// 口径与 [`extract_embedded_subdir`] 一致:strip `source_dir` 前缀、跳过 SOURCE.md。
fn collect_embedded_files(dir: &Dir<'_>, source_dir: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let prefix = format!("{source_dir}/");
    for file in dir.files() {
        let p = file.path().to_string_lossy();
        let rel = p.strip_prefix(&prefix).unwrap_or(&p);
        if Path::new(rel).file_name().and_then(|s| s.to_str()) == Some("SOURCE.md") {
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
fn extract_embedded_subdir(dir: &Dir<'_>, source_dir: &str, dest: &Path) -> std::io::Result<()> {
    let prefix = format!("{source_dir}/");
    for file in dir.files() {
        let p = file.path().to_string_lossy();
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
    read_skill_description_from_str(&std::fs::read_to_string(md_path).ok()?)
}

/// 解析 SKILL.md frontmatter 的 `description:`(仅展示用)。支持单行(含引号)与
/// `|`/`>` 块状(取块内非空行,折叠拼接为单行);缺失/空 → None;超 240 字截断。
fn read_skill_description_from_str(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    // by_ref:块状分支里还要接着消费同一个迭代器
    for line in lines.by_ref() {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(rest) = t.strip_prefix("description:") {
            let v = rest.trim();
            if v.is_empty() {
                return None;
            }
            if v == "|" || v == ">" {
                // 块状:收集后续缩进行,空行跳过,遇顶层字段(无缩进)结束
                let mut parts: Vec<String> = Vec::new();
                let mut total: usize = 0;
                for l in lines {
                    let lt = l.trim();
                    if lt.is_empty() {
                        continue;
                    }
                    let indent = l.len() - l.trim_start().len();
                    if indent == 0 {
                        break;
                    }
                    total += lt.chars().count();
                    parts.push(lt.to_string());
                    if total > 240 {
                        break;
                    }
                }
                let s = parts.join(" ");
                return if s.trim().is_empty() {
                    None
                } else {
                    Some(s.trim().chars().take(240).collect())
                };
            }
            let v = v.trim_matches('"').trim_matches('\'').trim();
            return if v.is_empty() {
                None
            } else {
                Some(v.chars().take(240).collect())
            };
        }
    }
    None
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

// =====================================================================
// Super-skill 协议：SKILL.md frontmatter 增加可选 `runtime` + `tools[]` 段，
// 让 skill 包承载可执行能力（取代老的 spanner 独立组件模型）。模型读完 SKILL.md
// 后看到「本 skill 有 tools：通过 skill-run <tool-name> '<json-args>' 调用」，
// stdout 必为合法 JSON。
//
// 已下线（保留作旧 plugin.json 反序列化兜底字段）：plugin.json 中的 `spanner` 字段
// 在 plugin_import.rs 通过 `extra` map 兜住，丢给类型丢弃——旧上传包的 legacy data
// 不会炸。仅作前向兼容读取，不再有对应的执行通路。

/// Skill 运行时声明（语言不限）。对应 SKILL.md frontmatter `runtime:` 段：
/// ```yaml
/// runtime:
///   kind: python        # python | python3 | node | nodejs | deno | ...
///   dir: runtime        # 可选，自带运行时目录相对 skill 根
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRuntimeSpec {
    pub kind: String,
    #[serde(default)]
    pub dir: Option<String>,
}

/// Skill 可执行工具声明。对应 frontmatter `tools:` 数组元素：
/// ```yaml
/// tools:
///   - name: generate_html
///     entry: scripts/generate.py
///     input_schema: {type: object, properties: {prompt: {type: string}}, required: [prompt]}
///     output_schema: {type: object}              # 可选
///     timeout_secs: 30                         # 可选，默认 20
///     background: false                        # 可选
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillToolSpec {
    pub name: String,
    pub entry: String,
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub background: Option<bool>,
}

/// skill 包的「可执行能力」声明集合。对应 frontmatter `runtime` + `tools` 段；
/// 两个同时存在才算完整（缺 runtime 的 tools 不可调用，缺 tools 的 runtime 无意义）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillExecSpec {
    #[serde(default)]
    pub runtime: Option<SkillRuntimeSpec>,
    #[serde(default)]
    pub tools: Vec<SkillToolSpec>,
}

impl SkillExecSpec {
    /// 是否声明了可执行能力（=runtime + 非空 tools）。
    pub fn is_executable(&self) -> bool {
        self.runtime.is_some() && !self.tools.is_empty()
    }
}

/// 解析 SKILL.md YAML frontmatter 中 `runtime` 与 `tools` 段。
///
/// 这是 YAML 1.2 的最小子集解析器（不引 serde_yaml），理由：避免额外依赖、代码可
/// 控、格式由本协议定义。前后由 `\n---\n` 标记包裹；frontmatter 不存在或这两段都不存
/// 在 → 返回 Ok(Default)（"无 exec 段"是合法）。
pub fn read_skill_exec_from_str(content: &str) -> Result<SkillExecSpec, String> {
    // 1) 抽取 frontmatter 段
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(SkillExecSpec::default());
    }
    let mut yaml_lines: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim_start() == "---" {
            closed = true;
            break;
        }
        yaml_lines.push(line);
    }
    if !closed {
        return Ok(SkillExecSpec::default());
    }
    if yaml_lines.is_empty() {
        return Ok(SkillExecSpec::default());
    }

    // 2) 极简 YAML：找到 `runtime:` 与 `tools:` 顶层段，裁出原始行（YAML 是缩进敏感
    //    的，我们只识别顶层无缩进 `key:` 与 `  - entry:` / `  - name:` 等）。
    let mut runtime: Option<SkillRuntimeSpec> = None;
    let mut tools: Vec<SkillToolSpec> = Vec::new();
    let mut i = 0;
    while i < yaml_lines.len() {
        let line = yaml_lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        // 顶层段 = 行首非空白 + 以 `key:` 结尾
        if line.starts_with(char::is_whitespace) == false && trimmed.ends_with(':') {
            let key = trimmed.trim_end_matches(':').trim();
            i += 1;
            if key == "runtime" {
                // 收集后续缩进行直到下一个顶层段或 EOF
                let mut block: Vec<&str> = Vec::new();
                while i < yaml_lines.len() {
                    let cur = yaml_lines[i];
                    if !cur.starts_with(' ') && !cur.starts_with('\t') && cur.trim().ends_with(':')
                    {
                        break;
                    }
                    if !cur.trim().is_empty() {
                        block.push(cur);
                    }
                    i += 1;
                }
                runtime = parse_runtime_block(&block)?;
            } else if key == "tools" {
                // tools 是数组：扫描 `- ` 起始项，每项收集缩进行直到下一个 `- `
                let mut item: Vec<String> = Vec::new();
                while i < yaml_lines.len() {
                    let cur = yaml_lines[i];
                    if cur.trim_start().starts_with("- ") {
                        if !item.is_empty() {
                            let parsed = parse_tool_item(&item)?;
                            tools.push(parsed);
                            item.clear();
                        }
                        item.push(cur.to_string());
                    } else if cur.starts_with(' ') || cur.starts_with('\t') {
                        if !item.is_empty() {
                            item.push(cur.to_string());
                        } else {
                            break;
                        }
                    } else if cur.trim().is_empty() {
                        i += 1;
                        continue;
                    } else {
                        break;
                    }
                    i += 1;
                }
                if !item.is_empty() {
                    let parsed = parse_tool_item(&item)?;
                    tools.push(parsed);
                }
            } else {
                // 未关注的段，跳过其缩进体
                while i < yaml_lines.len() {
                    let cur = yaml_lines[i];
                    if !cur.starts_with(' ') && !cur.starts_with('\t') && cur.trim().ends_with(':')
                    {
                        break;
                    }
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }

    Ok(SkillExecSpec { runtime, tools })
}

fn parse_runtime_block(lines: &[&str]) -> Result<Option<SkillRuntimeSpec>, String> {
    let mut kind: Option<String> = None;
    let mut dir: Option<String> = None;
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("kind:") {
            kind = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("dir:") {
            dir = Some(unquote_yaml_scalar(rest.trim()));
        }
    }
    let kind = match kind {
        Some(k) if !k.is_empty() => k,
        _ => return Ok(None), // 无 runtime 段 → Ok(None)，不是错
    };
    Ok(Some(SkillRuntimeSpec { kind, dir }))
}

fn parse_tool_item(lines: &[String]) -> Result<SkillToolSpec, String> {
    let mut name: Option<String> = None;
    let mut entry: Option<String> = None;
    let mut input_schema: Option<serde_json::Value> = None;
    let mut output_schema: Option<serde_json::Value> = None;
    let mut timeout_secs: Option<u64> = None;
    let mut background: Option<bool> = None;
    for raw in lines {
        let trimmed = raw.trim();
        // 去掉 `- ` 列表前缀
        let body = if let Some(stripped) = trimmed.strip_prefix("- ") {
            stripped
        } else {
            trimmed
        };
        if let Some(rest) = body.strip_prefix("name:") {
            name = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = body.strip_prefix("entry:") {
            entry = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = body.strip_prefix("input_schema:") {
            input_schema = Some(parse_inline_yaml_value(rest.trim())?);
        } else if let Some(rest) = body.strip_prefix("output_schema:") {
            output_schema = Some(parse_inline_yaml_value(rest.trim())?);
        } else if let Some(rest) = body.strip_prefix("timeout_secs:") {
            timeout_secs = rest.trim().parse::<u64>().ok();
        } else if let Some(rest) = body.strip_prefix("background:") {
            background = rest.trim().parse::<bool>().ok();
        }
    }
    let name = name.ok_or_else(|| "tools[] 缺 name".to_string())?;
    let entry = entry.ok_or_else(|| format!("tool '{name}' 缺 entry"))?;
    Ok(SkillToolSpec {
        name,
        entry,
        input_schema,
        output_schema,
        timeout_secs,
        background,
    })
}

fn unquote_yaml_scalar(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// 极简 YAML 标量解析：支持 `null` / `true` / `false` / 整数 / 字符串。
/// 复杂结构（多行 mapping / 嵌套）不支持——本协议规定 input_schema/output_schema
/// 写一行 JSON 字面量最简单。
fn parse_inline_yaml_value(s: &str) -> Result<serde_json::Value, String> {
    let trimmed = s.trim();
    if trimmed == "null" || trimmed == "~" || trimmed.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    if trimmed == "true" {
        return Ok(serde_json::Value::Bool(true));
    }
    if trimmed == "false" {
        return Ok(serde_json::Value::Bool(false));
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(serde_json::Value::Number(n.into()));
    }
    // 否则按裸字符串
    Ok(serde_json::Value::String(unquote_yaml_scalar(trimmed)))
}

#[cfg(test)]
mod super_skill_tests {
    use super::*;

    fn sample_md(body: &str) -> String {
        format!("---\nname: pua\n{body}\n---\n# skill body\n")
    }

    #[test]
    fn read_skill_exec_no_frontmatter() {
        // 无 frontmatter → Default
        let md = "# just body\n";
        let exec = read_skill_exec_from_str(md).unwrap();
        assert!(!exec.is_executable());
    }

    #[test]
    fn read_skill_exec_runtime_only_no_tools() {
        let md = sample_md("runtime:\n  kind: python\n");
        let exec = read_skill_exec_from_str(&md).unwrap();
        assert!(!exec.is_executable(), "缺 tools 不算可执行");
        assert_eq!(exec.runtime.as_ref().unwrap().kind, "python");
    }

    #[test]
    fn read_skill_exec_full() {
        let md = sample_md(
            "runtime:\n  kind: python\n  dir: runtime\ntools:\n  - name: generate_html\n    entry: scripts/generate.py\n    input_schema: {type: object, properties: {prompt: {type: string}}}\n    timeout_secs: 30\n  - name: render\n    entry: scripts/render.py\n",
        );
        let exec = read_skill_exec_from_str(&md).unwrap();
        assert!(exec.is_executable());
        assert_eq!(exec.runtime.as_ref().unwrap().kind, "python");
        assert_eq!(exec.runtime.as_ref().unwrap().dir.as_deref(), Some("runtime"));
        assert_eq!(exec.tools.len(), 2);
        assert_eq!(exec.tools[0].name, "generate_html");
        assert_eq!(exec.tools[0].entry, "scripts/generate.py");
        assert_eq!(exec.tools[0].timeout_secs, Some(30));
        assert_eq!(exec.tools[1].name, "render");
    }

    #[test]
    fn read_skill_exec_malformed_returns_err() {
        let md = "---\ntools:\n  - entry: x.py\n---\n";  // 缺 name
        let err = read_skill_exec_from_str(&md).unwrap_err();
        assert!(err.contains("name"), "err: {err}");
    }

    #[test]
    fn priority_paths_roundtrip() {
        use std::path::PathBuf;
        let _g = paths::tests::ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var(
            "PINVOU3_HOME",
            std::env::temp_dir().join(format!("pinvou3-skill-paths-{}", std::process::id())),
        );
        // 初始为空
        assert!(paths::skill_priority_paths().is_empty());
        // 加两个 + 重复一个（去重）
        paths::add_skill_priority_paths(&[
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/a"),
        ]);
        let got = paths::skill_priority_paths();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&PathBuf::from("/tmp/a")));
        assert!(got.contains(&PathBuf::from("/tmp/b")));
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // | 块状:非空行折叠拼接,空行跳过,遇顶层字段结束
        assert_eq!(
            read_skill_description_from_str(
                "---\ndescription: |\n  第一行\n\n  第二行\nname: x\n---\n"
            )
            .as_deref(),
            Some("第一行 第二行")
        );
        // > 块状
        assert_eq!(
            read_skill_description_from_str("---\ndescription: >\n  fold\n  ed\n---\n").as_deref(),
            Some("fold ed")
        );
        // 缺失 / 空 / 无 frontmatter
        assert!(read_skill_description_from_str("---\nname: x\n---\n").is_none());
        assert!(read_skill_description_from_str("---\ndescription: ''\n---\n").is_none());
        assert!(read_skill_description_from_str("no frontmatter").is_none());
        // 超长截断到 240 字符
        let long = format!("---\ndescription: {}\n---\n", "字".repeat(300));
        assert_eq!(
            read_skill_description_from_str(&long)
                .unwrap()
                .chars()
                .count(),
            240
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
        assert!(store
            .get("government-writing")
            .unwrap()
            .expect("install 应登记")
            .content_fingerprint
            .is_some());
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "government-writing" && s.installed));

        mgr.uninstall("government-writing").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "government-writing" && !s.installed));
        let _ = std::fs::remove_dir_all(&tmp);
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
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "pptx" && s.installed));

        mgr.uninstall("pptx").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "pptx" && !s.installed));
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
        let record = store.get("visualizer").unwrap().expect("install 应登记");
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
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "visualizer" && s.installed));

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
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "ima-skills" && s.installed));

        mgr.uninstall("ima-skills").unwrap();
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

    /// 更新检测（指纹口径）：刚安装（记录=嵌入=磁盘）→ false；本地篡改/删除/新增
    /// 文件（磁盘 ≠ 记录）→ true；模拟上游更新（记录指纹滞后于嵌入资源）→ true；
    /// 重装后恢复 false。未安装恒 false。
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
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧扁平布局 → 按包聚合迁移：四个分支（纯技能 / MCP companion / CLI companion /
    /// 上传）+ 内置技能不动 + 预置指纹补写 + 幂等。
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
            // gongwen manifest 声明 companion（companion 分支的归属依据）
            seed(
                "bundle/mcp-servers/gongwen/manifest.json",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"companion_skills":["government-writing"]}"#,
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
            // MCP companion
            seed(
                "bundle/skills/government-writing/SKILL.md",
                "---\nname: government-writing\n---\n",
            );
            seed(
                "bundle/skills/government-writing/.installed-from",
                "pinvou3-marketplace:government-writing",
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
                "companion → 所属 MCP 包目录"
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
            assert!(report.moved.len() == 4, "应移动 4 个: {report:?}");
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
}
