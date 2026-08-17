//! 插件包统一导入：spanner / mcp / skill / 组合包 + 图标落盘（plugin-protocol.md）。
//!
//! 统一上传路径：用户上传一个 zip，无论内容是哪种能力类型（spanner 扳手插件、
//! MCP server、skill、或它们的组合），都自动识别 → 安全校验 → 落盘到按包聚合的
//! `bundles/<id>/`，并在商店与运行时按同一套「包」模型读取/开关/卸载。
//!
//! zip 形态（plugin-protocol §3/§15）：
//! ```text
//! my-plugin.zip
//! ├── plugin.json        ← 权威声明（可选；组合包/凭据/元数据时必需）
//! ├── mcp/               ← MCP server（manifest.json + server.py）
//! ├── skills/<name>/     ← SKILL.md 目录（可多个）
//! ├── spanner/              ← 扳手插件（main.py：stdin JSON → stdout JSON）
//! ├── runtime/           ← 可选自带运行时
//! └── icon.svg|png       ← 可选图标
//! ```
//!
//! 图标：包内可选 `icon.svg`/`icon.png` → 落盘 `bundles/<id>/icon.<ext>`；缺省 →
//! 落盘内置默认 `icon.svg`。已装工具图标与工具同目录。

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::features::marketplace::bundle::BundleKind;

/// 插件包解压累计上限（自带运行时可能较大，放宽到 200 MiB）。
pub(crate) const MAX_PLUGIN_SIZE_BYTES: u64 = 200 * 1024 * 1024;

/// plugin.json 清单（插件包的权威声明）。未知字段 flatten 保留（前向兼容）。
/// 可执行能力现在通过 skill 包的 SKILL.md frontmatter `tools[]` + `runtime` 段声明，
/// 不再有「spanner 独立组件」入口——见 skill_marketplace.rs 与 skill-run wrapper。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// 可选图标，相对 zip 根（"icon.svg"/"icon.png"）。
    #[serde(default)]
    pub icon: Option<String>,
    /// 多组件声明（mcp_servers / skills）。脚本可执行能力迁移到 skill 包内：
    /// skill 根目录的 SKILL.md frontmatter `tools[]` + `runtime` 字段声明可执行入口。
    #[serde(default)]
    pub components: Option<PluginComponents>,
    /// 未知字段原样保留（前向兼容旧 spanner 字段）。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// plugin.json 的 `components` 声明（跨组件粘合到一个包 id）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginComponents {
    #[serde(default)]
    pub mcp_servers: Vec<ComponentRef>,
    #[serde(default)]
    pub skills: Vec<ComponentRef>,
}

/// 组件目录引用：`dir` 相对 zip 根，导入时校验存在。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRef {
    pub id: String,
    pub dir: String,
}

/// 图标扩展名 → 是否允许（只认 svg/png）。
pub fn is_supported_icon(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".svg") || lower.ends_with(".png")
}

/// 默认图标（lucide `package`，无品牌依赖的通用「工具包」图形）。
pub const DEFAULT_ICON_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/></svg>"#;

/// 落盘默认图标到包目录（`bundles/<id>/icon.svg`）。返回相对路径 "icon.svg"。
pub fn write_default_icon(pkg_dir: &Path) -> Result<String, String> {
    let icon_path = pkg_dir.join("icon.svg");
    std::fs::create_dir_all(pkg_dir).map_err(|e| format!("创建包目录: {e}"))?;
    std::fs::write(&icon_path, DEFAULT_ICON_SVG).map_err(|e| format!("写默认图标: {e}"))?;
    Ok("icon.svg".to_string())
}

/// 把图标字节落盘到包目录（`icon.<ext>`，ext 取自原文件名）。返回相对路径。
///
/// file_name 必须是无路径分隔符的纯文件名（不接受 `a/icon.png` / `../icon.svg` 等
/// 路径形式），扩展名限定为 `svg`/`png`。
pub fn write_icon_bytes(pkg_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<String, String> {
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(format!("非法图标文件名 '{file_name}'"));
    }
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| *e == "svg" || *e == "png")
        .ok_or_else(|| format!("不支持的图标格式 '{file_name}'"))?;
    let rel = format!("icon.{ext}");
    std::fs::create_dir_all(pkg_dir).map_err(|e| format!("创建包目录: {e}"))?;
    std::fs::write(pkg_dir.join(&rel), bytes).map_err(|e| format!("写图标: {e}"))?;
    Ok(rel)
}

