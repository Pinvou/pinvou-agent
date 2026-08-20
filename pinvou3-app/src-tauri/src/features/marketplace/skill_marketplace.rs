//! 技能市场管理器 — 管理 skill(SKILL.md 目录)的安装/卸载/上传导入。
//!
//! 与 MCP 工具市场([`super::marketplace`])刻意分开:MCP 工具是 server 进程(改
//! mcp.json),技能是磁盘上的 SKILL.md 目录(落 `bundle/skills/`,进 system prompt)。
//!
//! 预置技能(government-writing)随 app 编译进二进制(`include_dir`),从嵌入资源复制到
//! `~/.pinvou3/bundle/skills/<name>/`——这是底座聊天**唯一加载**的 pinvou3 私有
//! skill 目录(fork patch #41 砍掉了其余扫描路径)。安装入口为 MCP 工具的配套技能
//! 联动(见 `marketplace::companion_skills`:装「公文写作」gongwen MCP 时一并装
//! government-writing),已无独立「技能」市场页;用户上传 zip 技能包能力保留。
//!
//! 为何不复用底座 `skills::install`:那条通路对 monorepo / 带 plugin.json / 超
//! 5MiB 的仓库一律拒装,且选路逻辑私有硬编码。此处只做"已知来源的精确落盘",
//! 自带等价的路径穿越/symlink/大小安全防护(参照底座 install.rs 的判断)。

use std::io::Read;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};

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
    /// 安装目标:bundle/skills/(底座聊天唯一加载的 pinvou3 私有 skill 目录)
    skills_dir: PathBuf,
}

impl Default for SkillMarketplaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillMarketplaceManager {
    pub fn new() -> Self {
        Self {
            skills_dir: paths::bundle_skills_dir(),
        }
    }

    #[cfg(test)]
    fn with_skills_dir(dir: PathBuf) -> Self {
        Self { skills_dir: dir }
    }

