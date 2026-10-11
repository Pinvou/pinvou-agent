//! BundleStore —— 工具市场统一真相源的可写存储层（Phase 2 第一刀）。
//!
//! 设计依据：`docs/marketplace-unification.md` §3.1（存储层 BundleRecord）、§4（存储
//! 布局）、§9（首启一次性导入）。本模块只管 `~/.pinvou3/marketplace/bundles.json`
//! 的读写与旧布局**登记**，一律不动磁盘上的包内容；物理搬移与旧布局同名残留
//! 清理由 connectors 侧 `migrate_legacy_binary` 按 lock 校验执行。
//!
//! 纪律（§10）：
//! - 原子写（tmp + rename，走底座 `write_atomic`）+ 跨进程文件锁串行化读-改-写
//!   （进程内 Mutex + OS 文件锁，见 `file_lock.rs`，#521）；读/写入口拆「取锁
//!   包装 + 已持锁实现」两层，已持锁的 import/upsert 直接调 `_locked` 实现，
//!   避免锁重入死锁（#287 修过的竞态范式）；
//! - 不用 `#[serde(deny_unknown_fields)]`：未知字段经 `extra` flatten map 原样
//!   roundtrip，新 schema 字段在老版本二进制上不丢数据（前向兼容）。
//! - 损坏 JSON fail loud：bundles.json 是唯一真相源，静默重建会掩盖数据损坏，
//!   损坏时返回 Err 且绝不回写。
//!
// architecture-guard: allow-target-cfg -- the unix regression tests in this file (the round-23 MAJOR 3 legacy-import gate latch and the round-21 planted-FIFO load pin) need an unreadable (0o000) installed.json / FIFO fixtures; test-only inline cfg(unix)+PermissionsExt (same exemption precedent as scope.rs / package_export.rs, review #455); a real read() probe guards against running as root, Windows is covered by link checks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::MarketplaceManager;
use super::bundle;
use super::file_lock;
use crate::platform::connector_lock;
use crate::platform::filesystem::read_private_data_file;
use crate::platform::paths;

/// bundles.json 当前 schema 版本。后续 schema 演进时递增并在读路径做迁移。
const SCHEMA_VERSION: u32 = 1;

/// [`AssetRef::kind`] of a vendor CLI binary: a versioned external asset that
/// a package references but does not own (`docs/marketplace-unification.md`
/// §4, `assets/cli/<name>/<version>/`). Reserved schema constant: no writer
/// records these refs today (the headless CLI deliberately connects without
/// asset pins), so consumers must tolerate an always-empty `assets` list and
/// the first pin writer owns proving its entries against the on-disk files.
pub const ASSET_KIND_CLI: &str = "cli";

/// 上传包的用户自定义 UI 展示名/说明在记录 `extra` map 里的 key（只改展示，
/// 机读 id / 目录 / frontmatter name 一律不动；见 docs/plugin-package-spec.md）。
pub const EXTRA_DISPLAY_NAME: &str = "display_name";
pub const EXTRA_DISPLAY_DESCRIPTION: &str = "display_description";

/// 单技能包**首次回写 SKILL.md 前**留存的 frontmatter 原 description 备份 key
/// （清空展示说明时恢复原值用）。空串哨兵 = 原本没有 description；缺 key =
/// 从未回写过（多技能/纯 MCP 包不回写，也不存备份）。
pub(crate) const EXTRA_SKILL_DESC_BACKUP: &str = "skill_description_backup";

/// 展示名校验上限（字符数）。
pub(crate) const MAX_DISPLAY_NAME_CHARS: usize = 64;
/// 展示说明校验上限（字符数；对齐 skill_marketplace 的 description 展示截断口径）。
pub const MAX_DISPLAY_DESCRIPTION_CHARS: usize = 240;

// `bundles.json` 读-改-写的串行化走 `file_lock::with_file_lock`（统一管线登记、
// 首启导入、后续开关命令都可能并发触发同一份文件的读-改-写；GUI 与 headless
// 宿主共享同一 `~/.pinvou3`，进程内 Mutex 之上再加 OS 文件锁，锁序与失败处置
// 见 `file_lock.rs`，#515/#521）。

// ---------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------

/// 包来源（§3.1）。序列化为字符串：`preset` / `builtin` / `upload:<zip名>`。
/// 未识别的来源字符串反序列化为 `Unknown` 并原样写回 —— 新来源类型在老版本
/// 二进制上不应导致整个真相源读不出来（与 `extra` 字段同一条前向兼容纪律）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleSource {
    /// 市场预置（MCP manifest / 预置技能，随 app 发布）
    Preset,
    /// 内置快照（CLI 连接器等编译期内置能力包）
    Builtin,
    /// 用户上传的 zip 技能包（值为净化后的展示名）
    Upload(String),
    /// 前向兼容：本版本不认识的来源字符串，原样保留
    Unknown(String),
}

impl BundleSource {
    fn parse(raw: &str) -> Self {
        match raw {
            "preset" => Self::Preset,
            "builtin" => Self::Builtin,
            _ => match raw.strip_prefix("upload:") {
                Some(zip) => Self::Upload(zip.to_string()),
                None => Self::Unknown(raw.to_string()),
            },
        }
    }
}

impl std::fmt::Display for BundleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preset => f.write_str("preset"),
            Self::Builtin => f.write_str("builtin"),
            Self::Upload(zip) => write!(f, "upload:{zip}"),
            Self::Unknown(raw) => f.write_str(raw),
        }
    }
}

impl Serialize for BundleSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for BundleSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(deserializer)?))
    }
}

/// External asset reference (`docs/marketplace-unification.md` §3.1: name +
/// version + sha256). `kind` is a string rather than an enum so a kind this
/// binary does not know still parses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    /// Keys this binary does not model round-trip unchanged.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One `BundleRecord.assets` entry. An entry that does not parse as an
/// [`AssetRef`] (a shape written by another version, a missing field, a
/// non-string value) is kept verbatim as `Unrecognized`: it must neither fail
/// the whole `bundles.json` load nor be dropped by the next write. The
/// retained value is endpoint-authored data: render it as data, never as
/// markup, wherever a future consumer surfaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssetEntry {
    Ref(AssetRef),
    Unrecognized(serde_json::Value),
}

