// architecture-guard: allow-target-cfg -- rename_dir_with_retry retries
// PermissionDenied on Windows only (AV/indexers transiently hold the newly
// created directory's handle, producing rename os error 5 — hit in practice);
// on Unix that error is a real permission problem and must not be retried.
// The platform split is this single boolean; sinking it into the platform
// adapter layer costs more than it saves; the retry path is covered by
// reimport_same_plugin_package_is_allowed and the concurrent mutual exclusion
// by concurrent_same_id_import_is_serialized. Since round-21 there are
// additionally test-only cfg(unix) planted FIFO/symlink regressions (the
// mark_landing hardening pins), under the same precedent as the identical
// exemptions in scope.rs / store.rs.
//! 插件包统一导入：mcp / skill / 组合包 + 图标落盘（plugin-protocol.md）。
//!
//! 统一上传路径：用户上传一个 zip，无论内容是哪种能力类型（MCP server、skill、
//! 或它们的组合），都自动识别 → 安全校验 → 落盘到按包聚合的
//! `bundles/<id>/`，并在商店与运行时按同一套「包」模型读取/开关/卸载。
//!
//! zip 形态（plugin-protocol §3/§15）：
//! ```text
//! my-plugin.zip
//! ├── plugin.json        ← 权威声明（可选；组合包/凭据/元数据时必需）
//! ├── mcp/               ← MCP server（manifest.json + server.py）
//! ├── skills/<name>/     ← SKILL.md 目录（可多个）
//! └── icon.svg|png       ← 可选图标
//! ```
//!
//! 注：旧 `spanner/` 与 `runtime/` 子目录布局已删除。skill 包的 frontmatter
//! `tools[]` + `runtime` 可执行协议目前仅为 RFC 草案（docs/plugin-package-spec.md），
//! 执行通路未实施，导入侧不识别也不落盘这两类子树。
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
/// 不再有「spanner 独立组件」入口；skill 包的 `tools[]` + `runtime` 可执行协议
/// 为 RFC 草案（docs/plugin-package-spec.md），执行通路未实施。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginManifest {
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
struct PluginComponents {
    #[serde(default)]
    pub mcp_servers: Vec<ComponentRef>,
    #[serde(default)]
    pub skills: Vec<ComponentRef>,
}

/// 组件目录引用：`dir` 相对 zip 根，导入时校验存在。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComponentRef {
    pub id: String,
    pub dir: String,
}

/// 图标扩展名 → 是否允许（只认 svg/png，且必须是无路径分隔符的纯文件名，
/// 与 `write_icon_bytes` 的口径一致——`a/icon.png` / `../icon.svg` 一律拒收）。
fn is_supported_icon(path: &str) -> bool {
    if path.contains('/') || path.contains('\\') || path.contains("..") {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".svg") || lower.ends_with(".png")
}

/// 默认图标（lucide `package`，无品牌依赖的通用「工具包」图形）。
const DEFAULT_ICON_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/></svg>"#;

/// 落盘默认图标到包目录（`bundles/<id>/icon.svg`）。返回相对路径 "icon.svg"。
fn write_default_icon(pkg_dir: &Path) -> Result<String, String> {
    let icon_path = pkg_dir.join("icon.svg");
    std::fs::create_dir_all(pkg_dir).map_err(|e| format!("创建包目录: {e}"))?;
    std::fs::write(&icon_path, DEFAULT_ICON_SVG).map_err(|e| format!("写默认图标: {e}"))?;
    Ok("icon.svg".to_string())
}