/// 组件/包 id 安全校验：`[a-z0-9-_]{1,64}`，禁 `.`/路径分隔符。
pub fn is_safe_component_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// 目标包目录与本次导入是否「同一包」：带 plugin.json 的包按 plugin.json 字节
/// 比对；裸技能包（无 plugin.json）按落盘后的 SKILL.md（`skills/<id>/SKILL.md`）
/// 比对。任一主内容一致即视为同包重导（允许原子替换），否则视为不同包冲突。
fn same_package_content(
    pkg_dir: &std::path::Path,
    incoming_plugin: Option<&[u8]>,
    incoming_skill: Option<&(String, Vec<u8>)>,
) -> bool {
    if let Some(bytes) = incoming_plugin {
        return std::fs::read(pkg_dir.join("plugin.json"))
            .map(|existing| existing.as_slice() == bytes)
            .unwrap_or(false);
    }
    if let Some((_, bytes)) = incoming_skill {
        let name = pkg_dir.file_name().map(|s| s.to_string_lossy().into_owned());
        if let Some(name) = name {
            let skill_md = pkg_dir.join("skills").join(name).join("SKILL.md");
            return std::fs::read(&skill_md)
                .map(|existing| existing.as_slice() == bytes)
                .unwrap_or(false);
        }
    }
    false
}

/// 插件导入报告（统一上传路径的返回）。
#[derive(Debug, Clone)]
pub struct PluginImportReport {
    pub id: String,
    pub kind: BundleKind,
    /// 落盘后的图标相对路径（`icon.svg`/`icon.png`）。
    pub icon: String,
}

/// 组件识别结果（含无 plugin.json 时的裸技能/裸 MCP 回退根，供落盘规范化）。
struct ComponentDetection {
    id: String,
    mcp_servers: Vec<String>,
    skills: Vec<String>,
    /// 裸技能回退：(skill_root, name)。`skill_root=""` 表示根级 SKILL.md。
    bare_skill: Option<(String, String)>,
    /// 裸 MCP 回退：(mcp_root, mcp_id)。`mcp_root=""` 表示根级 manifest.json。
    bare_mcp: Option<(String, String)>,
}

/// 识别组件向量 + 包 id。
///
/// 有 `plugin.json` → 按 `components`（mcp_servers/skills）声明，逐组件校验目录存在；
/// 无 `plugin.json` → 结构回退（`mcp/manifest.json` → MCP、`skills/*/SKILL.md` → skills），
/// 并对「符合 skills 标准」的裸技能包（任意位置 SKILL.md，frontmatter name）
/// 与「符合 MCP 标准」的裸 MCP 包（任意位置 manifest.json 能解析出 ToolManifest）做回退
/// 兼容，落盘时规范化为本项目的 `skills/<name>/` / `mcp/` 布局。
///
/// 注意：可执行能力（曾经的 spanner）现在并入 skill 包 —— 检测脚本 + runtime 看
/// `skill_marketplace::install` 的 `tools` 段处理，此处不再识别独立「spanner」组件。
fn detect_components(
    manifest: &Option<PluginManifest>,
    mcp_manifest_bytes: &Option<Vec<u8>>,
    best_skill_md: &Option<(String, Vec<u8>)>,
    other_manifests: &[(String, Vec<u8>)],
    all_paths: &[String],
) -> Result<ComponentDetection, String> {
    // 有 manifest：声明优先。
    if let Some(m) = manifest {
        let id = m.id.clone();
        if !is_safe_component_id(&id) {
            return Err(format!("非法包 id '{id}'"));
        }
        let mut mcp = Vec::new();
        let mut skills = Vec::new();
        if let Some(comps) = &m.components {
            for c in &comps.mcp_servers {
                let dir = c.dir.trim_end_matches('/');
                if !all_paths.iter().any(|p| p.starts_with(&format!("{dir}/"))) {
                    return Err(format!("mcp 组件目录 '{dir}' 不存在"));
                }
                mcp.push(c.id.clone());
            }
            for c in &comps.skills {
                let dir = c.dir.trim_end_matches('/');
                if !all_paths.iter().any(|p| p == &format!("{dir}/SKILL.md")) {
                    return Err(format!("技能组件目录 '{dir}' 缺 SKILL.md"));
                }
                skills.push(c.id.clone());
            }
        }
        return Ok(ComponentDetection {
            id,
            mcp_servers: mcp,
            skills,
            bare_skill: None,
            bare_mcp: None,
        });
    }

    // 无 manifest：结构回退。
    let mut mcp = Vec::new();
    let mut skills = Vec::new();
    let mut id = String::new();
    let mut bare_skill = None;
    let mut bare_mcp = None;

    // 1) MCP：优先 `mcp/manifest.json`；否则回退任意 manifest.json（符合 MCP 标准的
    //    裸包——能解析出 ToolManifest 且声明了启动命令或远程 server）。
    if let Some(bytes) = mcp_manifest_bytes {
        if let Ok(tm) = serde_json::from_str::<crate::features::marketplace::ToolManifest>(
            std::str::from_utf8(bytes).unwrap_or(""),
        ) {
            if !tm.id.is_empty() {
                id = tm.id.clone();
                mcp.push(tm.id);
            }
        }
    }
    if mcp.is_empty() {
        for (path, bytes) in other_manifests {
            if let Ok(tm) = serde_json::from_str::<crate::features::marketplace::ToolManifest>(
                std::str::from_utf8(bytes).unwrap_or(""),
            ) {
                if !tm.id.is_empty() && (!tm.command.is_empty() || !tm.servers.is_empty()) {
                    id = tm.id.clone();
                    mcp.push(tm.id.clone());
                    bare_mcp = Some((super::skill_marketplace::skill_root_of(path), tm.id));
                    break;
                }
            }
        }
    }

    // 2) Skills：优先 `skills/<name>/SKILL.md`；否则回退任意 SKILL.md（裸技能，name 取
    //    frontmatter，与既有技能导入同一口径）。
    for p in all_paths {
        if let Some(r) = p.strip_prefix("skills/") {
            if let Some(name) = r.strip_suffix("/SKILL.md") {
                if !name.is_empty() && !name.contains('/') && !skills.iter().any(|s| s == name) {
                    skills.push(name.to_string());
                    if id.is_empty() {
                        id = name.to_string();
                    }
                }
            }
        }
    }
    if skills.is_empty() {
        if let Some((md_path, bytes)) = best_skill_md {
            if let Some(name) = super::skill_marketplace::read_skill_name_from_str(
                std::str::from_utf8(bytes).unwrap_or(""),
            ) {
                if super::skill_marketplace::is_safe_skill_name(&name) {
                    bare_skill = Some((
                        super::skill_marketplace::skill_root_of(md_path),
                        name.clone(),
                    ));
                    skills.push(name.clone());
                    if id.is_empty() {
                        id = name;
                    }
                }
            }
        }
    }

    if mcp.is_empty() && skills.is_empty() {
        return Err("插件包不含任何组件（空包）".to_string());
    }
    if id.is_empty() || !super::skill_marketplace::is_safe_skill_name(&id) {
        return Err(format!("非法包 id '{id}'"));
    }
    Ok(ComponentDetection {
        id,
        mcp_servers: mcp,
        skills,
        bare_skill,
        bare_mcp,
    })
}