    /// 前端列表:预置技能(带 installed 状态) + 用户上传的技能(扫 bundle/skills/ 里
    /// 带 `.installed-from=upload:` 标记、不在预置清单的目录)。
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
            })
            .collect();

        // 扫上传的(带 upload: 标记、非预置目录名)
        let preset_names: Vec<&str> = presets.iter().map(|m| m.skill_name).collect();
        if let Ok(rd) = std::fs::read_dir(&self.skills_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if preset_names.contains(&name.as_str()) {
                    continue;
                }
                let Ok(marker) = std::fs::read_to_string(path.join(INSTALLED_FROM_MARKER)) else {
                    continue;
                };
                if !marker.starts_with("upload:") {
                    continue;
                }
                out.push(MarketplaceSkillInfo {
                    id: name.clone(),
                    title: name.clone(),
                    // 空 subtitle 让前端回退三语 localized 文案(上传技能无自有副标题)
                    subtitle: String::new(),
                    // 解析 SKILL.md frontmatter description 展示;缺失则空
                    description: read_skill_description(&path.join("SKILL.md")).unwrap_or_default(),
                    icon: "Package".to_string(),
                    color: "bg-gradient-to-b from-slate-400 to-slate-600".to_string(),
                    installed: true,
                    user_uploaded: true,
                });
            }
        }
        out
    }

    fn is_installed(&self, skill_name: &str) -> bool {
        self.skills_dir.join(skill_name).join("SKILL.md").is_file()
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

    /// 安装预置技能:从嵌入资源复制到 `bundle/skills/<name>/`(原子:.tmp → rename)。
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

        std::fs::create_dir_all(&self.skills_dir).map_err(|e| format!("创建 skills 目录: {e}"))?;
        let staged = self.skills_dir.join(format!("{}.tmp", m.skill_name));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(|e| format!("创建暂存目录: {e}"))?;

        let result = (|| -> Result<(), String> {
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
            std::fs::write(
                staged.join(INSTALLED_FROM_MARKER),
                format!("pinvou3-marketplace:{}", m.id),
            )
            .map_err(|e| format!("写标记: {e}"))?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }

        let dest = self.skills_dir.join(m.skill_name);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;
        Ok(())
    }

    /// 卸载:校验 `.installed-from` 标记后删目录(保护内置/手放的 skill 不被误删)。
    pub fn uninstall(&self, skill_id: &str) -> Result<(), String> {
        // 预置 id(pua/nuwa) → skill_name;上传技能 id 即目录名本身。
        let dir_name = self
            .preset(skill_id)
            .map(|m| m.skill_name.to_string())
            .unwrap_or_else(|| skill_id.to_string());
        if !is_safe_skill_name(&dir_name) {
            return Err(format!("非法技能名 '{dir_name}'"));
        }
        let dir = self.skills_dir.join(&dir_name);
        if !dir.join(INSTALLED_FROM_MARKER).is_file() {
            return Err(format!("技能 '{dir_name}' 非市场安装(无标记),拒绝删除"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败: {e}"))?;
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

        // pass2:写出 skill_root 子树到 staged
        std::fs::create_dir_all(&self.skills_dir).map_err(|e| format!("创建 skills 目录: {e}"))?;
        let staged = self.skills_dir.join(format!("{name}.tmp"));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(|e| format!("暂存目录: {e}"))?;
        let prefix = if skill_root.is_empty() {
            String::new()
        } else {
            format!("{skill_root}/")
        };

        let result = (|| -> Result<(), String> {
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
            std::fs::write(
                staged.join(INSTALLED_FROM_MARKER),
                format!("upload:{display_name}"),
            )
            .map_err(|e| format!("写标记: {e}"))?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }

        let dest = self.skills_dir.join(&name);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staged, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("落盘: {e}")
        })?;
        Ok(name)
    }
}

// 辅助 ------------------------------------------------------------------------

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
fn read_skill_name_from_str(content: &str) -> Option<String> {
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
fn skill_md_rank(path: &str) -> Option<usize> {
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
fn skill_root_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn is_safe_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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

    /// 预置 government-writing 从嵌入资源落盘 → list 反映 installed → 卸载删目录的全链路。
    #[test]
    fn install_then_uninstall_preset_roundtrip() {
        let tmp = fresh_dir("roundtrip");
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());

        mgr.install("government-writing").unwrap();
        let skill_dir = tmp.join("government-writing");
        assert!(skill_dir.join("SKILL.md").is_file(), "SKILL.md 应落盘");
        assert!(skill_dir.join(".installed-from").is_file(), "应写安装标记");
        assert!(
            skill_dir.join("templates").is_dir(),
            "templates/ 应一并复制"
        );
        assert_eq!(
            read_skill_name(&skill_dir.join("SKILL.md")).as_deref(),
            Some("government-writing")
        );
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

    /// Visualizer 预置技能带 references/ 子树,安装后必须可被 SkillRegistry 读取。
    #[test]
    fn install_visualizer_preset_with_references() {
        let tmp = fresh_dir("visualizer");
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());

        mgr.install("visualizer").unwrap();
        let skill_dir = tmp.join("visualizer");
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
        assert_eq!(
            std::fs::read_to_string(skill_dir.join(".installed-from"))
                .unwrap()
                .trim(),
            "pinvou3-marketplace:visualizer"
        );
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
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());

        mgr.install("ima-skills").unwrap();
        let skill_dir = tmp.join("ima-skills");
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

    /// 腾讯文档官方 skill(vendored):安装落盘到 frontmatter name(tencent-docs),
    /// 品类参考与 smartcanvas 模板随包;mcporter 依赖脚本(setup.sh/import_file.sh/
    /// ocr.js)不应存在,适配版 get_slide_info.sh 应在。
    /// setup.js(slidep 全局安装脚本)已移除:工作流不调用 slidep CLI,保留一条可被
    /// 诱导执行的无校验和全局安装路径没有收益。
    #[test]
    fn install_tencent_docs_preset_with_official_references() {
        let tmp = fresh_dir("tdoc");
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());

        mgr.install("tencent-docs-skill").unwrap();
        let skill_dir = tmp.join("tencent-docs");
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
        assert!(mgr
            .list_skills()
            .iter()
            .any(|s| s.id == "tencent-docs-skill" && s.installed));

        mgr.uninstall("tencent-docs-skill").unwrap();
        assert!(!skill_dir.exists(), "卸载应删目录");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 没有 `.installed-from` 标记的目录(手放/内置)拒绝卸载,防误删。
    #[test]
    fn uninstall_refuses_unmarked_dir() {
        let tmp = fresh_dir("protect");
        std::fs::create_dir_all(tmp.join("pua")).unwrap();
        std::fs::write(tmp.join("pua").join("SKILL.md"), "---\nname: pua\n---").unwrap();
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());
        assert!(mgr.uninstall("pua").is_err(), "无标记应拒删");
        assert!(tmp.join("pua").exists(), "目录应保留");
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
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());
        mgr.import_package(zip_path.to_str().unwrap()).unwrap();

        let dest = tmp.join("my-test-skill");
        assert!(dest.join("SKILL.md").is_file(), "按 frontmatter name 落盘");
        assert!(dest.join("ref.md").is_file(), "辅助文件应带过来");
        assert!(!dest.join(".git").exists(), ".git 等隐藏目录应跳过");
        let marker = std::fs::read_to_string(dest.join(".installed-from")).unwrap();
        assert!(marker.starts_with("upload:"), "标记应为 upload:");
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

    /// `import_package_named` 用调用方给的 display_name 写 upload: 标记
    /// (拖放字节通道的 zip 名经命令层净化后传入)。
    #[test]
    fn import_package_named_writes_upload_marker() {
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
        let mgr = SkillMarketplaceManager::with_skills_dir(tmp.clone());
        mgr.import_package_named(zip_path.to_str().unwrap(), "my skill.zip")
            .unwrap();
        let marker =
            std::fs::read_to_string(tmp.join("named-skill").join(".installed-from")).unwrap();
        assert_eq!(marker.trim(), "upload:my skill.zip");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
