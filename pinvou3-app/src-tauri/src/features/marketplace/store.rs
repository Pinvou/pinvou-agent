//! BundleStore —— 工具市场统一真相源的可写存储层（Phase 2 第一刀）。
//!
//! 设计依据：`docs/marketplace-unification.md` §3.1（存储层 BundleRecord）、§4（存储
//! 布局）、§9（首启一次性导入）。本模块只管 `~/.pinvou3/marketplace/bundles.json`
//! 的读写与旧布局**登记**；物理目录搬移（`bundles/<id>/`、`assets/cli/`）与旧布局
//! 删除在后续 PR，本刀一律不动磁盘上的包内容。
//!
//! 纪律（§10）：
//! - 原子写（tmp + rename，走底座 `write_atomic`）+ 进程内 FILE_LOCK 串行化读-改-写；
//!   读/写入口拆「取锁包装 + 已持锁实现」两层，已持锁的 import/upsert 直接调 `_locked`
//!   实现，避免 Mutex 重入死锁（#287 修过的竞态范式）。
//! - 不用 `#[serde(deny_unknown_fields)]`：未知字段经 `extra` flatten map 原样
//!   roundtrip，新 schema 字段在老版本二进制上不丢数据（前向兼容）。
//! - 损坏 JSON fail loud：bundles.json 是唯一真相源，静默重建会掩盖数据损坏，
//!   损坏时返回 Err 且绝不回写。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::MarketplaceManager;
use super::bundle;
use crate::platform::connector_lock;
use crate::platform::paths;

/// bundles.json 当前 schema 版本。后续 schema 演进时递增并在读路径做迁移。
const SCHEMA_VERSION: u32 = 1;

/// 资产种类：厂商 CLI 二进制（版本化外部资产，终态住 `assets/cli/<name>/<version>/`，
/// 包只引用不拥有 —— §4 规则 2）。后续收编 pip 依赖时新增种类常量。
pub const ASSET_KIND_CLI: &str = "cli";

/// 上传包的用户自定义 UI 展示名/说明在记录 `extra` map 里的 key（只改展示，
/// 机读 id / 目录 / frontmatter name 一律不动；见 docs/plugin-package-spec.md）。
pub const EXTRA_DISPLAY_NAME: &str = "display_name";
pub const EXTRA_DISPLAY_DESCRIPTION: &str = "display_description";

/// 单技能包**首次回写 SKILL.md 前**留存的 frontmatter 原 description 备份 key
/// （清空展示说明时恢复原值用）。空串哨兵 = 原本没有 description；缺 key =
/// 从未回写过（多技能/纯 MCP 包不回写，也不存备份）。
pub const EXTRA_SKILL_DESC_BACKUP: &str = "skill_description_backup";

/// 展示名校验上限（字符数）。
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;
/// 展示说明校验上限（字符数；对齐 skill_marketplace 的 description 展示截断口径）。
pub const MAX_DISPLAY_DESCRIPTION_CHARS: usize = 240;

/// `bundles.json` 读-改-写的进程内串行化：统一管线登记、首启导入、后续开关命令
/// 都可能并发触发同一份文件的读-改-写，串行化避免交错丢更新（与
/// `disabled_connectors.json` / `disabled_skills.json` 同一范式）。
static BUNDLES_FILE_LOCK: Mutex<()> = Mutex::new(());

fn file_lock() -> MutexGuard<'static, ()> {
    BUNDLES_FILE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// 外部资产引用（§3.1：name + version + sha256；kind 区分 CLI 二进制 / 后续 pip 等）。
