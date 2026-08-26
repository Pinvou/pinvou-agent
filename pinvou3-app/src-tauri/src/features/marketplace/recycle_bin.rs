//! 插件中心回收站 —— Upload 来源包卸载的软删除层（marketplace-unification §4 修订）。
//!
//! 背景：上传包是用户唯一副本（不可重释放）。此前两条卸载路径行为割裂：
//! MCP 卸载对 Upload 原位保留（卡片以"未安装"重现，隐性残留）、技能卸载无条件
//! 物理删除（数据直接丢失）。回收站统一为「搬走不删」：卸载把整包
//! `bundles/<id>/`（含 mcp/ 与 skills/）搬入 `marketplace/recycle-bin/<id>/`，
//! 搬离 `bundles_root()` 后商店列表自然不再出现；恢复 = 搬回 + 重走安装管线；
//! 彻底删除（purge）由用户手动触发（首版不做自动过期清理）。
//! Preset/Builtin 可重释放，不进回收站，卸载仍物理删除。
//!
//! 存储纪律对齐 store.rs：
//! - 清单 `marketplace/recycle-bin.json` 原子写（底座 `write_atomic`）+ 进程内
//!   FILE_LOCK 串行化读-改-写；
//! - 不用 `#[serde(deny_unknown_fields)]`：未知字段经 `extra` flatten 原样
//!   roundtrip（前向兼容）；
//! - 损坏 JSON fail loud：返回 Err，绝不静默重建/回写；
//! - purge fail-closed：只删清单中存在的条目，绝不按外部传入路径删任意目录。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::store::{BundleRecord, BundleStore};
use crate::platform::paths;

/// 清单当前 schema 版本。后续 schema 演进时递增并在读路径做迁移。
const SCHEMA_VERSION: u32 = 1;

/// 回收站条目 kind：纯 MCP 包。
pub const KIND_MCP: &str = "mcp";
/// 回收站条目 kind：纯技能包。
pub const KIND_SKILL: &str = "skill";
/// 回收站条目 kind：组合包（mcp/ + skills/）。
pub const KIND_BUNDLE: &str = "bundle";

/// recycle-bin.json 读-改-写的进程内串行化（与 BUNDLES_FILE_LOCK 同一范式）。
static RECYCLE_BIN_FILE_LOCK: Mutex<()> = Mutex::new(());

fn file_lock() -> MutexGuard<'static, ()> {
    RECYCLE_BIN_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------

/// 清单条目：`record` 是回收时 bundles.json 原记录的快照（恢复重建登记用：
/// source=Upload、原 installed_at、credential_keys 等一并保留）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecycledEntry {
    pub id: String,
    /// Upload 来源保留的原 zip 展示名
    pub display_name: String,
    /// "mcp" | "skill" | "bundle"
    pub kind: String,
    /// 回收时间，RFC3339/ISO8601 UTC（对齐 BundleRecord.installed_at 的 chrono 惯例）
    pub recycled_at: String,
    pub record: BundleRecord,
    /// 前向兼容：未知字段原样 roundtrip（不用 deny_unknown_fields）。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// recycle-bin.json 顶层结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecycleBinFile {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<RecycledEntry>,
    /// 前向兼容：顶层未知字段原样 roundtrip。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn current_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for RecycleBinFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

/// 前端消费的回收站条目（`list_recycled_plugins` 命令契约）。
/// `package_missing` = 清单在、包目录已被外部删掉（只能 purge 清条目，不能恢复）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecycledPluginInfo {
    pub id: String,
    pub display_name: String,
    /// "mcp" | "skill" | "bundle"
    pub kind: String,
    pub recycled_at: String,
    #[serde(default)]
    pub package_missing: bool,
}

/// 恢复结果（`restore_recycled_plugin` 命令契约）：true = 包含 MCP 组件且
/// manifest 声明了 secrets —— 凭据卸载时已删，前端应提示用户重新填写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRecycledResult {
    pub credentials_required: bool,
}

// ---------------------------------------------------------------------------
// RecycleBin
// ---------------------------------------------------------------------------