/// 把图标字节落盘到包目录（`icon.<ext>`，ext 取自原文件名）。返回相对路径。
///
/// file_name 必须是无路径分隔符的纯文件名（不接受 `a/icon.png` / `../icon.svg` 等
/// 路径形式），扩展名限定为 `svg`/`png`。
fn write_icon_bytes(pkg_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<String, String> {
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

/// 目标包目录与本次导入是否「同一包」：全内容比对（五轮评审）——此前带
/// plugin.json 的包只比对 plugin.json 语义，`mcp/manifest.json`、`server.py`、
/// `skills/*/SKILL.md` 等执行代码都不参与冲突比对，重导同 id 包可静默替换
/// 已装工具的代码。现在按「落盘后的规范化布局」全量比对：
/// - plugin.json 按**语义**比对（落盘是 `to_string_pretty` 规范化字节，裸包为
///   派生清单，与 zip 原始字节直比必误判冲突）；
/// - 其余可落盘条目按 zip 字节与磁盘字节逐一比对（pass2 原样写字节，无规范化，
///   `server.py` / SKILL.md 正文 / `mcp/manifest.json` 任一改动都判冲突）；
/// - 包内图标参与比对；磁盘侧 `mcp/`、`skills/` 下多出的文件（zip 已不再携带
///   的内容）同样判不同，但容忍 Python 运行缓存（`__pycache__/`、`*.pyc`）。
/// 全部一致才视为同包重导（允许原子替换），任一不符视为不同包冲突。
fn same_package_content(
    pkg_dir: &std::path::Path,
    archive: &mut zip::ZipArchive<std::fs::File>,
    manifest_bytes: Option<&[u8]>,
    det: &ComponentDetection,
    all_paths: &[String],
    icon_entry: Option<&(String, Vec<u8>)>,
) -> bool {
    // 1) plugin.json 语义比对：带声明的包比原始声明，裸包比确定性派生清单。
    let plugin_eq = match manifest_bytes {
        Some(bytes) => plugin_manifest_semantic_eq(&pkg_dir.join("plugin.json"), bytes),
        None => {
            let synth = serde_json::to_value(synthesized_manifest(det)).ok();
            let disk = std::fs::read(pkg_dir.join("plugin.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            match (disk, synth) {
                (Some(disk), Some(synth)) => disk == synth,
                _ => false,
            }
        }
    };
    if !plugin_eq {
        return false;
    }

    // 2) 可落盘条目逐一比对（landing 与跳过口径与 pass2 完全一致）。
    let bare_skill = det
        .bare_skill
        .as_ref()
        .map(|(r, n)| (r.as_str(), n.as_str()));
    let bare_mcp = det.bare_mcp.as_ref().map(|(r, n)| (r.as_str(), n.as_str()));
    let mut landed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path_str in all_paths {
        let Some((subdir, rel)) = landing_target(path_str, bare_skill, bare_mcp) else {
            continue; // plugin.json / icon 单独比对
        };
        if rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
            continue; // 与 pass2 相同的跳过口径（未落盘，不参与比对）
        }
        let Ok(mut entry) = archive.by_name(path_str) else {
            return false;
        };
        let declared_size = entry.size();
        let Ok(buf) = read_zip_entry_bounded(&mut entry, declared_size, path_str) else {
            return false; // 条目不可读/伪造头部 → 保守判冲突
        };
        let Ok(existing) = std::fs::read(pkg_dir.join(&subdir).join(&rel)) else {
            return false; // zip 有而磁盘无 → 内容不同
        };
        if existing != buf {
            return false;
        }
        landed.insert(format!("{subdir}/{rel}"));
    }

    // 3) 包内图标比对（缺省图标是常量 DEFAULT_ICON_SVG，无需比对）。
    if let Some((icon_path, bytes)) = icon_entry {
        let ext = if icon_path.to_ascii_lowercase().ends_with(".png") {
            "png"
        } else {
            "svg"
        };
        match std::fs::read(pkg_dir.join(format!("icon.{ext}"))) {
            Ok(existing) if existing == *bytes => {}
            _ => return false,
        }
    }

    // 4) 反向比对：磁盘 `mcp/`、`skills/` 下存在而本次 zip 不再携带的文件 →
    //    内容不同（容忍 Python 运行缓存，其余子树如 plugin.json/icon 已单独处理）。
    for subdir in ["mcp", "skills"] {
        let root = pkg_dir.join(subdir);
        if !root.is_dir() {
            continue;
        }
        let mut disk_files = Vec::new();
        if collect_landed_disk_files(&root, &root, &mut disk_files).is_err() {
            return false;
        }
        for rel in disk_files {
            if !landed.contains(&format!("{subdir}/{rel}")) {
                return false;
            }
        }
    }
    true
}

/// 收集包子树（`mcp/`、`skills/`）下已落盘文件的相对路径，排除 Python 运行
/// 缓存（`__pycache__/` 子树与 `*.pyc`，MCP server 跑过会就地生成，不算内容差异）。
/// This walker follows directories and performs no symlink checks (the comparison target is a directory written by this pipeline itself,
/// with no surface for outside-package content) — the counterpart of `package_export::collect_export_files`'s
/// symlink-skip policy; both are deliberate choices, and the only thing shared is [`is_python_cache_rel_path`].
fn collect_landed_disk_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_landed_disk_files(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if is_python_cache_rel_path(&rel) {
            continue;
        }
        out.push(rel);
    }
    Ok(())
}

/// Whether a relative path ('/'-separated) belongs to the Python run cache: any path level is a `__pycache__`
/// directory, or the file name ends with `.pyc` (case-insensitive). The single predicate shared by three walkers —
/// package import comparison, package export packing, and skill-marketplace fingerprinting; symlink/directory-traversal policy is decided by each walker
/// on its own (export skips symlinks, import compare and skill fingerprints follow) and is not part of this predicate.
pub(crate) fn is_python_cache_rel_path(rel: &str) -> bool {
    rel.split('/').any(|c| c == "__pycache__") || rel.to_ascii_lowercase().ends_with(".pyc")
}

/// 目录 rename 的 Windows 瞬时占用重试：杀毒/索引器会短暂持有新建目录内
/// 文件的句柄，此时 rename 报 os error 5（拒绝访问），稍等即可恢复——真实
/// 导入与测试并发下都实测命中。仅 `PermissionDenied` 重试（Unix 上该错误是
/// 真实权限问题，由 `cfg!(windows)` 限定不进入重试）。
/// `pub(crate)`：回收站包目录搬移（recycle_bin）复用同一重试口径。
pub(crate) fn rename_dir_with_retry(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> std::io::Result<()> {
    const ATTEMPTS: u32 = 20;
    for attempt in 1..=ATTEMPTS {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied;
                if attempt == ATTEMPTS || !retryable {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    unreachable!()
}

/// plugin.json 的语义比对：落盘是 `PluginManifest` 解析后 `to_string_pretty` 的
/// 规范化字节（补了 `version: null` 等 default 字段），与 zip 原始字节不能直比——
/// 两侧各自解析为 `PluginManifest` 再序列化为 canonical Value 比较（键序/空白/
/// default 字段差异不误判）；任一侧解析失败回退字节直比（保守）。
fn plugin_manifest_semantic_eq(disk_path: &std::path::Path, incoming: &[u8]) -> bool {
    let Ok(existing) = std::fs::read(disk_path) else {
        return false;
    };
    match (
        serde_json::from_slice::<PluginManifest>(&existing),
        serde_json::from_slice::<PluginManifest>(incoming),
    ) {
        (Ok(existing), Ok(incoming)) => {
            serde_json::to_value(&existing).ok() == serde_json::to_value(&incoming).ok()
        }
        _ => existing.as_slice() == incoming,
    }
}

/// 有界读取 zip 条目到内存（四轮评审 M-5）：`take(声明 size + 1)` 截断底层流——
/// 伪造 `size=0` 头部的条目在不受限 `read_to_end` 下会绕过声明预算先撑爆内存；
/// 多读 1 字节用于发现「实际 > 声明」的伪造头部并响亮拒收（zip 规范要求解压
/// 大小等于头部声明，诚实的条目不会触发）。
/// `pub(crate)`：旧 zip 技能包管线（skill_marketplace pass2）复用同一收口
/// （五轮评审 M-5 残留：两条管线防护对齐）。
pub(crate) fn read_zip_entry_bounded(
    entry: &mut impl std::io::Read,
    declared_size: u64,
    what: &str,
) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    entry
        .take(declared_size.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("读{what}: {e}"))?;
    if buf.len() as u64 > declared_size {
        return Err(format!(
            "{what} 实际解压大小超过 zip 头声明（疑似伪造头部/zip bomb），拒绝"
        ));
    }
    Ok(buf)
}

/// zip pass-1 单条目安全闸（统一插件包导入与技能 zip 导入共用；穿越/symlink/
/// 大小防护对齐底座 install.rs 的判断）：
/// - `enclosed_name` 为 None = 路径穿越（`..` / 绝对路径），拒绝；
/// - unix_mode 文件类型位 = symlink，拒绝；
/// - 头部声明大小累计超 `max_bytes` 拒绝（真实解压字节由各自 pass2 的有界读取
///   兜底计量——头部声明可被伪造）。
/// 返回净化后的条目路径（分隔符归一为 `/`），并把本条目声明大小累进
/// `declared_total`。
pub(crate) fn checked_zip_entry_path(
    entry: &mut zip::read::ZipFile<'_, std::fs::File>,
    declared_total: &mut u64,
    max_bytes: u64,
    label: &str,
) -> Result<String, String> {
    let Some(enclosed) = entry.enclosed_name() else {
        return Err("zip 含不安全路径(穿越),拒绝".to_string());
    };
    if let Some(mode) = entry.unix_mode() {
        if mode & 0o170000 == 0o120000 {
            return Err("zip 含 symlink,拒绝".to_string());
        }
    }
    *declared_total = declared_total.saturating_add(entry.size());
    if *declared_total > max_bytes {
        return Err(format!(
            "{label}解压超过 {} MiB 上限",
            max_bytes / 1024 / 1024
        ));
    }
    Ok(enclosed.to_string_lossy().replace('\\', "/"))
}

/// 进程内导入互斥锁表（按包 id，四轮评审 M-4）：同一包 id 的并发导入共享
/// staged 路径 `bundles/<id>.tmp` 与 `.old` 备份（线程 B 可删线程 A 的在建目录），
/// 且 same_package_content 冲突检查与原子 rename 之间无锁即 TOCTOU——必须由调用方
/// 持锁覆盖「冲突检查 → 原子 rename 完成」整段临界区。不同包 id 持不同锁，互不阻塞。
///
/// `pub(crate)`：展示名/说明编辑（skill_marketplace::update_display_meta）的
/// SKILL.md 读改写段也持同一把锁——导入的「rename → 重基线备份」与编辑的
/// 「读 SKILL.md/备份 → 写回」因此真正互斥（锁序一致：本锁 → store file_lock，
/// 无死锁面），不再是仅靠注释声明的弱约定。
pub(crate) fn import_lock_for(id: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, std::sync::Arc<std::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let table = LOCKS.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
    table
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(id.to_string())
        .or_insert_with(|| std::sync::Arc::new(std::sync::Mutex::new(())))
        .clone()
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
                // 强制规范 dir（四轮评审 M-3）：放行任意 dir 时落盘只认 mcp//skills/
                // 前缀，声明组件会被静默丢弃（登记 installed=true 但磁盘无文件）。
                if dir != "mcp" {
                    return Err(format!(
                        "mcp 组件 '{}' 的 dir '{dir}' 非规范：MCP 组件必须使用规范化目录 'mcp'",
                        c.id
                    ));
                }
                if !all_paths.iter().any(|p| p.starts_with(&format!("{dir}/"))) {
                    return Err(format!("mcp 组件目录 '{dir}' 不存在"));
                }
                // 交叉校验（四轮评审 M-9）：组件声明 id 必须与 mcp/manifest.json 的
                // ToolManifest.id 一致 —— 不一致时 installed.json/bundles.json 记
                // 包 id、mcp.json 键却取 manifest.id，卸载只按包 id 清理，残留
                // 孤儿 server。
                let Some(bytes) = mcp_manifest_bytes.as_deref() else {
                    return Err(format!("mcp 组件目录 '{dir}' 缺 manifest.json"));
                };
                let tm: crate::features::marketplace::ToolManifest = serde_json::from_slice(bytes)
                    .map_err(|e| format!("解析 {dir}/manifest.json 失败: {e}"))?;
                if tm.id != c.id {
                    return Err(format!(
                        "mcp 组件声明 id '{}' 与 {dir}/manifest.json 的 id '{}' 不一致",
                        c.id, tm.id
                    ));
                }
                mcp.push(c.id.clone());
            }
            for c in &comps.skills {
                let dir = c.dir.trim_end_matches('/');
                let canonical = format!("skills/{}", c.id);
                if dir != canonical {
                    return Err(format!(
                        "技能组件 '{}' 的 dir '{dir}' 非规范：技能组件必须使用规范化目录 '{canonical}'",
                        c.id
                    ));
                }
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
/// 1. 固定前缀 `mcp/` / `skills/`（本项目标准布局；旧 `spanner/`、`runtime/`
///    前缀已随 spanner 退场删除，不再识别）；
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
    // 注：原 `spanner/` 前缀分支已删除，不再有独立的 spanner 子目录布局；skill 包
    // 的 `tools[]` 可执行协议为 RFC 草案（执行通路未实施），导入侧不识别该前缀。

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

/// Reject an uploaded MCP manifest that claims builtin-only semantics
/// (`builtin: true` / `visibility: "system"`). Payloads that do not parse as a
/// `ToolManifest` are left to component detection, which reports malformed
/// manifests on its own path.
fn reject_builtin_claims_in_upload(what: &str, bytes: &[u8]) -> Result<(), String> {
    let Ok(tm) = serde_json::from_slice::<crate::features::marketplace::ToolManifest>(bytes) else {
        return Ok(());
    };
    if tm.builtin {
        return Err(format!(
            "uploaded package manifest ({what}) declares builtin: true, which is reserved for plugins shipped with the application (docs/builtin-toolset-contract.md §3.1)"
        ));
    }
    if tm.visibility == "system" {
        return Err(format!(
            "uploaded package manifest ({what}) declares visibility: \"system\", which is reserved for builtin plugins (docs/builtin-toolset-contract.md §3.1)"
        ));
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// Round-32 minor 6 (review #455): import landing journal.
//
// A crash between the landing rename and the registry upsert leaves
// `bundles/<id>/` live with no record — "live-by-absence" in initialized
// scopes, zero consent, invisible to the round-24 in-process rollback. The
// journal closes it precisely: the import marks its id before the landing and
// clears the mark on any in-process exit (guard drop), so a leftover mark is
// proof of a crash in that exact window. The startup reconciliation acts ONLY
// on journaled ids (never a blind scan — zero misclassification surface): it
// moves the landed dir out of `bundles_root` into a recoverable holding dir,
// loudly logged and marked on the startup timeline.

fn landing_journal_dir() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("marketplace")
        .join("import_journal")
}

pub(crate) fn landing_mark_path(id: &str) -> std::path::PathBuf {
    landing_journal_dir().join(format!("{id}.pending"))
}

/// Whether an import for `id` is mid-landing (its journal mark is present).
/// The mark is a file in the shared home, so this is cross-process visible:
/// the startup sweep probes it before stripping a retired id's consent rows,
/// deferring when a concurrent import has marked its landing but not yet
/// finished (round-18 review — the mark now precedes the deny-first gate, so
/// mark-presence covers an import whose rows are already registered).
pub(crate) fn landing_in_progress(id: &str) -> bool {
    landing_mark_path(id).exists()
}

/// Cross-process landing lease path: `<journal>/<id>.landing.lock` (holds no
/// user data). The name never carries the `.pending` suffix, so the
/// reconcile's journal scan skips it by construction.
fn landing_lease_path(id: &str) -> std::path::PathBuf {
    landing_journal_dir().join(format!("{id}.landing.lock"))
}

/// Opens (creating if missing) the cross-process landing lease file for `id`.
/// Same discipline as the scope lock's open: a hardened private open, with
/// the parent directory created only on the cold NotFound path.
///
/// The lease is the liveness signal behind the landing mark: the mark alone
/// cannot distinguish a live import from a crashed one, but a live import
/// holds this lease (flock / LockFileEx via `fd-lock`) from before the mark
/// is written until the landing guard drops. Callers:
/// - the import pipeline blocks on `write()` — a peer's same-id import now
///   waits out here instead of racing the shared staged `.tmp` / `.old`
///   paths that only the in-process mutex used to serialize;
/// - `reconcile_import_journal` probes `try_write()` non-blockingly —
///   contention means a live import, so the entry is skipped untouched and
///   crash recovery retries next boot;
/// - the uninstall and the retired-tool sweep take it (the sweep with
///   `try_write`, deferring on contention) so a cross-process landing can
///   neither be stripped nor have its just-landed dir deleted underneath.
///
/// Lock order everywhere: landing lease → per-id import mutex → transaction
/// lock — except the reconcile, which never blocks (mutex → `try_write`, and
/// a failed try just skips the entry). The OS releases the lease when the
/// holder exits or crashes; only a frozen holder blocks a blocking acquirer,
/// the same accepted fail-stop tradeoff as the scope lock.
pub(crate) fn open_landing_lease(id: &str) -> std::io::Result<fd_lock::RwLock<std::fs::File>> {
    let path = landing_lease_path(id);
    let open = || crate::platform::filesystem::open_private_lock_file(&path);
    match open() {
        Ok(file) => Ok(fd_lock::RwLock::new(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            open().map(fd_lock::RwLock::new)
        }
        Err(error) => Err(error),
    }
}

fn mark_landing(id: &str) {
    let path = landing_mark_path(id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!(
                "[plugin-import] writing the landing journal failed (crash recovery for {id} degraded to live-by-absence): {e}"
            );
            return;
        }
    }
    // Round-33 minor 3 (review #455): a failed mark write degrades crash
    // recovery exactly like the create_dir_all failure above — it must be
    // just as loud instead of silently re-opening the live-by-absence window
    // the journal exists to close.
    // round-21 review: the write goes through the hardened private-write
    // primitive — it runs under the cross-process landing lease, so a
    // planted FIFO/symlink at the mark path must refuse, not block the
    // lock holder (same discipline as the scope lock's hardened reads).
    if let Err(e) = crate::platform::filesystem::write_private_data_file(&path, b"pending\n") {
        log::warn!(
            "[plugin-import] writing the landing journal mark failed (crash recovery for {id} degraded to live-by-absence): {e}"
        );
    }
}

fn clear_landing(id: &str) {
    let _ = std::fs::remove_file(landing_mark_path(id));
}

/// Clears the landing mark on any in-process exit; a crash skips the drop and
/// leaves the mark for the startup reconciliation.
struct LandingJournalGuard(String);

impl Drop for LandingJournalGuard {
    fn drop(&mut self) {
        clear_landing(&self.0);
    }
}

/// Round-32 minor 6 (review #455): resolve leftover landing marks from crashed
/// imports. For each journaled id: missing pack dir → the import never landed
/// or rolled back (clear); registry record present → the import completed and
/// only the mark-clear crashed (clear); landed dir with no record → crash
/// residue: move it out of `bundles_root` into a recoverable holding dir so no
/// scan admits it live-by-absence. Store-read failures keep the entry (retry
/// next boot, fail-closed). Actively holds the id's import lock per entry so
/// a same-process import cannot interleave with the inspection/move, and —
/// round-20 review — probes the id's landing lease non-blockingly before
/// treating anything as residue: the mark alone cannot distinguish a live
/// import from a crashed one, and a peer booting mid-import (the shared-home
/// GUI + headless premise) must not sweep a live import's staged `.tmp` dir
/// or clear its mark. Contention (or an un-lockable lease) defers the entry
/// to the next boot; the probe never blocks, so the mutex → try-lease order
/// cannot deadlock against the pipeline's lease → mutex order.
pub fn reconcile_import_journal() -> Result<(), String> {
    let dir = landing_journal_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("reading the import journal failed: {e}")),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Err(format!("reading an import journal entry failed: {e}")),
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".pending") else {
            // Not ours (or a residue dir from a previous reconcile) — leave it.
            continue;
        };
        if !super::skill_marketplace::is_safe_skill_name(&id) {
            // A journaled id must always be safe — the import validated the
            // fallback id with `is_safe_skill_name` (round-26 MAJOR 2: the
            // reader used to gate on the stricter `is_safe_component_id`, so
            // a legitimate mixed-case bare-skill id — frontmatter
            // `name: MySkill` arriving through the wrapped-`.md` channel —
            // wrote a mark this reconcile silently skipped forever, leaving
            // its crash windows unrecovered). An unsafe name means the file
            // is not a journal mark. Leave it.
            continue;
        }
        let lock = import_lock_for(id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        // Round-20 review (P1): the mark is only a hint — the lease proves
        // liveness. A live import (this process's pipeline thread or a peer
        // process sharing the home) holds the lease across its whole
        // mark→land→register span; a contended try means the entry is not
        // crash residue. Un-lockable lease files defer fail-closed too:
        // without provable liveness, destructive recovery waits.
        let mut landing_lease = match open_landing_lease(id) {
            Ok(lease) => lease,
            Err(error) => {
                log::warn!(
                    "[plugin-import] import journal: opening the landing lease for '{id}' failed, deferring crash recovery to the next startup: {error}"
                );
                continue;
            }
        };
        let _lease_guard = match landing_lease.try_write() {
            Ok(guard) => guard,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                log::info!(
                    "[plugin-import] import journal: '{id}' is mid-landing in another process; deferring crash recovery to the next startup"
                );
                continue;
            }
            Err(error) => {
                log::warn!(
                    "[plugin-import] import journal: the landing lease for '{id}' is un-lockable, deferring crash recovery to the next startup: {error}"
                );
                continue;
            }
        };
        let pkg_dir = crate::platform::paths::bundles_root().join(id);
        // Round-26 MAJOR 1: a crash between the re-import's two renames (the
        // pack dir already moved to `<id>.old`, the staged copy not yet
        // renamed in) leaves exactly the shape the next arm handles — no pack
        // dir, a staged `.tmp`, the pre-import copy in `<id>.old`, and the
        // record still `installed`. Sweeping the `.tmp` and clearing the mark
        // there used to converge that crash to "record installed, content
        // gone, the only copy stranded in the scan-invisible `.old`, nothing
        // ever revisits it, no log". Restore the backup in place first (the
        // pipeline's own rollback arm does exactly this rename), then let the
        // record arms below converge the rest: record present → the staged
        // `.tmp` sweep + mark clear; record gone → the quarantine arm. A
        // failed restore keeps the mark and retries next startup, the same
        // contract as the sweep arms.
        let backup = pkg_dir.with_extension("old");
        if !pkg_dir.exists() && backup.exists() {
            match rename_dir_with_retry(&backup, &pkg_dir) {
                Ok(()) => log::warn!(
                    "[plugin-import] import journal: restored the pre-import backup for '{id}' after a crash between the re-import renames"
                ),
                Err(e) => {
                    log::warn!(
                        "[plugin-import] import journal: restoring the re-import backup for '{id}' failed, keeping the mark for the next startup: {e}"
                    );
                    continue;
                }
            }
        }
        if !pkg_dir.exists() {
            // The import never landed, or the in-process rollback already
            // undid it. The staged `bundles/<id>.tmp` (written after the
            // pre-land gate, renamed into place at landing) has no owner
            // then: left alone it surfaces as a ghost uninstalled MCP card
            // through the available_tools walk, which — unlike every other
            // lens — has no `.tmp` exclusion. Sweep it (round-19 review);
            // the pipeline's own error paths remove exactly this dir. With
            // the backup restore above, a `<id>.old` found here has already
            // been moved back into place; a strand left by a FAILED
            // in-process rollback (whose mark the guard cleared) is the
            // disclosed residual in §3.2 — this arm must not guess at it.
            let staged = pkg_dir.with_extension("tmp");
            if staged.exists() {
                // Round-24 minor (arm A gets the round-21 P3 treatment): a
                // failed sweep must keep the mark so the next boot retries —
                // the unconditional clear here stranded the ghost `.tmp`
                // forever (nothing reschedules a swept mark) while the log
                // claimed success. Same contract as the record-present arm
                // below.
                if let Err(e) = std::fs::remove_dir_all(&staged) {
                    log::warn!(
                        "[plugin-import] import journal: sweeping the crash-orphaned staging dir for '{id}' failed, keeping the mark for the next startup: {e}"
                    );
                    continue;
                }
                log::warn!(
                    "[plugin-import] import journal: swept the crash-orphaned staging dir for '{id}'"
                );
            }
            clear_landing(id);
            continue;
        }
        let record = super::store::BundleStore::new().get(id);
        match record {
            Err(e) => {
                // The store truth is unknowable — keep the mark and retry on
                // the next boot (fail-closed: the dir may be crash residue,
                // but deleting on an unreadable registry is how user packs
                // get eaten).
                log::warn!(
                    "[plugin-import] import journal: the store read for '{id}' failed, deferring crash recovery to the next startup: {e}"
                );
            }
            Ok(Some(_)) => {
                // The import completed; only the mark-clear crashed. A
                // reimport's staged `.tmp` can still be crash-orphaned here
                // (the crash hit mid-staging of the new content): the
                // conflict check guarantees it is byte-identical to the
                // installed pack, but with the mark gone nothing would ever
                // delete it — up to 200MiB of permanent duplicate. Sweep it
                // like the no-pkg-dir arm; a failed sweep keeps the mark so
                // the next boot retries instead of silently stranding the
                // residue (round-21 review P3).
                let staged = pkg_dir.with_extension("tmp");
                if staged.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&staged) {
                        log::warn!(
                            "[plugin-import] import journal: sweeping the crash-orphaned staging dir for '{id}' failed, keeping the mark for the next startup: {e}"
                        );
                        continue;
                    }
                    log::warn!(
                        "[plugin-import] import journal: swept the crash-orphaned staging dir for '{id}' (a reimport crashed mid-staging)"
                    );
                }
                clear_landing(id);
            }
            Ok(None) => {
                // Landed live-by-absence: quarantine out of bundles_root so
                // no scan admits it, recoverable by hand from the holding
                // dir.
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let holding = landing_journal_dir().join(format!("{id}.crash-{stamp}"));
                match std::fs::rename(&pkg_dir, &holding) {
                    Ok(()) => {
                        log::warn!(
                            "[plugin-import] import journal: '{id}' landed without a registry record (crash between landing and registration); moved to {} — restore by hand if it is yours",
                            holding.display()
                        );
                        crate::platform::startup::mark_with_detail(
                            "rust",
                            "import_journal:crash_residue_quarantined",
                            id,
                        );
                        // Round-26 MAJOR 1 tail: with the backup restored
                        // above, a re-import's staged `.tmp` can still sit
                        // here (the record disappeared between the crash and
                        // this reconcile — e.g. a concurrent uninstall).
                        // Nothing else would ever delete it and
                        // `available_tools` has no `.tmp` exclusion, so sweep
                        // it like the other arms; a failed sweep keeps the
                        // mark for the retry.
                        let staged = pkg_dir.with_extension("tmp");
                        if staged.exists() {
                            if let Err(e) = std::fs::remove_dir_all(&staged) {
                                log::warn!(
                                    "[plugin-import] import journal: sweeping the crash-orphaned staging dir for '{id}' failed, keeping the mark for the next startup: {e}"
                                );
                                continue;
                            }
                            log::warn!(
                                "[plugin-import] import journal: swept the crash-orphaned staging dir for '{id}' (a re-import crashed mid-renames)"
                            );
                        }
                        clear_landing(id);
                    }
                    Err(e) => {
                        log::warn!(
                            "[plugin-import] import journal: moving crashed landing '{id}' aside failed, deferring to the next startup: {e}"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// 统一导入：解压插件包（mcp / skill / 组合）→ 安全校验 → 识别 → 落盘
/// `bundles/<id>/`（mcp/ + skills/ + 图标）→ 登记 BundleStore。
///
/// Test-only scaffolding: this wrapper runs the pipeline with NO consent
/// gate, so content would land without any deny registration. Every
/// production channel must go through `import_plugin_package_gated` (the
/// deny-first pre-land hook) — the same standing rule as
/// `skill_marketplace::import_package_named`.
#[cfg(test)]
pub fn import_plugin_package(
    zip_path: &str,
    display_name: &str,
) -> Result<PluginImportReport, String> {
    import_plugin_package_gated(zip_path, display_name, &|_, _| Ok(()))
}

/// Same pipeline with a pre-land gate hook: `pre_land` runs after the package
/// id is fixed and fully validated but BEFORE any content lands on disk or
/// replaces an existing installation. A `pre_land` refusal aborts the import
/// with nothing touched — the DenyAll consent gate uses this to register the
/// deny entry before the package is exposed (#515/#517 review round 4):
/// rolling an already-landed import back via uninstall would destroy a
/// pre-existing installation (re-import overwrites), while a deny-first
/// refusal loses nothing.
pub fn import_plugin_package_gated(
    zip_path: &str,
    display_name: &str,
    pre_land: &dyn Fn(&str, &[String]) -> Result<(), String>,
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
    // pass1 各分支共用的「有界读取 + 真实字节累计 + zip bomb 兜底」：zip 头声明
    // 可能与真实大小不符（见上方两口径预算注释），read_to_end 后按真实字节复核上限。
    let mut read_bounded_checked = |entry: &mut zip::read::ZipFile<'_, std::fs::File>,
                                    what: &str|
     -> Result<Vec<u8>, String> {
        let declared_size = entry.size();
        let buf = read_zip_entry_bounded(entry, declared_size, what)?;
        actual_total = actual_total.saturating_add(buf.len() as u64);
        if actual_total > MAX_PLUGIN_SIZE_BYTES {
            return Err(format!(
                "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
                MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
            ));
        }
        Ok(buf)
    };
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip 条目 #{i}: {e}"))?;
        // pass1 安全闸（穿越/symlink/声明大小上限）+ 净化路径，与技能 zip
        // 导入通道共用 `checked_zip_entry_path`。
        let path_str = checked_zip_entry_path(
            &mut entry,
            &mut declared_total,
            MAX_PLUGIN_SIZE_BYTES,
            "插件包",
        )?;
        if entry.is_dir() {
            continue;
        }
        all_paths.push(path_str.clone());
        if path_str == "plugin.json" {
            let buf = read_bounded_checked(&mut entry, "plugin.json")?;
            manifest_bytes = Some(buf);
        } else if path_str == "mcp/manifest.json" {
            let buf = read_bounded_checked(&mut entry, "mcp/manifest.json")?;
            mcp_manifest_bytes = Some(buf);
        } else if (path_str == "icon.svg" || path_str == "icon.png") && icon_entry.is_none() {
            let buf = read_bounded_checked(&mut entry, "图标")?;
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
                let buf = read_bounded_checked(&mut entry, "SKILL.md")?;
                best_skill_md = Some((path_str.clone(), buf));
            }
        }
        // 裸 MCP 回退：记住其它 manifest.json（排除 mcp/manifest.json 与 plugin.json）。
        if (path_str == "manifest.json" || path_str.ends_with("/manifest.json"))
            && path_str != "mcp/manifest.json"
            && path_str != "plugin.json"
        {
            let buf = read_bounded_checked(&mut entry, "manifest.json")?;
            other_manifests.push((path_str, buf));
        }
    }

    // Trust boundary (docs/builtin-toolset-contract.md §3.1): builtin status is
    // declared solely by the publisher through the compile-time embedded
    // catalog. An uploaded package claiming `builtin: true` or
    // `visibility: "system"` in any MCP manifest is rejected loudly — never
    // silently stripped, or the uploader would believe the claim took effect.
    if let Some(bytes) = &mcp_manifest_bytes {
        reject_builtin_claims_in_upload("mcp/manifest.json", bytes)?;
    }
    for (path, bytes) in &other_manifests {
        reject_builtin_claims_in_upload(path, bytes)?;
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
    let (id, mcp_servers, skills) = (det.id.clone(), det.mcp_servers.clone(), det.skills.clone());
    let kind = crate::features::marketplace::bundle::derive_bundle_kind(&mcp_servers, &skills)
        .map_err(|_| "插件包不含任何组件（空包）".to_string())?;

    // 未声明技能子树拒收（五轮评审）：detect 只登记声明/识别出的技能，而落盘的
    // `skills/` 前缀分支按路径无条件放行——zip 额外夹带的 `skills/<other>/` 会
    // 静默落盘成无卡片、无开关、卸载保护残留的不可见孤儿技能（与四轮 M-3 相反
    // 方向的缺口：M-3 修「声明的不落盘」，这里修「落盘的不在声明里」）。
    for p in &all_paths {
        if let Some(rest) = p.strip_prefix("skills/") {
            let name = rest.split('/').next().unwrap_or("");
            if !name.is_empty() && !skills.iter().any(|s| s == name) {
                return Err(format!(
                    "技能子树 'skills/{name}/' 未在包声明/识别的技能列表中，拒收（避免不可见孤儿技能落盘）"
                ));
            }
        }
    }

    // 拒绝与预置/内置包 id 冲突：用户上传包顶替市场预置会让 UI/默认值/资源池
    // 全部错位，且无法回滚（预置版本指纹与上传不同）。内置 CLI 连接器另由
    // `mcp_catalog` 索引覆盖（结构 Rust 函数式 API）。
    if crate::features::marketplace::mcp_catalog::spec_for_builtin_probe(&id).is_some() {
        return Err(format!(
            "包 id '{id}' 与市场预置 MCP 冲突，请改用其它 id 或通过市场直接安装"
        ));
    }
    // Component ids must not impersonate builtins either (review round-5 M1):
    // the package-id collision check cannot see them — a `my-tool` package
    // carrying an mcp component with id `session-reader` would have
    // add_to_mcp_json key the server entry by manifest.id and overwrite the
    // real builtin's engine registration (uninstalling my-tool removes only
    // its own key, orphaning the hijacked entry). The probe folds case, the
    // same discipline as the package-id check.
    for component in &mcp_servers {
        if crate::features::marketplace::mcp_catalog::spec_for_builtin_probe(component).is_some() {
            return Err(format!(
                "组件 id '{component}' 与市场预置 MCP 冲突，请改用其它 id 或通过市场直接安装"
            ));
        }
    }
    // The inner mcp/manifest.json id is force-equal to the component id by
    // detect_components, but a bare mcp/ package without plugin.json computes
    // its components from the inner manifest — rejected by the same builtin
    // probe.
    if let Some(bytes) = &mcp_manifest_bytes {
        if let Ok(inner) =
            serde_json::from_slice::<crate::features::marketplace::types::ToolManifest>(bytes)
        {
            if crate::features::marketplace::mcp_catalog::spec_for_builtin_probe(&inner.id)
                .is_some()
            {
                return Err(format!(
                    "组件 id '{}' 与市场预置 MCP 冲突，请改用其它 id 或通过市场直接安装",
                    inner.id
                ));
            }
        }
    }
    if !crate::features::marketplace::bundle::cli_bundle_skill_dirs(&id).is_empty()
        || crate::features::marketplace::bundle::builtin_cli_bundle_ids()
            .any(|cid| cid.eq_ignore_ascii_case(&id))
    {
        return Err(format!("包 id '{id}' 与内置 CLI 连接器冲突，请改用其它 id"));
    }
    // 已下线内置技能名拒收（plugin-package-spec §10 承诺的导入校验）：包 id 与
    // 任一技能组件名都不得与退役名单冲突（旧管线只拦技能路径，插件包管线此前
    // 未接，二轮评审文档失实）。
    if crate::features::marketplace::skill_marketplace::RETIRED_SKILL_NAMES.contains(&id.as_str()) {
        return Err(format!(
            "包 id '{id}' 与已下线内置技能名冲突，请改用其它 id"
        ));
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
    // Preset skill-market id rejection covers only the "alias-split" shape
    // (round-11 P2-2): an id unlike every preset skill name (e.g.
    // tencent-docs-skill) is an independent entry in the deny-list vocabulary,
    // so an upload claiming it would make the consent gate treat the real
    // preset as already known and skip its registration (fail-open), and
    // read-time normalization would fold the preset's alias vocabulary onto
    // the upload. Ids that ARE preset skill names (gongwen-family ids where
    // id == skill name) are NOT blanket-rejected: the export → re-import
    // round-trip contract (package_export's round-trip pin) allows such user
    // package ids while they self-map. The owner-claim divergence check
    // immediately below closes the remaining window: with an installed
    // claimant the id no longer self-maps, and importing under it would leave
    // the consent gate registering (or skipping on) the claimant's id — so
    // the round-trip contract holds without a fail-open exception.
    if crate::features::marketplace::skill_marketplace::is_preset_market_id(&id)
        && !crate::features::marketplace::skill_marketplace::is_preset_skill_name(&id)
    {
        return Err(format!(
            "包 id '{id}' 与市场预置技能 id 冲突，请改用其它 id 或从市场安装"
        ));
    }
    // Owner-claim divergence (round-12 review B1): `to_package_id` folds an
    // id onto an installed claimant's consent vocabulary — an installed
    // package's declared companion skill name, a CLI companion dir, or
    // `ima-skills`. Importing a PACKAGE under such an id would make the
    // consent gate register (or skip on) the claimant's id while this
    // package lands keyed by its own id, ungoverned in initialized DenyAll
    // scopes (live-reproduced: gongwen installed + import id
    // `government-writing`). The check is state-dependent on purpose: with
    // no claimant installed the id self-maps and stays importable, which
    // keeps the export → re-import round-trip contract
    // (`export_installed_plugin_allows_preset_named_id_without_preset_skill`)
    // intact. The skill-name half of the same collision is already rejected
    // by the component owner check below.
    let folded = crate::features::marketplace::scope::to_package_id(&id);
    if folded != id {
        return Err(format!(
            "包 id '{id}' 已被包 '{folded}' 的配套技能占用，请改用其它 id"
        ));
    }
    // Preset/companion/cross-package skill name collisions are rejected up
    // front, consistent with the two skill_marketplace channels (which sweep
    // duplicate copies after install — this pipeline never sweeps, so without
    // a guard a collision would escalate from "shadowed" to "swept away").
    // Self-claim is allowed (owner == this package id, e.g. a combo package
    // whose mcp manifest declares its own skill as companion on reimport).
    for skill_name in &skills {
        if crate::features::marketplace::skill_marketplace::is_preset_skill_name(skill_name) {
            return Err(format!(
                "技能 '{skill_name}' 与市场预置技能冲突，请改用其它名称"
            ));
        }
        if crate::features::marketplace::skill_marketplace::is_preset_market_id(skill_name) {
            return Err(format!(
                "技能 '{skill_name}' 与市场预置技能 id 冲突，请改用其它名称"
            ));
        }
        let owner = crate::features::marketplace::bundle::skill_owner_package(skill_name);
        if owner != *skill_name && owner != id {
            return Err(format!(
                "技能 '{skill_name}' 已被包 '{owner}' 的配套技能占用，请改用其它名称"
            ));
        }
        if let Some(other) =
            crate::features::marketplace::skill_marketplace::foreign_skill_copies_under(
                &crate::platform::paths::bundles_root(),
                skill_name,
                &id,
            )
            .into_iter()
            .next()
        {
            return Err(format!(
                "技能 '{skill_name}' 已存在于包 '{other}'，请先卸载该包或改用其它名称"
            ));
        }
    }
    // 技能组件一致性（plugin-package-spec §8 承诺的导入校验）：组件 id 必须与
    // SKILL.md frontmatter 的 `name` 一致——不一致会被注册表当两套技能处理。
    // 必读必验（四轮评审 M-3）：SKILL.md 缺失或读取失败一律响亮拒收，不再
    // continue 跳过（否则纯技能包可能登记 installed=true 但磁盘无技能文件）。
    for skill_name in &skills {
        // 裸技能回退包的 SKILL.md 在 zip 原始位置（detect 已确认其存在并解析出
        // frontmatter name），其余组件一律在规范化 `skills/<name>/` 下。
        let rel = match &det.bare_skill {
            Some((root, name)) if name == skill_name => {
                if root.is_empty() {
                    "SKILL.md".to_string()
                } else {
                    format!("{root}/SKILL.md")
                }
            }
            _ => format!("skills/{skill_name}/SKILL.md"),
        };
        let mut entry = archive
            .by_name(&rel)
            .map_err(|e| format!("技能 '{skill_name}' 的 {rel} 缺失或不可读，拒收: {e}"))?;
        let declared_size = entry.size();
        let buf = read_zip_entry_bounded(&mut entry, declared_size, &rel)?;
        if let Some(fm_name) =
            crate::features::marketplace::skill_marketplace::read_skill_name_from_str(
                std::str::from_utf8(&buf).unwrap_or(""),
            )
        {
            if fm_name != *skill_name {
                return Err(format!(
                    "技能 '{skill_name}' 的 SKILL.md frontmatter name 为 '{fm_name}'，与组件 id 不一致（plugin-package-spec §8）"
                ));
            }
        }
    }

    // Round-32 minor 6 (review #455), re-placed by the round-18 review: mark
    // the landing in the import journal BEFORE the pre-land consent gate —
    // i.e. before anything this import will write, including its deny-first
    // registration — and clear it when this call exits (any in-process
    // outcome — gate refusal, supply-failure rollback, or rename failure).
    // Only a process death between the mark and the exit leaves the mark
    // behind, which is exactly the window the round-24 rollback cannot
    // cover: the pack dir is live on disk with no registry record, admitted
    // live-by-absence in initialized scopes with zero consent. The startup
    // reconciliation ([`reconcile_import_journal`]) resolves the leftover.
    // The earlier placement is what makes the mark usable as a cross-process
    // in-flight signal: the startup sweep's pre-strip probe
    // ([`landing_in_progress`]) must be able to see an import that has
    // already registered its deny-first rows but not yet landed, or the
    // sweep's vacuous-uninstall retry leg would strip those fresh rows.
    // Round-20 review (P1): acquire the cross-process landing lease BEFORE
    // the mark. The mark alone cannot distinguish a live import from a
    // crashed one, so the boot reconcile treated "mark + no pack dir" as
    // crash residue and could sweep a live peer's staged dir mid-extraction
    // and clear its mark (the import's own crash protection) — and the
    // retired-tool sweep later in that boot could then strip the rows this
    // import had just registered. The lease is the liveness signal the
    // reconcile probes (and the uninstall / retired sweep hold): held from
    // here until the end of this call, it makes the whole mark→gate→land→
    // register span mutually exclusive against every LEASE-HOLDING same-id
    // peer — the MCP uninstall and restore take it blocking, the boot
    // reconcile and retired sweep probe it with try-and-defer. NOT covered
    // (round-26 review, disclosed in §3.2): the skill lane's install /
    // uninstall serialize in-process on the per-id import mutex only, and
    // the boot python-repair downgrade runs under the transaction lock only
    // — a cross-process skill-uninstall-vs-import of the same id can still
    // interleave; closing that needs the same lease in those lanes (with an
    // uninstall_leased-style held-lease variant for the nested companion
    // cleanup), registered as follow-up.
    // Declared
    // before the landing guard on purpose: locals drop in reverse order, so
    // the guard clears the mark while this process still holds the lease.
    // Blocking `write()`: a peer's same-id import waits out here instead of
    // racing the shared staged `.tmp` / `.old` paths that only the
    // in-process mutex below used to serialize (that mutex never excluded
    // other processes); the OS releases the lease if the peer dies, and a
    // frozen peer blocks the import (spawn_blocking), the same accepted
    // fail-stop tradeoff as the scope lock.
    let mut landing_lease = open_landing_lease(&id)
        .map_err(|error| format!("open the landing lease for {id}: {error}"))?;
    let _landing_lease_guard = landing_lease
        .write()
        .map_err(|error| format!("lock the landing lease for {id}: {error}"))?;
    mark_landing(&id);
    let _landing_guard = LandingJournalGuard(id.clone());

    // Pre-land gate hook: the id is final and validated here, nothing has
    // been written yet. A refusal aborts the whole import before any content
    // lands or an existing installation is replaced (deny-first consent gate,
    // #517 review round 4). The identified skill components ride along so
    // the gate can also register standalone components deny-first
    // (round-11 P2-3). Runs before the same-id import lock on purpose:
    // the gate then takes no import lock at all, so the import lock is never
    // held ACROSS a wait on another importer (the scope RMW lock itself may
    // be nested inside transaction/import/recycle locks one-directionally —
    // see `scope::with_scope_file_lock`'s lock-order note; the reverse
    // nesting never happens). The landing lease acquired above is the
    // load-bearing cross-process exclusion (round-20 review); the mark is
    // what a concurrent sweep keys on as the belt-and-braces signal — the
    // gate's registration is invisible to the sweep's record probe until
    // the import lands, but the mark and the lease are already in place.
    pre_land(&id, &skills)?;

    // 落盘到 staged：mcp/ + skills/ 子树 + 裸包回退规范化 → bundles/<id>/ 原子 rename。
    // 注：旧 spanner/ 与 runtime/ 子树已删除，导入侧不再识别这两类前缀。
    let pkg_dir = crate::platform::paths::bundles_root().join(&id);
    // 进程内导入互斥（四轮评审 M-4）：同 id 并发导入共享 staged `.tmp` 与 `.old`
    // 备份路径，且冲突检查与原子 rename 之间无锁即 TOCTOU——持锁覆盖「冲突检查
    // → 原子 rename 完成」整段临界区（guard 至函数尾生效，详见 import_lock_for）。
    let import_lock = import_lock_for(&id);
    let _import_guard = import_lock.lock().unwrap_or_else(|p| p.into_inner());
    // Re-check the fold divergence under the import lock (round-14 P2): the
    // first check ran before the consent gate, and a claimant (preset skill
    // install, CLI companion, ima connect) completing in between would fold
    // this id onto the claimant — the gate registers (or skips on) the
    // claimant's id while this package lands keyed by its own id, ungoverned.
    // Nothing has landed at this point, so the refusal aborts with the home
    // untouched (the gate's earlier registration is an over-denial row on an
    // installed claimant — fail-closed).
    let folded = crate::features::marketplace::scope::to_package_id(&id);
    if folded != id {
        return Err(format!(
            "包 id '{id}' 已被包 '{folded}' 的配套技能占用，请改用其它 id"
        ));
    }
    // 上传包 id 冲突：目标包目录已存在且内容不同 → 拒绝（提示改名重试），避免
    // 不同包静默互覆盖（二轮评审：冲突检查需覆盖上传包）。内容一致视为同包
    // 重导/升级，允许走原子替换。比对为全内容口径（五轮评审，详见
    // same_package_content）：plugin.json 语义 + 全部可落盘条目字节 + 图标 +
    // 磁盘多余文件，server.py/SKILL.md 等执行代码改动同样判冲突。
    if pkg_dir.exists()
        && !same_package_content(
            &pkg_dir,
            &mut archive,
            manifest_bytes.as_deref(),
            &det,
            &all_paths,
            icon_entry.as_ref(),
        )
    {
        return Err(format!(
            "包 id '{id}' 已存在且内容不同，请改名后重试（避免覆盖已有包）"
        ));
    }
    // bundles/<id> comes from joining bundles_root(), so a parent always
    // exists; still return an error as a fallback.
    let Some(parent) = pkg_dir.parent() else {
        return Err(format!("package dir missing parent: {}", pkg_dir.display()));
    };
    std::fs::create_dir_all(parent).map_err(|e| format!("创建 bundles 目录: {e}"))?;
    let staged = pkg_dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("暂存目录: {e}"))?;

    let write_result = (|| -> Result<(), String> {
        // 1) 统一落盘：mcp/ + skills/ 子树 + 裸包回退规范化（旧 spanner/、runtime/
        //    子树已删除，见 landing_target 注释）。
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
            let declared_size = entry.size();
            let buf =
                read_zip_entry_bounded(&mut entry, declared_size, &format!("条目 {path_str}"))?;
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
                    let declared_size = e.size();
                    let buf = read_zip_entry_bounded(&mut e, declared_size, "图标")?;
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

    // 安装时 smoke test：旧 spanner 自检已删除。当前只保留纯 MCP / 纯 skill 落盘。

    // The landing mark was written before the pre-land gate above (round-18
    // re-placement): the journal now covers the whole gate + staging + landing
    // span, so both a crash mid-gate and a crash mid-landing leave the
    // recoverable trace, and the sweep's pre-strip probe sees every state in
    // which this import's deny-first rows may already exist.

    // 原子落盘：先把旧目录挪到 .old 备份，rename 成功后再删 .old；rename 失败则
    // 把 .old 复原回去，保证「旧包不丢、新包不入」——避免既往版本 `remove+rename`
    // 任一环节失败导致新旧双丢的窗期。
    let backup = pkg_dir.with_extension("old");
    let mut moved_old = false;
    if pkg_dir.exists() {
        let _ = std::fs::remove_dir_all(&backup);
        if rename_dir_with_retry(&pkg_dir, &backup).is_ok() {
            moved_old = true;
        }
    }
    if let Err(e) = rename_dir_with_retry(&staged, &pkg_dir) {
        // rename 失败：尝试把旧目录复原，让已安装版本继续可用。
        // Round-24 minor: the restore must not be silent — `rename_dir_with_retry`
        // never logs, so a failed restore stranded the pre-import content at
        // `<id>.old` with no trace (§3.2 promises every rollback arm logs
        // loudly). Name the strand in the log and in the caller's error,
        // mirroring the supply-failure arm below.
        if moved_old {
            if let Err(re) = rename_dir_with_retry(&backup, &pkg_dir) {
                log::error!(
                    "[plugin-import] 落盘失败且旧目录复原失败（{id}）: 旧内容滞留在 {backup:?}（数据未丢，请手动移回 {pkg_dir:?}）: {re}"
                );
                let _ = std::fs::remove_dir_all(&staged);
                return Err(format!(
                    "落盘: {e}; the previous version's restore also failed: the old content is stranded at {backup:?} (no data lost; move it back by hand): {re}"
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("落盘: {e}"));
    }
    // Round-24 MAJOR 2: the backup is retained until supply resolves. A
    // supply failure after landing must not leave the new dir behind — its
    // skills materialize into every initialized scope with zero consent (the
    // consent sync never runs on this error path) and self_heal_skills skips
    // plugin dirs, so the half-state would persist across boots. The backup
    // is deleted only once supply has succeeded.

    // 导入即重基线（口径）：包内容整体替换后，旧包的 SKILL.md 说明备份（存于
    // extra，upsert_preserving 原样保留）随之失效——不清掉会让「清覆盖恢复」
    // 把**旧包**的原描述写进新包（口径同 skill_marketplace::import_package_named）。
    // 三条 UI 上传通道（选文件/拖 zip/拖 .md）都汇聚在这条统一导入路径上；首装
    // 无备份时不写，避免无谓 churn。
    // Round-29 m5 (review #455): the old placement contract that lived here
    // ("rebaseline immediately after the rename, BEFORE any supply step") is
    // superseded by the round-27 m5 reorder — the consume point now sits
    // AFTER supply (below): a moved_old supply-failure rollback restores the
    // old files, so the backup must survive until supply resolves. Do not
    // move the call back up on the old contract's authority.
    // 锁边界（MINOR 1 口径）：统一导入全程持有 import_lock（guard 至函数尾）；
    // 展示说明的回写/恢复（update_display_meta → sync_display_description）
    // 在 SKILL.md 读改写段**同样持有同 id 的 import_lock**（锁序一致：
    // import_lock → store file_lock），同 id 的导入与编辑因此真正互斥，无
    // 「读备份 → 写 SKILL.md」窗口被重导入插队的竞态。
    let store = super::store::BundleStore::new();
    // 供给：MCP 组件走 install 管线写 mcp.json + installed.json（底座据此拉起 server，
    // 工具才能注册可用）。纯 skill 包无 mcp/ 目录，跳过（技能走物化通道）。
    // 注：旧 spanner 供给路径已删除；skill 包无可执行供给（tools[]/runtime 协议
    // 为 RFC 草案，执行通路未实施）。
    // 上传包走 `install_upload`：pip_dependencies 不自动 pip install（供应链安全），
    // 非空时只在日志提示用户自行安装。Upload 来源随供给的镜像写一并登记（而不是
    // 靠下方补写订正）——补写失败仅 log::warn，若 source 在补写前停在 Preset，
    // 下次卸载会误删用户唯一副本（四轮评审 BLOCKER 1）。
    let mgr = super::MarketplaceManager::new();
    if !mcp_servers.is_empty() {
        if let Err(e) = mgr.install_upload(
            &id,
            super::store::BundleSource::Upload(display_name.to_string()),
        ) {
            // Round-24 MAJOR 2: mirror the restore path's supply-failure
            // rollback — restore the previous version (or remove the
            // first-install landing) instead of leaving the recordless dir
            // live. Rollback failure is logged loudly; the caller's error
            // stays the supply error either way.
            let rolled_back = if moved_old {
                let _ = std::fs::remove_dir_all(&pkg_dir);
                rename_dir_with_retry(&backup, &pkg_dir).is_ok()
            } else {
                std::fs::remove_dir_all(&pkg_dir).is_ok()
            };
            if !rolled_back {
                log::error!(
                    "[plugin-import] 供给失败且目录回滚失败（{id}）: 残留目录的技能将以零同意进入已初始化 scope，请检查 {pkg_dir:?}"
                );
            }
            // Round-26 minor 5 (review #455): when the rollback itself failed,
            // the landed dir stayed live with zero consent — the user-facing
            // error must carry that, not just the log line. Round-31 m7
            // (review #455): the suffix is diff-authored user-visible copy —
            // English sibling-form (like the surrounding English-language
            // skill_gate/ima errors), not CJK that would surface raw to
            // en/ja users.
            let suffix = if rolled_back {
                String::new()
            } else {
                format!(
                    "; the directory rollback also failed: the landed {id} directory is still on disk with zero consent — remove it manually or retry the import: {pkg_dir:?}"
                )
            };
            return Err(format!("MCP 供给失败（{id}）: {e}{suffix}"));
        }
    }
    // Round-27 m5 (review #455): rebaseline (consume) the description backup
    // only after supply succeeded — on a moved_old rollback the old files are
    // restored, so the restore point must survive for the next attempt; the
    // ordering comment's justification now matches the failure path.
    super::skill_marketplace::rebaseline_skill_desc_backup(&store, &id, "统一导入");
    if moved_old {
        // Round-24 minor: the post-supply backup delete is the same
        // silent-strand family — a failure leaves the pre-import content in
        // `bundles/<id>.old` with no mark and no log (the reconcile scans
        // marks only). Harmless to the import itself (the new content landed
        // and registered), but the strand is invisible residue; log it.
        if let Err(del_err) = std::fs::remove_dir_all(&backup) {
            log::warn!(
                "[plugin-import] 统一导入（{id}）: 旧内容备份删除失败，滞留在 {backup:?}（数据未丢，可手动删除）: {del_err}"
            );
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
    // 真实内容指纹（与 mcp_catalog 释放/技能 install 同一 dir_fingerprint 口径；
    // 计算失败不留假指纹，降级为 None）。
    record.content_fingerprint = match super::skill_marketplace::dir_fingerprint(&pkg_dir) {
        Ok(fp) => Some(fp),
        Err(e) => {
            log::warn!("[plugin-import] 计算包内容指纹失败（{id}）: {e}");
            None
        }
    };
    if let Err(e) = super::store::BundleStore::new().upsert_preserving(record) {
        // Round-20 P2: fail visible. The dir is live and supply succeeded, so
        // this is not rollback territory — but swallowing the failure into a
        // success report left a recordless-live pack (no Upload-source
        // protection, no recycle-on-uninstall, no display metadata) with the
        // journal mark already cleared, so the boot reconcile could never see
        // it. The caller surfaces the error; a retry re-imports the same id
        // (no record ⇒ no conflict) and converges. Consent stays closed
        // either way: the pre-land deny-first gate already registered the id
        // (no record vouched known then, and nothing vouches later without
        // one), so the pack stays default-off in initialized DenyAll scopes.
        log::error!("[plugin-import] bundles.json 镜像写入失败（import {id}）: {e}");
        return Err(format!(
            "{id} landed on disk, but the registration write failed ({e}): the pack is usable; retry the import to complete the registration, or uninstall and reinstall"
        ));
    }
    // （导入即重基线：包内容整体替换后旧包的说明备份随之失效。Round-29 m5
    // (review #455): the consume point's ordering contract now lives at the
    // rebaseline call above (round-27 m5: only AFTER supply succeeds — a
    // moved_old rollback restores the old files and the backup must survive
    // for the next attempt); this upsert site is not the rebaseline point.)

    Ok(PluginImportReport {
        id,
        kind,
        icon: icon_rel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-32 minor 6 (review #455): a leftover landing mark with a landed
    /// dir but no registry record is crash residue — the reconcile moves it
    /// out of `bundles_root` into a recoverable holding dir and clears the
    /// mark.
    #[test]
    fn reconcile_quarantines_landed_dir_without_registry_record() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-import-journal-crash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let pkg = crate::platform::paths::bundles_root().join("crash-pkg");
        std::fs::create_dir_all(pkg.join("skills/crash-pkg")).unwrap();
        std::fs::write(
            pkg.join("skills/crash-pkg/SKILL.md"),
            "---\nname: crash-pkg\n---\n",
        )
        .unwrap();
        mark_landing("crash-pkg");
        assert!(
            landing_mark_path("crash-pkg").exists(),
            "fixture: mark present"
        );

        reconcile_import_journal().unwrap();

        assert!(
            !pkg.exists(),
            "crash residue must not stay live-by-absence in bundles_root"
        );
        let moved: Vec<String> = std::fs::read_dir(landing_journal_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("crash-pkg.crash-"))
            .collect();
        assert_eq!(
            moved.len(),
            1,
            "the landed dir moves to exactly one recoverable holding dir: {moved:?}"
        );
        assert!(
            landing_journal_dir()
                .join(&moved[0])
                .join("plugin.json")
                .is_file()
                || landing_journal_dir()
                    .join(&moved[0])
                    .join("skills")
                    .is_dir(),
            "the holding dir keeps the pack contents for manual recovery"
        );
        assert!(
            !landing_mark_path("crash-pkg").exists(),
            "the resolved entry is cleared"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// round-21 review (P3): the record-present reconcile arm must also sweep
    /// a crash-orphaned reimport staging dir — with the mark cleared nothing
    /// else would ever delete it (a permanent duplicate of the installed
    /// pack). A FAILED sweep keeps the mark for the next boot instead of
    /// silently stranding the residue; the retry leg then sweeps and clears.
    #[test]
    fn record_present_arm_sweeps_orphaned_staging_and_keeps_mark_on_failure() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-arm-a-tmp-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // Fixture: installed pack (dir + record) + landing mark + a
        // crash-orphaned staged `.tmp`. Planted as a regular FILE so
        // remove_dir_all fails deterministically on every platform (ENOTDIR)
        // for the failure phase.
        let pkg_dir = crate::platform::paths::bundles_root().join("re-tmp");
        std::fs::create_dir_all(pkg_dir.join("skills/re-tmp")).unwrap();
        std::fs::write(
            pkg_dir.join("skills/re-tmp/SKILL.md"),
            "---\nname: re-tmp\n---\n",
        )
        .unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "re-tmp",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "re-tmp.zip".to_string(),
                    ),
                ),
            )
            .unwrap();
        mark_landing("re-tmp");
        let staged = pkg_dir.with_extension("tmp");
        std::fs::write(&staged, b"stale staging").unwrap();

        reconcile_import_journal().unwrap();

        assert!(
            staged.is_file(),
            "a failing sweep must not destroy what it could not remove"
        );
        assert!(
            landing_in_progress("re-tmp"),
            "a failed sweep must keep the mark so the next boot retries"
        );
        assert!(
            pkg_dir.join("skills/re-tmp/SKILL.md").is_file(),
            "the installed pack is untouched"
        );

        // Retry leg: make the sweep succeed (remove the blocking file) and
        // reconcile again — the staging path is swept and the mark cleared.
        std::fs::remove_file(&staged).unwrap();
        std::fs::create_dir(&staged).unwrap();
        reconcile_import_journal().unwrap();
        assert!(
            !staged.exists(),
            "the retry leg must sweep the orphaned staging dir"
        );
        assert!(
            !landing_in_progress("re-tmp"),
            "the resolved entry is cleared"
        );
        assert!(
            pkg_dir.join("skills/re-tmp/SKILL.md").is_file(),
            "the installed pack survives the sweep"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-24 minor: the no-pkg-dir reconcile arm (the import never
    /// landed) gets the same keep-mark-on-failure treatment as the
    /// record-present arm above — its unconditional clear stranded the
    /// ghost `.tmp` forever on EACCES/AV-hold (nothing reschedules a swept
    /// mark) while the log claimed success. Same planted-regular-file trick
    /// makes `remove_dir_all` fail deterministically on every platform;
    /// the retry leg then sweeps and clears.
    #[test]
    fn no_pkg_dir_arm_sweeps_orphaned_staging_and_keeps_mark_on_failure() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-arm-a2-tmp-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // Fixture: NO package dir (the import never landed), no store
        // record, landing mark, and a crash-orphaned staged `.tmp` planted
        // as a regular FILE so remove_dir_all fails with ENOTDIR.
        std::fs::create_dir_all(crate::platform::paths::bundles_root()).unwrap();
        let pkg_dir = crate::platform::paths::bundles_root().join("never-tmp");
        mark_landing("never-tmp");
        let staged = pkg_dir.with_extension("tmp");
        std::fs::write(&staged, b"stale staging").unwrap();

        reconcile_import_journal().unwrap();

        assert!(
            staged.is_file(),
            "a failing sweep must not destroy what it could not remove"
        );
        assert!(
            landing_in_progress("never-tmp"),
            "a failed sweep must keep the mark so the next boot retries"
        );
        assert!(
            !pkg_dir.exists(),
            "the no-pkg-dir precondition must hold throughout"
        );

        // Retry leg: make the sweep succeed and reconcile again — the
        // staging path is swept and the mark cleared.
        std::fs::remove_file(&staged).unwrap();
        std::fs::create_dir(&staged).unwrap();
        reconcile_import_journal().unwrap();
        assert!(
            !staged.exists(),
            "the retry leg must sweep the orphaned staging dir"
        );
        assert!(
            !landing_in_progress("never-tmp"),
            "the resolved entry is cleared"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-26 MAJOR 1: a crash between the re-import's two renames (pack
    /// dir already in `<id>.old`, staged copy not yet renamed in, mark still
    /// on file, record still installed) must converge: the reconcile restores
    /// the pre-import backup in place, and the record-present arm then sweeps
    /// the staged `.tmp` and clears the mark. Previously this converged to
    /// "record installed, content gone, the only copy stranded in the
    /// scan-invisible `.old`" with no log and no retry.
    #[test]
    fn reconcile_restores_the_reimport_backup_after_a_crash_between_the_renames() {
        crate::platform::test_support::with_temp_home("pinvou-import-reold", || {
            let pkg_dir = crate::platform::paths::bundles_root().join("re-old");
            std::fs::create_dir_all(&pkg_dir).unwrap();
            std::fs::write(pkg_dir.join("plugin.json"), "{}").unwrap();
            let backup = pkg_dir.with_extension("old");
            std::fs::rename(&pkg_dir, &backup).unwrap();
            let staged = pkg_dir.with_extension("tmp");
            std::fs::create_dir_all(&staged).unwrap();
            std::fs::write(staged.join("plugin.json"), r#"{"new":true}"#).unwrap();
            crate::features::marketplace::store::BundleStore::new()
                .upsert(crate::features::marketplace::store::BundleRecord::installed_now(
                    "re-old",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "re-old.zip".to_string(),
                    ),
                ))
                .unwrap();
            mark_landing("re-old");

            reconcile_import_journal().unwrap();

            assert!(
                pkg_dir.join("plugin.json").is_file(),
                "the pre-import backup must be restored in place"
            );
            assert!(
                !backup.exists(),
                "the restored backup must not remain as a strand"
            );
            assert!(
                !staged.exists(),
                "the never-landed staged copy is swept by the record-present arm"
            );
            assert!(
                !landing_mark_path("re-old").exists(),
                "the converged entry is cleared"
            );
        });
    }

    /// Round-26 MAJOR 1 tail: same fixture with the record gone (e.g. a
    /// concurrent uninstall between the crash and this reconcile) — the
    /// restored backup must still not stay live-by-absence: the record-absent
    /// arm quarantines it, and the staged `.tmp` is swept with it.
    #[test]
    fn reconcile_restored_backup_quarantines_when_the_record_is_gone() {
        crate::platform::test_support::with_temp_home("pinvou-import-reold2", || {
            let pkg_dir = crate::platform::paths::bundles_root().join("re-old2");
            std::fs::create_dir_all(&pkg_dir).unwrap();
            std::fs::write(pkg_dir.join("plugin.json"), "{}").unwrap();
            let backup = pkg_dir.with_extension("old");
            std::fs::rename(&pkg_dir, &backup).unwrap();
            let staged = pkg_dir.with_extension("tmp");
            std::fs::create_dir_all(&staged).unwrap();
            std::fs::write(staged.join("plugin.json"), r#"{"new":true}"#).unwrap();
            mark_landing("re-old2");

            reconcile_import_journal().unwrap();

            assert!(
                !pkg_dir.exists(),
                "a restored dir without a record must be quarantined out of bundles_root"
            );
            let moved: Vec<String> = std::fs::read_dir(landing_journal_dir())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("re-old2.crash-"))
                .collect();
            assert_eq!(
                moved.len(),
                1,
                "the restored copy moves to exactly one recoverable holding dir: {moved:?}"
            );
            assert!(
                landing_journal_dir()
                    .join(&moved[0])
                    .join("plugin.json")
                    .is_file(),
                "the holding dir keeps the restored pre-import content"
            );
            assert!(
                !staged.exists(),
                "the staged copy is swept with the quarantine"
            );
            assert!(
                !landing_mark_path("re-old2").exists(),
                "the resolved entry is cleared"
            );
        });
    }

    /// Round-26 MAJOR 1: a failed backup restore keeps the mark so the next
    /// boot retries, instead of consuming the strand silently.
    #[test]
    #[cfg(unix)]
    fn restore_failure_keeps_the_mark_and_retries() {
        use std::os::unix::fs::PermissionsExt as _;
        crate::platform::test_support::with_temp_home("pinvou-import-reold3", || {
            let bundles = crate::platform::paths::bundles_root();
            std::fs::create_dir_all(&bundles).unwrap();
            let pkg_dir = bundles.join("re-old3");
            std::fs::create_dir_all(&pkg_dir).unwrap();
            std::fs::write(pkg_dir.join("plugin.json"), "{}").unwrap();
            let backup = pkg_dir.with_extension("old");
            std::fs::rename(&pkg_dir, &backup).unwrap();
            mark_landing("re-old3");

            // Read-only bundles_root: the restore rename fails (EACCES).
            std::fs::set_permissions(&bundles, std::fs::Permissions::from_mode(0o555)).unwrap();
            // Root probe (mode bits are no-ops for root): if the dir is still
            // writable the fixture proves nothing — skip loudly.
            if std::fs::create_dir(bundles.join(".probe")).is_ok() {
                std::fs::set_permissions(&bundles, std::fs::Permissions::from_mode(0o755)).unwrap();
                eprintln!(
                    "ROOT-SKIP[restore_failure_keeps_the_mark_and_retries]: running as root - the read-only-dir fixture stays writable; NOT exercised"
                );
                return;
            }

            reconcile_import_journal().unwrap();

            std::fs::set_permissions(&bundles, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                !pkg_dir.exists(),
                "the failed restore must not have half-landed the backup"
            );
            assert!(
                backup.exists(),
                "a failed restore must not consume the backup"
            );
            assert!(
                landing_mark_path("re-old3").exists(),
                "a failed restore keeps the mark so the next boot retries"
            );
        });
    }

    /// Round-26 MAJOR 2: the journal writer validates the fallback id with
    /// `is_safe_skill_name` (mixed case allowed — frontmatter
    /// `name: MySkill` arrives through the wrapped-`.md` channel), so the
    /// reconcile reader must accept the same set. The stricter
    /// `is_safe_component_id` gate used to strand these marks forever, with
    /// the live-by-absence and ghost-`.tmp` windows unrecovered.
    #[test]
    fn reconcile_processes_mixed_case_journal_marks() {
        crate::platform::test_support::with_temp_home("pinvou-import-mixed", || {
            let pkg_dir = crate::platform::paths::bundles_root().join("MySkill");
            std::fs::create_dir_all(&pkg_dir).unwrap();
            std::fs::write(pkg_dir.join("plugin.json"), "{}").unwrap();
            mark_landing("MySkill");

            reconcile_import_journal().unwrap();

            assert!(
                !pkg_dir.exists(),
                "a mixed-case landed dir without a record must be quarantined, not skipped"
            );
            let moved: Vec<String> = std::fs::read_dir(landing_journal_dir())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("MySkill.crash-"))
                .collect();
            assert_eq!(
                moved.len(),
                1,
                "the mixed-case residue moves to a recoverable holding dir: {moved:?}"
            );
            assert!(
                !landing_mark_path("MySkill").exists(),
                "the mixed-case mark must not strand"
            );
        });
    }

    /// round-21 review (P3): every lease test held the lease as a FIXTURE —
    /// deleting the pipeline's own acquisition (`open_landing_lease` +
    /// blocking `write()` before the mark) broke no test, so the round-20
    /// P1 fix's core span (lease held across mark→gate→land→register) was
    /// regression-unsafe. The behavioral harness cannot spawn a second
    /// process here, so pin the structure like the skill lane's
    /// strips-under-import-lock pin: the pipeline body must acquire the
    /// lease in CODE (not comments), before the mark, and must not drop it
    /// early.
    #[test]
    fn import_pipeline_acquires_the_landing_lease_in_code() {
        let source = include_str!("plugin_import.rs");
        let start = source
            .find("pub fn import_plugin_package_gated")
            .expect("the gated pipeline must exist");
        let rest = &source[start..];
        // The pipeline is followed by the tests module; the next top-level
        // `#[cfg(test)]` marker ends the production body slice.
        let end = rest.find("\n#[cfg(test)]").expect("tests module follows");
        // Match CODE, not comments — whole-line `//` comments are filtered
        // before the assertions so a doc/comment mention of the lease can
        // no longer satisfy the pin (round-16 review convention).
        let body = rest[..end]
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let lease_open = body
            .find("open_landing_lease(&id)")
            .expect("the pipeline must open the landing lease itself");
        let lease_lock = body
            .find(".write()")
            .expect("the pipeline must take the lease's write lock");
        let mark = body
            .find("mark_landing(&id)")
            .expect("the pipeline must write the landing mark");
        assert!(
            lease_open < lease_lock && lease_lock < mark,
            "the lease must be acquired (open then write-locked) BEFORE the mark"
        );
        assert!(
            body.contains("_landing_lease_guard"),
            "the lease guard must be bound to a name that outlives the import"
        );
        assert!(
            !body.contains("drop("),
            "the pipeline must not drop any guard before the import completes"
        );
    }

    /// Round-24 MAJOR 6: behavioral twin of the source-text pin above. The
    /// literals it greps survive a scoped-block release
    /// (`let _g = { let mut l = open_landing_lease(&id)?; l.write()? };` —
    /// every literal present, ordering kept, no `drop(`), and its own
    /// comment concedes every behavioral lease test holds the lease as a
    /// fixture — so tenancy needs the suite's established blocked-worker
    /// harness (fixture-held lease + production fn on a thread + bounded
    /// `recv_timeout`, the uninstall/restore/reconcile lanes' shape): the
    /// import pipeline must PARK while a peer holds the landing lease and
    /// proceed only after release.
    #[test]
    fn import_parks_while_a_peer_holds_the_landing_lease() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-import-lease-park-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/server.py", opts).unwrap();
            zw.write_all(b"import json\nprint(json.dumps({'ok': True}))")
                .unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        // The peer-import shape: hold the landing lease on a second fd.
        let mut lease = open_landing_lease("demo").expect("fixture: the landing lease opens");
        let guard = lease.write().expect("fixture: the landing lease acquires");

        let (tx, rx) = std::sync::mpsc::channel();
        let worker_zip = zip_path.clone();
        let worker = std::thread::spawn(move || {
            let result = import_plugin_package(&worker_zip.to_string_lossy(), "combo.zip");
            tx.send(result).expect("worker should send its result");
        });
        // Bounded: a regressed pipeline that ignores the lease completes
        // here and fails this assertion with a diagnosable receive; a
        // correct one stays parked on the lease.
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(2)).is_err(),
            "the import must park while a peer import holds the landing lease"
        );
        drop(guard);
        let report = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the import completes once the lease is released")
            .expect("the import succeeds after waiting out the peer import");
        worker.join().expect("worker should finish");
        assert_eq!(report.id, "demo", "the released import landed its own pack");
        assert!(
            dir.join("bundles")
                .join("demo")
                .join("mcp")
                .join("manifest.json")
                .is_file(),
            "the released import landed the package content"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-20 review (P1): the boot reconcile must treat a held landing
    /// lease as a LIVE import and defer, not as crash residue — the mark
    /// alone cannot distinguish the two, and sweeping a live import's staged
    /// dir (or clearing its mark, which the retired-tool sweep keys on)
    /// destroyed the import in the shared-home two-process scenario. Once
    /// the lease is released (the importer exited or crashed), the same
    /// entry is genuine residue and the reconcile resolves it.
    #[test]
    fn reconcile_spares_a_live_import_and_sweeps_after_release() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-import-lease-live-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // "mark + no pack dir + a staged dir": to a mark-only reconcile this
        // is exactly the crash shape arm A sweeps.
        let id = "lease-pkg";
        std::fs::create_dir_all(landing_journal_dir()).unwrap();
        std::fs::write(landing_mark_path(id), "pending\n").unwrap();
        let staged = crate::platform::paths::bundles_root().join(format!("{id}.tmp"));
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("marker.txt"), b"staging").unwrap();

        // Hold the lease the way the pipeline does and run the reconcile.
        let mut lease = open_landing_lease(id).expect("fixture: lease opens");
        let guard = lease.write().expect("fixture: lease acquires");

        reconcile_import_journal().unwrap();
        assert!(
            staged.exists(),
            "a live import's staged dir must not be swept while it holds the landing lease"
        );
        assert!(
            landing_mark_path(id).exists(),
            "a live import's mark must not be cleared while it holds the landing lease"
        );

        // Release (the importer exited): the entry is now genuine residue.
        drop(guard);
        reconcile_import_journal().unwrap();
        assert!(
            !staged.exists(),
            "the orphaned staged dir is swept once the lease is free"
        );
        assert!(
            !landing_mark_path(id).exists(),
            "the residue mark is cleared once the lease is free"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-32 minor 6 (review #455): a leftover mark over a COMPLETED import
    /// (registry record present — crash hit after the upsert, before the
    /// guard drop) keeps the pack and just clears the stale mark.
    #[test]
    fn reconcile_keeps_completed_import_and_clears_stale_mark() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-import-journal-done-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let pkg = crate::platform::paths::bundles_root().join("done-pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
        crate::features::marketplace::store::BundleStore::new()
            .upsert(
                crate::features::marketplace::store::BundleRecord::installed_now(
                    "done-pkg",
                    crate::features::marketplace::store::BundleSource::Upload(
                        "done.zip".to_string(),
                    ),
                ),
            )
            .unwrap();
        mark_landing("done-pkg");

        reconcile_import_journal().unwrap();

        assert!(
            pkg.join("plugin.json").is_file(),
            "a registered pack is never quarantined"
        );
        assert!(
            !landing_mark_path("done-pkg").exists(),
            "the stale mark is cleared"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-32 minor 6 (review #455): a mark with no landed dir (crash before
    /// the landing rename, or after an in-process rollback that had already
    /// cleared the dir) resolves to a cleared mark and zero side effects.
    #[test]
    fn reconcile_clears_mark_without_landed_dir() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = std::env::temp_dir().join(format!(
            "pinvou-import-journal-none-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        mark_landing("ghost-pkg");
        reconcile_import_journal().unwrap();
        assert!(
            !landing_mark_path("ghost-pkg").exists(),
            "a mark without a landed dir clears"
        );
        assert!(
            !crate::platform::paths::bundles_root()
                .join("ghost-pkg")
                .exists(),
            "nothing may be materialized by the reconcile"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
            // serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
        assert!(
            legacy.is_some(),
            "旧 spanner 字段应被 forward-compat 保留到 extra"
        );
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
            zw.start_file("mcp/server.py", opts).unwrap();
            zw.write_all(b"import json\nprint(json.dumps({'ok': True}))")
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
        assert!(
            pkg.join("mcp/server.py").is_file(),
            "server.py must land on disk"
        );
        assert!(pkg.join("skills/demo/SKILL.md").is_file(), "skill 应落盘");
        assert!(
            pkg.join("plugin.json").is_file(),
            "plugin.json must land on disk"
        );
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
        // Base launch contract: non-empty command + args pointing at the
        // landed server.py (absolute path).
        let mcp: serde_json::Value = serde_json::from_str(&mcp_raw).unwrap();
        let server = &mcp["servers"]["demo"];
        assert!(
            server["command"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "mcp.json command must be non-empty, got: {server:?}"
        );
        let args = server["args"]
            .as_array()
            .expect("mcp.json args must be an array");
        assert!(
            args.iter().any(|a| a
                .as_str()
                .map(|s| s.ends_with("server.py"))
                .unwrap_or(false)),
            "mcp.json args must contain the server.py absolute path, got: {args:?}"
        );
        // 导入登记的来源必须是 Upload（zip 展示名）—— 若停在 Preset，卸载会误删
        // 用户唯一副本（四轮评审 BLOCKER 1）。
        let record = crate::features::marketplace::store::BundleStore::new()
            .get("demo")
            .unwrap()
            .expect("demo 应登记");
        assert_eq!(
            record.source,
            crate::features::marketplace::store::BundleSource::Upload("combo.zip".to_string()),
            "上传包登记来源应为 Upload"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Trust boundary (docs/builtin-toolset-contract.md §3.1): an uploaded
    /// package whose MCP manifest claims `builtin: true` or
    /// `visibility: "system"` is rejected with an explicit error instead of
    /// silently landing with the claim stripped — builtin status belongs to
    /// the publisher's embedded catalog only.
    #[test]
    fn import_rejects_manifest_claiming_builtin_semantics() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-builtin-claim-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        for (id, manifest_tail, needle) in [
            ("claim-builtin", r#","builtin":true}"#, "builtin: true"),
            ("claim-system", r#","visibility":"system"}"#, "system"),
        ] {
            let zip_path = dir.join(format!("{id}.zip"));
            {
                let f = std::fs::File::create(&zip_path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(
                    format!(
                        r#"{{"id":"{id}","name":"n","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"]{manifest_tail}"#
                    )
                    .as_bytes(),
                )
                .unwrap();
                zw.start_file("mcp/server.py", opts).unwrap();
                zw.write_all(b"print('hi')").unwrap();
                zw.finish().unwrap();
            }
            let err = import_plugin_package(&zip_path.to_string_lossy(), &format!("{id}.zip"))
                .unwrap_err();
            assert!(
                err.contains(needle),
                "rejection should name the claim ({needle}): {err}"
            );
            assert!(
                !dir.join("bundles").join(id).exists(),
                "rejected package must not land on disk"
            );
        }

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same trust boundary for the `other_manifests` fallback lane: a claim
    /// hidden in a secondary manifest (any `manifest.json` other than
    /// `mcp/manifest.json`, e.g. `extra/manifest.json` discovered by the bare
    /// MCP fallback) must be rejected just as loudly, naming the offending
    /// path.
    #[test]
    fn import_rejects_builtin_claims_in_secondary_manifests() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-builtin-claim-secondary-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        for (id, claim, needle) in [
            (
                "claim-secondary-builtin",
                r#","builtin":true}"#,
                "builtin: true",
            ),
            (
                "claim-secondary-system",
                r#","visibility":"system"}"#,
                "system",
            ),
        ] {
            let zip_path = dir.join(format!("{id}.zip"));
            {
                let f = std::fs::File::create(&zip_path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                // The primary MCP manifest is clean...
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(
                    format!(
                        r#"{{"id":"{id}","name":"n","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"]}}"#
                    )
                    .as_bytes(),
                )
                .unwrap();
                zw.start_file("mcp/server.py", opts).unwrap();
                zw.write_all(b"print('hi')").unwrap();
                // ...but a secondary manifest carries the builtin claim.
                zw.start_file("extra/manifest.json", opts).unwrap();
                zw.write_all(
                    format!(
                        r#"{{"id":"{id}-extra","name":"n","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"]{claim}"#
                    )
                    .as_bytes(),
                )
                .unwrap();
                zw.finish().unwrap();
            }
            let err = import_plugin_package(&zip_path.to_string_lossy(), &format!("{id}.zip"))
                .unwrap_err();
            assert!(
                err.contains(needle),
                "rejection should name the claim ({needle}): {err}"
            );
            assert!(
                err.contains("extra/manifest.json"),
                "rejection should name the offending secondary manifest path: {err}"
            );
            assert!(
                !dir.join("bundles").join(id).exists(),
                "rejected package must not land on disk"
            );
        }

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zip whose root holds a single SKILL.md → recognized by the bare-skill
    /// fallback and landed on disk. This is the path import_skill_md_bytes drives.
    #[test]
    fn root_skill_md_import_lands_canonical_layout() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-root-skill-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("single.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = import_plugin_package(&zip_path.to_string_lossy(), "single.zip").unwrap();
        assert_eq!(report.id, "greet");
        let pkg = dir.join("bundles").join("greet");
        assert!(
            pkg.join("skills/greet/SKILL.md").is_file(),
            "root SKILL.md must normalize to skills/greet/SKILL.md"
        );
        assert!(
            pkg.join("plugin.json").is_file(),
            "derived plugin.json must land on disk"
        );
        assert!(
            pkg.join("icon.svg").is_file(),
            "default icon must land on disk"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-18 review (P2): the landing journal mark must be written BEFORE
    /// the pre-land consent gate runs. The gate registers the import's
    /// deny-first rows before anything lands, and the startup sweep's
    /// pre-strip probe ([`landing_in_progress`]) can only see this import
    /// through the mark until its record lands — mark-after-gate would leave
    /// a cross-process sweep free to strip the just-registered rows.
    #[test]
    fn pre_land_gate_runs_under_the_landing_mark() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-mark-before-gate-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("single.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = import_plugin_package_gated(
            &zip_path.to_string_lossy(),
            "single.zip",
            &|id, _skills| {
                assert!(
                    landing_in_progress(id),
                    "the landing mark must already be visible to the pre-land gate"
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.id, "greet");
        // A completed import clears its mark: the sweep must not defer on a
        // finished import.
        assert!(
            !landing_in_progress("greet"),
            "the landing mark must be cleared when the import exits"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// round-21 review: `mark_landing` writes under the cross-process landing
    /// lease, so a planted FIFO or symlink at the mark path must refuse fast
    /// (the hardened private-write primitive's regular-file gate /
    /// O_NOFOLLOW), not block the lease holder's open — the lease wedges
    /// same-id imports, uninstalls, and the boot sweep. Bounded worker: a
    /// regressed bare `std::fs::write` hangs here and fails the recv_timeout.
    #[cfg(unix)]
    #[test]
    fn mark_landing_refuses_a_planted_fifo_and_symlink() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-mark-hardening-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // A planted FIFO at the mark path: the write must refuse, not open-block.
        let fifo = landing_mark_path("fifo-id");
        std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: mkfifo on a fresh temp path; no other thread touches it.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "fixture: mkfifo must succeed");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            mark_landing("fifo-id");
            tx.send(()).expect("worker should send");
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("mark_landing must refuse a planted FIFO, not hang");

        // A planted symlink at the mark path: O_NOFOLLOW must refuse without
        // touching the target.
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"victim").unwrap();
        let link = landing_mark_path("link-id");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        mark_landing("link-id");
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"victim",
            "the symlink's target must be untouched"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "the mark must not be written through a planted symlink (the path must still be the link itself)"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 统一导入路径（三条 UI 上传通道的汇聚点）重导入时必须重基线 SKILL.md 说明
    /// 备份：导入 v1 → 设说明（备份 orig1）→ 按文档建议删包目录后导入 v2 →
    /// 旧备份必须被丢弃，清覆盖后恢复的是 v2 的 orig2，而非旧包的 orig1
    /// （重基线修复最初只落在遗留路径 import_package_named 上，UI 全走不到）。
    #[test]
    fn unified_reimport_rebaselines_skill_description_backup() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-unified-rebaseline-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let make_zip = |path: &std::path::Path, desc: &str| {
            let f = std::fs::File::create(path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(format!("---\nname: greet\ndescription: {desc}\n---\n# hi").as_bytes())
                .unwrap();
            zw.finish().unwrap();
        };
        make_zip(&dir.join("v1.zip"), "orig1");

        let report =
            import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip").unwrap();
        assert_eq!(report.id, "greet");
        let store = crate::features::marketplace::store::BundleStore::new();
        assert!(
            store.skill_desc_backup("greet").unwrap().is_none(),
            "首装不应有备份"
        );

        // 设展示说明：单技能包回写 SKILL.md + 备份原值 orig1
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .update_display_meta("greet", None, Some("new desc"))
            .unwrap();
        assert_eq!(
            store.skill_desc_backup("greet").unwrap().as_deref(),
            Some("orig1"),
            "首次回写应备份原值"
        );

        // 文档 §12 的换版路径：删除包目录（保留登记）后重导入 v2（内容不同）
        let _ = std::fs::remove_dir_all(dir.join("bundles").join("greet"));
        make_zip(&dir.join("v2.zip"), "orig2");
        let report =
            import_plugin_package(&dir.join("v2.zip").to_string_lossy(), "v2.zip").unwrap();
        assert_eq!(report.id, "greet");

        // 重基线断言（回归点）：旧包备份必须被丢弃，展示覆盖本身按语义保留
        assert_eq!(
            store.skill_desc_backup("greet").unwrap(),
            None,
            "统一导入重导入必须重基线说明备份（旧包备份恢复进新包=数据损坏）"
        );
        assert_eq!(
            crate::features::marketplace::store::display_override(
                &store.get("greet").unwrap().unwrap(),
                crate::features::marketplace::store::EXTRA_DISPLAY_DESCRIPTION
            )
            .as_deref(),
            Some("new desc"),
            "展示覆盖按既定语义跨重导入保留"
        );

        // 清覆盖：无备份 → 不动文件，SKILL.md 保持 v2 的 orig2（而非被恢复成 orig1）
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .update_display_meta("greet", None, Some(""))
            .unwrap();
        let md = std::fs::read_to_string(dir.join("bundles/greet/skills/greet/SKILL.md")).unwrap();
        assert!(
            md.contains("description: orig2"),
            "SKILL.md 应保持新包原值: {md}"
        );
        assert!(!md.contains("orig1"), "旧包原值不得被恢复进新包: {md}");

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 说明备份与供给失败的顺序契约（round-27 m5 语义，review #455）：重基线
    /// 只在新内容**留存**时发生——moved_old 回滚把旧版文件还原后，旧包的说明
    /// 备份必须幸存（它正是旧内容的有效还原点；round-24 版本曾把重基线放在
    /// 供给之前，回滚会把还原点提前丢掉）。v2 附 mcp/manifest.json 触发供给，
    /// 并预置形状损坏的 mcp.json（合法 JSON 但 servers 非对象）令
    /// add_to_mcp_json 确定性失败——导入返回 Err、旧版还原后，备份必须仍是
    /// 旧值；随后成功导入新版本时重基线照常消费备份。
    #[test]
    fn unified_import_keeps_backup_through_supply_failure_rollback() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-unified-rebaseline-supply-fail-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let make_zip = |path: &std::path::Path, desc: &str, with_mcp: bool| {
            let f = std::fs::File::create(path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(format!("---\nname: greet\ndescription: {desc}\n---\n# hi").as_bytes())
                .unwrap();
            if with_mcp {
                // 字段完整的合法 manifest（ToolManifest 必填字段齐全）→ 探测
                // 阶段认作 MCP 组件（包 id 取自它，与技能 frontmatter name 同
                // 值以命中同一条登记）；供给真正必败点在下方预置的形状损坏
                // mcp.json（servers 非对象），与 manifest 内容无关。
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(
                    br#"{"id":"greet","name":"greet","description":"t","version":"0.0.1","icon":"","category":"custom","mcp_tools":[],"command":"echo","args":[]}"#,
                )
                .unwrap();
            }
            zw.finish().unwrap();
        };
        make_zip(&dir.join("v1.zip"), "orig1", false);

        let report =
            import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip").unwrap();
        assert_eq!(report.id, "greet");
        let store = crate::features::marketplace::store::BundleStore::new();
        crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .update_display_meta("greet", None, Some("new desc"))
            .unwrap();
        assert_eq!(
            store.skill_desc_backup("greet").unwrap().as_deref(),
            Some("orig1"),
            "首次回写应备份原值"
        );

        // 预置形状损坏的 mcp.json（合法 JSON 但 servers 非对象）：后续
        // install_upload 在 add_to_mcp_json 处确定性失败，无需外部依赖。
        let mcp_path = crate::platform::paths::mcp_config_path();
        std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
        std::fs::write(&mcp_path, r#"{"servers": "broken"}"#).unwrap();

        // 文档 §12 的换版路径：删除包目录（保留登记）后重导入 v2（附 MCP 组件）
        let _ = std::fs::remove_dir_all(dir.join("bundles").join("greet"));
        make_zip(&dir.join("v2.zip"), "orig2", true);
        let err = import_plugin_package(&dir.join("v2.zip").to_string_lossy(), "v2.zip")
            .expect_err("MCP 供给必须失败");
        assert!(err.contains("MCP 供给失败"), "失败须来自供给步骤: {err}");

        // Round-27 m5 回归点：moved_old 回滚还原了旧版文件——说明备份必须
        // 幸存（还原点与还原后的旧内容一致；若在此处丢弃，重试导入前的一次
        // 清覆盖恢复将没有还原点可用）。
        assert_eq!(
            store.skill_desc_backup("greet").unwrap().as_deref(),
            Some("orig1"),
            "供给失败回滚后备份必须幸存（旧内容仍以还原点保护）"
        );
        // Round-24 MAJOR 2：供给失败不得残留已落地的无登记目录——其技能会以
        // 零同意进入每个已初始化 scope（本次为首次安装路径，无旧版本可还原，
        // 回滚 = 移除落盘目录）。
        assert!(
            !dir.join("bundles/greet").exists(),
            "供给失败后落盘目录必须回滚移除"
        );

        // Round-24 MAJOR 2 换版路径（moved_old=true）：旧版本在盘时同一包重导、
        // 供给失败（同一 zip 过碰撞检查→落盘→供给必败）→ 回滚 = 还原旧版本目录
        // （旧包继续可用，新包不入），备份清理。
        std::fs::write(&mcp_path, r#"{"servers": {}}"#).unwrap();
        make_zip(&dir.join("v1m.zip"), "orig1-again", true);
        let report =
            import_plugin_package(&dir.join("v1m.zip").to_string_lossy(), "v1m.zip").unwrap();
        assert_eq!(report.id, "greet");
        std::fs::write(&mcp_path, r#"{"servers": "broken"}"#).unwrap();
        let err = import_plugin_package(&dir.join("v1m.zip").to_string_lossy(), "v1m.zip")
            .expect_err("同包重导的供给必须失败");
        assert!(err.contains("MCP 供给失败"), "失败须来自供给步骤: {err}");
        let md = std::fs::read_to_string(dir.join("bundles/greet/skills/greet/SKILL.md")).unwrap();
        assert!(
            md.contains("description: orig1-again"),
            "换版供给失败必须还原旧版本目录（旧包继续可用）: {md}"
        );
        assert!(
            !dir.join("bundles/greet.old").exists(),
            "回滚成功后备份目录必须清理"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同包重导：落盘的 plugin.json 是 `to_string_pretty` 规范化字节，与 zip 原始
    /// 字节直比必误判冲突（三轮评审）——必须按解析后的语义比对，同包重导应放行。
    #[test]
    fn reimport_same_plugin_package_is_allowed() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-reimport-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("combo.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }

        let first = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap();
        assert_eq!(first.id, "demo");
        // 同包重导：落盘 plugin.json 是规范化字节（与 zip 原始字节不同），语义比对
        // 一致 → 放行（原子替换），不得报「内容不同」冲突。
        let second = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip")
            .unwrap_or_else(|e| panic!("同包重导应放行，实际报错: {e}"));
        assert_eq!(second.id, "demo");

        // 对照：同 id 不同内容仍必须拒绝（防静默互覆盖）。
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"另一个包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"demo","name":"另一个包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap_err();
        assert!(err.contains("内容不同"), "不同内容应报冲突，实际: {err}");

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 裸 MCP 包（无 plugin.json 无 SKILL.md）重导：按规范化落盘的
    /// `mcp/manifest.json` 语义比对，同包重导应放行（此前恒 false 必误判冲突）。
    #[test]
    fn reimport_bare_mcp_package_is_allowed() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-reimport-mcp-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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

        let first = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip").unwrap();
        assert_eq!(first.id, "wcalc");
        let second = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip")
            .unwrap_or_else(|e| panic!("裸 MCP 同包重导应放行，实际报错: {e}"));
        assert_eq!(second.id, "wcalc");

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非规范组件 dir 拒收（四轮评审 M-3）：detect 此前放行任意 dir，而落盘只认
    /// mcp/、skills/ 前缀 → 声明组件被静默丢弃（登记 installed=true 但磁盘无文件）。
    /// 现在 detect 强制 skill 组件 dir == `skills/<id>`、MCP 组件 dir == `mcp`。
    #[test]
    fn non_canonical_component_dir_is_rejected() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-noncanon-dir-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // 非规范 skill dir：声明 dir="my-skill"（zip 内确有 my-skill/SKILL.md，
        // 旧逻辑 detect 放行、落盘静默丢弃）。
        let zip_path = dir.join("skill.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"d","components":{"skills":[{"id":"demo","dir":"my-skill"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "skill.zip").unwrap_err();
        assert!(
            err.contains("非规范"),
            "非规范 skill dir 应拒收，实际: {err}"
        );
        assert!(
            !dir.join("bundles").join("demo").exists(),
            "拒收后不得残留包目录"
        );

        // 非规范 mcp dir：声明 dir="server"。
        let zip_path = dir.join("mcp.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"d","components":{"mcp_servers":[{"id":"demo","dir":"server"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("server/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"demo","name":"d","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip").unwrap_err();
        assert!(err.contains("非规范"), "非规范 mcp dir 应拒收，实际: {err}");

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 并发互斥（四轮评审 M-4）：两线程同时导入同 id 同内容包，按 id 的进程内
    /// 互斥锁必须保证双方成功且落盘完整、无 staged/.old 残留（修复前线程 B 的
    /// `remove_dir_all(staged)` 可删线程 A 的在建目录，冲突检查与 rename 间为
    /// TOCTOU）。
    #[test]
    fn concurrent_same_id_import_is_serialized() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-concurrent-import-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

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
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let zip_str = zip_path.to_string_lossy().into_owned();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let z = zip_str.clone();
            handles.push(std::thread::spawn(move || {
                import_plugin_package(&z, "combo.zip")
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for r in &results {
            assert!(r.is_ok(), "并发同包导入应双方成功，实际: {results:?}");
        }

        let pkg = dir.join("bundles").join("demo");
        assert!(pkg.join("mcp/manifest.json").is_file());
        assert!(pkg.join("skills/demo/SKILL.md").is_file());
        assert!(pkg.join("plugin.json").is_file());
        assert!(
            !dir.join("bundles").join("demo.tmp").exists()
                && !dir.join("bundles").join("demo.old").exists(),
            "并发导入结束后不得残留 staged/.old 目录"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 有界读取（四轮评审 M-5）：实际解压字节超过头部声明 size 即判伪造头部
    /// 拒收，不再无界 read_to_end（伪造 size=0 头部可绕过声明预算先 OOM）。
    #[test]
    fn read_zip_entry_bounded_rejects_oversized_actual() {
        let data = b"0123456789";
        let ok = read_zip_entry_bounded(&mut &data[..5], 5, "x").unwrap();
        assert_eq!(ok, b"01234");
        let err = read_zip_entry_bounded(&mut &data[..], 5, "x").unwrap_err();
        assert!(
            err.contains("超过 zip 头声明"),
            "实际 > 声明应拒收，实际: {err}"
        );
    }

    /// 交叉校验（四轮评审 M-9）：MCP 组件声明 id 与 mcp/manifest.json 的 id
    /// 不一致时必须响亮拒收 —— 否则 installed.json/bundles.json 记包 id、
    /// mcp.json 键取 manifest.id，卸载只按包 id 清理，残留孤儿 server。
    #[test]
    fn import_rejects_mismatched_mcp_component_id() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-mcp-id-mismatch-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("combo.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            // manifest 的 id 与组件声明 id 不一致
            zw.start_file("mcp/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"other","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }

        let err = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap_err();
        assert!(
            err.contains("不一致"),
            "组件 id 与 manifest id 不一致应响亮拒收，实际: {err}"
        );
        assert!(
            !dir.join("bundles").join("demo").exists(),
            "拒收不得落盘包目录"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 全内容比对（五轮评审必修 1）：重导同 id 包，plugin.json 语义不变但
    /// server.py 换代码 / SKILL.md 正文全换（frontmatter name 保持以过 §8）——
    /// 此前只比对 plugin.json，冲突检查放行导致已装工具的执行代码被静默替换；
    /// 现在任一可落盘条目字节不同都必须判「内容不同」冲突。
    #[test]
    fn reimport_with_changed_component_content_is_rejected() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-reimport-content-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let write_zip = |server_py: &[u8], skill_md: &[u8]| {
            let zip_path = dir.join("combo.zip");
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
            zw.start_file("mcp/server.py", opts).unwrap();
            zw.write_all(server_py).unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(skill_md).unwrap();
            zw.finish().unwrap();
            zip_path
        };

        let zip_path = write_zip(b"print('v1')", b"---\nname: demo\n---\n# v1");
        let first = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap();
        assert_eq!(first.id, "demo");
        // 同包同内容重导仍放行（原子替换）。
        import_plugin_package(&zip_path.to_string_lossy(), "combo.zip")
            .unwrap_or_else(|e| panic!("同包同内容重导应放行，实际报错: {e}"));

        // 对照 1：plugin.json 不变、server.py 换任意代码 → 必须报冲突。
        let zip_path = write_zip(b"print('pwned')", b"---\nname: demo\n---\n# v1");
        let err = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap_err();
        assert!(
            err.contains("内容不同"),
            "server.py 变更应报冲突，实际: {err}"
        );

        // 对照 2：plugin.json 不变、SKILL.md 正文全换（frontmatter name 保持）→
        // 必须报冲突。
        let zip_path = write_zip(
            b"print('v1')",
            "---\nname: demo\n---\n# 全部改写".as_bytes(),
        );
        let err = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap_err();
        assert!(
            err.contains("内容不同"),
            "SKILL.md 正文变更应报冲突，实际: {err}"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未声明技能子树拒收（五轮评审必修 2）：detect 只登记声明/识别的技能，
    /// zip 额外夹带的 `skills/<other>/` 会静默落盘成不可见孤儿技能（无卡片、
    /// 无开关、卸载残留）。声明包与裸包两种形态都必须响亮拒收。
    #[test]
    fn undeclared_skills_subtree_is_rejected() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-undeclared-skill-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // 形态 1：plugin.json 只声明 skills:[demo]，zip 夹带 skills/other/。
        let zip_path = dir.join("declared.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"d","components":{"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.start_file("skills/other/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: other\n---\n# stowaway").unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "declared.zip").unwrap_err();
        assert!(
            err.contains("未在包声明/识别的技能列表中"),
            "未声明技能子树应拒收，实际: {err}"
        );
        assert!(
            !dir.join("bundles").join("demo").exists(),
            "拒收不得落盘包目录"
        );

        // 形态 2：裸技能包（无 plugin.json），夹带无 SKILL.md 的 skills/other/ 子树。
        let zip_path = dir.join("bare.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.start_file("skills/other/notes.txt", opts).unwrap();
            zw.write_all(b"stowaway").unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "bare.zip").unwrap_err();
        assert!(
            err.contains("未在包声明/识别的技能列表中"),
            "裸包夹带技能子树应拒收，实际: {err}"
        );
        assert!(
            !dir.join("bundles").join("greet").exists(),
            "拒收不得落盘包目录"
        );

        match prev {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Upload-side collision guard (review MINOR): a skill component named
    /// like a preset skill is rejected up front — the preset pipeline owns the
    /// name and its sweep/rehome lifecycle would destroy the uploaded copy.
    #[test]
    fn import_rejects_preset_skill_name_collision() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-preset-collision-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("skill.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("skills/visualizer/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: visualizer\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "skill.zip").unwrap_err();
        assert!(
            err.contains("预置技能冲突"),
            "预置技能撞名应拒收，实际: {err}"
        );
        assert!(
            !dir.join("bundles").join("visualizer").exists(),
            "拒收不得落盘包目录"
        );

        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PINVOU3_HOME", v),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Component-id impersonation (review round-5 M1): the package id is
    /// collision-free, but the inner MCP component id is a builtin's id —
    /// add_to_mcp_json keys servers by manifest.id, so importing would
    /// overwrite the builtin's engine registration. Must be refused.
    #[test]
    fn import_rejects_builtin_component_id_impersonation() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-component-impersonation-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("my-tool.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            // Package id is clean; the COMPONENT id impersonates the builtin.
            archive.start_file("plugin.json", options).unwrap();
            archive
                .write_all(
                    br#"{"manifest_version":1,"id":"my-tool","name":"innocent","components":{"mcp_servers":[{"id":"session-reader","dir":"mcp"}]}}"#,
                )
                .unwrap();
            archive.start_file("mcp/manifest.json", options).unwrap();
            archive
                .write_all(
                    br#"{"id":"session-reader","name":"innocent","description":"fixture","version":"1","icon":"x","category":"fixture","mcp_tools":[],"command":"python","args":["server.py"]}"#,
                )
                .unwrap();
            archive.start_file("mcp/server.py", options).unwrap();
            archive
                .write_all(
                    b"print('impersonation')
",
                )
                .unwrap();
            archive.finish().unwrap();
        }

        let error = import_plugin_package(&zip_path.to_string_lossy(), "my-tool.zip").unwrap_err();
        assert!(
            error.contains("市场预置 MCP 冲突"),
            "the component-id collision must be refused: {error}"
        );
        assert!(
            !crate::platform::paths::bundles_root()
                .join("my-tool")
                .exists(),
            "a refused impersonation must not leave package data"
        );

        if let Some(prev) = previous_home {
            // SAFETY: ENV_LOCK held.
            unsafe { std::env::set_var("PINVOU3_HOME", prev) };
        } else {
            unsafe { std::env::remove_var("PINVOU3_HOME") };
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Package-id collision against non-builtin presets (review round-6 M4):
    /// the id maps to a catalog package but not a builtin, so the builtin
    /// probe alone does not refuse it — the import must still reject an id
    /// that would overwrite an installed preset (`gongwen` / `weather`).
    #[test]
    fn import_rejects_embedded_preset_mcp_id_collision() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-preset-mcp-collision-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let zip_path = dir.join("gongwen.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            archive.start_file("plugin.json", options).unwrap();
            archive
                .write_all(
                    br#"{"manifest_version":1,"id":"gongwen","name":"collision","components":{"mcp_servers":[{"id":"gongwen","dir":"mcp"}]}}"#,
                )
                .unwrap();
            archive.start_file("mcp/manifest.json", options).unwrap();
            archive
                .write_all(
                    br#"{"id":"gongwen","name":"collision","description":"fixture","version":"1","icon":"x","category":"fixture","mcp_tools":[],"command":"python","args":["server.py"]}"#,
                )
                .unwrap();
            archive.start_file("mcp/server.py", options).unwrap();
            archive.write_all(b"print('collision')\n").unwrap();
            archive.finish().unwrap();
        }

        let error = import_plugin_package(&zip_path.to_string_lossy(), "gongwen.zip").unwrap_err();
        assert!(error.contains("市场预置 MCP"), "unexpected error: {error}");
        assert!(
            !crate::platform::paths::bundles_root()
                .join("gongwen")
                .exists(),
            "a rejected preset collision must not leave package data"
        );

        match previous_home {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Upload-side collision guard (review MINOR): a skill component whose
    /// name is already on disk under another package is rejected up front —
    /// this pipeline never sweeps, and the other package's copy must survive.
    #[test]
    fn import_rejects_skill_name_owned_by_another_package() {
        use std::io::Write;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou-foreign-skill-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // Existing plugin package owning skill "foo".
        let foreign = dir.join("bundles/other-pkg/skills/foo");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
        std::fs::write(dir.join("bundles/other-pkg/plugin.json"), "{}").unwrap();

        let zip_path = dir.join("skill.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("skills/foo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: foo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }
        let err = import_plugin_package(&zip_path.to_string_lossy(), "skill.zip").unwrap_err();
        // Merged-world note: the round-12-B1 owner-claim vocabulary check
        // (7a96f2a3) now fires before the skill-name collision check for this
        // fixture — the id "foo" folds onto the physically present
        // other-pkg's vocabulary. Either rejection is the pin: the import is
        // refused naming other-pkg.
        assert!(
            err.contains("other-pkg"),
            "a cross-pack name collision must be refused, got: {err}"
        );
        assert!(foreign.join("SKILL.md").is_file(), "外来包副本不得被动");
        assert!(
            !dir.join("bundles").join("foo").exists(),
            "拒收不得落盘包目录"
        );

        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PINVOU3_HOME", v),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