/// kind 用 String 而非枚举：新资产种类在老版本二进制上应能无损 roundtrip。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
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
    /// 外部资产引用（CLI 二进制等，包只引用不拥有）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetRef>,
    /// 凭据引用（只有 key；凭据本体在 keyring，永不落盘 —— §4 规则 3）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_keys: Vec<String>,
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
    /// `credential_keys` / `assets` / `degraded`。
    pub fn installed_now(id: impl Into<String>, source: BundleSource) -> Self {
        Self {
            id: id.into(),
            source,
            installed: true,
            content_fingerprint: None,
            assets: Vec::new(),
            credential_keys: Vec::new(),
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

    /// 读整个文件（取锁包装）。文件不存在 → 空 store；JSON 损坏 → Err（fail loud）。
    pub fn load(&self) -> Result<BundlesFile, String> {
        let _guard = file_lock();
        load_locked(&self.file)
    }

    /// 全部记录（便捷入口）。
    pub fn records(&self) -> Result<Vec<BundleRecord>, String> {
        Ok(self.load()?.records)
    }

    /// 按 id 查单条记录。
    pub fn get(&self, id: &str) -> Result<Option<BundleRecord>, String> {
        Ok(self.load()?.records.into_iter().find(|r| r.id == id))
    }

    /// 插入或按 id 替换一条记录（读-改-写全程持锁，原子落盘）。
    pub fn upsert(&self, record: BundleRecord) -> Result<(), String> {
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        match file.records.iter_mut().find(|r| r.id == record.id) {
            Some(existing) => *existing = record,
            None => file.records.push(record),
        }
        save_locked(&self.file, &file)
    }

    /// upsert 变体：记录已存在时保留 `source`（包来源只在首次登记时确定 —— 重装/
    /// 重复连接的镜像写一律带 Preset，若覆盖会把 Upload 翻成 Preset，下次卸载
    /// 误删用户唯一副本，四轮评审 BLOCKER 1）、`installed_at`（首次登记时间）、
    /// `extra`（前向兼容/用户字段）与 `content_fingerprint`（完整性校验层的数据），
    /// 其余字段以新值为准。安装/连接成功的镜像写统一走这里 —— 重装、重复连接
    /// 不应冲掉首次安装时间，也不应丢老版本二进制不认识的字段。
    pub fn upsert_preserving(&self, record: BundleRecord) -> Result<(), String> {
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        let merged = match file.records.iter().find(|r| r.id == record.id) {
            Some(existing) => BundleRecord {
                id: record.id,
                source: existing.source.clone(),
                installed: record.installed,
                content_fingerprint: record
                    .content_fingerprint
                    .or_else(|| existing.content_fingerprint.clone()),
                assets: record.assets,
                credential_keys: record.credential_keys,
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
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
            return Ok(false);
        };
        record.content_fingerprint = Some(fingerprint.to_string());
        save_locked(&self.file, &file)?;
        Ok(true)
    }

    /// 按 id 删除记录（读-改-写全程持锁，原子落盘）。id 不存在 → Ok(false)，不写盘。
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        let before = file.records.len();
        file.records.retain(|r| r.id != id);
        if file.records.len() == before {
            return Ok(false);
        }
        save_locked(&self.file, &file)?;
        Ok(true)
    }

    /// 局部更新：置 `Degraded` 原因（§3.2：登记在、资源缺），供 CLI 修复/断开
    /// 路径用。id 不存在 → Ok(false)；原因未变 → Ok(true) 但不写盘。
    pub fn mark_degraded(&self, id: &str, reason: &str) -> Result<bool, String> {
        self.set_degraded(id, Some(reason.to_string()))
    }

    /// 局部更新：清除 `Degraded`（修复完成/重新连接后调用）。语义同 [`Self::mark_degraded`]。
    pub fn clear_degraded(&self, id: &str) -> Result<bool, String> {
        self.set_degraded(id, None)
    }

    fn set_degraded(&self, id: &str, reason: Option<String>) -> Result<bool, String> {
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        let Some(record) = file.records.iter_mut().find(|r| r.id == id) else {
            return Ok(false);
        };
        if record.degraded == reason {
            return Ok(true);
        }
        record.degraded = reason;
        save_locked(&self.file, &file)?;
        Ok(true)
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
        let _guard = file_lock();
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
    }

    /// 读取记录的 SKILL.md 原说明备份（[`EXTRA_SKILL_DESC_BACKUP`]；缺 key /
    /// 非字符串 → None；`Some("")` = 原缺失哨兵）。清空展示说明的恢复路径用。
    pub fn skill_desc_backup(&self, id: &str) -> Result<Option<String>, String> {
        let _guard = file_lock();
        let file = load_locked(&self.file)?;
        Ok(file
            .records
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.extra.get(EXTRA_SKILL_DESC_BACKUP))
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    /// 设置/删除 [`EXTRA_SKILL_DESC_BACKUP`]（与 `set_display_meta` 同锁同门禁：
    /// 仅 Upload 记录可写）。值由内部读取/校验管线产生（引擎口径原值或空串哨兵），
    /// 不做展示字段校验；`None` = 删除 key。
    pub fn set_skill_desc_backup(&self, id: &str, backup: Option<&str>) -> Result<(), String> {
        let _guard = file_lock();
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
    }

    /// 首启一次性导入（§9）：从旧布局（installed.json + bundle/skills/* 的
    /// `.installed-from` 标记 + connectors/<platform>/bin/ 存量 CLI 二进制）反推
    /// 已装包，登记进 bundles.json。
    ///
    /// - **幂等**：`legacy_imported` 闸置位后直接跳过；闸未置位时也只补缺失 id，
    ///   已存在的记录永远保留（用户/新管线写入的赢）。
    /// - **非破坏性**：只读旧布局、只写 bundles.json；目录搬移与旧布局删除在后续 PR。
    /// - 全程持 FILE_LOCK（"读到即迁移"必须持锁，§9.4 / #287 竞态教训前置）。
    pub fn import_legacy(&self) -> Result<LegacyImportReport, String> {
        let _guard = file_lock();
        let mut file = load_locked(&self.file)?;
        let mut report = LegacyImportReport::default();
        if file.legacy_imported {
            report.already_imported = true;
            return Ok(report);
        }
        for candidate in collect_legacy_records() {
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
    }
}

// ---------------------------------------------------------------------------
// 已持锁实现（取锁包装之上的内层；调用前必须已持有 BUNDLES_FILE_LOCK）
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

/// 内层读：与取锁包装分离，已持锁的 import/upsert 直接调用，避免 Mutex 重入。
fn load_locked(path: &Path) -> Result<BundlesFile, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            format!(
                "解析 {} 失败: {e}（bundles.json 是唯一真相源，损坏时 fail loud，不静默重建）",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BundlesFile::default()),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
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
    deepseek_tui::utils::write_atomic(path, json.as_bytes())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 旧布局反推（只读旧文件，不动包内容）
// ---------------------------------------------------------------------------

/// 安装时间戳：RFC3339/ISO8601 UTC，对齐 SessionMetadata.updated_at 的 chrono 惯例。
fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn collect_legacy_records() -> Vec<BundleRecord> {
    let mut out = legacy_mcp_records();
    out.extend(legacy_skill_records());
    out.extend(legacy_cli_records());
    // id 去重（保序留先）：MCP 包与其同名 companion 技能（pptx↔pptx）会各扫到一次，
    // 终态模型里它们是同一个包（§5.2「一个包 = 一张卡」），MCP 侧记录含凭据声明，
    // 信息更全，故排在前面的 MCP 记录优先。
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| seen.insert(r.id.clone()));
    out
}

/// installed.json（MCP 安装态）→ 预置包记录；凭据 key 从 manifest 现算收敛
/// （与查询层 `bundle::tool_credentials` 同一推导，keyring 本体不动）。
fn legacy_mcp_records() -> Vec<BundleRecord> {
    let manager = MarketplaceManager::new();
    let installed = manager.installed_ids();
    if installed.is_empty() {
        return Vec::new();
    }
    let manifests = manager.available_tools();
    let now = now_iso8601();
    installed
        .into_iter()
        .map(|id| {
            let credential_keys = manifests
                .iter()
                .find(|m| m.id == id)
                .map(|m| {
                    bundle::tool_credentials(m)
                        .into_iter()
                        .map(|c| c.key)
                        .collect()
                })
                .unwrap_or_default();
            BundleRecord {
                id,
                source: BundleSource::Preset,
                installed: true,
                content_fingerprint: None,
                assets: Vec::new(),
                credential_keys,
                installed_at: now.clone(),
                degraded: None,
                extra: serde_json::Map::new(),
            }
        })
        .collect()
}

/// `bundle/skills/*/.installed-from` 标记 → 技能包记录：
/// `pinvou3-marketplace:<id>` → 预置技能包；`upload:<zip名>` → 上传技能包。
/// 无标记目录（内置 visual-design、CLI companion 技能）不在此登记 —— CLI
/// companion 由 [`legacy_cli_records`] 归并到所属 CLI 包。
fn legacy_skill_records() -> Vec<BundleRecord> {
    let skills_dir = paths::bundle_skills_dir();
    let Ok(rd) = std::fs::read_dir(&skills_dir) else {
        return Vec::new();
    };
    let now = now_iso8601();
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(marker) = std::fs::read_to_string(dir.join(".installed-from")) else {
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
            content_fingerprint: None,
            assets: Vec::new(),
            credential_keys: Vec::new(),
            installed_at: now.clone(),
            degraded: None,
            extra: serde_json::Map::new(),
        });
    }
    out
}