/// 存储层包记录（§3.1：bundles.json 里唯一可写的部分）。
/// `ready` 是派生态，永不进存储；`kind` 由查询层现算，同样不落盘。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleRecord {
    pub id: String,
    pub source: BundleSource,
    /// 存储二态：登记 ∧ 资源完整。异常态不另立枚举，登记在、资源缺时置 `degraded`。
    pub installed: bool,
    /// 包内容指纹；本刀导入不算指纹（磁盘遍历只在完整性校验时发生，§4 规则 4），
    /// 由后续完整性校验/统一管线填写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
    /// External asset references (CLI binaries and the like); the package
    /// references them but does not own them. Tolerance is per entry only:
    /// a missing key defaults to empty, but a `null`/non-array value fails
    /// the whole load by the file's fail-loud rule (never silently rebuilt).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetEntry>,
    /// 安装时间，RFC3339/ISO8601 UTC（对齐 SessionMetadata.updated_at 的 chrono 惯例）
    pub installed_at: String,
    /// `Degraded` 异常态（§3.2：登记在、资源缺）的原因；修复动作统一为按来源
    /// 重新获取。None = 资源完整。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    /// 前向兼容：未知字段原样 roundtrip（不用 deny_unknown_fields）。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl BundleRecord {
    /// 安装/连接登记的构造入口：`installed=true`、时间为现在、指纹留空
    /// （磁盘遍历只在完整性校验时发生，§4 规则 4）。调用方按需再填
    /// `degraded`。
    pub fn installed_now(id: impl Into<String>, source: BundleSource) -> Self {
        Self {
            id: id.into(),
            source,
            installed: true,
            assets: Vec::new(),
            content_fingerprint: None,
            installed_at: now_iso8601(),
            degraded: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// bundles.json 顶层结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundlesFile {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    /// 旧布局一次性导入是否已完成（幂等闸，§9）。true 后 `import_legacy` 直接跳过，
    /// 因此重复调用不会覆盖用户/统一管线在 bundles.json 里的既有记录。
    #[serde(default)]
    pub legacy_imported: bool,
    #[serde(default)]
    pub records: Vec<BundleRecord>,
    /// 前向兼容：顶层未知字段原样 roundtrip。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn current_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for BundlesFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            legacy_imported: false,
            records: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

/// 首启导入的结果报告（观测用；迁移决策成对落审计的纪律见 §10.5，调用方落日志）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyImportReport {
    /// true = 此前已完成过导入，本次直接跳过（幂等）
    pub already_imported: bool,
    /// 本次新登记的包 id
    pub imported: Vec<String>,
    /// bundles.json 已存在同 id 记录而被保留（导入永不覆盖既有记录）
    pub kept_existing: Vec<String>,
    /// 登记时资源不完整（如存量 CLI 二进制与 lock 表不符）而记了 degraded 的包 id
    pub degraded: Vec<String>,
}

// ---------------------------------------------------------------------------
// BundleStore
// ---------------------------------------------------------------------------

pub struct BundleStore {
    file: PathBuf,
}

impl Default for BundleStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BundleStore {
    pub fn new() -> Self {
        Self {
            file: paths::pinvou3_home()
                .join("marketplace")
                .join("bundles.json"),
        }
    }

    /// 测试用：显式指定存储文件（调用方不走 PINVOU3_HOME 环境变量时避免碰真实家目录）。
    #[cfg(test)]
    pub(crate) fn with_file(file: PathBuf) -> Self {
        Self { file }
    }

    pub fn file_path(&self) -> PathBuf {
        self.file.clone()
    }

    /// 读整个文件（try 锁包装：锁被占用/不可用时退化为不落盘的解锁快照，见
    /// `file_lock::try_file_lock_for_read`）。文件不存在 → 空 store；JSON 损坏
    /// → Err（fail loud）。
    pub fn load(&self) -> Result<BundlesFile, String> {
        file_lock::try_file_lock_for_read(&self.file, || load_locked(&self.file))
    }

    /// 全部记录（便捷入口）。
    pub fn records(&self) -> Result<Vec<BundleRecord>, String> {
        Ok(self.load()?.records)
    }

    /// 按 id 查单条记录。
    pub fn get(&self, id: &str) -> Result<Option<BundleRecord>, String> {
        Ok(self.load()?.records.into_iter().find(|r| r.id == id))
    }

    /// 插入或按 id 替换一条记录（读-改-写全程持跨进程文件锁，原子落盘）。
    pub fn upsert(&self, record: BundleRecord) -> Result<(), String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            match file.records.iter_mut().find(|r| r.id == record.id) {
                Some(existing) => *existing = record,
                None => file.records.push(record),
            }
            save_locked(&self.file, &file)
        })
    }

    /// upsert 变体：记录已存在时保留 `source`（包来源只在首次登记时确定 —— 重装/
    /// 重复连接的镜像写一律带 Preset，若覆盖会把 Upload 翻成 Preset，下次卸载
    /// 误删用户唯一副本，四轮评审 BLOCKER 1）、`installed_at`（首次登记时间）、
    /// `extra`（前向兼容/用户字段）与 `content_fingerprint`（完整性校验层的数据），
    /// 其余字段以新值为准。安装/连接成功的镜像写统一走这里 —— 重装、重复连接
    /// 不应冲掉首次安装时间，也不应丢老版本二进制不认识的字段。
    /// `assets` takes the new list when it is non-empty and otherwise keeps the
    /// existing one, so a writer that does not track assets must not wipe refs
    /// another writer recorded while a pin writer can still replace a stale
    /// pin. The flip side: "unpin all" is not expressible through this
    /// variant — a writer that genuinely wants an empty list must fall to
    /// plain `upsert`, which replaces the whole record. No code records pins
    /// yet; the rule exists so the first pin writer cannot silently wipe, and
    /// its protection is per-process: each process merges against its own
    /// load-time snapshot, so cross-process writers remain last-writer-wins
    /// on the whole file.
    pub fn upsert_preserving(&self, record: BundleRecord) -> Result<(), String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let merged = match file.records.iter().find(|r| r.id == record.id) {
                Some(existing) => BundleRecord {
                    id: record.id,
                    source: existing.source.clone(),
                    installed: record.installed,
                    assets: if record.assets.is_empty() {
                        existing.assets.clone()
                    } else {
                        record.assets
                    },
                    content_fingerprint: record
                        .content_fingerprint
                        .or_else(|| existing.content_fingerprint.clone()),
                    installed_at: existing.installed_at.clone(),
                    degraded: record.degraded,
                    extra: existing.extra.clone(),
                },
                None => record,
            };
            match file.records.iter_mut().find(|r| r.id == merged.id) {
                Some(slot) => *slot = merged,
                None => file.records.push(merged),
            }
            save_locked(&self.file, &file)
        })
    }

    /// 仅当记录仍存在时更新内容指纹（单锁 RMW，原子落盘）。id 不存在 →
    /// Ok(false) 不写盘——「读记录 → upsert_preserving 补写」的两段式在并发
    /// 卸载下会把已删除记录按 stale 快照复活（upsert_preserving 对不存在的
    /// id 直接插入），指纹补写必须走本方法而非两段式。
    pub fn update_content_fingerprint_if_exists(
        &self,
        id: &str,
        fingerprint: &str,
    ) -> Result<bool, String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
                return Ok(false);
            };
            record.content_fingerprint = Some(fingerprint.to_string());
            save_locked(&self.file, &file)?;
            Ok(true)
        })
    }

    /// 按 id 删除记录（读-改-写全程持跨进程文件锁，原子落盘）。id 不存在 →
    /// Ok(false)，不写盘。
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let before = file.records.len();
            file.records.retain(|r| r.id != id);
            if file.records.len() == before {
                return Ok(false);
            }
            save_locked(&self.file, &file)?;
            Ok(true)
        })
    }

    /// 局部更新：置 `Degraded` 原因（§3.2：登记在、资源缺），供 CLI 修复/断开
    /// 路径用。id 不存在 → Ok(false)；原因未变 → Ok(true) 但不写盘。
    pub fn mark_degraded(&self, id: &str, reason: &str) -> Result<bool, String> {
        self.set_degraded(id, reason.to_string())
    }

    fn set_degraded(&self, id: &str, reason: String) -> Result<bool, String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
                return Ok(false);
            };
            if record.degraded.as_deref() == Some(reason.as_str()) {
                return Ok(true);
            }
            record.degraded = Some(reason);
            save_locked(&self.file, &file)?;
            Ok(true)
        })
    }

    /// 设置上传包的用户自定义 UI 展示名/说明（写在记录 `extra` map 的
    /// `display_name` / `display_description` key，不动包目录与包清单）。
    ///
    /// - 记录不存在 → Err；`source` 非 Upload → Err（预置/内置包不可覆盖）；
    /// - `Some(v)`：trim 后为空 = 删除该 key（清空回退默认展示）；非空 = 长度/
    ///   控制字符校验后写入 trim 值，违规 → Err；
    /// - `None` = 该字段不动；两个都 None = no-op 成功（仍要求记录存在且为 Upload）。
    pub fn set_display_meta(
        &self,
        id: &str,
        display_name: Option<&str>,
        display_description: Option<&str>,
    ) -> Result<(), String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
                return Err(format!("包 '{id}' 未登记，无法设置展示名/说明"));
            };
            if !matches!(record.source, BundleSource::Upload(_)) {
                return Err(format!(
                    "包 '{id}' 非用户上传来源，预置/内置包不允许覆盖展示名/说明"
                ));
            }
            if display_name.is_none() && display_description.is_none() {
                return Ok(());
            }
            apply_display_meta(
                &mut record.extra,
                EXTRA_DISPLAY_NAME,
                "展示名",
                display_name,
                MAX_DISPLAY_NAME_CHARS,
            )?;
            apply_display_meta(
                &mut record.extra,
                EXTRA_DISPLAY_DESCRIPTION,
                "展示说明",
                display_description,
                MAX_DISPLAY_DESCRIPTION_CHARS,
            )?;
            save_locked(&self.file, &file)
        })
    }

    /// 读取记录的 SKILL.md 原说明备份（`EXTRA_SKILL_DESC_BACKUP`；缺 key /
    /// 非字符串 → None；`Some("")` = 原缺失哨兵）。清空展示说明的恢复路径用。
    pub fn skill_desc_backup(&self, id: &str) -> Result<Option<String>, String> {
        file_lock::try_file_lock_for_read(&self.file, || {
            let file = load_locked(&self.file)?;
            Ok(file
                .records
                .iter()
                .find(|r| r.id == id)
                .and_then(|r| r.extra.get(EXTRA_SKILL_DESC_BACKUP))
                .and_then(|v| v.as_str())
                .map(str::to_string))
        })
    }

    /// 设置/删除 `EXTRA_SKILL_DESC_BACKUP`（与 `set_display_meta` 同锁同门禁：
    /// 仅 Upload 记录可写）。值由内部读取/校验管线产生（引擎口径原值或空串哨兵），
    /// 不做展示字段校验；`None` = 删除 key。
    pub fn set_skill_desc_backup(&self, id: &str, backup: Option<&str>) -> Result<(), String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
                return Err(format!("包 '{id}' 未登记，无法备份技能说明原值"));
            };
            if !matches!(record.source, BundleSource::Upload(_)) {
                return Err(format!("包 '{id}' 非用户上传来源，不允许写说明备份"));
            }
            match backup {
                Some(v) => {
                    record.extra.insert(
                        EXTRA_SKILL_DESC_BACKUP.to_string(),
                        serde_json::Value::String(v.to_string()),
                    );
                }
                None => {
                    record.extra.remove(EXTRA_SKILL_DESC_BACKUP);
                }
            }
            save_locked(&self.file, &file)
        })
    }

    /// 首启一次性导入（§9）：从旧布局（installed.json + bundle/skills/* 的
    /// `.installed-from` 标记 + `connectors/<platform>/bin/` 存量 CLI 二进制）反推
    /// 已装包，登记进 bundles.json。
    ///
    /// - **幂等**：`legacy_imported` 闸置位后直接跳过；闸未置位时也只补缺失 id，
    ///   已存在的记录永远保留（用户/新管线写入的赢）。
    /// - **非破坏性**：只读旧布局、只写 bundles.json；目录搬移与同名残留清理由
    ///   connectors 侧 `migrate_legacy_binary` 按 lock 校验执行（boot 序列在
    ///   本 import 之后；#608）。
    /// - **注册表不可读 = 不导入、不落闸**（review #455 round-23 MAJOR 3）：
    ///   installed.json 存在但不可读时整体报错返回，`legacy_imported` 不置位——
    ///   宁可下次启动重试，也不把一次吞错读取得来的不完整镜像永久烘焙进
    ///   bundles.json（与 round-20 MAJOR A 的 reconcile 转换同类）。
    /// - 全程持跨进程文件锁（"读到即迁移"必须持锁，§9.4 / #287 竞态教训前置）。
    pub fn import_legacy(&self) -> Result<LegacyImportReport, String> {
        file_lock::with_file_lock(&self.file, || {
            let mut file = load_locked(&self.file)?;
            let mut report = LegacyImportReport::default();
            if file.legacy_imported {
                report.already_imported = true;
                return Ok(report);
            }
            for candidate in collect_legacy_records()? {
                if file.records.iter().any(|r| r.id == candidate.id) {
                    report.kept_existing.push(candidate.id);
                    continue;
                }
                if candidate.degraded.is_some() {
                    report.degraded.push(candidate.id.clone());
                }
                report.imported.push(candidate.id.clone());
                file.records.push(candidate);
            }
            file.legacy_imported = true;
            save_locked(&self.file, &file)?;
            Ok(report)
        })
    }
}

