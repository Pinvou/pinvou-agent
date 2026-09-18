// architecture-guard: allow-target-cfg -- the round-11 M3 regression test
// (restore_consent_gate_persist_failure_is_retryable) needs a read-only-home
// fixture (chmod 0555) to force the consent-gate persist failure; test-only
// inline cfg(unix)+PermissionsExt, same exemption precedent as
// package_export.rs / marketplace/mod.rs (review #455 R9-M5). A write probe
// guards against running as root (loud ROOT-SKIP marker, round-11 m12);
// Windows is covered by the POSIX-independent restore paths.
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
//!   FILE_LOCK 串行化读-改-写：各公开方法进入时持锁，覆盖整个
//!   load → 目录搬移/删除 → 条目修改 → save 区间（store.rs `upsert` 同范式），
//!   锁内只调 `load_locked`/`save_locked`，不调会再取同一把锁的公开方法；
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

    /// 回收 preflight：只校验「能不能收」（id 合法、源目录在、目标无残留、回收站
    /// 根可建），不搬动任何目录、不写清单。供卸载路径在拆供给面（installed.json /
    /// mcp.json / secrets）之前 fail fast —— 回收注定失败时零副作用中止（M2）。
    pub fn preflight_recycle(&self, pkg_id: &str) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let _guard = file_lock();
        self.preflight_recycle_locked(pkg_id)
    }

    /// 已持锁的 preflight 实现（`recycle_package` 持锁复用，避免 Mutex 重入）。
    fn preflight_recycle_locked(&self, pkg_id: &str) -> Result<(), String> {
        let src = self.bundles_root.join(pkg_id);
        let dst = self.root.join(pkg_id);
        // 源必须存在；目标已存在说明有同 id 残留条目，拒绝覆盖（可能是
        // 不同包同 id，静默覆盖即数据丢失 —— 与 retirement preflight 同一纪律）。
        if !src.is_dir() {
            return Err(format!("包目录 {} 不存在，无法移入回收站", src.display()));
        }
        if dst.exists() {
            return Err(format!("回收站目标 {} 已存在，拒绝覆盖", dst.display()));
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|e| format!("创建回收站目录 {} 失败: {e}", self.root.display()))?;
        Ok(())
    }

    /// 回收：preflight 检查 → rename 搬移 `bundles/<id>/` → `recycle-bin/<id>/`
    /// → 失败回滚（retirement.rs archive 同范式）→ 写清单。
    /// `record_snapshot` 为回收前的 bundles.json 原记录（恢复重建登记用）。
    ///
    /// 全程持 `file_lock()`（store.rs `upsert` 同范式）：load → 目录搬移 → 条目
    /// 修改 → save 是一个临界区，并发回收/取回/彻底删除不会 lost update。锁内
    /// 只调 `load_locked`/`save_locked`，不得再调会取同一把锁的公开方法（死锁）。
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
        let _guard = file_lock();
        let src = self.bundles_root.join(pkg_id);
        let dst = self.root.join(pkg_id);
        self.preflight_recycle_locked(pkg_id)?;
        // rename 走 plugin_import 的 Windows 瞬时占用重试口径（杀软/索引器短暂
        // 持有新建目录句柄会报 os error 5，实测命中）。
        if let Err(e) = super::plugin_import::rename_dir_with_retry(&src, &dst) {
            // rename 失败通常什么都没动；兜底尝试回滚（部分平台跨设备 rename 语义差异）。
            // 回滚失败必须留痕：目录可能处于半搬移状态，静默吞掉将无从排查。
            if let Err(re) = super::plugin_import::rename_dir_with_retry(&dst, &src) {
                log::error!(
                    "[recycle-bin] 回收 {pkg_id} rename 失败后的兜底回滚也失败（{} 可能处于半搬移状态）: {re}",
                    dst.display()
                );
            }
            return Err(format!(
                "搬移 {} → {} 失败: {e}",
                src.display(),
                dst.display()
            ));
        }
        // 搬移成功后写清单；清单写失败则把目录搬回原位（不留无清单的孤儿目录）。
        let mut file = load_locked(&self.file)?;
        file.entries.retain(|e| e.id != pkg_id);
        file.entries.push(RecycledEntry {
            id: pkg_id.to_string(),
            display_name: display_name.to_string(),
            kind: kind.to_string(),
            recycled_at: now_iso8601(),
            record: record_snapshot,
            extra: serde_json::Map::new(),
        });
        if let Err(e) = save_locked(&self.file, &file) {
            if let Err(re) = super::plugin_import::rename_dir_with_retry(&dst, &src) {
                // 清单无条目而目录滞留回收站根 = list/restore/purge 不可见的孤儿
                // （数据未丢）。必须响亮留痕，并如实上报（不能谎称已回滚）。
                log::error!(
                    "[recycle-bin] 回收 {pkg_id} 清单写入失败，目录回滚也失败：{} 滞留回收站根但无清单条目（数据未丢，需人工搬回）: {re}",
                    dst.display()
                );
                return Err(format!(
                    "写入回收站清单失败: {e}；目录回滚也失败，包目录滞留在 {}（数据未丢，需人工搬回）: {re}",
                    dst.display()
                ));
            }
            return Err(format!("写入回收站清单失败（已回滚目录）: {e}"));
        }
        log::info!(
            "[recycle-bin] 已回收包 {pkg_id}（kind={kind}）→ {}",
            dst.display()
        );
        Ok(())
    }

    /// 回收站列表：读清单 + 校验包目录存在（缺失标记 `package_missing`，
    /// 前端据此禁用"恢复"）。清单损坏 fail loud（返回 Err）。
    /// 持锁读取 + 校验，拿到的清单与目录是同一时刻的一致快照。
    pub fn list(&self) -> Result<Vec<RecycledPluginInfo>, String> {
        let _guard = file_lock();
        let file = load_locked(&self.file)?;
        Ok(file
            .entries
            .into_iter()
            .map(|e| {
                // 展示名优先取记录快照里的用户可见名（extra.display_name，如
                // 「初始化git」），缺失时回退源文件名——单 md 导入的包源文件名
                // 恒为 "SKILL.md"，直接展示认不出是哪个技能。
                let record_display = e
                    .record
                    .extra
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                RecycledPluginInfo {
                    package_missing: !self.root.join(&e.id).is_dir(),
                    id: e.id,
                    display_name: record_display.unwrap_or(e.display_name),
                    kind: e.kind,
                    recycled_at: e.recycled_at,
                }
            })
            .collect())
    }

    /// 取回：fail-closed（不在清单 → Err）→ preflight → 搬回 `bundles/<id>/`
    /// → 失败回滚 → 从清单移除 → 返回记录快照（供恢复管线重建登记）。
    /// 全程持 `file_lock()`（load → 搬回 → 条目移除 → save 一个临界区）。
    pub fn take_back(&self, pkg_id: &str) -> Result<BundleRecord, String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
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
        std::fs::create_dir_all(&self.bundles_root)
            .map_err(|e| format!("创建包目录根 {} 失败: {e}", self.bundles_root.display()))?;
        if let Err(e) = super::plugin_import::rename_dir_with_retry(&src, &dst) {
            // 搬回失败通常什么都没动；兜底回滚失败必须留痕（目录可能半搬移）。
            if let Err(re) = super::plugin_import::rename_dir_with_retry(&dst, &src) {
                log::error!(
                    "[recycle-bin] 取回 {pkg_id} rename 失败后的兜底回滚也失败（{} 可能处于半搬移状态）: {re}",
                    src.display()
                );
            }
            return Err(format!(
                "搬回 {} → {} 失败: {e}",
                src.display(),
                dst.display()
            ));
        }
        let entry = file.entries.remove(index);
        // 目录已搬回，清单移除失败不搬回目录（恢复主操作已成功），fail loud 到错误。
        save_locked(&self.file, &file)?;
        log::info!("[recycle-bin] 已取回包 {pkg_id} → {}", dst.display());
        Ok(entry.record)
    }

    /// 彻底删除：fail-closed，仅删清单中存在的条目（绝不按外部传入路径删任意
    /// 目录），物理删 `recycle-bin/<id>/` + 清单条目。包目录已被外部删除时
    /// （package_missing）同样允许 purge 清条目。
    /// 全程持 `file_lock()`（load → 删目录 → 条目移除 → save 一个临界区）。
    pub fn purge(&self, pkg_id: &str) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
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
        save_locked(&self.file, &file)?;
        log::info!("[recycle-bin] 已彻底删除包 {pkg_id}");
        Ok(())
    }

    /// 导出：fail-closed（不在清单 → Err，与 purge 同口径；package_missing → Err），
    /// 把回收站包目录 `recycle-bin/<id>/` 的内容打成 zip（plugin.json、mcp/、
    /// skills/ 等平铺在 zip 根，对齐 plugin-package-spec 的包结构，可经统一导入
    /// 管线 `plugin_import::import_plugin_package` 重新导入）。写出逻辑复用
    /// `package_export::write_package_zip`（只打包插件包本体，不含回收站清单等
    /// 元数据）。args 净化与已安装包导出同口径启用：当前版本卸载的包 manifest
    /// 保持相对 args（净化是 no-op），但旧版本/手改的 manifest 可能落了安装期
    /// 绝对路径——净化前缀必须按包的原安装位置 `bundles/<id>`（而非回收站目录）
    /// 匹配，否则漏净化产出在别的机器导不回的 zip；已是相对形式的 args 原样
    /// 透传，不会误改。
    /// 全程持 `file_lock()`：并发的 take_back/purge 会把包目录搬走/删掉，锁内
    /// 导出保证遍历期间目录不会被并发操作挪动（zip 较大时持锁偏久，正确性优先）。
    pub fn export_package(&self, pkg_id: &str, dest_zip: &Path) -> Result<(), String> {
        if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
            return Err(format!("非法包 id '{pkg_id}'"));
        }
        let _guard = file_lock();
        let file = load_locked(&self.file)?;
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
        // 净化前缀按原安装位置（回收前 manifest 里的绝对路径指向 bundles/<id>/mcp/）。
        let sanitize_root = self.bundles_root.join(pkg_id);
        let written =
            super::package_export::write_package_zip(&src, dest_zip, Some(&sanitize_root))?;
        log::info!(
            "[recycle-bin] 已导出包 {pkg_id}（{written} 个条目）→ {}",
            dest_zip.display()
        );
        Ok(())
    }
}