/// 计算单个 zip 条目落盘的目标 `(subdir, rel)`（None = 跳过：plugin.json / 图标单独处理）。
///
/// 优先级：
/// 1. 固定前缀 `mcp/` / `skills/` / `spanner/` / `runtime/`（本项目标准布局）；
/// 2. 裸 MCP 回退根（无 plugin.json 时，manifest.json 所在目录 → `mcp/`）；
/// 3. 裸技能回退根（无 plugin.json 时，SKILL.md 所在目录 → `skills/<name>/`）。
///
/// 根级（root=""）裸包的消歧：裸 MCP 只认 `manifest.json` 与同目录非 `.md` 文件
/// （服务器代码/依赖），裸技能认其余（SKILL.md 及资源），避免两者互相抢占。
fn landing_target(
    path_str: &str,
    bare_skill: Option<(&str, &str)>,
    bare_mcp: Option<(&str, &str)>,
) -> Option<(String, String)> {
    // 顶层元数据单独处理（plugin.json / icon 已由 pass1 捕获）。
    if path_str == "plugin.json" || path_str == "icon.svg" || path_str == "icon.png" {
        return None;
    }
    if let Some(r) = path_str.strip_prefix("mcp/") {
        return Some(("mcp".to_string(), r.to_string()));
    }
    if let Some(r) = path_str.strip_prefix("skills/") {
        return Some(("skills".to_string(), r.to_string()));
    }
    // 注：原 `spanner/` 前缀分支已删除——脚本可执行能力通过 skill 包的
    // SKILL.md frontmatter `tools[]` 段声明，由 skill_marketplace::install
    // 后置 hook 注册 skill-run wrapper；不再有独立的 spanner 子目录布局。

    // 裸 MCP 回退。
    if let Some((root, _mcp_id)) = bare_mcp {
        if root.is_empty() {
            // 根级裸 MCP：认 manifest.json 与同目录非 .md 文件（服务器代码/依赖）。
            if path_str == "manifest.json" || !path_str.to_ascii_lowercase().ends_with(".md") {
                return Some(("mcp".to_string(), path_str.to_string()));
            }
            return None;
        } else if let Some(r) = path_str.strip_prefix(&format!("{root}/")) {
            return Some(("mcp".to_string(), r.to_string()));
        }
    }

    // 裸技能回退。
    if let Some((root, name)) = bare_skill {
        if root.is_empty() {
            // 根级裸技能：认 SKILL.md 与其余资源（含 .md）；非 .md 且已有裸 MCP 时
            // 让位给 MCP（服务器代码/依赖归 mcp/）。
            if !path_str.to_ascii_lowercase().ends_with(".md") && bare_mcp.is_some() {
                return None;
            }
            return Some(("skills".to_string(), format!("{name}/{path_str}")));
        } else if let Some(r) = path_str.strip_prefix(&format!("{root}/")) {
            return Some(("skills".to_string(), format!("{name}/{r}")));
        }
    }

    None
}