// ---------------------------------------------------------------------------
// 已持锁实现（取锁包装之上的内层；调用前必须已持有 bundles.json 的跨进程文件锁）
// ---------------------------------------------------------------------------

/// 单个展示字段的写入语义（`set_display_meta` 用）：None 不动；trim 后空 = 删 key
/// （回退默认展示）；非空走 [`check_display_value`] 校验后写 trim 值。
fn apply_display_meta(
    extra: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    field_label: &str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<(), String> {
    let Some(v) = value else {
        return Ok(());
    };
    let trimmed = v.trim();
    if trimmed.is_empty() {
        extra.remove(key);
        return Ok(());
    }
    check_display_value(field_label, trimmed, max_chars)?;
    extra.insert(
        key.to_string(),
        serde_json::Value::String(trimmed.to_string()),
    );
    Ok(())
}

/// 展示字段值的统一校验（写入与预检共用同一口径）：控制字符/换行/不可见字符
/// （bidi 控制、零宽、BOM、行/段分隔符等，见 [`is_display_unsafe_char`]）一律
/// 拒绝（单行 UI 展示与 SKILL.md 单行回写都无法表达，或可视觉欺骗，此条各包
/// 形态行为一致），超过字符上限 → Err。`field_label` 用人类可读字段名，不外泄
/// 内部 key。
///
/// 注意：单技能包的 SKILL.md 同步在回写时另有更严的单行互洽限制（拒双引号/
/// 反斜杠/首尾单引号，见 `rewrite_frontmatter_description`）——同一值在非
/// 单技能包可存、在单技能包会被整体拒绝，这是同步能力差异使然，不是本函数的
/// 口径不一致。
/// 展示字段值的不可见字符拒绝集：`Cc`（控制字符/换行）之外，补充对卡片标题/
/// composer 菜单/SKILL.md 回写都有实际危害或显示异常的不可见字符——
/// - 双向控制（U+202A–202E、U+2066–2069）：可视觉重排/隐藏文本（bidi 欺骗）；
/// - 零宽（U+200B–200D）、BOM（U+FEFF）、软连字符（U+00AD）：不可见但参与
///   比较/搜索/指纹，还会被原样写进 SKILL.md；
/// - 行/段分隔符（U+2028/2029，`Zl`/`Zp`）：单行 UI 与单行 frontmatter 都
///   无法表达，效果等同换行。
/// std 无通用 Unicode 类别 API，此处按具名范围精确拒绝；未列出的 Cf 变体
/// （如变体选择符）不拦——宁可少拦可见字符，不误伤正常文案。
fn is_display_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{00AD}' // SOFT HYPHEN
            | '\u{200B}'..='\u{200D}' // ZERO WIDTH SPACE..JOINER
            | '\u{2028}'..='\u{2029}' // LINE/PARAGRAPH SEPARATOR
            | '\u{202A}'..='\u{202E}' // bidi embedding/override controls
            | '\u{2066}'..='\u{2069}' // bidi isolate controls
            | '\u{FEFF}' // BOM / ZERO WIDTH NO-BREAK SPACE
        )
}

fn check_display_value(field_label: &str, v: &str, max_chars: usize) -> Result<(), String> {
    if v.chars().any(is_display_unsafe_char) {
        return Err(format!("{field_label}含控制字符/不可见字符/换行，不支持"));
    }
    if v.chars().count() > max_chars {
        return Err(format!("{field_label}超过 {max_chars} 字符上限"));
    }
    Ok(())
}

/// 展示名/说明的长度/字符预检（`sync_display_meta` 在回写 SKILL.md **之前**
/// 调用；`apply_display_meta` 内仍有同口径校验兜底）。回写会先改包内容并重算
/// 指纹，校验若只留在 `set_display_meta`，超长值会先落盘再报错，留下
/// 「报错但包内容已变」的中间态——所以必须前置预检。None / trim 后空
/// （= 清覆盖）不检；非空走 [`check_display_value`]。
pub(crate) fn validate_display_meta(
    display_name: Option<&str>,
    display_description: Option<&str>,
) -> Result<(), String> {
    for (field_label, value, max_chars) in [
        ("展示名", display_name, MAX_DISPLAY_NAME_CHARS),
        (
            "展示说明",
            display_description,
            MAX_DISPLAY_DESCRIPTION_CHARS,
        ),
    ] {
        if let Some(v) = value.map(str::trim).filter(|s| !s.is_empty()) {
            check_display_value(field_label, v, max_chars)?;
        }
    }
    Ok(())
}

/// 上传来源的人类可读展示名回退：`Upload` 记录携带的原始文件名去扩展名
/// （导入时已净化捕获，见命令层 safe_name/display）。上传包未设展示覆盖时
/// 卡片标题回退到它，避免直接露出机读 id；扩展名剥空（如 ".zip"）或非
/// Upload 来源 = None，调用方继续回退记录 id / manifest name。
pub(crate) fn upload_display_fallback(record: &BundleRecord) -> Option<String> {
    match &record.source {
        BundleSource::Upload(filename) => {
            let stem = filename
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(filename)
                .trim();
            if stem.is_empty() {
                None
            } else {
                Some(stem.to_string())
            }
        }
        _ => None,
    }
}

