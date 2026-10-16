// architecture-guard: allow-target-cfg -- rename_dir_with_retry retries
// PermissionDenied on Windows only (AV/indexers transiently hold the newly
// created directory's handle, producing rename os error 5 — hit in practice);
// on Unix that error is a real permission problem and must not be retried.
// The platform split is this single boolean; sinking it into the platform
// adapter layer costs more than it saves; the retry path is covered by
// reimport_same_plugin_package_is_allowed and the concurrent mutual exclusion
// by concurrent_same_id_import_is_serialized.
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

/// Cumulative unarchive cap for a plugin package (bundled runtimes can be
/// large, hence the relaxed 200 MiB). `pub`: headless callers (the CLI's
/// plugins-import pre-wrap reads) bound themselves by the same limit and
/// reference this constant directly, so the two sides cannot drift.
pub const MAX_PLUGIN_SIZE_BYTES: u64 = 200 * 1024 * 1024;

/// plugin.json 清单（插件包的权威声明）。未知字段 flatten 保留（前向兼容）。
/// 不再有「spanner 独立组件」入口；skill 包的 `tools[]` + `runtime` 可执行协议
/// 为 RFC 草案（docs/plugin-package-spec.md），执行通路未实施。
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Default, Serialize, Deserialize)]
struct PluginComponents {
    #[serde(default)]
    pub mcp_servers: Vec<ComponentRef>,
    #[serde(default)]
    pub skills: Vec<ComponentRef>,
}

/// 组件目录引用：`dir` 相对 zip 根，导入时校验存在。
#[derive(Debug, Serialize, Deserialize)]
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
    if !is_supported_icon(file_name) {
        return Err(format!("非法或不受支持的图标文件名 '{file_name}'"));
    }
    let ext = if file_name.to_ascii_lowercase().ends_with(".png") {
        "png"
    } else {
        "svg"
    };
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
///
/// 磁盘侧读取全部走加固原语。本函数的调用契约
/// （下方 conflict-check doc）要求调用方持有 per-id 导入锁，生产调用点更在
/// 跨进程 landing lease 之内——裸 `fs::read` 会在持锁期间阻塞在植入 FIFO 的
/// `open()` 上，楔死同 id 导入道直至持有进程退出。加固原语拒绝非普通文件
/// 而不阻塞；所有 Err 一律映射到既有的「判冲突」保守口径，语义不变。
///
/// 磁盘侧载荷读取（同包比对专用）：普通条目的合法大小是包预算
/// （[`MAX_PLUGIN_SIZE_BYTES`]，zip 侧 pass1/pass2 的有界读取都按此收口），
/// 而非 16 MiB 私有状态上限——首导放行的 >16 MiB 条目在重导比对时会被状态
/// 上限拒收，同包重导被误判「内容不同」。开口加固
/// （O_NOFOLLOW|O_NONBLOCK、Windows 共享模式、非普通文件闸）与闸后仍生效的
/// 大小界语义全部来自共享原语，仅大小界不同。
fn read_landed_payload_bytes(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    crate::platform::filesystem::read_private_data_file_bytes_within(path, MAX_PLUGIN_SIZE_BYTES)
}