/// 内置 CLI 连接器 → Builtin 包记录。安装态判定：companion 技能目录在盘
/// （连接时才解包）或 CLI 二进制在盘。存量二进制对照 lock 表验 SHA-256：
/// 匹配 → 登记版本化 assets 引用；不匹配/无法校验 → 记 `degraded`（§9.3，
/// 修复动作 = 重新下载，物理搬移 `assets/cli/` 在后续 PR）。
fn legacy_cli_records() -> Vec<BundleRecord> {
    let skills_dir = paths::bundle_skills_dir();
    let now = now_iso8601();
    let mut out = Vec::new();
    for id in bundle::builtin_cli_bundle_ids() {
        let skills_present = bundle::cli_bundle_skill_dirs(id)
            .iter()
            .any(|d| skills_dir.join(d).join("SKILL.md").is_file());
        let mut assets = Vec::new();
        let mut degraded = None;
        if let Some(bin) = bundle::cli_bundle_bin(id) {
            match cli_asset_state(bin) {
                CliAssetState::Verified(asset) => assets.push(asset),
                CliAssetState::Mismatch(reason) => degraded = Some(reason),
                CliAssetState::Absent => {}
            }
        }
        // 技能目录与二进制都不在盘 = 未连接过，不登记
        if !skills_present && assets.is_empty() && degraded.is_none() {
            continue;
        }
        out.push(BundleRecord {
            id: id.to_string(),
            source: BundleSource::Builtin,
            installed: true,
            content_fingerprint: None,
            assets,
            credential_keys: Vec::new(),
            installed_at: now.clone(),
            degraded,
            extra: serde_json::Map::new(),
        });
    }
    out
}