pub struct RecycleBin {
    /// 回收站根目录（~/.pinvou3/marketplace/recycle-bin/）
    root: PathBuf,
    /// 清单文件（~/.pinvou3/marketplace/recycle-bin.json）
    file: PathBuf,
    /// 包目录根（恢复时搬回的目标）
    bundles_root: PathBuf,
}

impl Default for RecycleBin {
    fn default() -> Self {
        Self::new()
    }
}

impl RecycleBin {
    pub fn new() -> Self {
        let marketplace = paths::pinvou3_home().join("marketplace");
        Self {
            root: marketplace.join("recycle-bin"),
            file: marketplace.join("recycle-bin.json"),
            bundles_root: paths::bundles_root(),
        }
    }

    /// 测试用：三套路径都指到同一临时目录下，不碰真实 ~/.pinvou3。
    /// （与 `BundleStore::with_file` / `SkillMarketplaceManager::with_roots` 同范式）
    #[cfg(test)]
    pub(crate) fn with_roots(dir: PathBuf) -> Self {
        Self {
            root: dir.join("recycle-bin"),
            file: dir.join("recycle-bin.json"),
            bundles_root: dir.join("bundles"),
        }
    }

    fn load(&self) -> Result<RecycleBinFile, String> {
        let _guard = file_lock();
        load_locked(&self.file)
    }

    fn save(&self, file: &RecycleBinFile) -> Result<(), String> {
        let _guard = file_lock();
        save_locked(&self.file, file)
    }