/// 按包目录内容推导回收站 kind：`mcp/manifest.json` 存在（与恢复侧供给判定
/// `restore_plugin` 的 `has_mcp` 同口径）+ skills/ → bundle；仅 mcp → mcp；
/// 否则 skill。以 manifest 文件而非 `mcp/` 目录为准，避免「有 mcp/ 目录但
/// manifest 缺失/损坏」的包 kind 记为 mcp/bundle、恢复却零供给的口径劈叉。
pub(crate) fn package_kind(pkg_dir: &Path) -> &'static str {
    let has_mcp = pkg_dir.join("mcp").join("manifest.json").is_file();
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
///    供给失败则整体回滚到回收站（目录搬回 + 清单条目复原 + 登记移除），用户可
///    修复后从回收站重试，不残留「记录已安装、无供给面、回收站条目已消费、无从
///    重试」的半恢复态；
/// 4. 技能组件随包目录搬回 + 登记恢复即回到安装态（技能无独立供给管线）；
/// 5. The scope disabled set is handled in two cases (review #455 R5-m5 fixes
///    a consent gate hole):
///    - Uninitialized scope (the DenyAll on-the-fly expansion already covers
///      the package): fallback cleanup only, nothing written to disk (same
///      non-persisting policy as the disable arm) — the restored package
///      stays default-off in such scopes;
///    - Initialized scope (an explicit switch state existed before
///      uninstall): restore as **disabled** (re-added to the persisted list)
///      rather than enabled — uninstall wiped the persisted entry, so
///      restoring as enabled would bring a "package the user explicitly
///      turned off before uninstall" back online via the restore button with
///      zero consent, violating the "external capabilities require explicit
///      opt-in" consent model. The cost is that packages that were **on**
///      before uninstall must also be manually re-enabled once after restore
///      (the record leans to the safe side). The hidden set is only cleared,
///      never written (restored packages must stay visible to the user).
///
/// 并发契约：全程持同 id `import_lock_for`（与导入/卸载/展示编辑同一把锁；
/// 锁序 import → recycle → store，与卸载路径一致，无死锁面），恢复整链路
/// （取回 → 重建登记 → 供给）对并发的同 id 重导入/再卸载串行——取回前抢锁，
/// 避免与并发导入的「rename → 备份重基线」交错；`install_upload` 只取全局
/// 事务锁，不在本锁上重入。与卸载侧的 recycle preflight 对称：先锁再动目录。
pub fn restore_plugin(pkg_id: &str) -> Result<RestoreRecycledResult, String> {
    let import_lock = super::plugin_import::import_lock_for(pkg_id);
    let _import_guard = import_lock.lock().unwrap_or_else(|p| p.into_inner());
    let bin = RecycleBin::new();
    // 恢复碰撞 preflight（fail-closed，先于任何搬移）：回收期间市场状态可能已
    // 变（例如导入了把同名技能作为 companion 的包），碰撞状态下恢复会造出同
    // 技能双份物理副本，后续技能卸载的候选目录清理会连唯一副本一起删。检查
    // 与导入通道同口径（`ensure_skill_restorable`）；此刻包目录仍在回收站，
    // 自身副本不会被误判为他包副本。
    let recycled_skills_dir = bin.root.join(pkg_id).join("skills");
    if recycled_skills_dir.is_dir() {
        let mut skill_names: Vec<String> = std::fs::read_dir(&recycled_skills_dir)
            .map_err(|e| format!("读取 {} 失败: {e}", recycled_skills_dir.display()))?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        skill_names.sort();
        for name in &skill_names {
            super::skill_marketplace::ensure_skill_restorable(name)?;
        }
    }
    // Consent gate BEFORE take_back (round-11 M3): if its persist fails, the
    // bin entry is still intact and "retry the restore" is a real remedy (the
    // round-10 placement consumed the entry first, making the failure
    // unretryable and leaving the pack enabled). Skill ids are enumerated from
    // the bin-side package dir — the bundles-side dir only exists after
    // take_back. A later install_upload failure still rolls the entry back
    // with the gate already written (consistent: bin + disabled).
    let mut consent_ids = vec![pkg_id.to_string()];
    if let Ok(rd) = std::fs::read_dir(recycled_skills_dir.as_path()) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                consent_ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    super::scope::apply_restore_consent_gate(&consent_ids).map_err(|save_error| {
        format!(
            "恢复 {pkg_id} 前置检查失败（回收站条目未动，可直接重试）：重新禁用状态落盘失败: {save_error}"
        )
    })?;

    let record = bin.take_back(pkg_id)?;
    let mgr = super::MarketplaceManager::new();
    let pkg_dir = paths::bundles_root().join(pkg_id);
    let has_mcp = pkg_dir.join("mcp").join("manifest.json").is_file();

    // 重建登记：快照即原记录（installed_at/source/extra 原样保留）。upsert_preserving
    // 在记录已被卸载移除的常态下等价 upsert；并发重装写了新记录时保留其首装元数据。
    // 登记重建失败必须回滚到回收站（评审 #455 R13-B2，与下方供给失败分支同形）：
    // 此刻目录已搬回 bundles_root 而记录不存在——记录驱动的枚举（installed_ids、
    // list_skills 上传技能）看不到该包，但会话物化直接扫描 bundles_root 目录，
    // 包内技能会以零同意进入新的 plain 会话；且回收站条目已被消费，「重试」
    // 必失败。不回滚会残留这种半恢复态，回滚自身失败必须响亮留痕并如实上报。
    // 登记未写入成功（upsert 返回 Err）故无需 remove；并发重装的记录若存在，
    // 恰恰不应被本路径删除。
    let mut restored = record.clone();
    restored.installed = true;
    if let Err(e) = BundleStore::new().upsert_preserving(restored) {
        let rollback_display = match &record.source {
            super::store::BundleSource::Upload(zip) => zip.clone(),
            _ => pkg_id.to_string(),
        };
        let rollback_kind = package_kind(&pkg_dir);
        if let Err(re) =
            bin.recycle_package(pkg_id, rollback_kind, &rollback_display, record.clone())
        {
            log::error!(
                "[recycle-bin] 恢复 {pkg_id} 登记重建失败（{e}），回滚到回收站也失败：包目录仍在 {}、无登记、无回收站条目: {re}",
                pkg_dir.display()
            );
            return Err(format!(
                "恢复 {pkg_id} 失败: {e}；回滚到回收站也失败（包目录仍在原位，未登记，可手动删除）: {re}"
            ));
        }
        return Err(format!("恢复 {pkg_id} 失败（已回滚至回收站，可重试）: {e}"));
    }

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
        } else if let Err(e) = mgr.install_upload(pkg_id, record.source.clone()) {
            // 供给失败整体回滚到回收站（mcp.json/installed.json 已由 install_upload
            // 内部事务回滚）：目录搬回 + 清单条目复原 + 登记移除，用户可从回收站
            // 重试；不回滚会残留「记录已安装、无供给面、条目已消费无从重试」的
            // 半恢复态。回滚自身失败必须响亮留痕并如实上报。
            let rollback_display = match &record.source {
                super::store::BundleSource::Upload(zip) => zip.clone(),
                _ => pkg_id.to_string(),
            };
            let rollback_kind = package_kind(&pkg_dir);
            if let Err(re) =
                bin.recycle_package(pkg_id, rollback_kind, &rollback_display, record.clone())
            {
                log::error!(
                    "[recycle-bin] 恢复 {pkg_id} 供给失败（{e}），回滚到回收站也失败：包目录仍在 {}、登记 installed=true、无回收站条目: {re}",
                    pkg_dir.display()
                );
                return Err(format!(
                    "恢复 {pkg_id} 失败: {e}；回滚到回收站也失败（包目录仍在原位，登记为已安装）: {re}"
                ));
            }
            if let Err(se) = BundleStore::new().remove(pkg_id) {
                log::warn!("[recycle-bin] 恢复 {pkg_id} 回滚后登记移除失败: {se}");
            }
            return Err(format!("恢复 {pkg_id} 失败（已回滚至回收站，可重试）: {e}"));
        }
    }

    // Consent-gate rationale (review #455 R5-m5 / R9-M2, hoisted above
    // take_back in round-11 M3): uninstall wiped the stored entries, so
    // "explicitly off before uninstall" and "on before uninstall" are
    // indistinguishable — restore converges to disabled (conservative), with
    // install-like default markers so later user gestures can lift them;
    // uninitialized scopes stay unwritten (the DenyAll expansion already
    // covers the pack); hidden sets are cleared, never written.
    Ok(RestoreRecycledResult {
        credentials_required,
    })
}