enum CliAssetState {
    Verified(AssetRef),
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
    match connector_lock::file_sha256_hex(&path) {
        Ok(actual) if actual == pin.binary_sha256 => CliAssetState::Verified(AssetRef {
            kind: ASSET_KIND_CLI.to_string(),
            name: bin.to_string(),
            version: pin.version,
            sha256: pin.binary_sha256,
        }),
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

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
    /// 借 `platform::paths::tests::ENV_LOCK` 与其它 mutate PINVOU3_HOME 的测试串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-store-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn record(id: &str, source: BundleSource) -> BundleRecord {
        BundleRecord {
            id: id.to_string(),
            source,
            installed: true,
            content_fingerprint: None,
            assets: Vec::new(),
            credential_keys: Vec::new(),
            installed_at: "2026-08-14T00:00:00+00:00".to_string(),
            degraded: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_store_loads_missing_file_as_empty() {
        with_temp_home(|| {
            let store = BundleStore::new();
            let file = store.load().unwrap();
            assert!(file.records.is_empty());
            assert!(!file.legacy_imported);
            assert!(!store.file_path().exists(), "纯读不得创建 bundles.json");
        });
    }

    #[test]
    fn save_load_roundtrip_preserves_all_fields() {
        with_temp_home(|| {
            let store = BundleStore::new();
            let mut full = record("feishu", BundleSource::Builtin);
            full.content_fingerprint = Some("abc123".to_string());
            full.assets = vec![AssetRef {
                kind: ASSET_KIND_CLI.to_string(),
                name: "lark-cli".to_string(),
                version: "1.2.3".to_string(),
                sha256: "deadbeef".to_string(),
            }];
            full.credential_keys = vec!["LARK_APP_ID".to_string()];
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
        with_temp_home(|| {
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
                vec!["bundles.json".to_string()],
                "原子写不得残留 tmp 文件"
            );
        });
    }

    /// 前向兼容：顶层与记录级的未知字段、未知来源字符串都要无损 roundtrip
    /// （老版本二进制读写新 schema 文件不丢数据）。
    #[test]
    fn unknown_keys_survive_roundtrip() {
        with_temp_home(|| {
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
        with_temp_home(|| {
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

    /// 首启导入：MCP（installed.json + manifest 凭据）、预置/上传技能标记、
    /// CLI 连接器（companion 技能在盘 + 存量二进制对照 lock 表）全部归位。
    #[test]
    fn import_legacy_collects_mcp_skills_and_cli() {
        with_temp_home(|| {
            let home = paths::pinvou3_home();
            let write = |rel: &str, content: &str| {
                let p = home.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, content).unwrap();
            };
            write("marketplace/installed.json", r#"["gongwen"]"#);
            // MCP manifest 已迁按包聚合新布局：`legacy_mcp_records` 经 available_tools()
            // 读 `bundles/<id>/mcp/manifest.json`（+ 内嵌），凭据 key 从现算收敛。
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
            assert_eq!(
                gongwen.credential_keys,
                vec!["GONGWEN_KEY".to_string()],
                "凭据 key 应从 manifest 现算收敛"
            );

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
                assert!(feishu.assets.is_empty(), "校验不过不得登记 assets");
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
        with_temp_home(|| {
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
        with_temp_home(|| {
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

    #[test]
    fn remove_deletes_record_and_missing_id_is_false() {
        with_temp_home(|| {
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
    fn mark_and_clear_degraded_roundtrip() {
        with_temp_home(|| {
            let store = BundleStore::new();
            store
                .upsert(record("feishu", BundleSource::Builtin))
                .unwrap();

            assert!(store.mark_degraded("feishu", "二进制缺失").unwrap());
            assert_eq!(
                store.get("feishu").unwrap().unwrap().degraded,
                Some("二进制缺失".to_string())
            );
            assert!(store.clear_degraded("feishu").unwrap());
            assert_eq!(store.get("feishu").unwrap().unwrap().degraded, None);
            // 不存在的 id：Ok(false)，不误建记录
            assert!(!store.mark_degraded("ghost", "x").unwrap());
            assert!(!store.clear_degraded("ghost").unwrap());
            assert!(store.get("ghost").unwrap().is_none());
        });
    }

    /// 镜像写路径的语义：重装/重复连接不得冲掉首次安装时间、extra 与既有指纹。
    #[test]
    fn upsert_preserving_keeps_first_install_metadata() {
        with_temp_home(|| {
            let store = BundleStore::new();
            let mut first = record("gongwen", BundleSource::Preset);
            first.installed_at = "2026-01-01T00:00:00+00:00".to_string();
            first.content_fingerprint = Some("fp-v1".to_string());
            first
                .extra
                .insert("future_key".to_string(), serde_json::json!(1));
            store.upsert(first).unwrap();

            // 重装：新记录只带新凭据列表，时间/extra/指纹应保留
            let mut again = BundleRecord::installed_now("gongwen", BundleSource::Preset);
            again.credential_keys = vec!["GONGWEN_KEY".to_string()];
            store.upsert_preserving(again).unwrap();

            let merged = store.get("gongwen").unwrap().unwrap();
            assert_eq!(merged.installed_at, "2026-01-01T00:00:00+00:00");
            assert_eq!(merged.extra.get("future_key"), Some(&serde_json::json!(1)));
            assert_eq!(merged.content_fingerprint, Some("fp-v1".to_string()));
            assert_eq!(merged.credential_keys, vec!["GONGWEN_KEY".to_string()]);
            assert_eq!(store.records().unwrap().len(), 1, "不得产生重复记录");
        });
    }

    /// 回归（四轮评审 BLOCKER 1）：既有记录的来源必须保留 —— 上传包重装时
    /// install 镜像写带的是 Preset，若覆盖 source，下次卸载会把 Upload 误判为
    /// 可删目录（用户上传内容无其他副本）。source 只在无旧记录时取新值。
    #[test]
    fn upsert_preserving_keeps_existing_source() {
        with_temp_home(|| {
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
        with_temp_home(|| {
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
        with_temp_home(|| {
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
            assert!(store
                .set_display_meta("up", Some(&long_name), None)
                .is_err());
            assert!(store
                .set_display_meta("up", None, Some(&long_desc))
                .is_err());
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
        assert!(validate_display_meta(
            Some(&"名".repeat(MAX_DISPLAY_NAME_CHARS)),
            Some(&"述".repeat(MAX_DISPLAY_DESCRIPTION_CHARS))
        )
        .is_ok());
    }

    /// 指纹补写单锁 RMW 的「不复活」契约：记录已卸载（remove）后补写必须
    /// 返回 Ok(false) 且不重建记录。回归钉子——回退成「读记录 →
    /// upsert_preserving 补写」两段式会把已删记录按 stale 快照复活
    /// （upsert_preserving 对不存在的 id 直接插入），而整个套件仍全绿。
    #[test]
    fn update_content_fingerprint_does_not_resurrect_removed_record() {
        with_temp_home(|| {
            let store = BundleStore::new();
            store
                .upsert(record("up", BundleSource::Upload("pkg.zip".to_string())))
                .unwrap();
            assert!(store.remove("up").unwrap());
            assert!(!store
                .update_content_fingerprint_if_exists("up", "fp-after-uninstall")
                .unwrap());
            assert!(store.get("up").unwrap().is_none());
        });
    }

    /// SKILL.md 原说明备份：首写留存（含空串哨兵）、清 key、仅 Upload 可写、
    /// upsert_preserving 保留。
    #[test]
    fn skill_desc_backup_roundtrip_and_gates() {
        with_temp_home(|| {
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
}