    /// 回收：preflight 检查 → rename 搬移 `bundles/<id>/` → `recycle-bin/<id>/`
    /// → 失败回滚（retirement.rs archive 同范式）→ 写清单。
    /// `record_snapshot` 为回收前的 bundles.json 原记录（恢复重建登记用）。
    pub fn recycle_package(
        &self,
        pkg_id: &str,
        kind: &str,
        display_name: &str,
        record_snapshot: BundleRecord,
    ) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let src = self.bundles_root.join(pkg_id);
        let dst = self.root.join(pkg_id);
        // preflight：源必须存在；目标已存在说明有同 id 残留条目，拒绝覆盖（可能是
        // 不同包同 id，静默覆盖即数据丢失 —— 与 retirement preflight 同一纪律）。
        if !src.is_dir() {
            return Err(format!(
                "包目录 {} 不存在，无法移入回收站",
                src.display()
            ));
        }
        if dst.exists() {
            return Err(format!(
                "回收站目标 {} 已存在，拒绝覆盖",
                dst.display()
            ));
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|e| format!("创建回收站目录 {} 失败: {e}", self.root.display()))?;
        // rename 走 plugin_import 的 Windows 瞬时占用重试口径（杀软/索引器短暂
        // 持有新建目录句柄会报 os error 5，实测命中）。
        if let Err(e) = super::plugin_import::rename_dir_with_retry(&src, &dst) {
            // rename 失败通常什么都没动；兜底尝试回滚（部分平台跨设备 rename 语义差异）。
            let _ = super::plugin_import::rename_dir_with_retry(&dst, &src);
            return Err(format!(
                "搬移 {} → {} 失败: {e}",
                src.display(),
                dst.display()
            ));
        }
        // 搬移成功后写清单；清单写失败则把目录搬回原位（不留无清单的孤儿目录）。
        let mut file = self.load()?;
        file.entries.retain(|e| e.id != pkg_id);
        file.entries.push(RecycledEntry {
            id: pkg_id.to_string(),
            display_name: display_name.to_string(),
            kind: kind.to_string(),
            recycled_at: now_iso8601(),
            record: record_snapshot,
            extra: serde_json::Map::new(),
        });
        if let Err(e) = self.save(&file) {
            let _ = super::plugin_import::rename_dir_with_retry(&dst, &src);
            return Err(format!("写入回收站清单失败（已回滚目录）: {e}"));
        }
        log::info!("[recycle-bin] 已回收包 {pkg_id}（kind={kind}）→ {}", dst.display());
        Ok(())
    }

    /// 回收站列表：读清单 + 校验包目录存在（缺失标记 `package_missing`，
    /// 前端据此禁用"恢复"）。清单损坏 fail loud（返回 Err）。
    pub fn list(&self) -> Result<Vec<RecycledPluginInfo>, String> {
        let file = self.load()?;
        Ok(file
            .entries
            .into_iter()
            .map(|e| RecycledPluginInfo {
                package_missing: !self.root.join(&e.id).is_dir(),
                id: e.id,
                display_name: e.display_name,
                kind: e.kind,
                recycled_at: e.recycled_at,
            })
            .collect())
    }

    /// companion 技能随包回收的判定：回收站里存在某个包，其 `skills/<skill_name>/`
    /// 目录在（Upload 组合包卸载 MCP 时整包回收，companion 技能卸载据此跳过
    /// 物理删除、只清登记）。
    pub fn contains_recycled_skill(&self, skill_name: &str) -> bool {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return false;
        };
        rd.flatten()
            .any(|e| e.path().join("skills").join(skill_name).is_dir())
    }

    /// 取回：fail-closed（不在清单 → Err）→ preflight → 搬回 `bundles/<id>/`
    /// → 失败回滚 → 从清单移除 → 返回记录快照（供恢复管线重建登记）。
    pub fn take_back(&self, pkg_id: &str) -> Result<BundleRecord, String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let mut file = self.load()?;
        let Some(index) = file.entries.iter().position(|e| e.id == pkg_id) else {
            return Err(format!("包 '{pkg_id}' 不在回收站"));
        };
        let src = self.root.join(pkg_id);
        let dst = self.bundles_root.join(pkg_id);
        if !src.is_dir() {
            return Err(format!(
                "回收站包目录 {} 缺失，无法恢复（可选择彻底删除清理条目）",
                src.display()
            ));
        }
        if dst.exists() {
            return Err(format!("恢复目标 {} 已存在，拒绝覆盖", dst.display()));
        }
        std::fs::create_dir_all(&self.bundles_root).map_err(|e| {
            format!(
                "创建包目录根 {} 失败: {e}",
                self.bundles_root.display()
            )
        })?;
        if let Err(e) = super::plugin_import::rename_dir_with_retry(&src, &dst) {
            let _ = super::plugin_import::rename_dir_with_retry(&dst, &src);
            return Err(format!(
                "搬回 {} → {} 失败: {e}",
                src.display(),
                dst.display()
            ));
        }
        let entry = file.entries.remove(index);
        // 目录已搬回，清单移除失败不搬回目录（恢复主操作已成功），fail loud 到错误。
        self.save(&file)?;
        log::info!("[recycle-bin] 已取回包 {pkg_id} → {}", dst.display());
        Ok(entry.record)
    }

    /// 彻底删除：fail-closed，仅删清单中存在的条目（绝不按外部传入路径删任意
    /// 目录），物理删 `recycle-bin/<id>/` + 清单条目。包目录已被外部删除时
    /// （package_missing）同样允许 purge 清条目。
    pub fn purge(&self, pkg_id: &str) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let mut file = self.load()?;
        let before = file.entries.len();
        file.entries.retain(|e| e.id != pkg_id);
        if file.entries.len() == before {
            return Err(format!("包 '{pkg_id}' 不在回收站，拒绝删除"));
        }
        let dir = self.root.join(pkg_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("删除回收站目录 {} 失败: {e}", dir.display()))?;
        }
        self.save(&file)?;
        log::info!("[recycle-bin] 已彻底删除包 {pkg_id}");
        Ok(())
    }

    /// 导出：fail-closed（不在清单 → Err，与 purge 同口径；package_missing → Err），
    /// 把回收站包目录 `recycle-bin/<id>/` 的内容打成 zip（plugin.json、mcp/、
    /// skills/ 等平铺在 zip 根，对齐 plugin-package-spec 的包结构，可经统一导入
    /// 管线 `plugin_import::import_plugin_package` 重新导入）。只打包插件包本体，
    /// 不含回收站清单等元数据；Python 运行缓存（__pycache__/、*.pyc）不打包
    /// （与导入管线的磁盘比对豁免口径一致）。
    pub fn export_package(&self, pkg_id: &str, dest_zip: &Path) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let file = self.load()?;
        if !file.entries.iter().any(|e| e.id == pkg_id) {
            return Err(format!("包 '{pkg_id}' 不在回收站，拒绝导出"));
        }
        let src = self.root.join(pkg_id);
        if !src.is_dir() {
            return Err(format!(
                "回收站包目录 {} 缺失，无法导出（package_missing）",
                src.display()
            ));
        }
        let mut files = Vec::new();
        collect_export_files(&src, &src, &mut files)
            .map_err(|e| format!("遍历 {} 失败: {e}", src.display()))?;
        if files.is_empty() {
            return Err(format!("包目录 {} 为空，无法导出", src.display()));
        }
        files.sort();
        // 直接写用户选定的目标（保存对话框已确认覆盖）；中途失败清理半写文件，
        // 不留损坏 zip。
        let result = (|| -> Result<(), String> {
            let out = std::fs::File::create(dest_zip)
                .map_err(|e| format!("创建 {} 失败: {e}", dest_zip.display()))?;
            let mut zw = zip::ZipWriter::new(out);
            let opts = zip::write::SimpleFileOptions::default();
            for (rel, path) in &files {
                // 条目名即包内相对路径（盘上真实遍历产出，恒为 root 子孙，无穿越
                // 风险）；统一 '/' 分隔，与导入管线的路径口径一致。
                zw.start_file(rel, opts)
                    .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
                let mut reader = std::fs::File::open(path)
                    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
                std::io::copy(&mut reader, &mut zw)
                    .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
            }
            zw.finish().map_err(|e| format!("完成 zip 写入: {e}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(dest_zip);
        }
        result?;
        log::info!(
            "[recycle-bin] 已导出包 {pkg_id}（{} 个条目）→ {}",
            files.len(),
            dest_zip.display()
        );
        Ok(())
    }
}

/// 递归收集导出条目（相对路径用 '/' 分隔）：跳过 Python 运行缓存
/// （`__pycache__/` 子树与 `*.pyc`，与 plugin_import 的磁盘比对豁免口径一致）。
fn collect_export_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_export_files(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.split('/').any(|c| c == "__pycache__") || rel.to_ascii_lowercase().ends_with(".pyc")
        {
            continue;
        }
        out.push((rel, path));
    }
    Ok(())
}