/// 磁盘侧大小界走 [`read_landed_payload_bytes`]（包预算）
/// 而非私有状态上限——首导放行的 >16 MiB 普通条目在重导比对时曾被状态上限
/// 拒收，同包重导被误判冲突。
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
            let disk = read_landed_payload_bytes(&pkg_dir.join("plugin.json"))
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
    let mut landed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path_str in all_paths {
        let Some((subdir, rel)) = landing_target(det, path_str) else {
            continue; // plugin.json / 图标单独比对
        };
        let Ok(mut entry) = archive.by_name(path_str) else {
            return false;
        };
        let declared_size = entry.size();
        let Ok(buf) = read_zip_entry_bounded(&mut entry, declared_size, path_str) else {
            return false; // 条目不可读/伪造头部 → 保守判冲突
        };
        let Ok(existing) = read_landed_payload_bytes(&pkg_dir.join(&subdir).join(&rel)) else {
            return false; // zip 有而磁盘无（或磁盘侧拒绝）→ 内容不同
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
        match read_landed_payload_bytes(&pkg_dir.join(format!("icon.{ext}"))) {
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
///
/// 磁盘侧读取同样走 [`read_landed_payload_bytes`]（包预算）：显式 plugin.json
/// 经 `PluginManifest` 扁平 extra 字段保留未知元数据，首导可在 200 MiB 包预算
/// 内放行一张 >16 MiB 的清单，重导比对若走 16 MiB 私有状态上限会被拒收，
/// 同一包被误判「内容不同」。
fn plugin_manifest_semantic_eq(disk_path: &std::path::Path, incoming: &[u8]) -> bool {
    let Ok(existing) = read_landed_payload_bytes(disk_path) else {
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
        return Err(format!("{what}{}", REJECT_FORGED_HEADER));
    }
    Ok(buf)
}

/// zip pass-1 单条目安全闸（统一插件包导入与技能 zip 导入共用；与底座
/// `skills::install` 的安装安全面同源）：
/// - `enclosed_name` 归一化穿越/绝对路径判定走 zip crate 自带 sanitizer：其
///   归一化输出还会喂给 `skill_md_rank` 等下游布局逻辑，归一化本身是承重行为
///   （`./x`、`a/../b` 今日按净化后路径放行），不能换成底座 `is_safe_path`
///   的纯拒绝谓词——verdict 差异由 `forkguard_zip_gate_keeps_zip_sanitizer…`
///   等价性钉登记；
/// - unix_mode 文件类型位 = symlink，拒绝（底座 `entry_type_is_link` 的 zip
///   面：tar 面底座自用，zip 面按底座契约在条目 unix 位上判定）；
/// - 头部声明大小累计复用底座 `skills::install::add_entry_size`（饱和累加 +
///   严格大于才拒绝，与旧内联实现逐点等价，见等价性测试；真实解压字节由各自
///   pass2 的有界读取兜底计量——头部声明可被伪造）。
/// 返回净化后的条目路径（分隔符归一为 `/`），并把本条目声明大小累进
/// `declared_total`。
pub(crate) fn checked_zip_entry_path(
    entry: &mut zip::read::ZipFile<'_, std::fs::File>,
    declared_total: &mut u64,
    max_bytes: u64,
    label: &str,
) -> Result<String, String> {
    let Some(enclosed) = entry.enclosed_name() else {
        return Err(REJECT_TRAVERSAL.to_string());
    };
    if let Some(mode) = entry.unix_mode() {
        if mode & 0o170000 == 0o120000 {
            return Err(REJECT_SYMLINK.to_string());
        }
    }
    *declared_total =
        deepseek_tui::skills::install::add_entry_size(*declared_total, entry.size(), max_bytes)
            .map_err(|_| reject_declared_cap(label, max_bytes))?;
    Ok(enclosed.to_string_lossy().replace('\\', "/"))
}

/// The rejection literals carry the substrings the headless CLI's
/// `translate_plugin_import_error` anchors on; they are single-sourced here
/// so the anchor test below pins the production strings, not hand copies
/// (round-38 review: the copies had drifted from being a test of anything).
pub(crate) const REJECT_TRAVERSAL: &str = "zip 含不安全路径(穿越),拒绝";
pub(crate) const REJECT_SYMLINK: &str = "zip 含 symlink,拒绝";
pub(crate) const REJECT_FORGED_HEADER: &str =
    " 实际解压大小超过 zip 头声明（疑似伪造头部/zip bomb），拒绝";

/// Declared-size cap rejection (the head-declared budget `checked_zip_entry_path`
/// enforces). `label` names the entry; the fixed halves carry the substrings
/// the headless CLI's `translate_plugin_import_error` anchors on (解压+上限).
pub(crate) fn reject_declared_cap(label: &str, max_bytes: u64) -> String {
    format!("{label}解压超过 {} MiB 上限", max_bytes / 1024 / 1024)
}

/// Actual-size cap rejection (the zip-bomb backstop on the real bytes read).
/// Same anchor contract as [`reject_declared_cap`]; the two hand-copied
/// format sites (pass-1 read and pass-2 write) now share this helper
/// (round-39 review: rewording either site silently reverted the CLI's cap
/// rejection to verbatim Chinese while the anchor test stayed green).
pub(crate) fn reject_actual_cap(max_bytes: u64) -> String {
    format!(
        "插件包实际解压超过 {} MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
        max_bytes / 1024 / 1024
    )
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

/// [`import_lock_for`] 的跨进程补充：进程内互斥表看不见另一个进程，而
/// round-51 起 CLI 的 `plugins import` / `plugins skills install` /
/// `skills uninstall` 会在第二个进程里调用这些 mutator，与 GUI 共享同一批
/// 固定名 `.tmp` 暂存路径——两个写者会互相删掉对方的暂存内容，再把一份
/// 静默残缺的包装进正式目录并回 exit-0（与定时任务定义暂存别名同类的
/// 缺陷，那条已在上游以唯一暂存名修复；这里的根因修复在本地即可完成）。
/// 每个市场 mutator 在自己的「冲突检查 → 原子 rename」整段临界区持有本
/// 文件锁（round-52 review M2 起含回收搬移/退役残留/布局迁移），位置一律
/// 在各自 `import_lock_for` 之后、store `file_lock` 之前；与事务锁之间也
/// 统一本锁在前——凡事务临界区会触及市场 mutator 的入口（install_inner /
/// uninstall / repair / reconcile）都先持本锁再取事务锁，反向获取会与本锁
/// 外等事务的持锁路径构成 ABBA（round-52 review M1）。import_lock 与事务锁
/// 的配对仍属文档化未决类，不据此主张全仓锁序已定，以
/// `MARKETPLACE_TRANSACTION_LOCK` 的文档为准。同线程嵌套获取（import →
/// install_upload → release_package 这条真实调用链）按持有深度 join，不再
/// 对新 fd 重复 flock——flock 按 fd 排他，第二个 fd 会对自己的第一个 fd
/// 阻塞。锁文件不删除（崩溃不能楔住对端；进程死亡由内核释放锁），与
/// settings 部 `lock_settings_file` 同一纪律；0600 亦同（flock 只需读权限，
/// 世界可读的锁文件会让共享宿主上的第二个本地用户无限期楔住全部市场写者）。
pub(crate) struct MarketplaceWriteLock {
    /// `Some`：本线程的最外层（真实）获取，持有进程内段锁与跨进程 fd。
    /// `None`：同线程重入 join，不新增 fd。
    real: Option<MarketplaceWriteGuard>,
}

struct MarketplaceWriteGuard {
    // 字段顺序即释放顺序：先释放 fd（连同 flock），再放开进程内段锁让下
    // 一个本进程等待者进入。
    file: std::fs::File,
    _section: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Drop for MarketplaceWriteGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

thread_local! {
    /// 本线程持有深度：嵌套 `acquire` 按此 join（见类型文档）。真实守卫的
    /// Drop 把深度清零，join 守卫的 Drop 递减——两条路径在展开（panic）时
    /// 都会执行，不会留下幽灵持有者。
    static LOCK_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// 进程内段锁：本线程之外的获取者在此排队，胜者再做跨进程 flock。段锁随
/// 真实守卫持有整个临界区——提前松开会让第二个线程重新 flock，把同进程的
/// 互斥切成两段。
static LOCK_SECTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl MarketplaceWriteLock {
    pub(crate) fn acquire() -> std::io::Result<Self> {
        if LOCK_DEPTH.get() > 0 {
            LOCK_DEPTH.set(LOCK_DEPTH.get() + 1);
            return Ok(Self { real: None });
        }
        let section = LOCK_SECTION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = crate::platform::paths::bundles_root().join(".imports.lock");
        // 与 file_lock::open_lock_file 同一开启纪律（见
        // `open_private_lock_file`）：O_NOFOLLOW 拒绝被植入的符号链接，
        // O_NONBLOCK 不给 FIFO 阻塞 open 的机会，权限漂移收紧失败即整体
        // 失败——这里不再保留手写 open+chmod 的弱化副本。先开启，仅在
        // 缺失（父目录未建）时补建后重试。
        let open = || crate::platform::filesystem::open_private_lock_file(&path);
        let file = match open() {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                open()?
            }
            Err(e) => return Err(e),
        };
        // 阻塞等待而非 try+busy：导入/安装本就是秒级用户动作，持锁方死亡
        // 由内核释放，存活方只在各自的暂存窗口内持锁。
        file.lock()?;
        LOCK_DEPTH.set(1);
        Ok(Self {
            real: Some(MarketplaceWriteGuard {
                file,
                _section: section,
            }),
        })
    }

    /// [`Self::acquire`] + the uniform 「创建市场写入锁」 error mapping every
    /// market mutator returns; the literal is single-sourced here (the type's
    /// home) so the call sites cannot drift apart.
    pub(crate) fn acquire_map_err() -> Result<Self, String> {
        Self::acquire().map_err(|e| format!("创建市场写入锁: {e}"))
    }
}

impl std::ops::Drop for MarketplaceWriteLock {
    fn drop(&mut self) {
        if self.real.is_some() {
            LOCK_DEPTH.set(0);
        } else {
            LOCK_DEPTH.set(LOCK_DEPTH.get().saturating_sub(1));
        }
    }
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
        if let Ok(tm) = serde_json::from_slice::<crate::features::marketplace::ToolManifest>(bytes)
        {
            if !tm.id.is_empty() {
                id = tm.id.clone();
                mcp.push(tm.id);
            }
        }
    }
    if mcp.is_empty() {
        for (path, bytes) in other_manifests {
            if let Ok(tm) =
                serde_json::from_slice::<crate::features::marketplace::ToolManifest>(bytes)
            {
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

/// 计算单个 zip 条目落盘的目标 `(subdir, rel)`（None = 跳过：plugin.json / 图标单独处理，
/// 以及隐藏/空 rel 的未落盘条目——与 pass2 相同的跳过口径）。
///
/// 优先级：
/// 1. 固定前缀 `mcp/` / `skills/`（本项目标准布局；旧 `spanner/`、`runtime/`
///    前缀已随 spanner 退场删除，不再识别）；
/// 2. 裸 MCP 回退根（无 plugin.json 时，manifest.json 所在目录 → `mcp/`）；
/// 3. 裸技能回退根（无 plugin.json 时，SKILL.md 所在目录 → `skills/<name>/`）。
///
/// 根级（root=""）裸包的消歧：裸 MCP 只认 `manifest.json` 与同目录非 `.md` 文件
/// （服务器代码/依赖），裸技能认其余（SKILL.md 及资源），避免两者互相抢占。
fn landing_target(det: &ComponentDetection, path_str: &str) -> Option<(String, String)> {
    let bare_skill = det
        .bare_skill
        .as_ref()
        .map(|(r, n)| (r.as_str(), n.as_str()));
    let bare_mcp = det.bare_mcp.as_ref().map(|(r, n)| (r.as_str(), n.as_str()));
    let (subdir, rel) = landing_target_layout(path_str, bare_skill, bare_mcp)?;
    if rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
        return None;
    }
    Some((subdir, rel))
}

/// `landing_target` 的布局判定主体（固定前缀与裸包回退消歧），不含隐藏条目过滤。
fn landing_target_layout(
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
// The import landing journal.
//
// A crash between the landing rename and the registry upsert leaves
// `bundles/<id>/` live with no record — "live-by-absence" in initialized
// scopes, zero consent, invisible to the in-process rollback. The
// journal closes it precisely: the import marks its id before the landing and
// clears the mark on any in-process exit (guard drop) — except a failed
// registry upsert, which defuses the guard and fails the import while keeping
// the mark (bundles.json is a pure-skill upload's only registration; warning
// and continuing would end live-by-absence with no mark, unreachable by the
// reconcile forever). A leftover mark is therefore proof of a crash in that
// exact window or of a failed registration — both present the same
// "landed dir, no record" state. The startup reconciliation acts ONLY
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
/// finished. The mark precedes the deny-first gate, so mark-presence also
/// covers an import whose rows are already registered.
pub(crate) fn landing_in_progress(id: &str) -> bool {
    landing_mark_path(id).exists()
}

/// Opens (creating if missing) the cross-process landing lease file for `id`.
/// Same discipline as the scope lock's open: a hardened private open, with
/// the parent directory created only on the cold NotFound path.
///
/// The lease is the liveness signal behind the landing mark: the mark alone
/// cannot distinguish a live import from a crashed one, but a live import
/// holds this lease (flock / LockFileEx via `fd-lock`) from before the mark
/// is written until the landing guard drops. Callers:
/// - the import pipeline blocks on `write()` — a peer's same-id import
///   waits out here instead of racing the shared staged `.tmp` / `.old`
///   paths, a cross-process exclusion the in-process mutex never provided;
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
    // `<journal>/<id>.landing.lock` (holds no user data). The name never
    // carries the `.pending` suffix, so the reconcile's journal scan skips it
    // by construction.
    let path = landing_journal_dir().join(format!("{id}.landing.lock"));
    super::file_lock::open_lock_file(&path)
        .map(fd_lock::RwLock::new)
        .map_err(std::io::Error::other)
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
    // A failed mark write degrades crash recovery exactly like the
    // create_dir_all failure above — it must be just as loud instead of
    // silently re-opening the live-by-absence window the journal exists to
    // close. The write goes through the hardened private-write primitive:
    // it runs under the cross-process landing lease, so a planted
    // FIFO/symlink at the mark path must refuse, not block the lock holder
    // (same discipline as the scope lock's hardened reads).
    if let Err(e) = crate::platform::filesystem::write_private_data_file(&path, b"pending\n") {
        log::warn!(
            "[plugin-import] writing the landing journal mark failed (crash recovery for {id} degraded to live-by-absence): {e}"
        );
    }
}

fn clear_landing(id: &str) {
    if let Err(e) = std::fs::remove_file(landing_mark_path(id)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            // A failed mark clear must be loud, like every other journal
            // operation. The next startup reconcile resolves a leftover mark
            // idempotently (the completed-import arm), but a persistently
            // failing home should not defer in silence.
            log::warn!(
                "[plugin-import] import journal: clearing the landing mark for '{id}' failed, \
                 deferring to the next startup reconcile: {e}"
            );
        }
    }
}

/// Test-only failpoint for the tail half of the lease hold span:
/// parked immediately before the bundles.json
/// registration, so a pin can observe that the lease is still HELD at the
/// END of the span. The mid-span `pre_land` probe above cannot make that
/// observation — a scoped-block refactor releasing the lease anywhere in
/// the supply → registration tail stays green against it while leaving the
/// registration window cross-process-unguarded.
#[cfg(test)]
static LANDING_REGISTRATION_PARK: std::sync::Mutex<
    Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
> = std::sync::Mutex::new(None);

/// No-op outside tests: the call site in the gated pipeline is unconditional
/// on purpose, so the pin drives exactly the production body (no cfg-split
/// for a future edit to desync).
fn test_park_before_registration() {
    #[cfg(test)]
    {
        let park = LANDING_REGISTRATION_PARK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some((reached, release)) = park {
            let _ = reached.send(());
            let _ = release.recv();
        }
    }
}

/// Clears the landing mark on any in-process exit; a crash skips the drop and
/// leaves the mark for the startup reconciliation. `defuse` opts out of the
/// drop-clear for the registration-failure path: the mark must survive so the
/// next boot's reconcile quarantines the landed-but-unregistered pack (its
/// Ok(None) arm) instead of returning it to unmarked live-by-absence.
struct LandingJournalGuard {
    id: String,
    clear_on_drop: bool,
}

impl Drop for LandingJournalGuard {
    fn drop(&mut self) {
        if self.clear_on_drop {
            clear_landing(&self.id);
        }
    }
}

impl LandingJournalGuard {
    fn defuse(&mut self) {
        self.clear_on_drop = false;
    }
}

/// Sweeps a crash-orphaned staged `bundles/<id>.tmp` and a crash-stranded
/// reimport backup `bundles/<id>.old` out of the reconcile arms below, then
/// clears the landing mark. The mark is cleared ONLY when both legs swept
/// clean: a failed leg keeps the mark so the next startup retries — an
/// unconditional clear would strand the residue forever (nothing reschedules
/// a swept mark) while the log claimed success. `why` only distinguishes the
/// arm in the success log.
///
/// The `.old` leg: a crash between the reimport's landing rename and the
/// post-supply backup delete strands the full pre-import copy beside the
/// live pack — scan-invisible duplicate up to the zip budget, with no other
/// cleaner (a future same-id import's rotation delete aside). Reaching the
/// record arms means the restore arm already converted any "only copy"
/// `.old` into the pack dir, so a remaining `.old` is always the superseded
/// copy; the failed in-process rollback strand (§3.2) carries no mark, so
/// this sweep never guesses at it.
fn sweep_and_clear_crash_orphaned_staging(id: &str, pkg_dir: &Path, why: &str) {
    for (path, what) in [
        (
            pkg_dir.with_extension("tmp"),
            "the crash-orphaned staging dir",
        ),
        (
            pkg_dir.with_extension("old"),
            "the crash-stranded reimport backup",
        ),
    ] {
        if !path.exists() {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            log::warn!(
                "[plugin-import] import journal: sweeping {what} for '{id}' failed, keeping the mark for the next startup: {e}"
            );
            return;
        }
        log::warn!("[plugin-import] import journal: swept {what} for '{id}'{why}");
    }
    clear_landing(id);
}

/// Resolves leftover landing marks from crashed imports. For each journaled
/// id: missing pack dir → the import never landed
/// or rolled back (clear); registry record present → the import completed and
/// only the mark-clear crashed (clear); landed dir with no record → crash
/// residue: move it out of `bundles_root` into a recoverable holding dir so no
/// scan admits it live-by-absence. Store-read failures keep the entry (retry
/// next boot, fail-closed). Actively holds the id's import lock per entry, PLUS
/// the cross-process market write lock (order: import_lock → market write
/// lock), and probes the id's landing lease non-blockingly before
/// treating anything as residue: the in-process mutex cannot see a second
/// process's in-flight import — which otherwise looks exactly like crash
/// residue (mark, landed dir, no registry record yet) — so inspecting and
/// moving without exclusion would quarantine a pack mid-import, whether
/// that import excludes via the market lock or the landing lease. The lease
/// probe never blocks, so the mutex → try-lease order cannot deadlock
/// against the pipeline's lease → mutex order; market lock-acquisition
/// failures defer the entry the same fail-closed way (mark kept, retried
/// next boot).
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
            // fallback id with `is_safe_skill_name` (mixed case allowed: a
            // bare-skill id comes from the SKILL.md frontmatter `name`).
            // An unsafe name means the file
            // is not a journal mark. Leave it.
            continue;
        }
        let lock = import_lock_for(id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        // The mark is only a hint — the lease proves
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
        // The in-process import lock cannot see a second process: a CLI
        // `plugins import` that has landed its dir but not yet registered
        // looks exactly like crash residue here — mark, landed dir, no
        // registry record — and quarantining it mid-import would leave the
        // CLI's completed registration pointing at a moved-away dir. The
        // cross-process market write lock excludes the in-flight importer
        // (lock order: import_lock → market write lock, uniform at every
        // market mutator). On acquisition failure defer the entry like the
        // store-read failure below: keep the mark, retry next boot
        // (fail-closed).
        let _market_lock = match MarketplaceWriteLock::acquire() {
            Ok(guard) => guard,
            Err(e) => {
                log::warn!(
                    "[plugin-import] import journal: acquiring the market write lock for '{id}' failed, deferring this entry to the next startup: {e}"
                );
                continue;
            }
        };
        let pkg_dir = crate::platform::paths::bundles_root().join(id);
        // A crash between the re-import's two renames (the
        // pack dir already moved to `<id>.old`, the staged copy not yet
        // renamed in) leaves exactly the shape the next arm handles — no pack
        // dir, a staged `.tmp`, the pre-import copy in `<id>.old`, and the
        // record still `installed`. Sweeping the `.tmp` alone there would
        // converge that crash to "record installed, content
        // gone, the only copy stranded in the scan-invisible `.old`, nothing
        // ever revisits it". Restore the backup in place first (the
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
            // lens — has no `.tmp` exclusion. Sweep it; the pipeline's own
            // error paths remove exactly this dir. With
            // the backup restore above, a `<id>.old` found here has already
            // been moved back into place; a strand left by a FAILED
            // in-process rollback (whose mark the guard cleared) is the
            // disclosed residual in §3.2 — this arm must not guess at it.
            sweep_and_clear_crash_orphaned_staging(id, &pkg_dir, "");
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
                // residue.
                sweep_and_clear_crash_orphaned_staging(
                    id,
                    &pkg_dir,
                    " (a reimport crashed mid-staging)",
                );
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
                        // With the backup restored
                        // above, a re-import's staged `.tmp` can still sit
                        // here (the record disappeared between the crash and
                        // this reconcile — e.g. a concurrent uninstall).
                        // Nothing else would ever delete it and
                        // `available_tools` has no `.tmp` exclusion, so sweep
                        // it like the other arms; a failed sweep keeps the
                        // mark for the retry.
                        sweep_and_clear_crash_orphaned_staging(
                            id,
                            &pkg_dir,
                            " (a re-import crashed mid-renames)",
                        );
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

/// Refuses an id that an installed claimant's companion vocabulary folds
/// onto. Called once before the pre-land gate and once under the import
/// AND market locks (a TOCTOU re-check) — the market lock is the seam
/// that excludes a CROSS-PROCESS claimant (a CLI skills install contends
/// on it, not on this import's per-id lock).
fn ensure_package_id_self_mapping(id: &str) -> Result<(), String> {
    let folded = crate::features::marketplace::scope::to_package_id(id);
    if folded == id {
        Ok(())
    } else {
        Err(format!(
            "包 id '{id}' 已被包 '{folded}' 的配套技能占用，请改用其它 id"
        ))
    }
}

/// Same pipeline with a pre-land gate hook: `pre_land` runs after the package
/// id is fixed and fully validated but BEFORE any content lands on disk or
/// replaces an existing installation. A `pre_land` refusal aborts the import
/// with nothing touched — the DenyAll consent gate uses this to register the
/// deny entry before the package is exposed:
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
            return Err(reject_actual_cap(MAX_PLUGIN_SIZE_BYTES));
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
    let manifest: Option<PluginManifest> = manifest_bytes
        .as_ref()
        .map(|b| serde_json::from_slice(b).map_err(|e| format!("解析 plugin.json: {e}")))
        .transpose()?;
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
    let id = det.id.clone();
    let kind =
        crate::features::marketplace::bundle::derive_bundle_kind(&det.mcp_servers, &det.skills)
            .map_err(|_| "插件包不含任何组件（空包）".to_string())?;

    // 未声明技能子树拒收（五轮评审）：detect 只登记声明/识别出的技能，而落盘的
    // `skills/` 前缀分支按路径无条件放行——zip 额外夹带的 `skills/<other>/` 会
    // 静默落盘成无卡片、无开关、卸载保护残留的不可见孤儿技能（与四轮 M-3 相反
    // 方向的缺口：M-3 修「声明的不落盘」，这里修「落盘的不在声明里」）。
    for p in &all_paths {
        if let Some(rest) = p.strip_prefix("skills/") {
            let name = rest.split('/').next().unwrap_or("");
            if !name.is_empty() && !det.skills.iter().any(|s| s == name) {
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
    // Component ids must not impersonate builtins either:
    // the package-id collision check cannot see them — a `my-tool` package
    // carrying an mcp component with id `session-reader` would have
    // add_to_mcp_json key the server entry by manifest.id and overwrite the
    // real builtin's engine registration (uninstalling my-tool removes only
    // its own key, orphaning the hijacked entry). The probe folds case, the
    // same discipline as the package-id check.
    // The inner mcp/manifest.json id is force-equal to the component id by
    // detect_components, but a bare mcp/ package without plugin.json computes
    // its components from the inner manifest — rejected by the same builtin
    // probe.
    let inner_mcp_id: Option<String> = mcp_manifest_bytes.as_deref().and_then(|b| {
        serde_json::from_slice::<crate::features::marketplace::types::ToolManifest>(b)
            .ok()
            .map(|tm| tm.id)
    });
    for component in det.mcp_servers.iter().chain(inner_mcp_id.iter()) {
        if crate::features::marketplace::mcp_catalog::spec_for_builtin_probe(component).is_some() {
            return Err(format!(
                "组件 id '{component}' 与市场预置 MCP 冲突，请改用其它 id 或通过市场直接安装"
            ));
        }
    }
    // `cli_bundle_skill_dirs` matches bundle ids with exact equality, so a
    // non-empty result already implies the case-folded id match below — the
    // case-folded check alone covers both exact and case-variant collisions.
    if crate::features::marketplace::bundle::builtin_cli_bundle_ids()
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
    for skill_name in &det.skills {
        if crate::features::marketplace::skill_marketplace::RETIRED_SKILL_NAMES
            .contains(&skill_name.as_str())
        {
            return Err(format!(
                "技能 '{skill_name}' 与已下线内置技能名冲突，请改用其它名称"
            ));
        }
    }
    // Preset skill-market id rejection covers only the "alias-split" shape:
    // an id unlike every preset skill name (e.g.
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
    // Owner-claim divergence: `to_package_id` folds an
    // id onto an installed claimant's consent vocabulary — an installed
    // package's declared companion skill name, a CLI companion dir, or
    // `ima-skills`. Importing a PACKAGE under such an id would make the
    // consent gate register (or skip on) the claimant's id while this
    // package lands keyed by its own id, ungoverned in initialized DenyAll
    // scopes (e.g. gongwen installed + import id
    // `government-writing`). The check is state-dependent on purpose: with
    // no claimant installed the id self-maps and stays importable, which
    // keeps the export → re-import round-trip contract
    // (`export_installed_plugin_allows_preset_named_id_without_preset_skill`)
    // intact. The skill-name half of the same collision is already rejected
    // by the component owner check below.
    ensure_package_id_self_mapping(&id)?;
    // Preset/companion/cross-package skill name collisions are rejected up
    // front, consistent with the two skill_marketplace channels (which sweep
    // duplicate copies after install — this pipeline never sweeps, so without
    // a guard a collision would escalate from "shadowed" to "swept away").
    // Self-claim is allowed (owner == this package id, e.g. a combo package
    // whose mcp manifest declares its own skill as companion on reimport).
    for skill_name in &det.skills {
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
    for skill_name in &det.skills {
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

    // Journal the landing BEFORE the pre-land consent gate — before anything
    // this import writes, deny-first registration included — clearing the
    // mark on any in-process exit (gate refusal, supply-failure rollback, or
    // rename failure). The early placement makes the mark the cross-process
    // in-flight signal the startup sweep's pre-strip probe
    // ([`landing_in_progress`]) keys on: it must see an import that already
    // registered its deny-first rows but has not landed yet (what a leftover
    // mark means and why the journal exists: the module doc above).
    // The lease is acquired BEFORE the mark and held to the end of this
    // call: mutual exclusion of the whole mark→gate→land→register span
    // against every lease-holding same-id peer (the lease contract and its
    // lock-order slot live on [`open_landing_lease`]). NOT covered
    // (disclosed in §3.2): the skill lane's install /
    // uninstall serialize in-process on the per-id import mutex only, and
    // the boot python-repair downgrade runs under the transaction lock only
    // — a cross-process skill-uninstall-vs-import of the same id can still
    // interleave; closing that needs the same lease in those lanes
    // (registered as follow-up).
    // Locals drop in reverse order, so the lease guard is declared before
    // the landing guard on purpose: the landing guard clears the mark while
    // the lease is still held.
    let mut landing_lease = open_landing_lease(&id)
        .map_err(|error| format!("open the landing lease for {id}: {error}"))?;
    // Shared bounded transient retry: the funnel's Windows
    // ERROR_LOCK_VIOLATION discipline applies to the lease takers too.
    let _landing_lease_guard =
        super::file_lock::lock_write_with_transient_retry!(&mut landing_lease)
            .map_err(|error| format!("lock the landing lease for {id}: {error}"))?;
    mark_landing(&id);
    let mut landing_guard = LandingJournalGuard {
        id: id.clone(),
        clear_on_drop: true,
    };

    // Pre-land gate hook: the id is final and validated here, nothing has
    // been written yet. A refusal aborts the whole import before any content
    // lands or an existing installation is replaced (deny-first consent
    // gate). The identified skill components ride along so
    // the gate can also register standalone components deny-first.
    // Runs before the same-id import lock on purpose: the gate takes no
    // import lock, so the import lock is never held ACROSS a wait on another
    // importer (lock order — see `scope::load_disabled_bundles_file_locked`'s
    // lock-order note).
    pre_land(&id, &det.skills)?;

    // 落盘到 staged：mcp/ + skills/ 子树 + 裸包回退规范化 → bundles/<id>/ 原子 rename。
    // 注：旧 spanner/ 与 runtime/ 子树已删除，导入侧不再识别这两类前缀。
    let pkg_dir = crate::platform::paths::bundles_root().join(&id);
    // 进程内导入互斥（四轮评审 M-4）：同 id 并发导入共享 staged `.tmp` 与 `.old`
    // 备份路径，且冲突检查与原子 rename 之间无锁即 TOCTOU——持锁覆盖「冲突检查
    // → 原子 rename 完成」整段临界区（guard 至函数尾生效，详见 import_lock_for）。
    let import_lock = import_lock_for(&id);
    let _import_guard = import_lock.lock().unwrap_or_else(|p| p.into_inner());
    // Round-51 review M2: the in-process mutex above cannot see the CLI's
    // second process — hold the cross-process market lock for the whole
    // conflict-check → rename window (lock order: import_lock → market
    // file lock → store file lock, identical at every market mutator).
    let _market_lock = MarketplaceWriteLock::acquire_map_err()?;
    // Re-check the fold divergence under the import and market locks: the
    // first check ran before the consent gate, and a claimant (preset skill
    // install, CLI companion, ima connect) completing in between would fold
    // this id onto the claimant — the gate registers (or skips on) the
    // claimant's id while this package lands keyed by its own id, ungoverned.
    // Nothing has landed at this point, so the refusal aborts with the home
    // untouched (the gate's earlier registration is an over-denial row on an
    // installed claimant — fail-closed).
    ensure_package_id_self_mapping(&id)?;
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
    // bundles/<id> 的 id 已过 is_safe_component_id / is_safe_skill_name 校验
    //（单层、无分隔符的组件名），pkg_dir 必是 bundles_root() 的直接子目录——
    // 确保 bundles 根目录存在即可，不存在「包目录缺父目录」分支。
    std::fs::create_dir_all(crate::platform::paths::bundles_root())
        .map_err(|e| format!("创建 bundles 目录: {e}"))?;
    let staged = pkg_dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("暂存目录: {e}"))?;

    let write_result = (|| -> Result<String, String> {
        // 1) 统一落盘：mcp/ + skills/ 子树 + 裸包回退规范化（旧 spanner/、runtime/
        //    子树已删除，见 landing_target 注释）。
        for path_str in &all_paths {
            let Some((subdir, rel)) = landing_target(&det, path_str) else {
                continue; // plugin.json / icon / 隐藏与空路径条目单独处理
            };
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
                return Err(reject_actual_cap(MAX_PLUGIN_SIZE_BYTES));
            }
            std::fs::write(&target, buf).map_err(|e| format!("写文件: {e}"))?;
        }

        // 2) spanner 旧路径删除——脚本可执行能力通过 skill 包的 SKILL.md frontmatter
        //    `tools[]` + `runtime` 段声明。此处不再合成 mcp/manifest.json。

        // 3) plugin.json 落盘：有原声明的写规范化副本；无 plugin.json 的裸包合成
        //    最小自描述清单（§5.2 派生 manifest），保证按包聚合目录始终带 plugin.json。
        let synth;
        let doc = match &manifest {
            Some(m) => m,
            None => {
                synth = synthesized_manifest(&det);
                &synth
            }
        };
        std::fs::write(
            staged.join("plugin.json"),
            serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("写 plugin.json: {e}"))?;

        // 4) 图标：包内可选图标优先，缺省写默认图标。落定的相对路径由
        //    write_icon_bytes / write_default_icon 的返回值直接带回（唯一写入方），
        //    报告不再在 landing 后重新探测磁盘求同一事实。
        if let Some((icon_path, bytes)) = &icon_entry {
            Ok(write_icon_bytes(&staged, icon_path, bytes)?)
        } else if let Some(declared) = manifest.as_ref().and_then(|m| m.icon.clone()) {
            let mut found = false;
            let mut icon_rel = String::new();
            for p in &all_paths {
                if *p == declared && is_supported_icon(&declared) {
                    let mut e = archive.by_name(p).map_err(|e| format!("读图标: {e}"))?;
                    let declared_size = e.size();
                    let buf = read_zip_entry_bounded(&mut e, declared_size, "图标")?;
                    icon_rel = write_icon_bytes(&staged, &declared, &buf)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!(
                    "plugin.json 声明的图标 '{declared}' 不存在或格式不支持"
                ));
            }
            Ok(icon_rel)
        } else {
            Ok(write_default_icon(&staged)?)
        }
    })();
    let icon_rel = match write_result {
        Ok(icon_rel) => icon_rel,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }
    };

    // 安装时 smoke test：旧 spanner 自检已删除。当前只保留纯 MCP / 纯 skill 落盘。

    // The landing mark was written before the pre-land gate above: the
    // journal covers the whole gate + staging + landing
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
        // The restore must not be silent — `rename_dir_with_retry`
        // never logs, so a failed restore would strand the pre-import content at
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
    // The backup is retained until supply resolves. A
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
    if !det.mcp_servers.is_empty() {
        if let Err(e) = mgr.install_upload(
            &id,
            super::store::BundleSource::Upload(display_name.to_string()),
        ) {
            // Mirror the restore path's supply-failure
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
            // When the rollback itself failed,
            // the landed dir stays live with zero consent — the user-facing
            // error must carry that, not just the log line. The suffix is
            // user-visible copy — English sibling-form (like the surrounding
            // English-language skill_gate/ima errors), not CJK that would
            // surface raw to en/ja users.
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
    // Rebaseline (consume) the description backup only after supply
    // succeeded: on a moved_old rollback the old files are
    // restored, so the restore point must survive for the next attempt.
    super::skill_marketplace::rebaseline_skill_desc_backup(&store, &id, "统一导入");
    if moved_old {
        // The post-supply backup delete is in the same silent-strand family
        // — a failure leaves the pre-import content in
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
    // Tail failpoint: lets the hold-span pin observe the lease at the very
    // end of the span. No-op outside tests.
    test_park_before_registration();
    if let Err(e) = super::store::BundleStore::new().upsert_preserving(record) {
        // 登记失败必须如实失败：bundles.json 是纯技能上传包的唯一登记，警告
        // 后照常返回会让包落盘无登记、标记又被 guard 清掉——live-by-absence
        // 且下次启动的 reconcile 永远追不到。defuse 保留 <id>.pending，本次
        // 导入报错；下个启动周期按「有标记 + 有目录 + 无记录」把包搬进可恢复
        // 隔离目录。
        landing_guard.defuse();
        return Err(format!(
            "登记 bundles.json 失败（import {id}）: {e}；已保留恢复标记，下次启动会把该包对账隔离到可恢复目录，可从隔离目录手动恢复后重试导入"
        ));
    }
    // （导入即重基线：消费点在上方 rebaseline 调用处——仅供给成功后消费；
    // 本 upsert 位点不是重基线点。）

    Ok(PluginImportReport {
        id,
        kind,
        icon: icon_rel,
    })
}

#[cfg(test)]
mod tests {
    // 断言中途 unwrap 恐慌也不遗留临时 zip（panic 走 unwind，Drop 仍执行）。
    struct RemoveTempZip<'a>(&'a std::path::Path);
    impl Drop for RemoveTempZip<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }

    // Round-37 review anti-drift pin: the CLI's error translation keys on
    // substrings of the rejection strings below (不安全路径 / symlink /
    // 伪造头部 / 解压+上限). Rewording any of them silently reverts the
    // headless surface to verbatim Chinese on exactly the zip-slip/bomb
    // refusals; if you must reword, update
    // pinvou-cli/crates/cli/src/plugins.rs `translate_plugin_import_error`
    // in the same change.
    const CLI_TRANSLATION_ANCHORS: [(&str, &[&str]); 4] = [
        ("zip-slip traversal", &["不安全路径"]),
        ("symlink entry", &["symlink"]),
        ("forged header / zip bomb", &["伪造头部", "解压"]),
        ("extraction cap", &["解压", "上限"]),
    ];

    /// 底座安装安全原语复用的等价性钉：旧内联「饱和累加 + 严格大于才拒绝」
    /// 的大小闸 vs `deepseek_tui::skills::install::add_entry_size`，含 `== max`
    /// 放行边界与 u64 饱和边界。委托实现的错误路径不再把新累计值写回
    /// `declared_total`（旧实现先写回再返回 Err）——错误会直接中止导入，累计
    /// 值随之作废，无可观察差异。
    ///
    /// 钉的是引擎语义而非生产接线：接线由唯一的编译期生产调用点
    /// （`checked_zip_entry_path`）约束，把调用点静默改回内联算术本钉不红，
    /// 引擎侧语义漂移才在此变红——两类漂移各有归属，勿混淆。
    #[test]
    fn forkguard_declared_size_cap_delegates_to_engine_add_entry_size() {
        let max_bytes: u64 = 1000;
        let cases = [
            (0, 0),
            (0, 100),
            (100, 50),
            (950, 50),
            (950, 51),
            (999, 1),
            (1000, 1),
            (1000, 0),
            (u64::MAX, 0),
            (u64::MAX, 1),
            (u64::MAX - 1, 1),
            (u64::MAX - 1, 2),
        ];
        for (total, added) in cases {
            let old = {
                let t = total.saturating_add(added);
                if t > max_bytes { Err("over") } else { Ok(t) }
            };
            let new = deepseek_tui::skills::install::add_entry_size(total, added, max_bytes)
                .map_err(|_| "over");
            assert_eq!(new, old, "add_entry_size({total}, {added}, {max_bytes})");
        }
    }

    /// 穿越面等价性钉：`checked_zip_entry_path` 的穿越判定保留 zip crate 的
    /// `enclosed_name` sanitizer 而非底座 `is_safe_path` 拒绝谓词——sanitizer
    /// 的归一化输出喂给 `skill_md_rank` 等下游布局逻辑（`./SKILL.md` 今日归一
    /// 成 `SKILL.md` 后才参与 rank），且其对根前缀/中段 `..` 的「净化放行」
    /// 语义是既有放行面。本钉把底座谓词在这些输入上的 verdict 差异登记为刻意
    /// 选择，防止后续无意识互换（zip crate 的归一化器不可独立复用，只能经
    /// `ZipFile::enclosed_name` 触达）。
    /// sanitizer 侧 verdict 由下方内存 zip 断言直接 executable 化，不依赖
    /// 端到端导入测试的间接覆盖。
    #[test]
    fn forkguard_zip_gate_keeps_zip_sanitizer_verdicts_over_engine_reject_predicate() {
        use deepseek_tui::skills::install::is_safe_path;
        // 逃逸面：底座谓词拒绝（zip sanitizer 同样拒绝，端到端由导入测试覆盖）。
        assert!(!is_safe_path(Path::new("../x")));
        assert!(!is_safe_path(Path::new("a/../../x")));
        // 正常面：两边都放行。
        assert!(is_safe_path(Path::new("skills/x/SKILL.md")));
        assert!(is_safe_path(Path::new("plugin.json")));
        // 刻意登记的 verdict 差异：sanitizer 归一化放行、底座拒绝谓词会拒绝
        // 的输入（`a/../b`、根前缀；Windows 驱动器前缀）。
        assert!(!is_safe_path(Path::new("a/../b")));
        assert!(!is_safe_path(Path::new("/abs/x")));
        if cfg!(windows) {
            assert!(!is_safe_path(Path::new("C:/x")));
        }
        // 两边都放行但输出不同：sanitizer 把 `./b` 塌缩成 `b`，底座谓词放行
        // 原串——归一化输出承重，是保留 sanitizer 的另一原因。
        assert!(is_safe_path(Path::new("./b")));

        // 内存 zip executable 钉：把登记的 sanitizer 语义（depth-0 前缀剥离
        // 放行、`./`/中段 `..` 塌缩、前置 `..` 逃逸拒绝、Windows 驱动器前缀
        // 剥离——zip 8.x 在所有平台经 Utf8WindowsPath 解析条目名，Unix 上
        // 同样放行 `C:/x`）直接钉在 `enclosed_name` 的真实 verdict 上。zip
        // crate 升级若改变其中任何一条（比如把 depth-0 前缀从剥离改成拒绝），
        // 这里先红，登记随之更新。
        let zip_path = std::env::temp_dir().join(format!(
            "pinvu-sanitizer-verdicts-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _temp_guard = RemoveTempZip(&zip_path);
        let entries = [
            ("../x", None::<&str>),
            ("/abs/x", Some("abs/x")),
            ("./b", Some("b")),
            ("a/../b", Some("b")),
            ("C:/x", Some("x")),
            ("ok.txt", Some("ok.txt")),
        ];
        let verdicts: Vec<Option<String>> = {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, _) in entries {
                zw.start_file(name, opts).unwrap();
                use std::io::Write;
                zw.write_all(b"x").unwrap();
            }
            zw.add_symlink("link.txt", "/etc/ssh/ssh_config", opts)
                .unwrap();
            zw.finish().unwrap();
            let mut archive =
                zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
            (0..archive.len())
                .map(|i| {
                    archive
                        .by_index(i)
                        .unwrap()
                        .enclosed_name()
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .collect()
        };
        for ((name, expected), actual) in entries
            .iter()
            .chain(std::iter::once(&("link.txt", Some("link.txt"))))
            .zip(verdicts.iter())
        {
            let expected = expected.map(|e| e.to_string());
            assert_eq!(
                actual, &expected,
                "enclosed_name verdict for {name:?} 与登记的 sanitizer 语义不符"
            );
        }

        // symlink 面 executable 化：unix_mode 文件类型位 = symlink 的条目被
        // 生产闸拒绝（底座 `entry_type_is_link` 的 zip 面）。链接条目名本身
        // 的 enclosed_name 是放行的（上一循环），拒绝必须来自 symlink 判定。
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut link = archive.by_name("link.txt").unwrap();
        assert!(link.enclosed_name().is_some(), "链接名本身不触发穿越拒绝");
        let mut total = 0u64;
        let err = checked_zip_entry_path(&mut link, &mut total, u64::MAX, "link")
            .expect_err("symlink 条目必须被生产闸拒绝");
        assert_eq!(err, REJECT_SYMLINK);
    }

    /// 生产闸承担声明大小上限的 executable 钉：无论 `checked_zip_entry_path`
    /// 内部是委派 `add_entry_size` 还是旧内联实现，上限执行本身不得被移除——
    /// 等价性测试（上方）钉的是引擎语义，本钉钉的是生产接线。三条目声明大小
    /// 累计越过 `max_bytes` 时第三条必须被拒，且前两条照常放行、累计正确。
    #[test]
    fn forkguard_zip_gate_enforces_declared_size_cap() {
        let zip_path = std::env::temp_dir().join(format!(
            "pinvu-declared-cap-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _temp_guard = RemoveTempZip(&zip_path);
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            for name in ["a.txt", "b.txt", "c.txt"] {
                zw.start_file(name, opts).unwrap();
                use std::io::Write;
                zw.write_all(&[b'x'; 400]).unwrap();
            }
            zw.finish().unwrap();
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut total = 0u64;
        let max_bytes: u64 = 1000;
        for name in ["a.txt", "b.txt"] {
            let mut entry = archive.by_name(name).unwrap();
            let path = checked_zip_entry_path(&mut entry, &mut total, max_bytes, name)
                .unwrap_or_else(|e| panic!("{name} 在 {total}/{max_bytes} 内应放行: {e}"));
            assert_eq!(path, name);
        }
        assert_eq!(total, 800, "前两条目声明大小应已累计");
        let mut entry = archive.by_name("c.txt").unwrap();
        let err = checked_zip_entry_path(&mut entry, &mut total, max_bytes, "c.txt")
            .expect_err("越过声明上限必须被拒");
        assert_eq!(err, reject_declared_cap("c.txt", max_bytes));
    }

    /// The literal rejection messages must keep carrying the substrings the
    /// CLI translator searches for. Asserts on the PRODUCTION constants
    /// (round-38 review: hand-copied strings made this vacuous — rewording
    /// the real messages passed while the translator silently stopped
    /// matching). The extraction-cap arms are pinned through the production
    /// `reject_*_cap` helpers, whose formatted output is what the CLI's
    /// 解压+上限 branch actually sees (round-39 review: the cap branch was
    /// claimed here but never anchored).
    #[test]
    fn cli_error_translation_anchors_are_present_in_the_messages() {
        let joined = format!(
            "{REJECT_TRAVERSAL}{REJECT_SYMLINK}x{REJECT_FORGED_HEADER}x{}x{}",
            reject_declared_cap("条目 x", MAX_PLUGIN_SIZE_BYTES),
            reject_actual_cap(MAX_PLUGIN_SIZE_BYTES),
        );
        for (_, anchors) in CLI_TRANSLATION_ANCHORS {
            for anchor in anchors {
                assert!(
                    joined.contains(anchor),
                    "the CLI translator anchors on '{anchor}', which no longer appears in the \
                     rejection messages"
                );
            }
        }
    }

    use super::*;

    use crate::platform::test_support::write_test_zip;

    /// Shared rejection tail for the refusal pins: the import must fail with
    /// an error naming `needle`, and leave no `bundles/<id>/` behind. Returns
    /// the error for the sites that additionally assert a second needle or a
    /// surviving foreign copy.
    fn assert_import_refused(
        zip_path: &Path,
        display_name: &str,
        id: &str,
        needle: &str,
    ) -> String {
        let err = import_plugin_package(&zip_path.to_string_lossy(), display_name).unwrap_err();
        assert!(
            err.contains(needle),
            "rejection should name {needle:?}: {err}"
        );
        assert!(
            !crate::platform::paths::bundles_root().join(id).exists(),
            "拒收不得落盘包目录"
        );
        err
    }

    /// A leftover landing mark with a landed
    /// dir but no registry record is crash residue — the reconcile moves it
    /// out of `bundles_root` into a recoverable holding dir and clears the
    /// mark. The mixed-case fixture id also pins that the reconcile reader
    /// accepts every id the journal writer accepts (`is_safe_skill_name`,
    /// mixed case included).
    #[test]
    fn reconcile_quarantines_landed_dir_without_registry_record() {
        crate::platform::test_support::with_temp_home("pinvou-import-journal-crash", || {
            let pkg = crate::platform::paths::bundles_root().join("Crash-Pkg");
            std::fs::create_dir_all(pkg.join("skills/Crash-Pkg")).unwrap();
            std::fs::write(
                pkg.join("skills/Crash-Pkg/SKILL.md"),
                "---\nname: Crash-Pkg\n---\n",
            )
            .unwrap();
            mark_landing("Crash-Pkg");
            assert!(
                landing_mark_path("Crash-Pkg").exists(),
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
                .filter(|n| n.starts_with("Crash-Pkg.crash-"))
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
                !landing_mark_path("Crash-Pkg").exists(),
                "the resolved entry is cleared"
            );
        });
    }

    /// The record-present reconcile arm must also sweep
    /// a crash-orphaned reimport staging dir — with the mark cleared nothing
    /// else would ever delete it (a permanent duplicate of the installed
    /// pack). A FAILED sweep keeps the mark for the next boot instead of
    /// silently stranding the residue; the retry leg then sweeps and clears.
    #[test]
    fn record_present_arm_sweeps_orphaned_staging_and_keeps_mark_on_failure() {
        crate::platform::test_support::with_temp_home("pinvou-import-arm-a", || {
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
            crate::features::marketplace::store::plant_installed_record(
                "re-tmp",
                crate::features::marketplace::store::BundleSource::Upload("re-tmp.zip".to_string()),
            );
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
        });
    }

    /// A crash between the re-import's two renames (pack
    /// dir already in `<id>.old`, staged copy not yet renamed in, mark still
    /// on file, record still installed) must converge: the reconcile restores
    /// the pre-import backup in place, and the record-present arm then sweeps
    /// the staged `.tmp` and clears the mark — otherwise the crash would
    /// converge to "record installed, content gone, the only copy stranded
    /// in the scan-invisible `.old`" with no retry.
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
            crate::features::marketplace::store::plant_installed_record(
                "re-old",
                crate::features::marketplace::store::BundleSource::Upload("re-old.zip".to_string()),
            );
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

    /// Same fixture with the record gone (e.g. a
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

    /// A failed backup restore must not consume the strand silently.
    #[test]
    #[cfg(unix)]
    fn restore_failure_keeps_the_mark_and_retries() {
        use crate::platform::test_support::dir_read_only_or_skip;
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
            // Root probe (mode bits are no-ops for root): if the dir is still
            // writable the fixture proves nothing — skip loudly.
            if !dir_read_only_or_skip(&bundles, "restore_failure_keeps_the_mark_and_retries") {
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

    /// The three lease tests' shared demo combo-pack fixture: byte-identical
    /// entries (plugin.json + mcp/manifest.json + mcp/server.py +
    /// skills/demo/SKILL.md), written to each test's differing archive path.
    fn write_demo_combo_zip(path: &std::path::Path) {
        use std::io::Write;
        let f = std::fs::File::create(path).unwrap();
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

    /// The import pipeline must PARK while a peer holds
    /// the landing lease and proceed only after release. Tenancy is not
    /// observable by any structural pin — a scoped-block release
    /// (`let _g = { let mut l = open_landing_lease(&id)?; l.write()? };`)
    /// keeps every acquisition literal present and ordered — so the property
    /// uses the suite's established blocked-worker harness (fixture-held
    /// lease + production fn on a thread + bounded `recv_timeout`, the
    /// uninstall/restore/reconcile lanes' shape).
    #[test]
    fn import_parks_while_a_peer_holds_the_landing_lease() {
        crate::platform::test_support::with_temp_home("pinvou-import-lease-park", || {
            // The helper's temp home: both the fixture zip and the landed
            // layout are addressed relative to it.
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = dir.join("combo.zip");
            write_demo_combo_zip(&zip_path);

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
        });
    }

    /// The lease HOLD-SPAN: the landing lease must stay held from acquisition
    /// through landing and registration, not merely be acquired before the
    /// mark. The park test above cannot distinguish a scoped-block release
    /// (the worker parks identically at acquire and lands after the peer
    /// releases), so this test drives the production pipeline with a
    /// `pre_land` closure — which runs after the lease + mark, before
    /// anything lands — and asserts a peer's try-acquire FAILS mid-flight,
    /// then that it SUCCEEDS once the import completed and dropped its
    /// guards.
    #[test]
    fn import_holds_the_landing_lease_across_the_whole_span() {
        crate::platform::test_support::with_temp_home("pinvou-import-lease-span", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = dir.join("span.zip");
            write_demo_combo_zip(&zip_path);

            let report = import_plugin_package_gated(
                &zip_path.to_string_lossy(),
                "combo.zip",
                &|id, _skills| {
                    // Mid-flight (lease + mark held, nothing landed): a
                    // peer's try-acquire must FAIL. The second open is a
                    // distinct file handle, so a contended try means the
                    // pipeline really holds the lease.
                    let mut peer = open_landing_lease(id).expect("the peer lease opens");
                    assert!(
                        peer.try_write().is_err(),
                        "the landing lease must stay HELD while the import is mid-flight"
                    );
                    Ok(())
                },
            )
            .expect("the import completes under its own lease");
            assert_eq!(report.id, "demo");
            assert!(
                dir.join("bundles")
                    .join("demo")
                    .join("mcp")
                    .join("manifest.json")
                    .is_file(),
                "the import landed its package content"
            );

            // After completion (mark cleared, guards dropped) the lease is
            // free again. The probe's point is the `expect`ed acquisition
            // itself; the must_use guard it returns is intentionally
            // discarded, bound in an explicit drop to state that intent.
            let mut after = open_landing_lease("demo").expect("the post-completion lease opens");
            drop(
                after
                    .try_write()
                    .expect("the landing lease must be released once the import completes"),
            );
        });
    }

    /// The mid-span probe above cannot distinguish "the
    /// lease is released at function end" from "released right after the
    /// landing rename" — a scoped-block refactor in the supply →
    /// registration tail would leave that window cross-process-unguarded
    /// while every existing pin stays green. This pin parks immediately
    /// before the bundles.json registration and requires the lease to still
    /// be HELD there, closing the tail half of the hold span.
    #[test]
    fn import_holds_the_landing_lease_through_registration() {
        crate::platform::test_support::with_temp_home("pinvou-import-lease-tail", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = dir.join("span-tail.zip");
            write_demo_combo_zip(&zip_path);

            struct ResetTailPark;
            impl Drop for ResetTailPark {
                fn drop(&mut self) {
                    *LANDING_REGISTRATION_PARK
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()) = None;
                }
            }
            let (reached_tx, reached_rx) = std::sync::mpsc::channel::<()>();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            *LANDING_REGISTRATION_PARK
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some((reached_tx, release_rx));
            let _reset = ResetTailPark;

            let worker_zip = zip_path.clone();
            let worker = std::thread::spawn(move || {
                import_plugin_package(&worker_zip.to_string_lossy(), "combo.zip")
            });
            reached_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("the import must reach the pre-registration failpoint");

            let mut peer = open_landing_lease("demo").expect("the peer lease opens");
            assert!(
                peer.try_write().is_err(),
                "the landing lease must still be HELD immediately before registration"
            );

            release_tx
                .send(())
                .expect("the failpoint release channel is live");
            let report = worker
                .join()
                .expect("the import worker should not panic")
                .expect("the import completes after the failpoint releases");
            assert_eq!(report.id, "demo");
            assert!(
                dir.join("bundles")
                    .join("demo")
                    .join("mcp")
                    .join("manifest.json")
                    .is_file(),
                "the released import landed its package content"
            );
        });
    }

    /// The boot reconcile must treat a held landing
    /// lease as a LIVE import and defer, not as crash residue — the mark
    /// alone cannot distinguish the two, and sweeping a live import's staged
    /// dir (or clearing its mark, which the retired-tool sweep keys on)
    /// destroyed the import in the shared-home two-process scenario. Once
    /// the lease is released (the importer exited or crashed), the same
    /// entry is genuine residue and the reconcile resolves it.
    #[test]
    fn reconcile_spares_a_live_import_and_sweeps_after_release() {
        crate::platform::test_support::with_temp_home("pinvou-import-lease-live", || {
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
        });
    }

    /// A leftover mark over a COMPLETED import
    /// (registry record present — crash hit after the upsert, before the
    /// guard drop) keeps the pack and just clears the stale mark.
    #[test]
    fn reconcile_keeps_completed_import_and_clears_stale_mark() {
        crate::platform::test_support::with_temp_home("pinvou-import-journal-done", || {
            let pkg = crate::platform::paths::bundles_root().join("done-pkg");
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
            crate::features::marketplace::store::plant_installed_record(
                "done-pkg",
                crate::features::marketplace::store::BundleSource::Upload("done.zip".to_string()),
            );
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
        });
    }

    /// A crash between the reimport's landing
    /// rename and the post-supply backup delete strands the full pre-import
    /// copy at `bundles/<id>.old` beside the landed pack — the record-present
    /// arm's `.tmp` sweep alone would leave it forever (up to the zip budget of
    /// scan-invisible duplicate). The reconcile must sweep the stranded
    /// backup as superseded residue while keeping the landed pack and its
    /// record (the restore arm has already converted any "only copy" `.old`
    /// into the pack dir before the record arms run, so this shape is always
    /// the superseded copy).
    #[test]
    fn reconcile_sweeps_the_crash_stranded_reimport_backup() {
        crate::platform::test_support::with_temp_home("pinvou-import-stranded-old", || {
            let id = "old-pkg";
            let pkg = crate::platform::paths::bundles_root().join(id);
            std::fs::create_dir_all(pkg.join("mcp")).unwrap();
            std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
            std::fs::write(pkg.join("mcp").join("manifest.json"), "{}").unwrap();
            // The stranded pre-import backup: distinct content so the assert
            // below proves the LANDED pack survived, not the backup.
            let backup = pkg.with_extension("old");
            std::fs::create_dir_all(&backup).unwrap();
            std::fs::write(backup.join("pre-import-marker"), b"old").unwrap();
            crate::features::marketplace::store::plant_installed_record(
                id,
                crate::features::marketplace::store::BundleSource::Upload("old.zip".to_string()),
            );
            mark_landing(id);

            reconcile_import_journal().unwrap();

            assert!(
                !backup.exists(),
                "the crash-stranded reimport backup must be swept by the record-present arm"
            );
            assert!(
                pkg.join("plugin.json").is_file(),
                "the landed pack itself must survive the sweep"
            );
            assert!(
                !landing_mark_path(id).exists(),
                "the stale mark is cleared after a converged reconcile"
            );
        });
    }

    /// A mark with no landed dir (crash before
    /// the landing rename, or after an in-process rollback that had already
    /// cleared the dir) resolves to a cleared mark and zero side effects.
    #[test]
    fn reconcile_clears_mark_without_landed_dir() {
        crate::platform::test_support::with_temp_home("pinvou-import-journal-none", || {
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
        });
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
        crate::platform::test_support::with_temp_home("pinvou-combo-import", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = write_test_zip(
                "pinvou-combo-import",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}],"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["demo"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/server.py",
                        b"import json\nprint(json.dumps({'ok': True}))".to_vec(),
                    ),
                    (
                        "skills/demo/SKILL.md",
                        b"---\nname: demo\n---\n# hi".to_vec(),
                    ),
                ],
            );

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
            let mcp_raw = std::fs::read_to_string(crate::platform::paths::mcp_config_path())
                .unwrap_or_default();
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
            // 用户唯一副本。
            let record = crate::features::marketplace::store::BundleStore::new()
                .get("demo")
                .unwrap()
                .expect("demo 应登记");
            assert_eq!(
                record.source,
                crate::features::marketplace::store::BundleSource::Upload("combo.zip".to_string()),
                "上传包登记来源应为 Upload"
            );
        });
    }

    /// Trust boundary (docs/builtin-toolset-contract.md §3.1): an uploaded
    /// package whose MCP manifest claims `builtin: true` or
    /// `visibility: "system"` is rejected with an explicit error instead of
    /// silently landing with the claim stripped — builtin status belongs to
    /// the publisher's embedded catalog only. The 2×2 loop covers both claim
    /// kinds in both manifest lanes the import scans: the primary
    /// `mcp/manifest.json`, and the `other_manifests` fallback lane (a claim
    /// hidden in a secondary `manifest.json`, e.g. `extra/manifest.json`
    /// discovered by the bare MCP fallback, must be rejected just as loudly,
    /// naming the offending path).
    #[test]
    fn import_rejects_manifest_claiming_builtin_semantics() {
        crate::platform::test_support::with_temp_home("pinvou-builtin-claim", || {
            let manifest = |manifest_id: &str, tail: &str| {
                format!(
                    r#"{{"id":"{manifest_id}","name":"n","description":"d","version":"1","icon":"x","category":"c","mcp_tools":[],"command":"python","args":["server.py"]{tail}}}"#
                )
            };
            for (secondary, id, claim, needle) in [
                (
                    false,
                    "claim-builtin",
                    r#","builtin":true"#,
                    "builtin: true",
                ),
                (false, "claim-system", r#","visibility":"system""#, "system"),
                (
                    true,
                    "claim-secondary-builtin",
                    r#","builtin":true"#,
                    "builtin: true",
                ),
                (
                    true,
                    "claim-secondary-system",
                    r#","visibility":"system""#,
                    "system",
                ),
            ] {
                let mut entries: Vec<(&str, Vec<u8>)> =
                    vec![("mcp/server.py", b"print('hi')".to_vec())];
                if secondary {
                    // The primary MCP manifest is clean...
                    entries.push(("mcp/manifest.json", manifest(id, "").into_bytes()));
                    // ...but a secondary manifest carries the builtin claim.
                    entries.push((
                        "extra/manifest.json",
                        manifest(&format!("{id}-extra"), claim).into_bytes(),
                    ));
                } else {
                    entries.push(("mcp/manifest.json", manifest(id, claim).into_bytes()));
                }
                let zip_path = write_test_zip("pinvou-builtin-claim", &entries);
                let err = assert_import_refused(&zip_path, &format!("{id}.zip"), id, needle);
                if secondary {
                    assert!(
                        err.contains("extra/manifest.json"),
                        "rejection should name the offending secondary manifest path: {err}"
                    );
                }
            }
        });
    }

    /// 回退兼容：符合 skills 标准的裸技能包（无 plugin.json，SKILL.md 在命名目录下，
    /// name 取自 frontmatter）→ 规范化为 `skills/<name>/` 布局并识别为纯技能包。
    #[test]
    fn import_bare_skill_package_lands_canonical_layout() {
        crate::platform::test_support::with_temp_home("pinvou-bare-skill", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = write_test_zip(
                "pinvou-bare-skill",
                &[
                    ("my-skill/SKILL.md", b"---\nname: greet\n---\n# hi".to_vec()),
                    ("my-skill/scripts/helper.py", b"print('hi')".to_vec()),
                ],
            );

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
        });
    }

    /// A zip whose root holds a single SKILL.md → recognized by the bare-skill
    /// fallback and landed on disk. This is the path import_skill_md_bytes drives.
    #[test]
    fn root_skill_md_import_lands_canonical_layout() {
        crate::platform::test_support::with_temp_home("pinvou-root-skill", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = write_test_zip(
                "pinvou-root-skill",
                &[("SKILL.md", b"---\nname: greet\n---\n# hi".to_vec())],
            );

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
        });
    }

    /// The landing journal mark must be written BEFORE
    /// the pre-land consent gate runs. The gate registers the import's
    /// deny-first rows before anything lands, and the startup sweep's
    /// pre-strip probe ([`landing_in_progress`]) can only see this import
    /// through the mark until its record lands — mark-after-gate would leave
    /// a cross-process sweep free to strip the just-registered rows.
    #[test]
    fn pre_land_gate_runs_under_the_landing_mark() {
        crate::platform::test_support::with_temp_home("pinvou-import-mark-before-gate", || {
            let zip_path = write_test_zip(
                "pinvou-import-mark-before-gate",
                &[("SKILL.md", b"---\nname: greet\n---\n# hi".to_vec())],
            );

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
        });
    }

    /// `mark_landing` writes under the cross-process landing
    /// lease, so a planted FIFO or symlink at the mark path must refuse fast
    /// (the hardened private-write primitive's regular-file gate /
    /// O_NOFOLLOW), not block the lease holder's open — the lease wedges
    /// same-id imports, uninstalls, and the boot sweep. Bounded worker: a
    /// regressed bare `std::fs::write` hangs here and fails the recv_timeout.
    #[cfg(unix)]
    #[test]
    fn mark_landing_refuses_a_planted_fifo_and_symlink() {
        // RAII containment: a failing assertion unwinds past a
        // straight-line restore and would leave PINVOU3_HOME pointed at a
        // deleted temp dir for every later test in the process.
        crate::platform::test_support::with_temp_home("pinvou-mark-hardening", || {
            let dir = crate::platform::paths::pinvou3_home();

            // A planted FIFO at the mark path: the write must refuse, not open-block.
            let fifo = landing_mark_path("fifo-id");
            std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
            crate::platform::paths::tests::plant_fifo(&fifo);
            // Uniform abort containment with the other FIFO pins: a future
            // edit that takes a lock around mark_landing must not turn one
            // regression into a lane-wide cascade hang.
            crate::platform::test_support::run_bounded_worker_or_abort(
                "mark_landing still blocks on a planted FIFO",
                || mark_landing("fifo-id"),
            );
            // The refuse half of the contract: the hardened primitive must
            // have refused the non-regular target, so the FIFO is still the
            // FIFO. `mark_landing` degrades a refused write to
            // live-by-absence by design (the loud log is the trace), but a
            // regression that "succeeds" by unlinking the planted FIFO and
            // writing a fresh regular mark would silently replace the
            // evidence and land here.
            assert!(
                std::fs::symlink_metadata(&fifo)
                    .map(|m| {
                        use std::os::unix::fs::FileTypeExt as _;
                        m.file_type().is_fifo()
                    })
                    .unwrap_or(false),
                "the planted FIFO must survive the refused mark write (nothing may replace it)"
            );

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
        });
    }

    /// 统一导入路径（三条 UI 上传通道的汇聚点）重导入时必须重基线 SKILL.md 说明
    /// 备份：导入 v1 → 设说明（备份 orig1）→ 按文档建议删包目录后导入 v2 →
    /// 旧备份必须被丢弃，清覆盖后恢复的是 v2 的 orig2，而非旧包的 orig1
    /// （重基线修复最初只落在遗留路径 import_package_named 上，UI 全走不到）。
    #[test]
    fn unified_reimport_rebaselines_skill_description_backup() {
        crate::platform::test_support::with_temp_home("pinvou-unified-rebaseline", || {
            let dir = crate::platform::paths::pinvou3_home();

            let make_zip = |desc: &str| {
                write_test_zip(
                    "pinvou-unified-rebaseline",
                    &[(
                        "my-skill/SKILL.md",
                        format!("---\nname: greet\ndescription: {desc}\n---\n# hi").into_bytes(),
                    )],
                )
            };
            let v1 = make_zip("orig1");

            let report = import_plugin_package(&v1.to_string_lossy(), "v1.zip").unwrap();
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
            let v2 = make_zip("orig2");
            let report = import_plugin_package(&v2.to_string_lossy(), "v2.zip").unwrap();
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
            let md =
                std::fs::read_to_string(dir.join("bundles/greet/skills/greet/SKILL.md")).unwrap();
            assert!(
                md.contains("description: orig2"),
                "SKILL.md 应保持新包原值: {md}"
            );
            assert!(!md.contains("orig1"), "旧包原值不得被恢复进新包: {md}");
        });
    }

    /// 说明备份与供给失败的顺序契约：重基线
    /// 只在新内容**留存**时发生——moved_old 回滚把旧版文件还原后，旧包的说明
    /// 备份必须幸存（它正是旧内容的有效还原点）。v2 附 mcp/manifest.json 触发供给，
    /// 并预置形状损坏的 mcp.json（合法 JSON 但 servers 非对象）令
    /// add_to_mcp_json 确定性失败——导入返回 Err、旧版还原后，备份必须仍是
    /// 旧值；随后成功导入新版本时重基线照常消费备份。
    #[test]
    fn unified_import_keeps_backup_through_supply_failure_rollback() {
        crate::platform::test_support::with_temp_home(
            "pinvou-unified-rebaseline-supply-fail",
            || {
                let dir = crate::platform::paths::pinvou3_home();

                let make_zip = |desc: &str, with_mcp: bool| {
                    let mut entries = vec![(
                        "my-skill/SKILL.md",
                        format!("---\nname: greet\ndescription: {desc}\n---\n# hi").into_bytes(),
                    )];
                    if with_mcp {
                        // 字段完整的合法 manifest（ToolManifest 必填字段齐全）→ 探测
                        // 阶段认作 MCP 组件（包 id 取自它，与技能 frontmatter name 同
                        // 值以命中同一条登记）；供给真正必败点在下方预置的形状损坏
                        // mcp.json（servers 非对象），与 manifest 内容无关。
                        entries.push((
                        "mcp/manifest.json",
                        br#"{"id":"greet","name":"greet","description":"t","version":"0.0.1","icon":"","category":"custom","mcp_tools":[],"command":"echo","args":[]}"#
                            .to_vec(),
                    ));
                    }
                    write_test_zip("pinvou-unified-rebaseline-supply-fail", &entries)
                };
                let v1 = make_zip("orig1", false);

                let report = import_plugin_package(&v1.to_string_lossy(), "v1.zip").unwrap();
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
                let v2 = make_zip("orig2", true);
                let err = import_plugin_package(&v2.to_string_lossy(), "v2.zip")
                    .expect_err("MCP 供给必须失败");
                assert!(err.contains("MCP 供给失败"), "失败须来自供给步骤: {err}");

                // 回归点：moved_old 回滚还原了旧版文件——说明备份必须
                // 幸存（还原点与还原后的旧内容一致；若在此处丢弃，重试导入前的一次
                // 清覆盖恢复将没有还原点可用）。
                assert_eq!(
                    store.skill_desc_backup("greet").unwrap().as_deref(),
                    Some("orig1"),
                    "供给失败回滚后备份必须幸存（旧内容仍以还原点保护）"
                );
                // 供给失败不得残留已落地的无登记目录——其技能会以
                // 零同意进入每个已初始化 scope（本次为首次安装路径，无旧版本可还原，
                // 回滚 = 移除落盘目录）。
                assert!(
                    !dir.join("bundles/greet").exists(),
                    "供给失败后落盘目录必须回滚移除"
                );

                // 换版路径（moved_old=true）：旧版本在盘时同一包重导、
                // 供给失败（同一 zip 过碰撞检查→落盘→供给必败）→ 回滚 = 还原旧版本目录
                // （旧包继续可用，新包不入），备份清理。
                std::fs::write(&mcp_path, r#"{"servers": {}}"#).unwrap();
                let v1m = make_zip("orig1-again", true);
                let report = import_plugin_package(&v1m.to_string_lossy(), "v1m.zip").unwrap();
                assert_eq!(report.id, "greet");
                std::fs::write(&mcp_path, r#"{"servers": "broken"}"#).unwrap();
                let err = import_plugin_package(&v1m.to_string_lossy(), "v1m.zip")
                    .expect_err("同包重导的供给必须失败");
                assert!(err.contains("MCP 供给失败"), "失败须来自供给步骤: {err}");
                let md = std::fs::read_to_string(dir.join("bundles/greet/skills/greet/SKILL.md"))
                    .unwrap();
                assert!(
                    md.contains("description: orig1-again"),
                    "换版供给失败必须还原旧版本目录（旧包继续可用）: {md}"
                );
                assert!(
                    !dir.join("bundles/greet.old").exists(),
                    "回滚成功后备份目录必须清理"
                );
            },
        );
    }

    /// 同包重导：落盘的 plugin.json 是 `to_string_pretty` 规范化字节，与 zip 原始
    /// 字节直比必误判冲突（三轮评审）——必须按解析后的语义比对，同包重导应放行。
    #[test]
    fn reimport_same_plugin_package_is_allowed() {
        crate::platform::test_support::with_temp_home("pinvou-reimport", || {
            let zip_path = write_test_zip(
                "pinvou-reimport",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                ],
            );

            let first = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip").unwrap();
            assert_eq!(first.id, "demo");
            // 同包重导：落盘 plugin.json 是规范化字节（与 zip 原始字节不同），语义比对
            // 一致 → 放行（原子替换），不得报「内容不同」冲突。
            let second = import_plugin_package(&zip_path.to_string_lossy(), "combo.zip")
                .unwrap_or_else(|e| panic!("同包重导应放行，实际报错: {e}"));
            assert_eq!(second.id, "demo");

            // 对照：同 id 不同内容仍必须拒绝（防静默互覆盖）。
            let changed = write_test_zip(
                "pinvou-reimport",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"另一个包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"demo","name":"另一个包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                ],
            );
            let err = import_plugin_package(&changed.to_string_lossy(), "combo.zip").unwrap_err();
            assert!(err.contains("内容不同"), "不同内容应报冲突，实际: {err}");
        });
    }

    /// 超过 16 MiB 私有状态读取上限的普通
    /// 条目是合法包载荷（包预算 200 MiB 收口），同包重导必须仍判「同一包」——
    /// 磁盘侧若按状态上限收口，首导成功而每次同包重导都被误判「内容不同」。
    /// 对照组：同 id、载荷内容不同（同样超限）仍必须判冲突。回归测试
    /// 走完整 导入 → 重导 路径，而非只测读原语。
    #[test]
    fn reimport_above_the_private_read_cap_is_still_same_content() {
        use std::io::Write;
        // RAII containment: a failing assertion unwinds past a
        // straight-line env restore and would leave PINVOU3_HOME pointed at
        // a deleted temp dir for every later test in the process.
        crate::platform::test_support::with_temp_home("pinvou-reimport-large", || {
            let dir = crate::platform::paths::pinvou3_home();

            // 高度可压缩的 17 MiB 载荷：超过 16 MiB 状态上限，远低于 200 MiB 包预算。
            let payload = vec![0xABu8; 17 * 1024 * 1024];
            let make_zip = |path: &std::path::Path, payload: &[u8]| {
                let f = std::fs::File::create(path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("my-skill/SKILL.md", opts).unwrap();
                zw.write_all(b"---\nname: greet\ndescription: large\n---\n# hi")
                    .unwrap();
                zw.start_file("my-skill/assets/data.bin", opts).unwrap();
                zw.write_all(payload).unwrap();
                zw.finish().unwrap();
            };
            make_zip(&dir.join("v1.zip"), &payload);

            let first =
                import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip").unwrap();
            assert_eq!(first.id, "greet");
            let landed = dir.join("bundles/greet/skills/greet/assets/data.bin");
            assert_eq!(
                std::fs::metadata(&landed).unwrap().len(),
                payload.len() as u64,
                "载荷必须按普通条目落盘（fixtures 的前提）"
            );

            // 回归点：同包重导放行（磁盘侧按包预算读取，而非 16 MiB 状态上限）。
            let second = import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip")
                .unwrap_or_else(|e| panic!("同包重导（>16 MiB 普通条目）应放行，实际报错: {e}"));
            assert_eq!(second.id, "greet");

            // 对照：同 id、载荷内容不同（同样超限）仍必须判冲突。
            let changed = vec![0x01u8; payload.len()];
            make_zip(&dir.join("v2.zip"), &changed);
            let err = import_plugin_package(&dir.join("v2.zip").to_string_lossy(), "v2.zip")
                .expect_err("内容不同的大包必须判冲突");
            assert!(
                err.contains("已存在且内容不同"),
                "不同内容应报冲突，实际: {err}"
            );
        });
    }

    /// 显式清单支线：plugin.json 可经 flatten extra 字段
    /// 保留 >16 MiB 的未知元数据——首导按 200 MiB 包预算放行，落盘的规范化
    /// 清单字节同样超 16 MiB 私有状态上限；重导的清单语义比对若按状态上限
    /// 读取磁盘侧，同一包会被误判「内容不同」。磁盘侧必须与首导同按包预算
    /// 收口。对照组：同 id、清单内容不同（同样超限）仍必须判冲突。回归测试
    /// 走完整 导入 → 重导 路径，而非只测读原语。
    #[test]
    fn reimport_above_the_private_read_cap_with_a_large_explicit_manifest_is_still_same_content() {
        use std::io::Write;
        // RAII containment: a failing assertion unwinds past a
        // straight-line env restore and would leave PINVOU3_HOME pointed at
        // a deleted temp dir for every later test in the process.
        crate::platform::test_support::with_temp_home("pinvou-reimport-large-manifest", || {
            let dir = crate::platform::paths::pinvou3_home();

            // 17 MiB 未知元数据字符串：经 flatten extra 原样保留，清单超过 16 MiB
            // 状态上限、远低于 200 MiB 包预算（首导的放行口径）。
            let retained = format!("\"x_retained_note\":\"{}\"", "A".repeat(17 * 1024 * 1024));
            let make_zip = |path: &std::path::Path, name: &str| {
                let manifest = format!(
                    r#"{{"manifest_version":1,"id":"demo","name":"{name}",{retained},"components":{{"mcp_servers":[{{"id":"demo","dir":"mcp"}}]}}}}"#
                );
                let f = std::fs::File::create(path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("plugin.json", opts).unwrap();
                zw.write_all(manifest.as_bytes()).unwrap();
                zw.start_file("mcp/manifest.json", opts).unwrap();
                zw.write_all(
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
                zw.finish().unwrap();
            };
            make_zip(&dir.join("v1.zip"), "演示组合包");

            let first =
                import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip").unwrap();
            assert_eq!(first.id, "demo");
            // fixture 前提：落盘规范化清单经 flatten extra 保留大元数据，本身超过
            // 16 MiB 状态上限（否则测试根本走不到回归支线）。
            let landed_manifest = dir.join("bundles/demo/plugin.json");
            assert!(
                std::fs::metadata(&landed_manifest).unwrap().len() > 16 * 1024 * 1024,
                "落盘 plugin.json 必须保留大元数据（fixtures 的前提）"
            );

            // 回归点：同包重导放行（清单语义比对的磁盘侧按包预算读取，而非
            // 16 MiB 状态上限）。
            let second = import_plugin_package(&dir.join("v1.zip").to_string_lossy(), "v1.zip")
                .unwrap_or_else(|e| panic!("同包重导（>16 MiB 显式清单）应放行，实际报错: {e}"));
            assert_eq!(second.id, "demo");

            // 对照：同 id、清单内容不同（同样超限）仍必须判冲突。
            make_zip(&dir.join("v2.zip"), "另一个包");
            let err = import_plugin_package(&dir.join("v2.zip").to_string_lossy(), "v2.zip")
                .expect_err("清单内容不同的大包必须判冲突");
            assert!(
                err.contains("已存在且内容不同"),
                "不同内容应报冲突，实际: {err}"
            );
        });
    }

    /// 同包比对的磁盘侧读取必须保持加固开口（修大小时不得回退）：
    /// 落在载荷路径上的植入 FIFO 应被非普通文件闸拒收（保守判冲突）而非阻塞
    /// `open()`——调用方持有 per-id 导入锁与跨进程 landing lease，阻塞式开口
    /// 会楔死同 id 导入道直至持有进程退出。本测试同时
    /// 是挂起金丝雀：若开口回退为阻塞式，测试将超时而非假绿。
    #[test]
    #[cfg(unix)]
    fn same_content_disk_read_refuses_a_planted_fifo() {
        use std::io::Write;
        // RAII containment: a failing assertion unwinds past a
        // straight-line env restore and would leave PINVOU3_HOME pointed at
        // a deleted temp dir for every later test in the process.
        crate::platform::test_support::with_temp_home("pinvou-reimport-fifo", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = dir.join("v1.zip");
            {
                let f = std::fs::File::create(&zip_path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("my-skill/SKILL.md", opts).unwrap();
                zw.write_all(b"---\nname: greet\ndescription: fifo\n---\n# hi")
                    .unwrap();
                zw.start_file("my-skill/assets/data.bin", opts).unwrap();
                zw.write_all(b"payload").unwrap();
                zw.finish().unwrap();
            }
            let first = import_plugin_package(&zip_path.to_string_lossy(), "v1.zip").unwrap();
            assert_eq!(first.id, "greet");

            // 把落盘载荷换成植入 FIFO 后同包重导：磁盘侧必须拒收非普通文件
            // （判冲突、快速失败），而不是阻塞在持锁的 open() 上。
            let landed = dir.join("bundles/greet/skills/greet/assets/data.bin");
            std::fs::remove_file(&landed).unwrap();
            crate::platform::paths::tests::plant_fifo(&landed);
            let err = import_plugin_package(&zip_path.to_string_lossy(), "v1.zip")
                .expect_err("植入 FIFO 必须经非普通文件闸判冲突");
            assert!(
                err.contains("已存在且内容不同"),
                "FIFO 拒收应走保守判冲突口径，实际: {err}"
            );
        });
    }

    /// 裸 MCP 包（无 plugin.json 无 SKILL.md）重导：按规范化落盘的
    /// `mcp/manifest.json` 语义比对，同包重导应放行（此前恒 false 必误判冲突）。
    #[test]
    fn reimport_bare_mcp_package_is_allowed() {
        crate::platform::test_support::with_temp_home("pinvou-reimport-mcp", || {
            let zip_path = write_test_zip(
                "pinvou-reimport-mcp",
                &[
                    (
                        "manifest.json",
                        r#"{"id":"wcalc","name":"计算器","description":"d","version":"1.0.0","icon":"","category":"dev","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    ("server.py", b"print('server')".to_vec()),
                ],
            );

            let first = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip").unwrap();
            assert_eq!(first.id, "wcalc");
            let second = import_plugin_package(&zip_path.to_string_lossy(), "mcp.zip")
                .unwrap_or_else(|e| panic!("裸 MCP 同包重导应放行，实际报错: {e}"));
            assert_eq!(second.id, "wcalc");
        });
    }

    /// 回退兼容：符合 MCP 标准的裸 MCP 包（无 plugin.json，manifest.json 在根目录，
    /// 声明 command/args）→ 规范化为 `mcp/` 布局并识别为纯 MCP 包。
    #[test]
    fn import_bare_mcp_package_lands_canonical_layout() {
        crate::platform::test_support::with_temp_home("pinvou-bare-mcp", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = write_test_zip(
                "pinvou-bare-mcp",
                &[
                    (
                        "manifest.json",
                        r#"{"id":"wcalc","name":"计算器","description":"d","version":"1.0.0","icon":"","category":"dev","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    ("server.py", b"print('server')".to_vec()),
                ],
            );

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
            let mcp_raw = std::fs::read_to_string(crate::platform::paths::mcp_config_path())
                .unwrap_or_default();
            assert!(
                mcp_raw.contains("\"wcalc\""),
                "mcp.json 应含 wcalc server 供给，实际: {mcp_raw}"
            );
        });
    }

    /// 非规范组件 dir 拒收（四轮评审 M-3）：detect 此前放行任意 dir，而落盘只认
    /// mcp/、skills/ 前缀 → 声明组件被静默丢弃（登记 installed=true 但磁盘无文件）。
    /// 现在 detect 强制 skill 组件 dir == `skills/<id>`、MCP 组件 dir == `mcp`。
    #[test]
    fn non_canonical_component_dir_is_rejected() {
        crate::platform::test_support::with_temp_home("pinvou-noncanon-dir", || {
            // 非规范 skill dir：声明 dir="my-skill"（zip 内确有 my-skill/SKILL.md，
            // 旧逻辑 detect 放行、落盘静默丢弃）。
            let skill_zip = write_test_zip(
                "pinvou-noncanon-dir",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"d","components":{"skills":[{"id":"demo","dir":"my-skill"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    ("my-skill/SKILL.md", b"---\nname: demo\n---\n# hi".to_vec()),
                ],
            );
            assert_import_refused(&skill_zip, "skill.zip", "demo", "非规范");

            // 非规范 mcp dir：声明 dir="server"。
            let mcp_zip = write_test_zip(
                "pinvou-noncanon-dir",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"d","components":{"mcp_servers":[{"id":"demo","dir":"server"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "server/manifest.json",
                        r#"{"id":"demo","name":"d","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                ],
            );
            assert_import_refused(&mcp_zip, "mcp.zip", "demo", "非规范");
        });
    }

    /// zip-slip regression: an entry whose name escapes the package root via
    /// `../` must be rejected wholesale by the pass1 `enclosed_name()` safety
    /// check — before any detection, extraction, or disk writes. (The
    /// absolute-path entry documents, and this test relies on, the sanitizer's
    /// registered normalize-and-accept semantics: at depth 0 the root prefix
    /// is stripped so `/abs/evil.txt` would land inside the package root as
    /// `abs/evil.txt` — it is NOT itself a rejection. The traversal entry is
    /// written first, so pass1 aborts on it before the absolute entry is
    /// even reached; the executable per-entry verdicts live in
    /// `forkguard_zip_gate_keeps_zip_sanitizer_verdicts_over_engine_reject_predicate`.)
    /// The guard existed since the unified pipeline landed; this test pins it
    /// so refactors cannot silently drop the enforcement.
    #[test]
    fn import_rejects_zip_slip_traversal_entries() {
        use std::io::Write;
        crate::platform::test_support::with_temp_home("pinvou-zip-slip", || {
            // Entry names are written verbatim by the zip writer, so the archive
            // carries the traversal / absolute paths exactly as a malicious
            // package would.
            let zip_path = crate::platform::paths::pinvou3_home().join("zipslip.zip");
            {
                let f = std::fs::File::create(&zip_path).unwrap();
                let mut zw = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                zw.start_file("../outside.txt", opts).unwrap();
                zw.write_all(b"traversal").unwrap();
                zw.start_file("/abs/evil.txt", opts).unwrap();
                zw.write_all(b"absolute").unwrap();
                zw.finish().unwrap();
            }

            let err =
                import_plugin_package(&zip_path.to_string_lossy(), "zipslip.zip").unwrap_err();
            assert!(
                err.contains("不安全路径"),
                "zip-slip traversal entry must be rejected, got: {err}"
            );
            assert!(
                !crate::platform::paths::bundles_root().exists(),
                "rejected package must not land any bundle directory"
            );
        });
    }

    /// 并发互斥（四轮评审 M-4）：两线程同时导入同 id 同内容包，按 id 的进程内
    /// 互斥锁必须保证双方成功且落盘完整、无 staged/.old 残留（修复前线程 B 的
    /// `remove_dir_all(staged)` 可删线程 A 的在建目录，冲突检查与 rename 间为
    /// TOCTOU）。
    #[test]
    fn concurrent_same_id_import_is_serialized() {
        crate::platform::test_support::with_temp_home("pinvou-concurrent-import", || {
            let dir = crate::platform::paths::pinvou3_home();

            let zip_path = write_test_zip(
                "pinvou-concurrent-import",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}],"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "skills/demo/SKILL.md",
                        b"---\nname: demo\n---\n# hi".to_vec(),
                    ),
                ],
            );

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
        });
    }

    /// Round-51 review M2: the cross-process market write lock must actually
    /// exclude a second open file description — that is the whole point (the
    /// CLI's plugins/skills lanes are a second process against the same
    /// staging paths). flock conflicts are per open description, so a second
    /// in-process handle hitting `WouldBlock` while the guard is held (and
    /// succeeding after the drop) proves the exclusion with no threads and
    /// no sleeps.
    #[test]
    fn marketplace_write_lock_excludes_a_second_handle() {
        crate::platform::test_support::with_temp_home("pinvou-market-write-lock", || {
            let guard = MarketplaceWriteLock::acquire().expect("acquire market write lock");
            let lock_path = crate::platform::paths::bundles_root().join(".imports.lock");
            assert!(
                lock_path.exists(),
                "the lock file must exist under bundles root"
            );
            let second = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .unwrap();
            let conflict = second.try_lock();
            assert!(
                matches!(conflict, Err(std::fs::TryLockError::WouldBlock)),
                "a second handle must be excluded while the guard is held: {conflict:?}"
            );
            drop(guard);
            second
                .try_lock()
                .expect("a dropped guard must release the lock");
        });
    }

    /// Round-52 review M1：同线程嵌套获取按持有深度 join，不再发起新的
    /// flock——`import_plugin_package → install_upload → release_package` 这
    /// 条真实调用链要求嵌套获取必须成功（flock 按 fd 排他，第二个 fd 会对
    /// 自己阻塞）。join 释放后外层仍持锁；全部释放后锁空闲。排他本身由上
    /// 面的裸 fd 用例证明，这里用同一探针复核 join 未削弱排他。
    #[test]
    fn marketplace_write_lock_joins_reentrantly_on_the_holding_thread() {
        crate::platform::test_support::with_temp_home("pinvou-market-lock-reenter", || {
            let lock_path = crate::platform::paths::bundles_root().join(".imports.lock");
            let outer = MarketplaceWriteLock::acquire().expect("acquire market write lock");
            let inner = MarketplaceWriteLock::acquire()
                .expect("a nested acquire on the holding thread must join, not block");
            let probe = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .unwrap();
            assert!(
                matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "the joined hold must stay exclusive to other fds"
            );
            drop(inner);
            assert!(
                matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "dropping the inner join must keep the outer hold"
            );
            drop(outer);
            probe
                .try_lock()
                .expect("dropping every guard must release the lock");
        });
    }

    /// Round-52 review M1：跨线程仍是真实互斥——第二个线程先以裸 fd 探测
    /// （确定性 WouldBlock，不靠时序），主线程释放后其阻塞获取必然完成，
    /// 且它持有期间对第三个探针保持排他。这条用例守住段锁 + flock 的生命
    /// 周期：section 随真实守卫跨线程移交，fd 随之释放与重取。
    #[test]
    fn marketplace_write_lock_excludes_a_second_thread_until_release() {
        use std::sync::Arc;
        crate::platform::test_support::with_temp_home("pinvou-market-lock-threads", || {
            let lock_path = Arc::new(crate::platform::paths::bundles_root().join(".imports.lock"));
            let guard = MarketplaceWriteLock::acquire().expect("acquire market write lock");
            let path_for_thread = Arc::clone(&lock_path);
            // 会合：先等对方线程完成「主线程仍持锁」的裸 fd 探测，再释放——
            // 探针的 WouldBlock 断言因此是确定性的，不靠调度时序。
            let (probe_done, probe_seen) = std::sync::mpsc::channel();
            // 守卫含 MutexGuard（!Send），线程内完成全部探测后释放再返回。
            let holder = std::thread::spawn(move || {
                let probe = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&*path_for_thread)
                    .unwrap();
                let excluded = matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock));
                drop(probe);
                probe_done
                    .send(excluded)
                    .expect("the main thread waits for this send before releasing");
                // 主线程仍持锁（它在等 probe_seen），这里阻塞；主线程 drop 后
                // 必然完成。
                let acquired = MarketplaceWriteLock::acquire()
                    .expect("the blocking acquire must finish once the first thread releases");
                let reprobe = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&*path_for_thread)
                    .unwrap();
                assert!(
                    matches!(reprobe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                    "the second thread's own hold must exclude a fresh fd"
                );
                drop(acquired);
                let free = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&*path_for_thread)
                    .unwrap();
                free.try_lock()
                    .expect("the second thread's release must free the lock");
            });
            assert!(
                probe_seen
                    .recv()
                    .expect("the probe thread must report before release"),
                "the first thread's hold must exclude the second thread's raw fd"
            );
            drop(guard);
            holder.join().expect("the second thread must not deadlock");
        });
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
        crate::platform::test_support::with_temp_home("pinvou-mcp-id-mismatch", || {
            let zip_path = write_test_zip(
                "pinvou-mcp-id-mismatch",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    // manifest 的 id 与组件声明 id 不一致
                    (
                        "mcp/manifest.json",
                        r#"{"id":"other","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                ],
            );

            assert_import_refused(&zip_path, "combo.zip", "demo", "不一致");
        });
    }

    /// 全内容比对（五轮评审必修 1）：重导同 id 包，plugin.json 语义不变但
    /// server.py 换代码 / SKILL.md 正文全换（frontmatter name 保持以过 §8）——
    /// 此前只比对 plugin.json，冲突检查放行导致已装工具的执行代码被静默替换；
    /// 现在任一可落盘条目字节不同都必须判「内容不同」冲突。
    #[test]
    fn reimport_with_changed_component_content_is_rejected() {
        crate::platform::test_support::with_temp_home("pinvou-reimport-content", || {
            let write_zip = |server_py: &[u8], skill_md: &[u8]| {
                write_test_zip(
                    "pinvou-reimport-content",
                    &[
                        (
                            "plugin.json",
                            r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}],"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                                .as_bytes()
                                .to_vec(),
                        ),
                        (
                            "mcp/manifest.json",
                            r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["demo"]}"#
                                .as_bytes()
                                .to_vec(),
                        ),
                        ("mcp/server.py", server_py.to_vec()),
                        ("skills/demo/SKILL.md", skill_md.to_vec()),
                    ],
                )
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
        });
    }

    /// Owner-claim divergence must also be refused from the UNDER-LOCK
    /// re-check, not only the pre-gate check: a claimant install (preset
    /// skill install, CLI companion, ima connect) completing while this
    /// import waits on the landing lease / import lock / market lock folds
    /// the pending id onto the claimant — the pack would land keyed by its
    /// own id, ungoverned in initialized DenyAll scopes. Round-54 audit A1:
    /// commit 1656d739c deleted the re-check call while this site's comment
    /// kept describing it. The pre-land gate hook is the hermetic seam that
    /// completes a claimant install inside exactly that window (after the
    /// pre-gate check, before the landing), so this pin is deterministic.
    #[test]
    fn claimant_completing_inside_the_import_window_is_refused_at_recheck() {
        crate::platform::test_support::with_temp_home("pinvou-recheck-fold", || {
            let claimant_zip = write_test_zip(
                "pinvou-recheck-fold",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"local-claimant","name":"claimant","components":{"mcp_servers":[{"id":"local-claimant","dir":"mcp"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"local-claimant","name":"claimant","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["local-victim"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    ("mcp/server.py", b"print('c')".to_vec()),
                ],
            );
            let victim_zip = write_test_zip(
                "pinvou-recheck-fold",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"local-victim","name":"victim","components":{"mcp_servers":[{"id":"local-victim","dir":"mcp"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "mcp/manifest.json",
                        r#"{"id":"local-victim","name":"victim","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"]}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    ("mcp/server.py", b"print('v')".to_vec()),
                ],
            );

            // The victim's pre-land hook installs the claimant for real
            // (full pipeline): by the time the under-lock re-check runs, the
            // claimant's companion vocabulary folds "local-victim" onto
            // "local-claimant". The pre-gate check ran BEFORE the hook, so
            // only the re-check can produce this refusal.
            let pre_land = |id: &str, _skills: &[String]| {
                if id == "local-victim" {
                    import_plugin_package(&claimant_zip.to_string_lossy(), "claimant.zip")
                        .unwrap_or_else(|e| {
                            panic!("claimant install inside the window failed: {e}")
                        });
                }
                Ok(())
            };
            let error = import_plugin_package_gated(
                &victim_zip.to_string_lossy(),
                "victim.zip",
                &pre_land,
            )
            .expect_err(
                "a claimant completing inside the import window must fold the id and refuse the import",
            );
            assert!(
                error.contains("配套技能占用"),
                "expected the fold-divergence refusal, got: {error}"
            );
            // The refusal aborts with the home untouched: no victim pack.
            assert!(
                !crate::platform::paths::bundles_root()
                    .join("local-victim")
                    .exists(),
                "the refused import must not leave the victim pack on disk"
            );
        });
    }

    /// Round-1 review m1 / round-2 M3 repair: the re-check's POSITION is
    /// load-bearing — it must sit AFTER the market-lock acquisition,
    /// because the market lock is what a CROSS-PROCESS claimant (a CLI
    /// `plugins skills install`) contends on; moved before it, the
    /// behavioral pin above stays green (its claimant is in-process and
    /// stays excluded by the import lock the moved call still sits behind)
    /// while the second-process window reopens. The round-1 form asserted
    /// this with a whole-file `rfind`, which self-matched this test's own
    /// needle literal and could never fail (round-2 mutation-verified:
    /// green under both a move of the call and a full deletion). The
    /// repaired pin scopes the search to the production slice and the
    /// pipeline function: the re-check must keep exactly two call sites
    /// there (the pre-gate twin and the under-lock call), the market lock
    /// is acquired once, and the LAST re-check must follow it.
    #[test]
    fn the_under_lock_recheck_sits_after_the_market_lock_acquisition() {
        let code: String = include_str!("plugin_import.rs")
            .lines()
            // Stop at the test module: needles must match production code
            // only — the round-1 defect was the test's own literal
            // satisfying the rfind.
            .take_while(|line| !line.trim_start().starts_with("mod tests"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let pipeline = &code[code
            .find("fn import_plugin_package_gated")
            .expect("the gated import pipeline must exist")..];
        let recheck = "ensure_package_id_self_mapping(&id)?;";
        assert_eq!(
            pipeline.matches(recheck).count(),
            2,
            "the pipeline must keep both re-checks (pre-gate twin + under-lock)"
        );
        let lock_at = pipeline
            .find("MarketplaceWriteLock::acquire_map_err()?")
            .expect("the import pipeline must acquire the market lock");
        let recheck_at = pipeline
            .rfind(recheck)
            .expect("the under-lock re-check call must exist");
        assert!(
            lock_at < recheck_at,
            "the TOCTOU re-check must sit after the market-lock acquisition \
             (that position is what excludes a cross-process claimant)"
        );
    }

    /// 未声明技能子树拒收（五轮评审必修 2）：detect 只登记声明/识别的技能，
    /// zip 额外夹带的 `skills/<other>/` 会静默落盘成不可见孤儿技能（无卡片、
    /// 无开关、卸载残留）。声明包与裸包两种形态都必须响亮拒收。
    #[test]
    fn undeclared_skills_subtree_is_rejected() {
        crate::platform::test_support::with_temp_home("pinvou-undeclared-skill", || {
            // 形态 1：plugin.json 只声明 skills:[demo]，zip 夹带 skills/other/。
            let declared_zip = write_test_zip(
                "pinvou-undeclared-skill",
                &[
                    (
                        "plugin.json",
                        r#"{"manifest_version":1,"id":"demo","name":"d","components":{"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                            .as_bytes()
                            .to_vec(),
                    ),
                    (
                        "skills/demo/SKILL.md",
                        b"---\nname: demo\n---\n# hi".to_vec(),
                    ),
                    (
                        "skills/other/SKILL.md",
                        b"---\nname: other\n---\n# stowaway".to_vec(),
                    ),
                ],
            );
            assert_import_refused(
                &declared_zip,
                "declared.zip",
                "demo",
                "未在包声明/识别的技能列表中",
            );

            // 形态 2：裸技能包（无 plugin.json），夹带无 SKILL.md 的 skills/other/ 子树。
            let bare_zip = write_test_zip(
                "pinvou-undeclared-skill",
                &[
                    ("my-skill/SKILL.md", b"---\nname: greet\n---\n# hi".to_vec()),
                    ("skills/other/notes.txt", b"stowaway".to_vec()),
                ],
            );
            assert_import_refused(
                &bare_zip,
                "bare.zip",
                "greet",
                "未在包声明/识别的技能列表中",
            );
        });
    }

    /// Upload-side collision guard (review MINOR): a skill component named
    /// like a preset skill is rejected up front — the preset pipeline owns the
    /// name and its sweep/rehome lifecycle would destroy the uploaded copy.
    #[test]
    fn import_rejects_preset_skill_name_collision() {
        crate::platform::test_support::with_temp_home("pinvou-preset-collision", || {
            let zip_path = write_test_zip(
                "pinvou-preset-collision",
                &[(
                    "skills/visualizer/SKILL.md",
                    b"---\nname: visualizer\n---\n# hi".to_vec(),
                )],
            );
            assert_import_refused(&zip_path, "skill.zip", "visualizer", "预置技能冲突");
        });
    }

    /// MCP id collisions against the embedded catalog, folded as two constant
    /// cases — both must be refused with an explicit "市场预置 MCP" conflict
    /// error and leave no package data.
    /// Case 1 (component-id impersonation): the package id (`my-tool`) is
    /// collision-free, but the inner MCP component id (`session-reader`) is a
    /// builtin's id — add_to_mcp_json keys servers by manifest.id, so
    /// importing would overwrite the builtin's engine registration.
    /// Case 2 (package-id preset collision): the id (`gongwen`) maps to a
    /// catalog package but not a builtin, so the builtin probe alone does not
    /// refuse it — the import must still reject an id that would overwrite an
    /// installed preset (`gongwen` / `weather`).
    #[test]
    fn import_rejects_builtin_component_id_impersonation() {
        crate::platform::test_support::with_temp_home("pinvou-component-impersonation", || {
            for (id, name, component, server_py, needle) in [
                (
                    "my-tool",
                    "innocent",
                    "session-reader",
                    &b"print('impersonation')\n"[..],
                    "市场预置 MCP 冲突",
                ),
                (
                    "gongwen",
                    "collision",
                    "gongwen",
                    &b"print('collision')\n"[..],
                    "市场预置 MCP",
                ),
            ] {
                let zip_path = write_test_zip(
                    "pinvou-component-impersonation",
                    &[
                        (
                            "plugin.json",
                            format!(
                                r#"{{"manifest_version":1,"id":"{id}","name":"{name}","components":{{"mcp_servers":[{{"id":"{component}","dir":"mcp"}}]}}}}"#
                            )
                            .into_bytes(),
                        ),
                        (
                            "mcp/manifest.json",
                            format!(
                                r#"{{"id":"{component}","name":"{name}","description":"fixture","version":"1","icon":"x","category":"fixture","mcp_tools":[],"command":"python","args":["server.py"]}}"#
                            )
                            .into_bytes(),
                        ),
                        ("mcp/server.py", server_py.to_vec()),
                    ],
                );
                assert_import_refused(&zip_path, &format!("{id}.zip"), id, needle);
            }
        });
    }

    /// Upload-side collision guard (review MINOR): a skill component whose
    /// name is already on disk under another package is rejected up front —
    /// this pipeline never sweeps, and the other package's copy must survive.
    #[test]
    fn import_rejects_skill_name_owned_by_another_package() {
        crate::platform::test_support::with_temp_home("pinvou-foreign-skill", || {
            let dir = crate::platform::paths::pinvou3_home();

            // Existing plugin package owning skill "foo".
            let foreign = dir.join("bundles/other-pkg/skills/foo");
            std::fs::create_dir_all(&foreign).unwrap();
            std::fs::write(foreign.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
            std::fs::write(dir.join("bundles/other-pkg/plugin.json"), "{}").unwrap();

            let zip_path = write_test_zip(
                "pinvou-foreign-skill",
                &[("skills/foo/SKILL.md", b"---\nname: foo\n---\n# hi".to_vec())],
            );
            // The owner-claim vocabulary check fires before the skill-name
            // collision check for this fixture — the id "foo" folds onto the
            // physically present other-pkg's vocabulary. Either rejection is
            // the pin: the import is refused naming other-pkg.
            assert_import_refused(&zip_path, "skill.zip", "foo", "other-pkg");
            assert!(foreign.join("SKILL.md").is_file(), "外来包副本不得被动");
        });
    }
}