/// 为无 plugin.json 的裸包合成一份最小规范化清单（自描述，§5.2 派生 manifest），
/// 落盘 `bundles/<id>/plugin.json`。组件目录统一用规范化布局（`mcp/`、`skills/<name>/`），
/// 与落盘结果一致，保证后续重装/卸载/枚举按同一口径读。
fn synthesized_manifest(det: &ComponentDetection) -> PluginManifest {
    let mut comps = PluginComponents::default();
    for id in &det.mcp_servers {
        comps.mcp_servers.push(ComponentRef {
            id: id.clone(),
            dir: "mcp".to_string(),
        });
    }
    for id in &det.skills {
        comps.skills.push(ComponentRef {
            id: id.clone(),
            dir: format!("skills/{id}"),
        });
    }
    PluginManifest {
        manifest_version: 1,
        id: det.id.clone(),
        name: det.id.clone(),
        version: None,
        description: None,
        icon: None,
        components: Some(comps),
        extra: std::collections::BTreeMap::new(),
    }
}

/// 统一导入：解压插件包（mcp / skill / 组合）→ 安全校验 → 识别 → 落盘
/// `bundles/<id>/`（mcp/ + skills/ + 图标）→ 登记 BundleStore。
pub fn import_plugin_package(
    zip_path: &str,
    display_name: &str,
) -> Result<PluginImportReport, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip: {e}"))?;

    // pass1: 安全校验 + 收集文件路径 + 读 plugin.json / mcp manifest / 图标 /
    // 裸技能 SKILL.md / 其它 manifest.json（回退兼容用）。
    let mut all_paths: Vec<String> = Vec::new();
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut mcp_manifest_bytes: Option<Vec<u8>> = None;
    let mut icon_entry: Option<(String, Vec<u8>)> = None;
    let mut best_skill_md: Option<(String, Vec<u8>)> = None;
    let mut other_manifests: Vec<(String, Vec<u8>)> = Vec::new();
    // 两口径预算：（头部声明）避免一开始就放大异常大的条目；实际字节用于兜底
    // zip bomb ——攻击者可伪造 entry.size() 让头部累计通过，但 read_to_end 后真实
    // 解压字节仍超限。两个计数器都触发上限拒绝。
    let mut declared_total: u64 = 0;
    let mut actual_total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip 条目 #{i}: {e}"))?;
        let Some(enclosed) = entry.enclosed_name() else {
            return Err("zip 含不安全路径(穿越),拒绝".to_string());
        };
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err("zip 含 symlink,拒绝".to_string());
            }
        }
        declared_total = declared_total.saturating_add(entry.size());
        if declared_total > MAX_PLUGIN_SIZE_BYTES {
            return Err(format!(
                "插件包解压超过 {} MiB 上限",
                MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
            ));
        }
        if entry.is_dir() {
            continue;
        }
        let path_str = enclosed.to_string_lossy().replace('\\', "/");
        all_paths.push(path_str.clone());
        // 任一`read_to_end` 后同步累计 actual_total（zip 头声明可能与真实大小不符）
        if path_str == "plugin.json" {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读 plugin.json: {e}"))?;
            actual_total = actual_total.saturating_add(buf.len() as u64);
            if actual_total > MAX_PLUGIN_SIZE_BYTES {
                return Err(format!(
                    "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                    MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                ));
            }
            manifest_bytes = Some(buf);
        } else if path_str == "mcp/manifest.json" {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读 mcp/manifest.json: {e}"))?;
            actual_total = actual_total.saturating_add(buf.len() as u64);
            if actual_total > MAX_PLUGIN_SIZE_BYTES {
                return Err(format!(
                    "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                    MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                ));
            }
            mcp_manifest_bytes = Some(buf);
        } else if (path_str == "icon.svg" || path_str == "icon.png") && icon_entry.is_none() {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读图标: {e}"))?;
            actual_total = actual_total.saturating_add(buf.len() as u64);
            if actual_total > MAX_PLUGIN_SIZE_BYTES {
                return Err(format!(
                    "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                    MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                ));
            }
            icon_entry = Some((path_str.clone(), buf));
        }

        // 裸技能回退：记住布局最优的 SKILL.md（读 frontmatter name 用）。
        if let Some(rank) = super::skill_marketplace::skill_md_rank(&path_str) {
            let better = match &best_skill_md {
                None => true,
                Some((prev, _)) => super::skill_marketplace::skill_md_rank(prev)
                    .map(|pr| rank < pr)
                    .unwrap_or(false),
            };
            if better {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("读 SKILL.md: {e}"))?;
                actual_total = actual_total.saturating_add(buf.len() as u64);
                if actual_total > MAX_PLUGIN_SIZE_BYTES {
                    return Err(format!(
                        "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                        MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                    ));
                }
                best_skill_md = Some((path_str.clone(), buf));
            }
        }
        // 裸 MCP 回退：记住其它 manifest.json（排除 mcp/manifest.json 与 plugin.json）。
        if (path_str == "manifest.json" || path_str.ends_with("/manifest.json"))
            && path_str != "mcp/manifest.json"
            && path_str != "plugin.json"
        {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读 manifest.json: {e}"))?;
            actual_total = actual_total.saturating_add(buf.len() as u64);
            if actual_total > MAX_PLUGIN_SIZE_BYTES {
                return Err(format!(
                    "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                    MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                ));
            }
            other_manifests.push((path_str, buf));
        }
    }

    // 识别 manifest（可选）。
    let manifest: Option<PluginManifest> = match &manifest_bytes {
        Some(b) => Some(serde_json::from_slice(b).map_err(|e| format!("解析 plugin.json: {e}"))?),
        None => None,
    };
    if let Some(m) = &manifest {
        if m.manifest_version > 1 {
            return Err(format!(
                "plugin.json manifest_version {} 高于当前支持的 1，请升级应用",
                m.manifest_version
            ));
        }
    }

    // 识别组件向量 + 包 id → kind 现算。
    let det = detect_components(
        &manifest,
        &mcp_manifest_bytes,
        &best_skill_md,
        &other_manifests,
        &all_paths,
    )?;
    let (id, mcp_servers, skills) = (
        det.id.clone(),
        det.mcp_servers.clone(),
        det.skills.clone(),
    );
    let kind = crate::features::marketplace::bundle::derive_bundle_kind(
        &mcp_servers,
        &skills,
        &[],
    )
    .map_err(|_| "插件包不含任何组件（空包）".to_string())?;

    // 拒绝与预置/内置包 id 冲突：用户上传包顶替市场预置会让 UI/默认值/资源池
    // 全部错位，且无法回滚（预置版本指纹与上传不同）。内置 CLI 连接器另由
    // `mcp_catalog` 索引覆盖（结构 Rust 函数式 API）。
    if !crate::features::marketplace::mcp_catalog::spec_for(&id).is_none() {
        return Err(format!(
            "包 id '{id}' 与市场预置 MCP 冲突，请改用其它 id 或通过市场直接安装"
        ));
    }
    if !crate::features::marketplace::bundle::cli_bundle_skill_dirs(&id).is_empty() {
        return Err(format!("包 id '{id}' 与内置 CLI 连接器冲突，请改用其它 id"));
    }
    // 已下线内置技能名拒收（plugin-package-spec §10 承诺的导入校验）：包 id 与
    // 任一技能组件名都不得与退役名单冲突（旧管线只拦技能路径，插件包管线此前
    // 未接，二轮评审文档失实）。
    if crate::features::marketplace::skill_marketplace::RETIRED_SKILL_NAMES
        .contains(&id.as_str())
    {
        return Err(format!("包 id '{id}' 与已下线内置技能名冲突，请改用其它 id"));
    }
    for skill_name in &skills {
        if crate::features::marketplace::skill_marketplace::RETIRED_SKILL_NAMES
            .contains(&skill_name.as_str())
        {
            return Err(format!(
                "技能 '{skill_name}' 与已下线内置技能名冲突，请改用其它名称"
            ));
        }
    }
    // 技能组件一致性（plugin-package-spec §8 承诺的导入校验）：组件 id 必须与
    // SKILL.md frontmatter 的 `name` 一致——不一致会被注册表当两套技能处理。
    for skill_name in &skills {
        let rel = format!("skills/{skill_name}/SKILL.md");
        let mut buf = Vec::new();
        let Ok(mut entry) = archive.by_name(&rel) else {
            continue; // 组件目录存在性在 detect_components 已校验
        };
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }
        if let Some(fm_name) = crate::features::marketplace::skill_marketplace::read_skill_name_from_str(
            std::str::from_utf8(&buf).unwrap_or(""),
        ) {
            if fm_name != *skill_name {
                return Err(format!(
                    "技能 '{skill_name}' 的 SKILL.md frontmatter name 为 '{fm_name}'，与组件 id 不一致（plugin-package-spec §8）"
                ));
            }
        }
    }

    // 落盘到 staged：mcp/ + skills/ 子树 + 裸包回退规范化 → bundles/<id>/ 原子 rename。
    // 注：旧 spanner/ 与 runtime/ 子树已删除，skill 包的脚本由 skill_marketplace
    //     后置 hook 单独处理。
 (refactor(marketplace): 移除 spanner 扳手插件（向 skill-with-runtime 协议迁移）)
    let pkg_dir = crate::platform::paths::bundles_root().join(&id);
    // 上传包 id 冲突：目标包目录已存在且内容不同 → 拒绝（提示改名重试），避免
    // 不同包静默互覆盖（二轮评审：冲突检查需覆盖上传包）。内容一致视为同包
    // 重导/升级，允许走原子替换。
    if pkg_dir.exists()
        && !same_package_content(
            &pkg_dir,
            manifest_bytes.as_deref(),
            best_skill_md.as_ref(),
        )
    {
        return Err(format!(
            "包 id '{id}' 已存在且内容不同，请改名后重试（避免覆盖已有包）"
        ));
    }
    let parent = pkg_dir.parent().expect("bundles 目录必有父级");
    std::fs::create_dir_all(parent).map_err(|e| format!("创建 bundles 目录: {e}"))?;
    let staged = pkg_dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("暂存目录: {e}"))?;

    let write_result = (|| -> Result<(), String> {
        // 1) 统一落盘：mcp/ + skills/ + spanner/ + runtime/ 子树 + 裸包回退规范化。
        let bare_skill = det
            .bare_skill
            .as_ref()
            .map(|(r, n)| (r.as_str(), n.as_str()));
        let bare_mcp = det.bare_mcp.as_ref().map(|(r, n)| (r.as_str(), n.as_str()));
        for path_str in &all_paths {
            let Some((subdir, rel)) = landing_target(path_str, bare_skill, bare_mcp) else {
                continue; // plugin.json / icon 单独处理
            };
            if rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
                continue;
            }
            let target = staged.join(&subdir).join(&rel);
            if !target.starts_with(&staged) {
                return Err("路径穿越,拒绝".to_string());
            }
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("建目录: {e}"))?;
            }
            let mut entry = archive
                .by_name(path_str)
                .map_err(|e| format!("读条目 {path_str}: {e}"))?;
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读条目: {e}"))?;
            // 实际写盘计量：pass1 的头部声明预算可被伪造，这里按真实读出字节累计
            // （zip bomb 兜底，二轮评审 M-4）。超限由调用方清 staged 拒收。
            actual_total = actual_total.saturating_add(buf.len() as u64);
            if actual_total > MAX_PLUGIN_SIZE_BYTES {
                return Err(format!(
                    "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                    MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
                ));
            }
            std::fs::write(&target, buf).map_err(|e| format!("写文件: {e}"))?;
        }

        // 2) spanner 旧路径删除——脚本可执行能力通过 skill 包的 SKILL.md frontmatter
        //    `tools[]` + `runtime` 段声明。此处不再合成 mcp/manifest.json。

        // 3) plugin.json 落盘：有原声明的写规范化副本；无 plugin.json 的裸包合成
        //    最小自描述清单（§5.2 派生 manifest），保证按包聚合目录始终带 plugin.json。
        if let Some(m) = &manifest {
            std::fs::write(
                staged.join("plugin.json"),
                serde_json::to_string_pretty(m).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("写 plugin.json: {e}"))?;
        } else {
            let synth = synthesized_manifest(&det);
            std::fs::write(
                staged.join("plugin.json"),
                serde_json::to_string_pretty(&synth).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("写派生 plugin.json: {e}"))?;
        }

        // 4) 图标：包内可选图标优先，缺省写默认图标。
        if let Some((icon_path, bytes)) = &icon_entry {
            write_icon_bytes(&staged, icon_path, bytes)?;
        } else if let Some(declared) = manifest.as_ref().and_then(|m| m.icon.clone()) {
            let mut found = false;
            for p in &all_paths {
                if *p == declared && is_supported_icon(&declared) {
                    let mut e = archive.by_name(p).map_err(|e| format!("读图标: {e}"))?;
                    let mut buf = Vec::new();
                    e.read_to_end(&mut buf)
                        .map_err(|e| format!("读图标: {e}"))?;
                    write_icon_bytes(&staged, &declared, &buf)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!(
                    "plugin.json 声明的图标 '{declared}' 不存在或格式不支持"
                ));
            }
        } else {
            write_default_icon(&staged)?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }

    // 安装时 smoke test：旧 spanner 自检已删除。当前只保留纯 MCP / 纯 skill 落盘，
    // 脚本可执行能力改为 skill 包自身的后置 smoke（看 skill_marketplace.rs::install）。

    // 原子落盘：先把旧目录挪到 .old 备份，rename 成功后再删 .old；rename 失败则
    // 把 .old 复原回去，保证「旧包不丢、新包不入」——避免既往版本 `remove+rename`
    // 任一环节失败导致新旧双丢的窗期。
    let backup = pkg_dir.with_extension("old");
    let mut moved_old = false;
    if pkg_dir.exists() {
        let _ = std::fs::remove_dir_all(&backup);
        if std::fs::rename(&pkg_dir, &backup).is_ok() {
            moved_old = true;
        }
    }
    if let Err(e) = std::fs::rename(&staged, &pkg_dir) {
        // rename 失败：尝试把旧目录复原，让已安装版本继续可用
        if moved_old {
            let _ = std::fs::rename(&backup, &pkg_dir);
        }
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("落盘: {e}"));
    }
    if moved_old {
        let _ = std::fs::remove_dir_all(&backup);
    }

    // 供给：MCP 组件走 install 管线写 mcp.json + installed.json（底座据此拉起 server，
    // 工具才能注册可用）。纯 skill 包无 mcp/ 目录，跳过（技能走物化通道）。
    // 注：旧 spanner 路径已删除——脚本可执行能力的供给在 skill_marketplace.rs 走
    // skill-run wrapper + execpolicy deny rule。
    if !mcp_servers.is_empty() {
 (refactor(marketplace): 移除 spanner 扳手插件（向 skill-with-runtime 协议迁移）)
        if let Err(e) =
            super::MarketplaceManager::new().install(&id, &std::collections::HashMap::new())
        {
            return Err(format!("MCP 供给失败（{id}）: {e}"));
        }
    }

    // 登记 BundleStore（上传 source=Upload(zip 展示名)，installed=true）。
    let icon_rel = if pkg_dir.join("icon.svg").is_file() {
        "icon.svg".to_string()
    } else {
        "icon.png".to_string()
    };
    let mut record = super::store::BundleRecord::installed_now(
        id.clone(),
        super::store::BundleSource::Upload(display_name.to_string()),
    );
    record.content_fingerprint = Some(id.clone());
    if let Err(e) = super::store::BundleStore::new().upsert_preserving(record) {
        log::warn!("[plugin-import] bundles.json 镜像写入失败（import {id}）: {e}");
    }

    Ok(PluginImportReport {
        id,
        kind,
        icon: icon_rel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_ext_whitelist() {
        assert!(is_supported_icon("icon.svg"));
        assert!(is_supported_icon("icon.PNG"));
        assert!(!is_supported_icon("icon.jpg"));
        assert!(!is_supported_icon("icon.gif"));
        assert!(!is_supported_icon("../icon.svg"));
    }

    #[test]
    fn default_icon_lands_in_pkg_dir() {
        let tmp = std::env::temp_dir().join(format!("pinvou-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rel = write_default_icon(&tmp).unwrap();
        assert_eq!(rel, "icon.svg");
        assert!(tmp.join("icon.svg").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn icon_bytes_reject_bad_ext() {
        let tmp = std::env::temp_dir().join(format!("pinvou-icon-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(write_icon_bytes(&tmp, "icon.jpg", b"x").is_err());
        assert!(write_icon_bytes(&tmp, "a/icon.png", b"x").is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn plugin_manifest_parses_minimal_components_only() {
        // 旧的 spanner 字段（已废弃）应当仍能被 serde 容忍进入 `extra` map
        // （前向兼容字段保留）。本测试校验 components-only 包能正常解析。
        let json = r#"{
            "manifest_version": 1,
            "id": "weather",
            "name": "天气查询",
            "icon": "icon.svg",
            "components": {
                "mcp_servers": [{"id":"weather","dir":"mcp"}],
                "skills": [{"id":"weather","dir":"skills/weather"}]
            }
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "weather");
        assert_eq!(m.icon.as_deref(), Some("icon.svg"));
        assert!(m.components.is_some());
        let comps = m.components.unwrap();
        assert_eq!(comps.mcp_servers.len(), 1);
        assert_eq!(comps.skills.len(), 1);
    }

    /// 前向兼容：plugin.json 含已废弃的 `spanner` 字段（旧上传包）应被解析为
    /// `extra` 兜底，未映射到结构体字段上，但仍可通过 deser 进入。
    #[test]
    fn plugin_manifest_parses_legacy_spanner_field_as_extra() {
        let json = r#"{
            "manifest_version": 1,
            "id": "weather",
            "name": "天气查询",
            "icon": "icon.svg",
            "spanner": {
                "entry": "main.py",
                "input_schema": {"type":"object","properties":{"city":{"type":"string"}}}
            }
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "weather");
        // 旧 spanner 字段落到 `extra` map 里以便日志/排障，仍可读。
        let legacy = m.extra.get("spanner");
        assert!(legacy.is_some(), "旧 spanner 字段应被 forward-compat 保留到 extra");
    }

    #[test]
    fn plugin_manifest_parses_components() {
        let json = r#"{
            "manifest_version": 1,
            "id": "combo-demo",
            "name": "演示组合包",
            "components": {
                "mcp_servers": [{"id":"combo-demo","dir":"mcp"}],
                "skills": [{"id":"combo-demo","dir":"skills/combo-demo"}]
            }
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        let comps = m.components.unwrap();
        assert_eq!(comps.mcp_servers.len(), 1);
        assert_eq!(comps.mcp_servers[0].id, "combo-demo");
        assert_eq!(comps.skills.len(), 1);
        assert_eq!(comps.skills[0].dir, "skills/combo-demo");
    }

    #[test]
    fn import_mcp_skill_combo_package_lands_layout() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-combo-import-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        let zip_path = dir.join("combo.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}],"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["demo"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap();
        assert_eq!(report.id, "demo");
        assert_eq!(report.kind, BundleKind::Bundle);
        assert_eq!(report.icon, "icon.svg");

        let pkg = dir.join("bundles").join("demo");
        assert!(
            pkg.join("mcp/manifest.json").is_file(),
            "mcp manifest 应落盘"
        );
        assert!(pkg.join("skills/demo/SKILL.md").is_file(), "skill 应落盘");
        assert!(pkg.join("icon.svg").is_file(), "缺省图标应落盘");

        // 整条链路关键断言：import 内部的 install() 已把 MCP 供给写进 mcp.json +
        // installed.json（底座据此拉起 server，工具才可用）。
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        assert!(
            mgr.installed_ids().contains(&"demo".to_string()),
            "installed.json 应记录 demo"
        );
        let mcp_raw =
            std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap_or_default();
        assert!(
            mcp_raw.contains("\"demo\""),
            "mcp.json 应含 demo server 供给，实际: {mcp_raw}"
        );

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回退兼容：符合 skills 标准的裸技能包（无 plugin.json，SKILL.md 在命名目录下，
    /// name 取自 frontmatter）→ 规范化为 `skills/<name>/` 布局并识别为纯技能包。
    #[test]
    fn import_bare_skill_package_lands_canonical_layout() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-bare-skill-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        let zip_path = dir.join("skill.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.start_file("my-skill/scripts/helper.py", opts).unwrap();
            zw.write_all(b"print('hi')").unwrap();
            zw.finish().unwrap();
        }

        let report = import_plugin_package(&zip_path.to_string_lossy(), "skill.zip").unwrap();
        assert_eq!(report.id, "greet");
        assert_eq!(report.kind, BundleKind::Skill);
        assert_eq!(report.icon, "icon.svg");

        let pkg = dir.join("bundles").join("greet");
        assert!(
            pkg.join("skills/greet/SKILL.md").is_file(),
            "裸技能 SKILL.md 应规范化为 skills/greet/SKILL.md"
        );
        assert!(
            pkg.join("skills/greet/scripts/helper.py").is_file(),
            "裸技能资源应随技能根一并落盘"
        );
        assert!(pkg.join("icon.svg").is_file(), "缺省图标应落盘");
        assert!(pkg.join("plugin.json").is_file(), "派生 plugin.json 应落盘");

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回退兼容：符合 MCP 标准的裸 MCP 包（无 plugin.json，manifest.json 在根目录，
    /// 声明 command/args）→ 规范化为 `mcp/` 布局并识别为纯 MCP 包。
    #[test]
    fn import_bare_mcp_package_lands_canonical_layout() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-bare-mcp-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        let zip_path = dir.join("mcp.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"wcalc","name":"计算器","description":"d","version":"1.0.0","icon":"","category":"dev","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("server.py", opts).unwrap();
            zw.write_all(b"print('server')").unwrap();
            zw.finish().unwrap();
        }

        let report = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip").unwrap();
        assert_eq!(report.id, "wcalc");
        assert_eq!(report.kind, BundleKind::Mcp);
        assert_eq!(report.icon, "icon.svg");

        let pkg = dir.join("bundles").join("wcalc");
        assert!(
            pkg.join("mcp/manifest.json").is_file(),
            "裸 MCP manifest.json 应规范化为 mcp/manifest.json"
        );
        assert!(
            pkg.join("mcp/server.py").is_file(),
            "裸 MCP server.py 应随 manifest 目录一并落盘"
        );
        assert!(pkg.join("icon.svg").is_file(), "缺省图标应落盘");
        assert!(pkg.join("plugin.json").is_file(), "派生 plugin.json 应落盘");

        // 裸 MCP 同样走 install() 供给：mcp.json + installed.json 应记录 wcalc。
        let mgr = crate::features::marketplace::MarketplaceManager::new();
        assert!(
            mgr.installed_ids().contains(&"wcalc".to_string()),
            "installed.json 应记录 wcalc"
        );
        let mcp_raw =
            std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap_or_default();
        assert!(
            mcp_raw.contains("\"wcalc\""),
            "mcp.json 应含 wcalc server 供给，实际: {mcp_raw}"
        );

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