/// 按包目录内容推导回收站 kind：mcp/ + skills/ → bundle；仅 mcp/ → mcp；否则 skill。
pub(crate) fn package_kind(pkg_dir: &Path) -> &'static str {
    let has_mcp = pkg_dir.join("mcp").is_dir();
    let has_skills = pkg_dir.join("skills").is_dir();
    match (has_mcp, has_skills) {
        (true, true) => KIND_BUNDLE,
        (true, false) => KIND_MCP,
        _ => KIND_SKILL,
    }
}

// ---------------------------------------------------------------------------
// 恢复管线
// ---------------------------------------------------------------------------

/// 恢复 = 恢复为已安装状态：
/// 1. `take_back` 搬回 `bundles/<id>/`（fail-closed：不在清单拒绝）；
/// 2. 重建 bundles.json 登记（快照恢复：source=Upload、保留原 installed_at、installed）；
/// 3. MCP 组件复用 `install_upload` 供给管线（写 mcp.json/installed.json）。
///    manifest 声明了 secrets 的包跳过供给：凭据卸载时已删，install 缺凭据必失败
///    （`resolve_secret_placeholder` 响亮报错）——登记已恢复 installed=true，
///    `credentials_required=true` 由前端引导重填，重填走 install 幂等补齐
///    mcp.json/installed.json；
/// 4. 技能组件随包目录搬回 + 登记恢复即回到安装态（技能无独立供给管线）；
/// 5. scope 禁用集兜底清理（卸载时命令层已清，恢复后不应残留禁用）。
pub fn restore_plugin(pkg_id: &str) -> Result<RestoreRecycledResult, String> {
    let bin = RecycleBin::new();
    let record = bin.take_back(pkg_id)?;
    let mgr = super::MarketplaceManager::new();
    let pkg_dir = paths::bundles_root().join(pkg_id);
    let has_mcp = pkg_dir.join("mcp").join("manifest.json").is_file();

    // 重建登记：快照即原记录（installed_at/source/extra 原样保留）。upsert_preserving
    // 在记录已被卸载移除的常态下等价 upsert；并发重装写了新记录时保留其首装元数据。
    let mut restored = record.clone();
    restored.installed = true;
    BundleStore::new().upsert_preserving(restored)?;

    let mut credentials_required = false;
    if has_mcp {
        let declares_secrets = mgr
            .load_manifest(pkg_id)
            .map(|m| !super::secrets::manifest_secret_targets(&m).is_empty())
            .unwrap_or(false);
        if declares_secrets {
            credentials_required = true;
            log::info!(
                "[recycle-bin] 恢复 {pkg_id}：manifest 声明了 secrets，凭据已在卸载时删除，跳过 MCP 供给，待用户重填凭据"
            );
        } else {
            mgr.install_upload(pkg_id, record.source.clone())?;
        }
    }

    // scope 禁用集：包 id + 包内技能目录名一并兜底清理。
    super::scope::remove_bundle_from_disabled_scopes(pkg_id);
    if let Ok(rd) = std::fs::read_dir(pkg_dir.join("skills")) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                super::scope::remove_bundle_from_disabled_scopes(
                    &entry.file_name().to_string_lossy(),
                );
            }
        }
    }
    Ok(RestoreRecycledResult {
        credentials_required,
    })
}