/// 读记录的用户自定义展示字段（trim 后非空才生效；缺 key/空串/非字符串 = None，
/// 调用方回退默认展示）。list 组装与 BundleInfo 组装共用此口径。
pub(crate) fn display_override(record: &BundleRecord, key: &str) -> Option<String> {
    record
        .extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads an Upload record's user-defined display name/description overrides in one pass (bundles.json extra's
/// `display_name`/`display_description`, effective only when non-empty after trim, see
/// [`display_override`]). Non-Upload records always get `(None, None)` — preset/built-in packages do not accept
/// overrides, and extra fields written out of turn must be ignored by the display layer. All three list assemblies
/// (`list_tools` / `list_bundles` / `list_skills`) share this same read, so the tool card,
/// readiness card, and skill card titles/descriptions do not diverge.
pub(crate) fn apply_display_override(
    record: &BundleRecord,
    name: Option<String>,
    description: Option<String>,
) -> (Option<String>, Option<String>) {
    if !matches!(record.source, BundleSource::Upload(_)) {
        return (name, description);
    }
    (
        display_override(record, EXTRA_DISPLAY_NAME).or(name),
        display_override(record, EXTRA_DISPLAY_DESCRIPTION).or(description),
    )
}

/// The original zip display name carried by an Upload record (a sanitized source marker captured at import); non-Upload
/// sources fall back to `fallback` (the caller passes the package id). The three recycle call sites of the whole-package
/// recycle listing share this same display_name policy.
pub(crate) fn upload_display_name(record: &BundleRecord, fallback: &str) -> String {
    match &record.source {
        BundleSource::Upload(zip) => zip.clone(),
        _ => fallback.to_string(),
    }
}

/// Inner read: separated from the lock-taking wrapper so already-locked
/// import/upsert callers invoke it directly, avoiding mutex re-entry. The
/// read goes through the hardened primitive (round-21 review): this function
/// runs inside the bundles.json file lock and — via the scope's DenyAll
/// expansion — inside the disabled_bundles.lock critical section, so a FIFO
/// or symlink planted at the path must be refused rather than left hanging
/// the lock holder — the same hardening discipline as disabled_bundles.json's
/// identical reads. NotFound semantics unchanged (the kind passes through).
fn load_locked(path: &Path) -> Result<BundlesFile, String> {
    // Hardened read (round-20 P2): bundles.json is private-home consent state
    // read inside the scope critical section (DenyAll expansion) and under
    // the transaction lock — the open must not hang on a planted FIFO.
    match read_private_data_file(path) {
        Ok(content) => {
            let file: BundlesFile = serde_json::from_str(&content).map_err(|e| {
                format!(
                    "解析 {} 失败: {e}（bundles.json 是唯一真相源，损坏时 fail loud，不静默重建）",
                    path.display()
                )
            })?;
            warn_unrecognized_assets(path, &file);
            Ok(file)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BundlesFile::default()),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

/// Unparsable `assets` entries are kept verbatim per the `AssetEntry`
/// contract, but a silent degrade hides data-quality problems (a `sha_256`
/// misspelling from a future pin writer, for example). Report them once per
/// load so the anomaly is diagnosable instead of just invisible.
fn warn_unrecognized_assets(path: &Path, file: &BundlesFile) {
    let flagged = unrecognized_asset_record_ids(file);
    if !flagged.is_empty() {
        log::warn!(
            "[marketplace] {}: unreadable assets entries kept verbatim in {}",
            path.display(),
            flagged.join(", ")
        );
    }
}

/// Record ids carrying at least one entry this binary cannot read; split
/// from `warn_unrecognized_assets` so the targeting rule is testable
/// without capturing log output.
fn unrecognized_asset_record_ids(file: &BundlesFile) -> Vec<String> {
    file.records
        .iter()
        .filter(|record| {
            record
                .assets
                .iter()
                .any(|entry| !matches!(entry, AssetEntry::Ref(_)))
        })
        .map(|record| record.id.clone())
        .collect()
}

/// 内层写：tmp + rename 原子替换（底座 `write_atomic`，含 Windows 替换重试），
/// 并发读者不会看到半写文件。
fn save_locked(path: &Path, file: &BundlesFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(file).map_err(|e| format!("序列化 bundles.json 失败: {e}"))?;
    crate::platform::filesystem::atomic_write_private(path, json.as_bytes())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 旧布局反推（只读旧文件，不动包内容）
// ---------------------------------------------------------------------------

/// Install/recycle timestamps: RFC3339/ISO8601 UTC, following the same chrono convention
/// as SessionMetadata.updated_at. recycle_bin.rs reuses this implementation so the two
/// timestamp formats cannot drift apart.
pub(super) fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 旧布局扫描 → 预置包记录。MCP 安装态读走 `try_installed_ids`（review #455
/// round-23 MAJOR 3）：installed.json 存在但不可读时整体报错，调用方据此跳过
/// 本次导入且不落 `legacy_imported` 闸，避免把不完整镜像永久烘焙进 bundles.json。
fn collect_legacy_records() -> Result<Vec<BundleRecord>, String> {
    let mut out = legacy_mcp_records()?;
    // Round-24 minor 2: the skill/CLI scan legs propagate a scan-root read
    // error the same way the registry leg does — a degraded scan latching the
    // one-shot gate would bake an incomplete mirror in permanently (the same
    // class the registry conversion closed). A missing legacy dir stays the
    // common Ok(empty) case; only an existing-but-unreadable root errors.
    out.extend(legacy_skill_records()?);
    out.extend(legacy_cli_records());
    // id 去重（保序留先）：MCP 包与其同名 companion 技能（pptx↔pptx）会各扫到一次，
    // 终态模型里它们是同一个包（§5.2「一个包 = 一张卡」），MCP 侧记录含凭据声明，
    // 信息更全，故排在前面的 MCP 记录优先。
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| seen.insert(r.id.clone()));
    Ok(out)
}

/// installed.json（MCP 安装态）→ 预置包记录。读失败报 Err（不塌缩为空集）。
fn legacy_mcp_records() -> Result<Vec<BundleRecord>, String> {
    let manager = MarketplaceManager::new();
    let installed = manager.try_installed_ids()?;
    if installed.is_empty() {
        return Ok(Vec::new());
    }
    let now = now_iso8601();
    Ok(installed
        .into_iter()
        .map(|id| BundleRecord {
            id,
            source: BundleSource::Preset,
            installed: true,
            assets: Vec::new(),
            content_fingerprint: None,
            installed_at: now.clone(),
            degraded: None,
            extra: serde_json::Map::new(),
        })
        .collect())
}

/// `bundle/skills/*/.installed-from` 标记 → 技能包记录：
/// `pinvou3-marketplace:<id>` → 预置技能包；`upload:<zip名>` → 上传技能包。
/// 无标记目录（内置 visual-design、CLI companion 技能）不在此登记 —— CLI
/// companion 由 [`legacy_cli_records`] 归并到所属 CLI 包。
fn legacy_skill_records() -> Result<Vec<BundleRecord>, String> {
    let skills_dir = paths::bundle_skills_dir();
    // Round-24 minor 2: a missing legacy dir is the common fresh case; an
    // existing-but-unreadable root propagates so the import aborts before the
    // gate latches (same fail-closed as the registry leg).
    let rd = match std::fs::read_dir(&skills_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取旧布局技能目录失败: {e}")),
    };
    let now = now_iso8601();
    let mut out = Vec::new();
    // Round-30 m6 (review #455): `flatten()` silently drops mid-scan
    // iteration errors — the one-shot `legacy_imported` gate could latch
    // over an incomplete mirror (records permanently skipped). Propagate
    // like the root read above and the per-entry markers below: the last
    // leg of the round-23 MAJOR-3 class in this function.
    for entry in rd {
        let entry = entry.map_err(|e| format!("遍历旧布局技能目录失败: {e}"))?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // Round-29 m4 (review #455): NotFound is the common no-marker skip,
        // but an EXISTS-but-unreadable marker must abort the import exactly
        // like the unreadable root above — the one-shot `legacy_imported`
        // gate must not latch over an incomplete mirror (round-23 MAJOR-3 /
        // round-24 m2 class, per-entry form).
        let marker_path = dir.join(".installed-from");
        if marker_path.is_dir() {
            // Round-27 review (NIT): a planted DIRECTORY at the marker path
            // fell through the is_file gate into the "no marker" skip below,
            // silently latching the one-shot legacy_imported gate over a
            // skipped mirror — the pre-round-20 shape failed closed with
            // EISDIR, so restore that. Only the FIFO case (where open itself
            // would hang) deserves the skip treatment.
            return Err(format!(
                "读取 {} 失败: 是目录而非标注文件",
                marker_path.display()
            ));
        }
        let marker = if marker_path.is_file() {
            // Hardened read (round-20 P2): a non-regular entry (planted FIFO)
            // now reads as "no marker" (skip) instead of hanging the boot
            // import; an unreadable regular file still aborts fail-closed.
            match read_private_data_file(&marker_path) {
                Ok(marker) => marker,
                Err(e) => return Err(format!("读取 {} 失败: {e}", marker_path.display())),
            }
        } else {
            continue;
        };
        let marker = marker.trim();
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let (id, source) = if let Some(sid) = marker.strip_prefix("pinvou3-marketplace:") {
            (sid.trim().to_string(), BundleSource::Preset)
        } else if let Some(zip) = marker.strip_prefix("upload:") {
            // 上传技能的包 id 即落盘目录名（= SKILL.md frontmatter name）
            (dir_name, BundleSource::Upload(zip.trim().to_string()))
        } else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        out.push(BundleRecord {
            id,
            source,
            installed: true,
            assets: Vec::new(),
            content_fingerprint: None,
            installed_at: now.clone(),
            degraded: None,
            extra: serde_json::Map::new(),
        });
    }
    Ok(out)
}

/// 内置 CLI 连接器 → Builtin 包记录。安装态判定：companion 技能目录在盘
/// （连接时才解包）或 CLI 二进制在盘。存量二进制对照 lock 表验 SHA-256：
/// 匹配 → 正常登记；不匹配/无法校验 → 记 `degraded`（§9.3，
/// 修复动作 = 重新下载；旧布局搬移与同名残留清理由 connectors 侧
/// `migrate_legacy_binary` 按 lock 校验执行（#608））。
/// CLI 腿只做 `is_file` 存在性探测（无 read_dir 根可失败）——#608 移除了
/// 仅为与另两腿签名对称而保留的空壳 `Result`（round-24 minor 2 的对称性
/// 理由随空壳一并失效）；另两腿（registry / skill 扫描）保留 `Result`，
/// 其 fail-closed 传播是 review #455 round-23 MAJOR 3 / round-29 m4 /
/// round-30 m6 的契约。
fn legacy_cli_records() -> Vec<BundleRecord> {
    let skills_dir = paths::bundle_skills_dir();
    let now = now_iso8601();
    let mut out = Vec::new();
    for id in bundle::builtin_cli_bundle_ids() {
        let skills_present = bundle::cli_bundle_skill_dirs(id)
            .iter()
            .any(|d| skills_dir.join(d).join("SKILL.md").is_file());
        let mut binary_verified = false;
        let mut degraded = None;
        if let Some(bin) = bundle::cli_bundle_bin(id) {
            match cli_asset_state(bin) {
                CliAssetState::Verified => binary_verified = true,
                CliAssetState::Mismatch(reason) => degraded = Some(reason),
                CliAssetState::Absent => {}
            }
        }
        // 技能目录与二进制都不在盘 = 未连接过，不登记
        if !skills_present && !binary_verified && degraded.is_none() {
            continue;
        }
        out.push(BundleRecord {
            id: id.to_string(),
            source: BundleSource::Builtin,
            installed: true,
            assets: Vec::new(),
            content_fingerprint: None,
            installed_at: now.clone(),
            degraded,
            extra: serde_json::Map::new(),
        });
    }
    out
}

enum CliAssetState {
    Verified,
    Mismatch(String),
    Absent,
}

fn cli_asset_state(bin: &str) -> CliAssetState {
    // 路径经单点入口解析：版本化资产库（已迁移/新装）优先，旧布局
    // （connectors/<platform>/bin/，首启导入时迁移尚未跑）其次。
    let path = connector_lock::locked_cli_path(bin)
        .filter(|p| p.is_file())
        .or_else(|| {
            paths::managed_connector_bin_dir()
                .map(|dir| dir.join(connector_lock::executable_name(bin)))
                .filter(|p| p.is_file())
        });
    let Some(path) = path else {
        return CliAssetState::Absent;
    };
    let Some(pin) = connector_lock::artifact_pin(bin) else {
        return CliAssetState::Mismatch(format!(
            "lock 表无 {bin} 条目，存量二进制无法校验，待重新下载"
        ));
    };
    match crate::platform::hashing::sha256_file(&path) {
        Ok(actual) if actual == pin.binary_sha256 => CliAssetState::Verified,
        Ok(actual) => CliAssetState::Mismatch(format!(
            "CLI 二进制 SHA-256 与 lock 表不符（expected {}, got {actual}），待重新下载",
            pin.binary_sha256
        )),
        Err(e) => CliAssetState::Mismatch(format!("读取 CLI 二进制失败: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::test_support::with_temp_home;

    fn record(id: &str, source: BundleSource) -> BundleRecord {
        BundleRecord {
            id: id.to_string(),
            source,
            installed: true,
            assets: Vec::new(),
            content_fingerprint: None,
            installed_at: "2026-08-14T00:00:00+00:00".to_string(),
            degraded: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_store_loads_missing_file_as_empty() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let file = store.load().unwrap();
            assert!(file.records.is_empty());
            assert!(!file.legacy_imported);
            assert!(!store.file_path().exists(), "纯读不得创建 bundles.json");
        });
    }

    #[test]
    fn save_load_roundtrip_preserves_all_fields() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let mut full = record("feishu", BundleSource::Builtin);
            full.content_fingerprint = Some("abc123".to_string());
            full.degraded = Some("reason".to_string());
            store.upsert(full.clone()).unwrap();
            store
                .upsert(record(
                    "my-skill",
                    BundleSource::Upload("pkg.zip".to_string()),
                ))
                .unwrap();

            let file = store.load().unwrap();
            assert_eq!(file.records.len(), 2);
            assert_eq!(file.records[0], full);
            assert_eq!(
                file.records[1].source,
                BundleSource::Upload("pkg.zip".to_string())
            );
            // 序列化形态：snake_case 字段名 + source 的字符串形式
            let content = std::fs::read_to_string(store.file_path()).unwrap();
            assert!(content.contains("\"content_fingerprint\""), "{content}");
            assert!(content.contains("\"upload:pkg.zip\""), "{content}");
        });
    }

    #[test]
    fn atomic_write_leaves_no_tmp_files() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store.upsert(record("a", BundleSource::Preset)).unwrap();
            let dir = paths::pinvou3_home().join("marketplace");
            let mut entries: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            entries.sort();
            assert_eq!(
                entries,
                // bundles.lock is the cross-process file lock (#521), not a
                // leftover tmp: the assertion's job is that no tmp residue
                // survives an atomic write.
                vec!["bundles.json".to_string(), "bundles.lock".to_string()],
                "原子写不得残留 tmp 文件"
            );
        });
    }

    /// 前向兼容：顶层与记录级的未知字段、未知来源字符串都要无损 roundtrip
    /// （老版本二进制读写新 schema 文件不丢数据）。
    #[test]
    fn unknown_keys_survive_roundtrip() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let path = store.file_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{
  "schema_version": 1,
  "legacy_imported": true,
  "future_top_level": {"nested": 1},
  "records": [
    {"id": "weather", "source": "preset", "installed": true,
     "installed_at": "2026-08-14T00:00:00Z", "future_record_key": [1, 2, 3]},
    {"id": "future", "source": "quantum:x", "installed": false,
     "installed_at": "2026-08-14T00:00:00Z"}
  ]
}"#,
            )
            .unwrap();

            let file = store.load().unwrap();
            assert_eq!(
                file.records[0].extra.get("future_record_key"),
                Some(&serde_json::json!([1, 2, 3]))
            );
            assert_eq!(
                file.records[1].source,
                BundleSource::Unknown("quantum:x".to_string())
            );

            // 触发一次读-改-写后，未知内容仍在盘上
            store
                .upsert(record("visualizer", BundleSource::Preset))
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(value["future_top_level"], serde_json::json!({"nested": 1}));
            assert_eq!(
                value["records"][0]["future_record_key"],
                serde_json::json!([1, 2, 3])
            );
            assert_eq!(value["records"][1]["source"], "quantum:x");
        });
    }

    /// 损坏 JSON 必须 fail loud：报错且绝不回写（真相源不静默重建）。
    #[test]
    fn corrupt_json_fails_loud_without_overwrite() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let path = store.file_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "not-json{{{").unwrap();

            assert!(store.load().is_err(), "损坏 JSON 读取应报错");
            assert!(store.import_legacy().is_err(), "损坏 JSON 导入应报错");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "not-json{{{",
                "损坏文件不得被静默覆盖"
            );
        });
    }

    /// Round-23 MAJOR 3: an unreadable installed.json must fail the legacy
    /// import WITHOUT latching the one-shot `legacy_imported` gate and without
    /// writing bundles.json — a swallowed read here would bake an incomplete
    /// mirror into the store permanently (no later boot can repair it). After
    /// the permissions are restored, the same call imports and latches.
    #[cfg(unix)]
    #[test]
    fn legacy_import_unreadable_registry_fails_without_latching_gate() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let home = paths::pinvou3_home();
            let installed_path = home.join("marketplace/installed.json");
            std::fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
            std::fs::write(&installed_path, r#"["gongwen"]"#).unwrap();
            std::fs::set_permissions(&installed_path, std::fs::Permissions::from_mode(0o000))
                .unwrap();
            // Root probe (mode bits are no-ops for root): if the file is still
            // readable, the unreadable-read branch never runs — skip loudly.
            if std::fs::read(&installed_path).is_ok() {
                std::fs::set_permissions(&installed_path, std::fs::Permissions::from_mode(0o644))
                    .unwrap();
                eprintln!(
                    "ROOT-SKIP[legacy_import_unreadable_registry_fails_without_latching_gate]: running as root - the unreadable-file fixture stays readable; NOT exercised"
                );
                return;
            }

            assert!(
                store.import_legacy().is_err(),
                "an unreadable registry must fail the import (fail loud, not swallow)"
            );
            let file = store.load().unwrap();
            assert!(
                !file.legacy_imported,
                "the one-shot gate must stay unset so a later boot retries the import"
            );
            assert!(
                file.records.is_empty(),
                "no incomplete mirror may be persisted off a failed read"
            );

            // Permissions restored: the same call now imports and latches.
            std::fs::set_permissions(&installed_path, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            let report = store.import_legacy().unwrap();
            assert_eq!(report.imported, vec!["gongwen".to_string()]);
            let file = store.load().unwrap();
            assert!(
                file.legacy_imported,
                "the successful import latches the gate"
            );
        });
    }

    /// 首启导入：MCP（installed.json + manifest 凭据）、预置/上传技能标记、
    /// CLI 连接器（companion 技能在盘 + 存量二进制对照 lock 表）全部归位。
    #[test]
    fn import_legacy_collects_mcp_skills_and_cli() {
        with_temp_home("pinvou3-store-test", || {
            let home = paths::pinvou3_home();
            let write = |rel: &str, content: &str| {
                let p = home.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, content).unwrap();
            };
            write("marketplace/installed.json", r#"["gongwen"]"#);
            // MCP manifest 已迁按包聚合新布局：`legacy_mcp_records` 经
            // try_installed_ids() 读登记 id（manifest 本体现在只影响 kind/凭据查询，
            // 不进记录）。
            write(
                "bundles/gongwen/mcp/manifest.json",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"","category":"office","mcp_tools":[],"command":"","args":[],"config_fields":[{"key":"GONGWEN_KEY","label":"k","required":true}]}"#,
            );
            write(
                "bundle/skills/government-writing/SKILL.md",
                "---\nname: government-writing\n---\n",
            );
            write(
                "bundle/skills/government-writing/.installed-from",
                "pinvou3-marketplace:government-writing",
            );
            write(
                "bundle/skills/my-upload/SKILL.md",
                "---\nname: my-upload\n---\n",
            );
            write("bundle/skills/my-upload/.installed-from", "upload:pkg.zip");
            // CLI companion 技能无标记：目录在盘即「连接过」
            write(
                "bundle/skills/lark-shared/SKILL.md",
                "---\nname: lark-shared\n---\n",
            );
            // 存量 CLI 二进制：内容对不上 lock 表 → degraded（而非 assets 引用）
            let cli_verifiable = paths::managed_connector_bin_dir().is_some()
                && connector_lock::artifact_pin("lark-cli").is_some();
            if let Some(bin_dir) = paths::managed_connector_bin_dir() {
                std::fs::create_dir_all(&bin_dir).unwrap();
                std::fs::write(
                    bin_dir.join(connector_lock::executable_name("lark-cli")),
                    b"fake-binary",
                )
                .unwrap();
            }

            let store = BundleStore::new();
            let report = store.import_legacy().unwrap();
            assert!(!report.already_imported);
            let file = store.load().unwrap();
            assert!(file.legacy_imported);

            let gongwen = file
                .records
                .iter()
                .find(|r| r.id == "gongwen")
                .expect("gongwen 应登记");
            assert_eq!(gongwen.source, BundleSource::Preset);
            assert!(gongwen.installed);

            let preset = file
                .records
                .iter()
                .find(|r| r.id == "government-writing")
                .expect("预置技能应登记");
            assert_eq!(preset.source, BundleSource::Preset);

            let upload = file
                .records
                .iter()
                .find(|r| r.id == "my-upload")
                .expect("上传技能应登记");
            assert_eq!(upload.source, BundleSource::Upload("pkg.zip".to_string()));

            let feishu = file
                .records
                .iter()
                .find(|r| r.id == "feishu")
                .expect("CLI companion 技能在盘应归并出 feishu 包");
            assert_eq!(feishu.source, BundleSource::Builtin);
            if cli_verifiable {
                assert!(
                    feishu.degraded.is_some(),
                    "二进制与 lock 表不符应记 degraded"
                );
                assert!(report.degraded.contains(&"feishu".to_string()));
            }
            // companion 技能目录自身不独立登记
            assert!(!file.records.iter().any(|r| r.id == "lark-shared"));
            // 未连接的连接器不登记
            for id in ["wecom", "dingtalk", "tmeet"] {
                assert!(
                    !file.records.iter().any(|r| r.id == id),
                    "{id} 未连接不应登记"
                );
            }
        });
    }

    /// 首启登记：企微按 1.1.0 的 14 新名识别「连接过」（五轮评审必修 3：旧 7 名
    /// 表会让升级自 wecom-cli 1.1.0 的用户漏登 wecom 安装记录——`legacy_imported`
    /// 闸一次性置位，错过即永久错过）。
    #[test]
    fn import_legacy_registers_wecom_by_new_skill_dirs() {
        with_temp_home("pinvou3-store-test", || {
            let legacy = paths::bundle_skills_dir();
            // 只放 1.1.0 新名（0.1.9 旧 7 名表覆盖不到的名字）
            let dir = legacy.join("wecomcli-calendar");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "---\nname: wecomcli-calendar\n---\n").unwrap();

            let store = BundleStore::new();
            store.import_legacy().unwrap();
            let file = store.load().unwrap();
            let wecom = file
                .records
                .iter()
                .find(|r| r.id == "wecom")
                .expect("新名 companion 技能在盘应归并出 wecom 包");
            assert_eq!(wecom.source, BundleSource::Builtin);
            assert!(wecom.installed);
            // companion 技能目录自身不独立登记
            assert!(!file.records.iter().any(|r| r.id == "wecomcli-calendar"));
        });
    }

    /// 导入幂等：二次调用直接跳过；已有同 id 记录永远保留，不被反推结果覆盖。
    #[test]
    fn import_legacy_is_idempotent_and_never_overwrites_existing() {
        with_temp_home("pinvou3-store-test", || {
            let home = paths::pinvou3_home();
            let marketplace = home.join("marketplace");
            std::fs::create_dir_all(&marketplace).unwrap();
            std::fs::write(marketplace.join("installed.json"), r#"["weather"]"#).unwrap();

            // 用户/统一管线已写入的同 id 记录（含自定义字段）
            let store = BundleStore::new();
            let mut existing = record("weather", BundleSource::Preset);
            existing.installed_at = "2026-01-01T00:00:00+00:00".to_string();
            existing
                .extra
                .insert("user_note".to_string(), serde_json::json!("keep-me"));
            store.upsert(existing).unwrap();

            let report1 = store.import_legacy().unwrap();
            assert!(!report1.already_imported);
            assert_eq!(report1.kept_existing, vec!["weather".to_string()]);
            assert!(!report1.imported.iter().any(|id| id == "weather"));

            let weather = store.get("weather").unwrap().expect("weather 应存在");
            assert_eq!(weather.installed_at, "2026-01-01T00:00:00+00:00");
            assert_eq!(
                weather.extra.get("user_note"),
                Some(&serde_json::json!("keep-me")),
                "既有记录不得被导入覆盖"
            );

            let after_first = std::fs::read_to_string(store.file_path()).unwrap();
            let report2 = store.import_legacy().unwrap();
            assert!(report2.already_imported, "二次导入应直接跳过");
            assert!(report2.imported.is_empty());
            let after_second = std::fs::read_to_string(store.file_path()).unwrap();
            assert_eq!(after_first, after_second, "二次导入不得改写文件");
        });
    }

    /// Round-29 m4 (review #455): an EXISTS-but-unreadable `.installed-from`
    /// marker must abort the legacy import (Err) instead of being silently
    /// skipped — the one-shot `legacy_imported` gate must not latch over an
    /// incomplete mirror (round-23 MAJOR-3 / round-24 m2 class, per-entry
    /// form). A no-marker dir stays a plain skip.
    #[cfg(unix)]
    #[test]
    fn legacy_import_unreadable_marker_aborts_before_gate_latches() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("pinvou3-store-test", || {
            let skills = paths::bundle_skills_dir();
            let good = skills.join("good");
            std::fs::create_dir_all(&good).unwrap();
            std::fs::write(good.join(".installed-from"), "pinvou3-marketplace:legacy-a").unwrap();
            let sealed = skills.join("sealed");
            std::fs::create_dir_all(&sealed).unwrap();
            std::fs::write(
                sealed.join(".installed-from"),
                "pinvou3-marketplace:legacy-b",
            )
            .unwrap();
            let sealed_marker = sealed.join(".installed-from");
            std::fs::set_permissions(&sealed_marker, std::fs::Permissions::from_mode(0o000))
                .unwrap();
            if std::fs::read_to_string(&sealed_marker).is_ok() {
                eprintln!(
                    "ROOT-SKIP[legacy_import_unreadable_marker_aborts_before_gate_latches]: unreadable-file fixture not effective (root); NOT exercised"
                );
                return;
            }
            // 无标记目录仍是普通跳过，不得触发任何错误路径。
            std::fs::create_dir_all(skills.join("bare")).unwrap();

            let store = BundleStore::new();
            let err = store.import_legacy().unwrap_err();
            assert!(
                err.contains(".installed-from") || err.contains("读取"),
                "the unreadable marker must abort the import: {err}"
            );
            assert!(
                !store.load().unwrap().legacy_imported,
                "the one-shot gate must not latch over an aborted import"
            );
        });
    }

    /// Round-27 review (NIT): a planted DIRECTORY at `.installed-from` must
    /// abort the legacy import exactly like the unreadable regular file —
    /// the is_file gate reads it as "no marker", which would silently latch
    /// the one-shot `legacy_imported` gate over a skipped mirror.
    #[test]
    fn legacy_import_marker_directory_aborts_before_gate_latches() {
        with_temp_home("pinvou3-store-test", || {
            let skills = paths::bundle_skills_dir();
            let planted = skills.join("planted");
            std::fs::create_dir_all(planted.join(".installed-from")).unwrap();
            std::fs::write(
                planted.join(".installed-from").join("keep"),
                b"pinvou3-marketplace:legacy-a",
            )
            .unwrap();

            let store = BundleStore::new();
            let err = store.import_legacy().unwrap_err();
            assert!(
                err.contains(".installed-from"),
                "the directory marker must abort the import: {err}"
            );
            assert!(
                !store.load().unwrap().legacy_imported,
                "the one-shot gate must not latch over an aborted import"
            );
        });
    }

    #[test]
    fn remove_deletes_record_and_missing_id_is_false() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("weather", BundleSource::Preset))
                .unwrap();
            store.upsert(record("pptx", BundleSource::Preset)).unwrap();

            assert!(store.remove("weather").unwrap(), "存在的 id 应删除成功");
            assert_eq!(store.records().unwrap().len(), 1, "删除后只剩一条记录");
            assert!(store.get("weather").unwrap().is_none());
            assert!(!store.remove("weather").unwrap(), "重复删除应返回 false");
            assert!(
                !store.remove("never-existed").unwrap(),
                "不存在的 id 应返回 false"
            );
        });
    }

    #[test]
    fn mark_degraded_roundtrip() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("feishu", BundleSource::Builtin))
                .unwrap();

            assert!(store.mark_degraded("feishu", "二进制缺失").unwrap());
            assert_eq!(
                store.get("feishu").unwrap().unwrap().degraded,
                Some("二进制缺失".to_string())
            );
            // 不存在的 id：Ok(false)，不误建记录
            assert!(!store.mark_degraded("ghost", "x").unwrap());
            assert!(store.get("ghost").unwrap().is_none());
        });
    }

    /// 镜像写路径的语义：重装/重复连接不得冲掉首次安装时间、extra 与既有指纹。
    #[test]
    fn upsert_preserving_keeps_first_install_metadata() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let mut first = record("gongwen", BundleSource::Preset);
            first.installed_at = "2026-01-01T00:00:00+00:00".to_string();
            first.content_fingerprint = Some("fp-v1".to_string());
            first
                .extra
                .insert("future_key".to_string(), serde_json::json!(1));
            store.upsert(first).unwrap();

            // 重装：新记录只带新 degraded/installed 值，时间/extra/指纹应保留
            let again = BundleRecord::installed_now("gongwen", BundleSource::Preset);
            store.upsert_preserving(again).unwrap();

            let merged = store.get("gongwen").unwrap().unwrap();
            assert_eq!(merged.installed_at, "2026-01-01T00:00:00+00:00");
            assert_eq!(merged.extra.get("future_key"), Some(&serde_json::json!(1)));
            assert_eq!(merged.content_fingerprint, Some("fp-v1".to_string()));
            assert_eq!(store.records().unwrap().len(), 1, "不得产生重复记录");
        });
    }

    fn cli_asset(version: &str) -> AssetEntry {
        AssetEntry::Ref(AssetRef {
            kind: ASSET_KIND_CLI.to_string(),
            name: "lark-cli".to_string(),
            version: version.to_string(),
            sha256: "ab".repeat(32),
            ..AssetRef::default()
        })
    }

    #[test]
    fn upsert_preserving_keeps_assets_unless_the_new_record_carries_some() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let mut first = record("feishu", BundleSource::Builtin);
            first.assets = vec![cli_asset("1.0.0")];
            store.upsert(first).unwrap();

            // A writer that does not track assets (the GUI connector mirror)
            // must not wipe them.
            store
                .upsert_preserving(BundleRecord::installed_now("feishu", BundleSource::Builtin))
                .unwrap();
            assert_eq!(
                store.get("feishu").unwrap().unwrap().assets,
                vec![cli_asset("1.0.0")]
            );

            // A writer that does track them can replace a stale pin.
            let mut repinned = BundleRecord::installed_now("feishu", BundleSource::Builtin);
            repinned.assets = vec![cli_asset("1.1.0")];
            store.upsert_preserving(repinned).unwrap();
            assert_eq!(
                store.get("feishu").unwrap().unwrap().assets,
                vec![cli_asset("1.1.0")]
            );
        });
    }

    #[test]
    fn unreadable_asset_entries_neither_fail_the_load_nor_get_dropped() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let path = store.file_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let assets = serde_json::json!([
                {"kind": "cli", "name": "lark-cli", "version": "1.0.0",
                 "sha256": "abc", "platforms": ["darwin"]},
                {"kind": "cli", "name": "wecom-cli", "version": 2},
                {"kind": "pip", "name": "requests"},
                "not-an-object",
            ]);
            let file = serde_json::json!({
                "schema_version": 1,
                "legacy_imported": true,
                "records": [{"id": "feishu", "source": "builtin", "installed": true,
                             "installed_at": "2026-08-14T00:00:00Z", "assets": assets}],
            });
            std::fs::write(&path, file.to_string()).unwrap();

            let loaded = store.get("feishu").unwrap().unwrap();
            let AssetEntry::Ref(first) = &loaded.assets[0] else {
                panic!("a well-formed entry must parse as a typed ref");
            };
            assert_eq!(first.name, "lark-cli");
            assert_eq!(
                first.extra.get("platforms"),
                Some(&serde_json::json!(["darwin"]))
            );
            assert!(
                loaded.assets[1..]
                    .iter()
                    .all(|entry| matches!(entry, AssetEntry::Unrecognized(_))),
                "entries this binary cannot read must stay unrecognized, not fail the load"
            );

            // A read-modify-write preserves every entry as an equal JSON
            // value: unrecognized entries round-trip verbatim, recognized
            // ones re-serialize canonically with their unknown fields kept.
            // Not byte-exact — object key order is normalized on save, and
            // the whole file is re-pretty-printed, before and after this PR.
            store.mark_degraded("feishu", "probe").unwrap();
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(value["records"][0]["assets"], assets);
        });
    }

    /// The `assets` container itself stays fail-loud (see the field doc):
    /// a record whose `assets` is not an array fails the whole load like
    /// corrupt JSON rather than reading as "no assets" — only the
    /// per-entry degrade is tolerant.
    #[test]
    fn unreadable_assets_container_fails_the_load_loud() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            let path = store.file_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            for bad in ["null", "\"pins\"", "{\"kind\": \"cli\"}"] {
                // `installed_at` is present so the assets container is the
                // ONLY thing that can fail this load — otherwise a missing
                // required field fails it first and the assertion passes
                // while blind to the rule it guards.
                let file = format!(
                    r#"{{"schema_version": 1, "records": [{{"id": "feishu", "source": "builtin", "installed": true, "installed_at": "2026-09-30T00:00:00Z", "assets": {bad}}}]}}"#
                );
                std::fs::write(&path, file).unwrap();
                assert!(
                    store.load().is_err(),
                    "`assets: {bad}` must fail the whole load, not read as empty"
                );
            }
        });
    }

    /// The load's unreadable-entry announcement targets exactly the records
    /// that carry an unrecognized entry — not clean records, and not
    /// records that merely have no assets at all.
    #[test]
    fn unrecognized_asset_announce_targets_the_right_records() {
        let mut flagged = record("feishu", BundleSource::Builtin);
        flagged.assets = vec![AssetEntry::Unrecognized(serde_json::json!("not-an-object"))];
        let mut clean = record("gongwen", BundleSource::Builtin);
        clean.assets = vec![cli_asset("1.0.0")];
        let pinless = record("dingtalk", BundleSource::Builtin);
        let file = BundlesFile {
            records: vec![flagged, clean, pinless],
            ..BundlesFile::default()
        };
        assert_eq!(
            unrecognized_asset_record_ids(&file),
            vec!["feishu".to_string()]
        );
    }

    /// 回归（四轮评审 BLOCKER 1）：既有记录的来源必须保留 —— 上传包重装时
    /// install 镜像写带的是 Preset，若覆盖 source，下次卸载会把 Upload 误判为
    /// 可删目录（用户上传内容无其他副本）。source 只在无旧记录时取新值。
    #[test]
    fn upsert_preserving_keeps_existing_source() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();

            // 重装：镜像写来源是 Preset，既有 Upload 必须保留
            store
                .upsert_preserving(record("up", BundleSource::Preset))
                .unwrap();
            assert_eq!(
                store.get("up").unwrap().unwrap().source,
                BundleSource::Upload("pkg.zip".to_string()),
                "重装不得把 Upload 来源翻成 Preset"
            );

            // 无旧记录：取新值（首次登记的语义不变）
            store
                .upsert_preserving(record("new-one", BundleSource::Preset))
                .unwrap();
            assert_eq!(
                store.get("new-one").unwrap().unwrap().source,
                BundleSource::Preset
            );
        });
    }

    /// 展示名/说明覆盖：设置 → 读回；trim 空串 = 删 key 回退默认；None = 不动；
    /// 镜像写（upsert_preserving）不得丢 extra 里的展示覆盖。
    #[test]
    fn set_display_meta_roundtrip_and_clear() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();

            store
                .set_display_meta("up", Some("  我的天气包 "), Some("查天气、看预警"))
                .unwrap();
            let rec = store.get("up").unwrap().unwrap();
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_NAME).as_deref(),
                Some("我的天气包"),
                "写入值应 trim 后落盘"
            );
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_DESCRIPTION).as_deref(),
                Some("查天气、看预警")
            );

            // None = 该字段不动
            store.set_display_meta("up", None, Some("新说明")).unwrap();
            let rec = store.get("up").unwrap().unwrap();
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_NAME).as_deref(),
                Some("我的天气包")
            );
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_DESCRIPTION).as_deref(),
                Some("新说明")
            );

            // trim 后空串 = 删除该 key（回退默认展示）
            store.set_display_meta("up", Some("   "), None).unwrap();
            let rec = store.get("up").unwrap().unwrap();
            assert_eq!(display_override(&rec, EXTRA_DISPLAY_NAME), None);
            assert!(!rec.extra.contains_key(EXTRA_DISPLAY_NAME));
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_DESCRIPTION).as_deref(),
                Some("新说明")
            );

            // 两个都 None = no-op 成功
            store.set_display_meta("up", None, None).unwrap();

            // 镜像写保留 extra 里的展示覆盖（重装不丢用户设置）
            store
                .upsert_preserving(record("up", BundleSource::Preset))
                .unwrap();
            let rec = store.get("up").unwrap().unwrap();
            assert_eq!(rec.source, BundleSource::Upload("pkg.zip".to_string()));
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_DESCRIPTION).as_deref(),
                Some("新说明"),
                "upsert_preserving 不得丢展示覆盖"
            );
        });
    }

    /// 上传文件名展示名回退：去扩展名 + trim；非 Upload 来源 / 扩展名剥空 = None。
    #[test]
    fn upload_display_fallback_strips_extension() {
        let cases = [
            ("my skill.zip", Some("my skill")),
            ("notes.md", Some("notes")),
            ("archive.tar.gz", Some("archive.tar")),
            ("no-ext", Some("no-ext")),
            ("  spaced .zip ", Some("spaced")),
            (".zip", None),
        ];
        for (filename, expected) in cases {
            let rec = record("up", BundleSource::Upload(filename.to_string()));
            assert_eq!(
                upload_display_fallback(&rec).as_deref(),
                expected,
                "filename={filename}"
            );
        }
        assert_eq!(
            upload_display_fallback(&record("p", BundleSource::Preset)),
            None,
            "非 Upload 来源无文件名回退"
        );
    }

    /// 展示覆盖的写入门禁：记录不存在 / 非 Upload 来源 / 超长一律 Err 且不写盘。
    #[test]
    fn set_display_meta_rejects_invalid_targets_and_lengths() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("weather", BundleSource::Preset))
                .unwrap();
            store
                .upsert(record("feishu", BundleSource::Builtin))
                .unwrap();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();

            assert!(
                store.set_display_meta("ghost", Some("x"), None).is_err(),
                "记录不存在应 Err"
            );
            for id in ["weather", "feishu"] {
                assert!(
                    store.set_display_meta(id, Some("x"), None).is_err(),
                    "非 Upload 来源（{id}）应拒绝覆盖"
                );
                assert!(
                    store.get(id).unwrap().unwrap().extra.is_empty(),
                    "拒绝后不得写入 extra"
                );
            }

            let long_name = "名".repeat(MAX_DISPLAY_NAME_CHARS + 1);
            let long_desc = "述".repeat(MAX_DISPLAY_DESCRIPTION_CHARS + 1);
            assert!(
                store
                    .set_display_meta("up", Some(&long_name), None)
                    .is_err()
            );
            assert!(
                store
                    .set_display_meta("up", None, Some(&long_desc))
                    .is_err()
            );
            // 控制字符/换行/不可见字符：展示名与展示说明一律拒绝（单行 UI 展示
            // 与单行 SKILL.md 回写都无法表达；bidi/零宽类可视觉欺骗或污染比较）
            for bad in [
                "含\n换行",
                "含\t制表",
                "含\u{7}控制符",
                "bidi\u{202E}逆转",
                "bidi\u{2066}隔离",
                "零宽\u{200B}字符",
                "\u{FEFF}BOM开头",
                "软\u{00AD}连字符",
                "行分隔\u{2028}符",
            ] {
                assert!(
                    store.set_display_meta("up", Some(bad), None).is_err(),
                    "展示名含控制/不可见字符应拒绝: {bad:?}"
                );
                assert!(
                    store.set_display_meta("up", None, Some(bad)).is_err(),
                    "展示说明含控制/不可见字符应拒绝: {bad:?}"
                );
            }
            // 对照：emoji / 中文 / 空格等可见字符正常放行
            store
                .set_display_meta("up", Some("天气 ⛅ v2"), None)
                .unwrap();
            // 边界：恰好上限可写
            let ok_name = "名".repeat(MAX_DISPLAY_NAME_CHARS);
            let ok_desc = "述".repeat(MAX_DISPLAY_DESCRIPTION_CHARS);
            store
                .set_display_meta("up", Some(&ok_name), Some(&ok_desc))
                .unwrap();
            let rec = store.get("up").unwrap().unwrap();
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_NAME).as_deref(),
                Some(ok_name.as_str())
            );
            assert_eq!(
                display_override(&rec, EXTRA_DISPLAY_DESCRIPTION).as_deref(),
                Some(ok_desc.as_str())
            );
        });
    }

    /// 长度预检（update_display_meta 编排在回写 SKILL.md 之前调用）：
    /// 超限/控制字符 Err、None/清空（trim 后空）放行、恰好上限放行——与
    /// apply_display_meta 同口径，保证前置校验不会先改包内容再报错。
    #[test]
    fn validate_display_meta_matches_apply_limits() {
        assert!(
            validate_display_meta(Some(&"名".repeat(MAX_DISPLAY_NAME_CHARS + 1)), None).is_err()
        );
        assert!(
            validate_display_meta(None, Some(&"述".repeat(MAX_DISPLAY_DESCRIPTION_CHARS + 1)))
                .is_err()
        );
        assert!(validate_display_meta(Some("含\n换行"), None).is_err());
        assert!(validate_display_meta(None, Some("含\t制表")).is_err());
        // None / trim 后空（= 清覆盖，不触发回写）不检
        assert!(validate_display_meta(None, None).is_ok());
        assert!(validate_display_meta(Some("   "), Some("")).is_ok());
        // 恰好上限可过
        assert!(
            validate_display_meta(
                Some(&"名".repeat(MAX_DISPLAY_NAME_CHARS)),
                Some(&"述".repeat(MAX_DISPLAY_DESCRIPTION_CHARS))
            )
            .is_ok()
        );
    }

    /// 指纹补写单锁 RMW 的「不复活」契约：记录已卸载（remove）后补写必须
    /// 返回 Ok(false) 且不重建记录。回归钉子——回退成「读记录 →
    /// upsert_preserving 补写」两段式会把已删记录按 stale 快照复活
    /// （upsert_preserving 对不存在的 id 直接插入），而整个套件仍全绿。
    #[test]
    fn update_content_fingerprint_does_not_resurrect_removed_record() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();
            assert!(store.remove("up").unwrap());
            assert!(
                !store
                    .update_content_fingerprint_if_exists("up", "fp-after-uninstall")
                    .unwrap()
            );
            assert!(store.get("up").unwrap().is_none());
        });
    }

    /// SKILL.md 原说明备份：首写留存（含空串哨兵）、清 key、仅 Upload 可写、
    /// upsert_preserving 保留。
    #[test]
    fn skill_desc_backup_roundtrip_and_gates() {
        with_temp_home("pinvou3-store-test", || {
            let store = BundleStore::new();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();
            store
                .upsert(record("preset", BundleSource::Preset))
                .unwrap();

            assert!(store.skill_desc_backup("up").unwrap().is_none());
            store.set_skill_desc_backup("up", Some("原描述")).unwrap();
            assert_eq!(
                store.skill_desc_backup("up").unwrap().as_deref(),
                Some("原描述")
            );
            // 空串哨兵 ≠ None
            store.set_skill_desc_backup("up", Some("")).unwrap();
            assert_eq!(store.skill_desc_backup("up").unwrap().as_deref(), Some(""));
            // 清 key
            store.set_skill_desc_backup("up", None).unwrap();
            assert!(store.skill_desc_backup("up").unwrap().is_none());
            // 门禁：ghost / 预置拒绝
            assert!(store.set_skill_desc_backup("ghost", Some("x")).is_err());
            assert!(store.set_skill_desc_backup("preset", Some("x")).is_err());
            // upsert_preserving 不丢备份
            store.set_skill_desc_backup("up", Some("原描述")).unwrap();
            let mut rec = store.get("up").unwrap().unwrap();
            rec.content_fingerprint = Some("fp".to_string());
            store.upsert_preserving(rec).unwrap();
            assert_eq!(
                store.skill_desc_backup("up").unwrap().as_deref(),
                Some("原描述"),
                "upsert_preserving 不得丢备份 key"
            );
        });
    }

    /// Blocks until the spawned worker holds the bundles in-process mutex: with
    /// the foreign OS lock held by the test, a worker past the mutex is parked
    /// on (or just failed) the OS-lock acquisition. Without this handshake the
    /// absence assert below could pass before the worker even reached the lock
    /// (#517 test shape, generalized to the per-path registry mutex).
    fn wait_until_bundles_mutex_held(lock_path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match file_lock::process_mutex_for(lock_path).try_lock() {
                // Not held by the worker yet — retry shortly.
                Ok(guard) => drop(guard),
                // Held by the worker: with the foreign lock held, it is now
                // either blocked on the OS lock or its acquisition failed.
                Err(std::sync::TryLockError::WouldBlock) => return,
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    drop(p.into_inner());
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker never reached the bundles lock acquisition point"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// #515/#521 cross-process contention on the WRITE path: while a foreign
    /// fd holds the bundles OS lock, the save must not land first; after the
    /// release the write lands with its full content.
    #[test]
    fn cross_process_lock_blocks_save_write_until_release() {
        with_temp_home("pinvou3-store-cross-process-save", || {
            let store = BundleStore::new();
            let data_path = store.file_path();
            let lock_path = file_lock::lock_path_for(&data_path);
            std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&lock_path)
                .expect("test should be able to open the lock file");
            let mut foreign = fd_lock::RwLock::new(file);
            let foreign_guard = foreign
                .write()
                .expect("test should be able to take the foreign cross-process write lock");

            let worker = std::thread::spawn(move || {
                store
                    .upsert(record("weather", BundleSource::Preset))
                    .expect("write should succeed once the foreign lock is released");
            });
            wait_until_bundles_mutex_held(&lock_path);
            // The write must not land while the peer holds the lock.
            let window = std::time::Duration::from_secs(2);
            let deadline = std::time::Instant::now() + window;
            while std::time::Instant::now() < deadline {
                assert!(
                    !data_path.exists(),
                    "data file written while the lock was still held — serialization is broken"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            drop(foreign_guard);
            worker
                .join()
                .expect("worker should finish once the foreign lock is released");
            let content = std::fs::read_to_string(&data_path).unwrap();
            assert!(
                content.contains("\"weather\""),
                "write should land after the lock is released: {content}"
            );
        });
    }

    /// #515/#521 cross-process contention on the READ path: while a peer holds
    /// the OS lock, a load must degrade promptly to the unlocked, never-
    /// persisting view (bounded — readers never couple to a peer's critical
    /// section) instead of blocking or writing. A missing file stays missing
    /// (a pure read must not create the store), contended or not.
    #[test]
    fn cross_process_lock_contention_degrades_read_without_persist() {
        with_temp_home("pinvou3-store-cross-process-read", || {
            let store = BundleStore::new();
            let lock_path = file_lock::lock_path_for(&store.file_path());
            std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&lock_path)
                .expect("test should be able to open the lock file");
            let mut foreign = fd_lock::RwLock::new(file);
            let foreign_guard = foreign
                .write()
                .expect("test should be able to take the foreign cross-process write lock");

            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let got = BundleStore::new().load();
                done_tx.send(got).expect("reader should send its result");
            });
            // A contended read must return promptly. recv_timeout doubles as
            // the regression assertion: a blocking read hangs here and fails
            // the test with a bounded, diagnosable timeout.
            let got = done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("contended read must degrade instead of blocking on the peer's lock");
            assert!(
                got.unwrap().records.is_empty(),
                "a missing store reads as empty under contention"
            );
            assert!(
                !store.file_path().exists(),
                "a contended read must not persist (create) the store file"
            );

            drop(foreign_guard);
            reader.join().expect("reader thread should finish");
        });
    }

    /// round-21 review: `load_locked` runs inside the bundles.json file lock
    /// and — via the scope DenyAll expansion — inside the scope lock's
    /// critical section, so it must route through the hardened read: a
    /// planted FIFO at the path must refuse, not block the lock holder.
    /// Behavioral pin that the routing survived refactors (the platform
    /// primitive's own FIFO tests cover the mechanism).
    #[cfg(unix)]
    #[test]
    fn load_locked_refuses_a_planted_fifo() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("bundles.json");
        crate::platform::paths::tests::plant_fifo(&file);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            tx.send(load_locked(&file))
                .expect("worker should send its result");
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| {
                // Round-27 review (minor): a regressed worker stays blocked
                // holding the store lock — contain the cascade with a loud
                // process death instead of hanging the serial lane.
                eprintln!(
                    "FAIL: load_locked still blocks on a planted FIFO — \
                     aborting the test process to contain the leaked lock holder"
                );
                std::process::abort();
            });
        assert!(
            result.is_err(),
            "a planted FIFO must be refused by the regular-file gate"
        );
    }
}