// ---------------------------------------------------------------------------
// 已持锁实现（公开方法的临界区内层；调用前必须已持有 RECYCLE_BIN_FILE_LOCK）
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
        std::fs::write(
            pkg.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---",
        )
        .unwrap();

        bin.recycle_package("my-pkg", KIND_BUNDLE, "my-pkg.zip", upload_record("my-pkg"))
            .unwrap();
        assert!(!pkg.exists(), "回收后原包目录应搬走");
        assert!(tmp.join("recycle-bin/my-pkg/mcp/manifest.json").is_file());
        assert!(
            tmp.join("recycle-bin/my-pkg/skills/my-skill/SKILL.md")
                .is_file()
        );

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

    /// list 展示名回退链：记录快照 extra.display_name（用户可见名）优先于
    /// 源文件名；extra 无该字段（或为空）时回退源文件名。单 md 导入的包源
    /// 文件名恒为 "SKILL.md"，必须展示用户可见名才认得出是哪个技能。
    #[test]
    fn list_display_name_prefers_record_snapshot_over_source_file() {
        let tmp = fresh_dir("display_name");
        let bin = RecycleBin::with_roots(tmp.clone());

        let pkg = tmp.join("bundles/md-skill");
        std::fs::create_dir_all(pkg.join("skills/md-skill")).unwrap();
        let mut record = upload_record("md-skill");
        record.extra.insert(
            "display_name".to_string(),
            serde_json::Value::String("初始化git".to_string()),
        );
        bin.recycle_package("md-skill", KIND_SKILL, "SKILL.md", record)
            .unwrap();

        let pkg2 = tmp.join("bundles/zip-skill");
        std::fs::create_dir_all(pkg2.join("skills/zip-skill")).unwrap();
        bin.recycle_package(
            "zip-skill",
            KIND_SKILL,
            "zip-skill.zip",
            upload_record("zip-skill"),
        )
        .unwrap();

        let list = bin.list().unwrap();
        assert_eq!(list.len(), 2);
        let md = list.iter().find(|i| i.id == "md-skill").unwrap();
        assert_eq!(md.display_name, "初始化git", "应展示记录快照里的用户可见名");
        let zip = list.iter().find(|i| i.id == "zip-skill").unwrap();
        assert_eq!(zip.display_name, "zip-skill.zip", "无快照名时回退源文件名");

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
        bin.recycle_package(
            "my-skill",
            KIND_SKILL,
            "my-skill.zip",
            upload_record("my-skill"),
        )
        .unwrap();
        assert!(tmp.join("recycle-bin/my-skill").is_dir());

        assert!(bin.purge("ghost").is_err(), "不在清单的 id purge 应拒绝");
        assert!(
            bin.take_back("ghost").is_err(),
            "不在清单的 id take_back 应拒绝"
        );
        assert!(
            tmp.join("recycle-bin/my-skill").is_dir(),
            "误 purge 不得删他人目录"
        );

        bin.purge("my-skill").unwrap();
        assert!(
            !tmp.join("recycle-bin/my-skill").exists(),
            "purge 应物理删除目录"
        );
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

        assert!(
            bin.recycle_package("my-pkg", KIND_MCP, "my-pkg.zip", upload_record("my-pkg"))
                .is_err()
        );
        assert!(tmp.join("bundles/my-pkg").is_dir(), "源目录应保持原位");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Restore consent gate (review #455 R5-m5): in initialized scopes a
    /// restored package comes back **disabled** rather than enabled —
    /// uninstall wiped the persisted entry, so "on before uninstall" and
    /// "off before uninstall" are indistinguishable; restore uniformly as
    /// disabled, never bringing a package back online via the restore button
    /// with zero consent. Uninitialized scopes are not written (the DenyAll
    /// on-the-fly expansion already covers them).
    #[test]
    fn restore_redisables_in_initialized_scopes() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-redisable");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let pkg = paths::bundles_root().join("my-skill-rr");
        std::fs::create_dir_all(pkg.join("skills/my-skill-rr")).unwrap();
        std::fs::write(
            pkg.join("skills/my-skill-rr/SKILL.md"),
            "---\nname: my-skill-rr\n---\n",
        )
        .unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("my-skill-rr")).unwrap();

        // Before uninstall: plain is initialized and the package is
        // explicitly off (a disabled entry is left in storage).
        crate::features::marketplace::scope::save_disabled_bundles_for(
            crate::features::marketplace::ConnectorScope::Plain,
            &["my-skill-rr".to_string()],
        );
        assert!(
            crate::features::marketplace::scope::load_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Plain
            )
            .iter()
            .any(|id| id == "my-skill-rr")
        );

        // Simulate a full uninstall: registry removal + whole-package
        // recycle + command-layer disabled-set cleanup.
        let record = store.get("my-skill-rr").unwrap().unwrap();
        store.remove("my-skill-rr").unwrap();
        RecycleBin::new()
            .recycle_package("my-skill-rr", KIND_SKILL, "my-skill-rr.zip", record)
            .unwrap();
        crate::features::marketplace::scope::remove_bundle_from_disabled_scopes("my-skill-rr");
        assert!(
            !crate::features::marketplace::scope::load_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Plain
            )
            .iter()
            .any(|id| id == "my-skill-rr"),
            "卸载后禁用条目应被清理"
        );

        let result = restore_plugin("my-skill-rr").unwrap();
        assert!(!result.credentials_required);
        // After restore it must be re-disabled: plain is initialized and the
        // persisted list contains the package id again — the DenyAll
        // "explicit opt-in" consent gate still holds on the restore path.
        assert!(
            crate::features::marketplace::scope::load_disabled_bundles_for(
                crate::features::marketplace::ConnectorScope::Plain
            )
            .iter()
            .any(|id| id == "my-skill-rr"),
            "恢复后已初始化 scope 必须回到禁用态（同意门）"
        );
        // The uninitialized scope (code) is not written: the DenyAll
        // on-the-fly expansion already covers it; user state is not persisted.
        let file = crate::features::marketplace::scope::load_disabled_bundles_file();
        assert!(
            !file.initialized.contains("code"),
            "恢复不得初始化未初始化 scope: {file:?}"
        );

        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
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
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

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
        assert!(
            RecycleBin::new().list().unwrap().is_empty(),
            "恢复后清单应清空"
        );

        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 恢复碰撞 preflight：回收期间导入了同名 companion 的包后，恢复 fail-closed
    /// 拒绝（清单与回收站目录原样保留）。碰撞状态下恢复会造出同技能双份物理副本，
    /// 此后技能卸载的候选目录清理会把用户唯一副本连他包副本一起删（review P1）。
    #[test]
    fn restore_refuses_skill_name_colliding_with_foreign_package() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-collide");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let pkg = paths::bundles_root().join("my-skill");
        std::fs::create_dir_all(pkg.join("skills/my-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---\n",
        )
        .unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("my-skill")).unwrap();
        let record = store.get("my-skill").unwrap().unwrap();
        store.remove("my-skill").unwrap();
        RecycleBin::new()
            .recycle_package("my-skill", KIND_SKILL, "my-skill.zip", record)
            .unwrap();

        // 冲突注入：回收期间他包 m 实体化了同名 companion 副本。
        let foreign = paths::bundles_root().join("m/skills/my-skill");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "---\nname: my-skill\n---\n").unwrap();

        let err = restore_plugin("my-skill").unwrap_err();
        assert!(
            err.contains("无法恢复"),
            "应拒绝碰撞恢复并提示先处理冲突包: {err}"
        );
        assert!(
            RecycleBin::new().list().unwrap().len() == 1,
            "拒绝后回收清单应原样保留"
        );
        assert!(
            tmp.join("marketplace/recycle-bin/my-skill/skills/my-skill/SKILL.md")
                .is_file(),
            "回收站目录不得被搬出"
        );
        assert!(foreign.join("SKILL.md").is_file(), "他包同名副本不得受影响");
        assert!(store.get("my-skill").unwrap().is_none(), "不得重建登记");

        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Round-11 M3 regression: the consent gate runs BEFORE take_back, so a
    /// gate persist failure leaves the bin entry untouched and "retry the
    /// restore" is a real remedy (the round-10 placement consumed the entry
    /// first: the failure was unretryable and the pack stayed enabled with
    /// zero consent). Fixture: an initialized plain scope (the gate has state
    /// to persist) + a read-only home (the persist fails).
    #[cfg(unix)]
    #[test]
    fn restore_consent_gate_persist_failure_is_retryable() {
        use std::os::unix::fs::PermissionsExt;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-gate-retry");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let restore_env = |prev: &Option<String>| match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        };

        // Skill-only package in the bin (no MCP supply on the restore path).
        let pkg = paths::bundles_root().join("gate-skill");
        std::fs::create_dir_all(pkg.join("skills/gate-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/gate-skill/SKILL.md"),
            "---\nname: gate-skill\n---\n",
        )
        .unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("gate-skill")).unwrap();
        let record = store.get("gate-skill").unwrap().unwrap();
        store.remove("gate-skill").unwrap();
        RecycleBin::new()
            .recycle_package("gate-skill", KIND_SKILL, "gate-skill.zip", record)
            .unwrap();

        // An initialized plain scope gives the consent gate state to persist.
        crate::features::marketplace::scope::save_disabled_bundles_for(
            crate::features::marketplace::ConnectorScope::Plain,
            &[],
        );

        // Read-only home: the gate's save must fail. Root probe first (mode
        // bits are no-ops for root) — loud skip per round-11 m12.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = tmp.join(".root-probe");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!(
                "ROOT-SKIP[restore_consent_gate_persist_failure_is_retryable]: running as root - read-only home fixture stays writable; NOT exercised"
            );
            restore_env(&prev);
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }

        let err = restore_plugin("gate-skill").unwrap_err();
        assert!(
            err.contains("重试"),
            "the failure must name retry as the remedy: {err}"
        );
        assert_eq!(
            RecycleBin::new().list().unwrap().len(),
            1,
            "the bin entry survives the gate failure (retryable)"
        );
        assert!(
            tmp.join("marketplace/recycle-bin/gate-skill/skills/gate-skill/SKILL.md")
                .is_file(),
            "the package directory stays in the bin"
        );
        assert!(!pkg.exists(), "take_back must not have run");
        assert!(
            store.get("gate-skill").unwrap().is_none(),
            "no registration was rebuilt"
        );

        // Fix the environment and retry: the restore succeeds end to end and
        // the consent gate holds (restored pack disabled in plain, marked as
        // install-default so a user gesture can lift it, round-11 B2).
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        restore_plugin("gate-skill").expect("retry after fixing the persist failure must succeed");
        assert!(pkg.join("skills/gate-skill/SKILL.md").is_file());
        assert!(store.get("gate-skill").unwrap().unwrap().installed);
        assert!(RecycleBin::new().list().unwrap().is_empty());
        let file = crate::features::marketplace::scope::load_disabled_bundles_file();
        let plain_disabled = file.scopes.get("plain").cloned().unwrap_or_default();
        assert!(
            plain_disabled.iter().any(|id| id == "gate-skill"),
            "consent gate: restored pack disabled in plain: {plain_disabled:?}"
        );
        let plain_defaults = file
            .default_off_scopes
            .get("plain")
            .cloned()
            .unwrap_or_default();
        assert!(
            plain_defaults.iter().any(|id| id == "gate-skill"),
            "gate-written off is install-default (liftable), not a user verdict: {plain_defaults:?}"
        );

        restore_env(&prev);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 恢复供给失败整体回滚到回收站：MCP 包 manifest 损坏 → install_upload 失败
    /// （load_manifest 读不出），此时必须目录搬回 + 清单条目复原 + 登记移除，
    /// 用户可修复后从回收站重试；不得残留「记录已安装、无供给面、条目已消费
    /// 无从重试」的半恢复态（review P2）。
    #[test]
    fn restore_mcp_supply_failure_rolls_back_to_recycle_bin() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-rollback");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // 损坏 manifest 的 MCP 包：has_mcp 成立（文件在）但 load_manifest 读不出，
        // install_upload 必失败（且 manifest 无 secrets 声明，不走进凭据分支）。
        let pkg = paths::bundles_root().join("broken-mcp");
        std::fs::create_dir_all(pkg.join("mcp")).unwrap();
        std::fs::write(pkg.join("mcp/manifest.json"), "not-json{{{").unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("broken-mcp")).unwrap();
        let record = store.get("broken-mcp").unwrap().unwrap();
        store.remove("broken-mcp").unwrap();
        RecycleBin::new()
            .recycle_package("broken-mcp", KIND_MCP, "broken-mcp.zip", record)
            .unwrap();

        let err = restore_plugin("broken-mcp").unwrap_err();
        assert!(
            err.contains("已回滚至回收站"),
            "供给失败应整体回滚并提示可重试: {err}"
        );
        assert!(
            !pkg.exists(),
            "回滚后包目录应搬回回收站，bundles/<id>/ 不得残留"
        );
        assert!(
            tmp.join("marketplace/recycle-bin/broken-mcp/mcp/manifest.json")
                .is_file(),
            "回滚后回收站目录应完整"
        );
        let list = RecycleBin::new().list().unwrap();
        assert_eq!(list.len(), 1, "回滚后清单条目应复原（可重试）");
        assert_eq!(list[0].id, "broken-mcp");
        assert!(!list[0].package_missing);
        assert!(
            store.get("broken-mcp").unwrap().is_none(),
            "回滚后登记应移除（与卸载后状态一致）"
        );

        // 修复 manifest 后可从回收站重试并成功（恢复路径全链路可自愈）。
        std::fs::write(
            tmp.join("marketplace/recycle-bin/broken-mcp/mcp/manifest.json"),
            r#"{"id":"broken-mcp","name":"b","description":"d","version":"1","icon":"","category":"c","mcp_tools":[],"command":"python","args":["server.py"]}"#,
        )
        .unwrap();
        restore_plugin("broken-mcp").expect("修复后重试恢复应成功");
        assert!(pkg.join("mcp/manifest.json").is_file());
        assert!(store.get("broken-mcp").unwrap().unwrap().installed);
        assert!(RecycleBin::new().list().unwrap().is_empty());

        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
        bin.recycle_package(
            "exp-pkg",
            KIND_BUNDLE,
            "exp-pkg.zip",
            upload_record("exp-pkg"),
        )
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
        std::io::Read::read_to_string(&mut archive.by_name("plugin.json").unwrap(), &mut content)
            .unwrap();
        assert!(
            content.contains("\"exp-pkg\""),
            "plugin.json 内容应完整: {content}"
        );
        let mut skill = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("skills/exp-skill/SKILL.md").unwrap(),
            &mut skill,
        )
        .unwrap();
        assert!(skill.contains("name: exp-skill"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 回收站导出的 args 净化：旧版本/手改 manifest 落了安装期绝对路径（指向
    /// 原安装位置 bundles/<id>/mcp/），导出必须还原为相对入口脚本名——净化前缀
    /// 按原安装位置匹配（回收站目录前缀会漏匹配，产出导不回的 zip）。
    #[test]
    fn export_package_sanitizes_legacy_absolute_args() {
        let tmp = fresh_dir("export-sanitize");
        let bin = RecycleBin::with_roots(tmp.clone());
        let pkg = tmp.join("bundles/legacy-mcp");
        std::fs::create_dir_all(pkg.join("mcp")).unwrap();
        std::fs::write(pkg.join("mcp/server.py"), b"print('hi')").unwrap();
        let abs_entry = pkg
            .join("mcp/server.py")
            .to_string_lossy()
            .replace('\\', "\\\\");
        std::fs::write(
            pkg.join("mcp/manifest.json"),
            format!(
                r#"{{"id":"legacy-mcp","name":"L","description":"d","version":"1","icon":"","category":"c","mcp_tools":[],"command":"python","args":["{abs_entry}"]}}"#
            ),
        )
        .unwrap();
        bin.recycle_package(
            "legacy-mcp",
            KIND_MCP,
            "legacy-mcp.zip",
            upload_record("legacy-mcp"),
        )
        .unwrap();

        let dest = tmp.join("export.zip");
        bin.export_package("legacy-mcp", &dest).unwrap();

        let archive_file = std::fs::File::open(&dest).unwrap();
        let mut archive = zip::ZipArchive::new(archive_file).unwrap();
        let mut manifest = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("mcp/manifest.json").unwrap(),
            &mut manifest,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(
            value["args"],
            serde_json::json!(["server.py"]),
            "安装期绝对路径应还原为相对入口脚本名: {manifest}"
        );
        // 源文件不被净化修改（导出只读源）
        let on_disk =
            std::fs::read_to_string(tmp.join("recycle-bin/legacy-mcp/mcp/manifest.json")).unwrap();
        assert!(on_disk.contains(&abs_entry));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导出 fail-closed：不在清单的 id 拒绝；清单在、目录缺失（package_missing）报错。
    #[test]
    fn export_unknown_id_and_missing_package_rejected() {
        let tmp = fresh_dir("export-failclosed");
        let bin = RecycleBin::with_roots(tmp.clone());
        let dest = tmp.join("export.zip");

        assert!(
            bin.export_package("ghost", &dest).is_err(),
            "未知 id 应拒绝导出"
        );
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
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

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
        RecycleBin::new()
            .export_package("exp-skill", &dest)
            .unwrap();

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
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 并发读-改-写不丢条目：多线程对同一批 id 同时 recycle/take_back/purge
    /// 竞争（结果不可预期，目标已存在/不在清单等 Err 都合法），终态必须满足
    /// 「清单条目 ⟺ 回收站目录」一一对应、无重复条目、无 id 同时存在于
    /// bundles/ 与 recycle-bin/（无丢失条目、无复活条目、无孤儿目录）。
    #[test]
    fn concurrent_recycle_take_back_purge_stay_consistent() {
        let tmp = fresh_dir("concurrent");
        let bin = std::sync::Arc::new(RecycleBin::with_roots(tmp.clone()));
        let ids: Vec<String> = (0..6).map(|i| format!("pkg-{i}")).collect();
        for id in &ids {
            std::fs::create_dir_all(tmp.join("bundles").join(id)).unwrap();
        }

        let mut handles = Vec::new();
        for tid in 0..6usize {
            let bin = bin.clone();
            let ids = ids.clone();
            handles.push(std::thread::spawn(move || {
                for round in 0..12usize {
                    let id = &ids[(tid + round) % ids.len()];
                    let _ =
                        bin.recycle_package(id, KIND_MCP, &format!("{id}.zip"), upload_record(id));
                    let _ = bin.take_back(id);
                    if round % 3 == 2 {
                        let _ = bin.purge(id);
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // 终态一致性：清单可读（无撕裂写），条目 ⟺ 目录一一对应。
        let file = load_locked(&tmp.join("recycle-bin.json")).unwrap();
        let mut seen = std::collections::HashSet::new();
        for entry in &file.entries {
            assert!(
                seen.insert(entry.id.clone()),
                "清单存在重复条目 {}",
                entry.id
            );
            assert!(
                tmp.join("recycle-bin").join(&entry.id).is_dir(),
                "清单条目 {} 必须有对应回收站目录（条目不得丢失目录）",
                entry.id
            );
            assert!(
                !tmp.join("bundles").join(&entry.id).exists(),
                "{} 不得同时存在于 bundles/ 与 recycle-bin/（复活/双份）",
                entry.id
            );
        }
        let rb = tmp.join("recycle-bin");
        if rb.is_dir() {
            for dir in std::fs::read_dir(&rb).unwrap().flatten() {
                let name = dir.file_name().to_string_lossy().to_string();
                assert!(
                    file.entries.iter().any(|e| e.id == name),
                    "回收站目录 {name} 必须有清单条目（不得有无清单孤儿目录）"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 登记重建失败必须回滚到回收站（评审 #455 R13-B2，与供给失败分支同形）：
    /// take_back 之后 upsert_preserving 失败（此处以不可读的 bundles.json 注入）
    /// 会留下「目录在 bundles_root、无登记、回收站条目已消费」的半恢复态——
    /// 记录驱动的枚举看不到该包，但会话物化按目录扫描，包内技能会以零同意进入
    /// 新 plain 会话，且「重试」必失败。回滚 = 目录搬回 + 清单条目复原。
    #[cfg(unix)]
    #[test]
    fn restore_registration_rebuild_failure_rolls_back_to_bin() {
        use std::os::unix::fs::PermissionsExt;
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let tmp = fresh_dir("restore-upsert-fail");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let pkg = paths::bundles_root().join("my-skill");
        std::fs::create_dir_all(pkg.join("skills/my-skill")).unwrap();
        std::fs::write(
            pkg.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---\n",
        )
        .unwrap();
        let store = BundleStore::new();
        store.upsert(upload_record("my-skill")).unwrap();
        let record = store.get("my-skill").unwrap().unwrap();
        store.remove("my-skill").unwrap();
        RecycleBin::new()
            .recycle_package("my-skill", KIND_SKILL, "my-skill.zip", record)
            .unwrap();

        // 失败注入：bundles.json 存在但不可读 → upsert_preserving 读取即 Err。
        let store_path = tmp.join("marketplace").join("bundles.json");
        std::fs::write(&store_path, "{}").unwrap();
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&store_path).is_ok() {
            std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o644)).unwrap();
            eprintln!(
                "ROOT-SKIP[restore_registration_rebuild_failure_rolls_back_to_bin]: running as root - chmod-000 fixture stays readable; NOT exercised"
            );
            match prev {
                Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }

        let err = restore_plugin("my-skill").unwrap_err();
        assert!(
            err.contains("回滚"),
            "失败必须如实上报已回滚到回收站（重试是真实补救）: {err}"
        );
        assert!(
            !pkg.exists(),
            "包目录必须搬回回收站，不得残留无登记的半恢复态"
        );
        assert!(
            RecycleBin::new()
                .list()
                .unwrap()
                .iter()
                .any(|e| e.id == "my-skill"),
            "回收站条目必须复原"
        );
        assert!(
            store.get("my-skill").unwrap().is_none(),
            "登记重建失败不得留下半写入的记录"
        );

        // 收尾：恢复权限以便清理临时目录。
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