// ---------------------------------------------------------------------------
// 已持锁实现（取锁包装之上的内层；调用前必须已持有 RECYCLE_BIN_FILE_LOCK）
// ---------------------------------------------------------------------------

/// 内层读：文件不存在 → 空清单；JSON 损坏 → Err（fail loud，不静默重建）。
fn load_locked(path: &Path) -> Result<RecycleBinFile, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            format!(
                "解析 {} 失败: {e}（recycle-bin.json 损坏时 fail loud，不静默重建）",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RecycleBinFile::default()),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

/// 内层写：tmp + rename 原子替换（底座 `write_atomic`，含 Windows 替换重试）。
fn save_locked(path: &Path, file: &RecycleBinFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| format!("序列化 recycle-bin.json 失败: {e}"))?;
    deepseek_tui::utils::write_atomic(path, json.as_bytes())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 回收时间戳：RFC3339/ISO8601 UTC，对齐 store.rs 的 chrono 惯例。
fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::marketplace::store::BundleSource;

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-recyclebin-test-{tag}-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn upload_record(id: &str) -> BundleRecord {
        BundleRecord {
            id: id.to_string(),
            source: BundleSource::Upload(format!("{id}.zip")),
            installed: true,
            content_fingerprint: Some("fp".to_string()),
            assets: Vec::new(),
            credential_keys: vec!["KEY".to_string()],
            installed_at: "2026-08-20T00:00:00+00:00".to_string(),
            degraded: None,
            extra: serde_json::Map::new(),
        }
    }

    /// 回收 → list → 取回的完整往返：目录搬动、清单条目、记录快照逐字段保留。
    #[test]
    fn recycle_list_take_back_roundtrip() {
        let tmp = fresh_dir("roundtrip");
        let bin = RecycleBin::with_roots(tmp.clone());
        let pkg = tmp.join("bundles/my-pkg");
        std::fs::create_dir_all(pkg.join("mcp")).unwrap();
        std::fs::create_dir_all(pkg.join("skills/my-skill")).unwrap();
        std::fs::write(pkg.join("mcp/manifest.json"), "{}").unwrap();
        std::fs::write(pkg.join("skills/my-skill/SKILL.md"), "---\nname: my-skill\n---").unwrap();

        bin.recycle_package("my-pkg", KIND_BUNDLE, "my-pkg.zip", upload_record("my-pkg"))
            .unwrap();
        assert!(!pkg.exists(), "回收后原包目录应搬走");
        assert!(tmp.join("recycle-bin/my-pkg/mcp/manifest.json").is_file());
        assert!(tmp.join("recycle-bin/my-pkg/skills/my-skill/SKILL.md").is_file());

        let list = bin.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "my-pkg");
        assert_eq!(list[0].display_name, "my-pkg.zip");
        assert_eq!(list[0].kind, KIND_BUNDLE);
        assert!(!list[0].package_missing);
        assert!(!list[0].recycled_at.is_empty());

        let snapshot = bin.take_back("my-pkg").unwrap();
        assert_eq!(snapshot, upload_record("my-pkg"), "快照应逐字段保留");
        assert!(pkg.join("mcp/manifest.json").is_file(), "取回应搬回原位");
        assert!(!tmp.join("recycle-bin/my-pkg").exists());
        assert!(bin.list().unwrap().is_empty(), "取回后清单应移除条目");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// purge：物理删除目录 + 清单条目；不在清单的 id 拒绝（fail-closed）；
    /// take_back 对不在清单的 id 同样拒绝。
    #[test]
    fn purge_removes_dir_and_unknown_id_is_rejected() {
        let tmp = fresh_dir("purge");
        let bin = RecycleBin::with_roots(tmp.clone());
        let pkg = tmp.join("bundles/my-skill");
        std::fs::create_dir_all(pkg.join("skills/my-skill")).unwrap();
        bin.recycle_package("my-skill", KIND_SKILL, "my-skill.zip", upload_record("my-skill"))
            .unwrap();
        assert!(tmp.join("recycle-bin/my-skill").is_dir());

        assert!(bin.purge("ghost").is_err(), "不在清单的 id purge 应拒绝");
        assert!(bin.take_back("ghost").is_err(), "不在清单的 id take_back 应拒绝");
        assert!(tmp.join("recycle-bin/my-skill").is_dir(), "误 purge 不得删他人目录");

        bin.purge("my-skill").unwrap();
        assert!(!tmp.join("recycle-bin/my-skill").exists(), "purge 应物理删除目录");
        assert!(bin.list().unwrap().is_empty(), "purge 后清单应移除条目");
        assert!(bin.purge("my-skill").is_err(), "重复 purge 应拒绝");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 清单在、包目录被外部删掉 → package_missing 标记；此时 take_back 拒绝、
    /// purge 仍允许（清条目）。
    #[test]
    fn list_marks_missing_package_and_purge_still_allowed() {
        let tmp = fresh_dir("missing");
        let bin = RecycleBin::with_roots(tmp.clone());
        let pkg = tmp.join("bundles/my-pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        bin.recycle_package("my-pkg", KIND_MCP, "my-pkg.zip", upload_record("my-pkg"))
            .unwrap();
        std::fs::remove_dir_all(tmp.join("recycle-bin/my-pkg")).unwrap();

        let list = bin.list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].package_missing, "目录缺失应标记 package_missing");
        assert!(bin.take_back("my-pkg").is_err(), "目录缺失不得恢复");
        bin.purge("my-pkg").unwrap();
        assert!(bin.list().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 同 id 回收站目标已存在 → preflight 拒绝覆盖（不静默顶掉残留条目）。
    #[test]
    fn recycle_refuses_to_overwrite_existing_target() {
        let tmp = fresh_dir("preflight");
        let bin = RecycleBin::with_roots(tmp.clone());
        std::fs::create_dir_all(tmp.join("bundles/my-pkg")).unwrap();
        std::fs::create_dir_all(tmp.join("recycle-bin/my-pkg")).unwrap();

        assert!(bin
            .recycle_package("my-pkg", KIND_MCP, "my-pkg.zip", upload_record("my-pkg"))
            .is_err());
        assert!(tmp.join("bundles/my-pkg").is_dir(), "源目录应保持原位");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 损坏清单 fail loud：读取报错且绝不回写（与 bundles.json 同一纪律）。
    #[test]
    fn corrupt_manifest_fails_loud_without_overwrite() {
        let tmp = fresh_dir("corrupt");
        let bin = RecycleBin::with_roots(tmp.clone());
        std::fs::write(tmp.join("recycle-bin.json"), "not-json{{{").unwrap();

        assert!(bin.list().is_err(), "损坏清单读取应报错");
        assert!(bin.purge("x").is_err(), "损坏清单 purge 应报错");
        assert_eq!(
            std::fs::read_to_string(tmp.join("recycle-bin.json")).unwrap(),
            "not-json{{{",
            "损坏文件不得被静默覆盖"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 恢复管线（纯技能包）：目录搬回 + bundles.json 登记重建（source=Upload、
    /// 保留原 installed_at、installed=true），无 MCP 组件 → credentials_required=false。
    /// 走真实 paths（PINVOU3_HOME 指临时目录），借 ENV_LOCK 与其它 env 测试串行。
    #[test]
    fn restore_skill_package_rebuilds_registration() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-skill");
        std::env::set_var("PINVOU3_HOME", &tmp);

        let pkg = paths::bundles_root().join("my-skill");
        std::fs::create_dir_all(pkg.join("skills/my-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---\n",
        )
        .unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("my-skill")).unwrap();

        // 卸载（模拟 skill_marketplace 的回收路径）：删登记 + 整包回收。
        let record = store.get("my-skill").unwrap().unwrap();
        store.remove("my-skill").unwrap();
        RecycleBin::new()
            .recycle_package("my-skill", KIND_SKILL, "my-skill.zip", record)
            .unwrap();
        assert!(store.get("my-skill").unwrap().is_none());

        let result = restore_plugin("my-skill").unwrap();
        assert!(!result.credentials_required, "纯技能包无需凭据");
        assert!(
            pkg.join("skills/my-skill/SKILL.md").is_file(),
            "恢复后目录应回到 bundles/<id>/"
        );
        let restored = store.get("my-skill").unwrap().expect("登记应重建");
        assert!(restored.installed);
        assert_eq!(
            restored.source,
            BundleSource::Upload("my-skill.zip".to_string())
        );
        assert_eq!(
            restored.installed_at, "2026-08-20T00:00:00+00:00",
            "原 installed_at 应保留"
        );
        assert!(RecycleBin::new().list().unwrap().is_empty(), "恢复后清单应清空");

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导出：清单内条目生成 zip，plugin.json/mcp/skills 条目完整可读回；Python
    /// 运行缓存（__pycache__/、*.pyc）不打包。
    #[test]
    fn export_package_writes_complete_zip() {
        let tmp = fresh_dir("export");
        let bin = RecycleBin::with_roots(tmp.clone());
        let pkg = tmp.join("bundles/exp-pkg");
        std::fs::create_dir_all(pkg.join("mcp/__pycache__")).unwrap();
        std::fs::create_dir_all(pkg.join("skills/exp-skill")).unwrap();
        std::fs::write(
            pkg.join("plugin.json"),
            r#"{"manifest_version":1,"id":"exp-pkg","name":"Exp"}"#,
        )
        .unwrap();
        std::fs::write(pkg.join("mcp/manifest.json"), r#"{"id":"exp-pkg"}"#).unwrap();
        std::fs::write(pkg.join("mcp/server.py"), b"print('hi')").unwrap();
        std::fs::write(pkg.join("mcp/__pycache__/server.cpython-311.pyc"), b"cache").unwrap();
        std::fs::write(
            pkg.join("skills/exp-skill/SKILL.md"),
            "---\nname: exp-skill\n---\n",
        )
        .unwrap();
        bin.recycle_package("exp-pkg", KIND_BUNDLE, "exp-pkg.zip", upload_record("exp-pkg"))
            .unwrap();

        let dest = tmp.join("export.zip");
        bin.export_package("exp-pkg", &dest).unwrap();

        // 回收站内容不受导出影响（导出 ≠ 取回/删除）
        assert!(tmp.join("recycle-bin/exp-pkg/plugin.json").is_file());
        assert_eq!(bin.list().unwrap().len(), 1);

        let archive_file = std::fs::File::open(&dest).unwrap();
        let mut archive = zip::ZipArchive::new(archive_file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "mcp/manifest.json".to_string(),
                "mcp/server.py".to_string(),
                "plugin.json".to_string(),
                "skills/exp-skill/SKILL.md".to_string(),
            ],
            "zip 条目应为包本体平铺在根，且不含 Python 缓存"
        );
        let mut content = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("plugin.json").unwrap(),
            &mut content,
        )
        .unwrap();
        assert!(content.contains("\"exp-pkg\""), "plugin.json 内容应完整: {content}");
        let mut skill = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("skills/exp-skill/SKILL.md").unwrap(),
            &mut skill,
        )
        .unwrap();
        assert!(skill.contains("name: exp-skill"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导出 fail-closed：不在清单的 id 拒绝；清单在、目录缺失（package_missing）报错。
    #[test]
    fn export_unknown_id_and_missing_package_rejected() {
        let tmp = fresh_dir("export-failclosed");
        let bin = RecycleBin::with_roots(tmp.clone());
        let dest = tmp.join("export.zip");

        assert!(bin.export_package("ghost", &dest).is_err(), "未知 id 应拒绝导出");
        assert!(!dest.exists(), "拒绝导出不得留文件");

        let pkg = tmp.join("bundles/my-pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
        bin.recycle_package("my-pkg", KIND_MCP, "my-pkg.zip", upload_record("my-pkg"))
            .unwrap();
        std::fs::remove_dir_all(tmp.join("recycle-bin/my-pkg")).unwrap();
        assert!(
            bin.export_package("my-pkg", &dest).is_err(),
            "package_missing 应拒绝导出"
        );
        assert!(!dest.exists(), "失败导出不得留半写文件");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导出的 zip 可经统一导入管线（plugin_import::import_plugin_package）重新
    /// 导入：组件识别、落盘、登记（source=Upload）全链路还原。
    /// 走真实 paths（PINVOU3_HOME 指临时目录），借 ENV_LOCK 与其它 env 测试串行。
    #[test]
    fn exported_zip_reimports_via_plugin_pipeline() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("export-reimport");
        std::env::set_var("PINVOU3_HOME", &tmp);

        // 构造与统一导入管线落盘形态一致的包目录（plugin.json 声明组件 +
        // skills/<name>/SKILL.md），回收后导出。
        let pkg = paths::bundles_root().join("exp-skill");
        std::fs::create_dir_all(pkg.join("skills/exp-skill")).unwrap();
        std::fs::write(
            pkg.join("plugin.json"),
            r#"{
                "manifest_version":1,"id":"exp-skill","name":"exp-skill",
                "components":{"skills":[{"id":"exp-skill","dir":"skills/exp-skill"}]}
            }"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("skills/exp-skill/SKILL.md"),
            "---\nname: exp-skill\ndescription: d\n---\n# hi\n",
        )
        .unwrap();
        RecycleBin::new()
            .recycle_package(
                "exp-skill",
                KIND_SKILL,
                "exp-skill.zip",
                upload_record("exp-skill"),
            )
            .unwrap();
        assert!(!pkg.exists(), "回收后原包目录应搬走");

        let dest = tmp.join("export.zip");
        RecycleBin::new().export_package("exp-skill", &dest).unwrap();

        let report = crate::features::marketplace::plugin_import::import_plugin_package(
            &dest.to_string_lossy(),
            "exp-skill.zip",
        )
        .expect("导出的 zip 应可经统一导入管线重新导入");
        assert_eq!(report.id, "exp-skill");
        assert_eq!(
            report.kind,
            crate::features::marketplace::bundle::BundleKind::Skill
        );
        assert!(
            pkg.join("skills/exp-skill/SKILL.md").is_file(),
            "重新导入应落盘回 bundles/<id>/"
        );
        assert!(pkg.join("plugin.json").is_file());
        let record = BundleStore::new()
            .get("exp-skill")
            .unwrap()
            .expect("重新导入应登记");
        assert_eq!(
            record.source,
            BundleSource::Upload("exp-skill.zip".to_string())
        );

        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
