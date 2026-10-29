//! 代码会话 checkpoint：每轮用户消息（turn）开始时对执行根做快照，支持回滚。
//!
//! 移植自 fork 分支 `qiuYliangM/feat-full-code-mode` 提交 `32b5fdf9e` 的
//! `code_sessions/checkpoints.rs`（设计文档 `docs/code-mode-改动随对话回退-设计.md`
//! §3），砍掉 ACP 钩子、仅保留品悟原生 code 车道。与 feat 分支的差异：
//! - 模块落位改为 `features/code_checkpoints`（main 无 `code_sessions` 拆分）；
//! - Turn-count semantics corrected: counting by `role == "user"` on the feat
//!   branch would include tool_result entries; the predicate in `turns` (a
//!   private module) matches fork `8cc61b609`'s `is_user_turn_prompt` instead.
//!
//! 快照策略：**每会话一个影子 git 仓库**（shadow git-dir 落账本根
//! `checkpoints/repo`，`--work-tree` 指向执行根）。
//!
//! 选型论证（对比任务书给出的两条路线）：
//! - 「git 项目记 HEAD + diff patch，非 git 项目复制被写文件」需要两条恢复路径，
//!   且非 git 分支要精确还原必须在本轮写类工具执行**前**拦截拿到原始内容——拦截
//!   点在 Engine 工具循环（CodeWhale fork）内，越出 app 侧边界；事后补复制拿不到
//!   原始内容，根本不可行。
//! - 影子仓库对 git/非 git 执行根一视同仁；所有 git 写操作都带 `--git-dir=<影子>`，
//!   **从不触碰用户项目自己的 `.git`**（不动 index/stash/ref，比 stash/apply 安全）；
//!   内容寻址天然去重；每条 checkpoint 一个 `refs/checkpoints/<id>` ref 保持对象
//!   可达，LRU 裁剪 = 删索引条目 + 删 ref + 后台 gc。
//! - 已知限制：被 gitignore（含影子 exclude 列表）排除的文件不进入快照，恢复时
//!   不会删除 turn 中新建的此类文件；`clean -fd` 不用 `-x`，避免误删 node_modules。
//!
//! 数据布局（账本根下，与审计/产物同体系；会话删除时随私有目录整体清理）：
//! ```text
//! <ledger>/checkpoints/
//!   repo/        # 影子 git-dir（objects/refs/index/info/exclude）
//!   index.json   # CheckpointIndex { version, entries }，按创建顺序，上限 20
//! ```
//!
//! 恢复语义：`read-tree <commit>` + `checkout-index -f -a` 把快照内容写回执行根，
//! `clean -fd` 删除快照后新建的文件；恢复前先自动打一个「回滚点」快照，回滚可反悔。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) mod turns;

pub(crate) use turns::count_user_turns;
pub use turns::count_user_turns_in_json;

/// 每会话保留的 checkpoint 上限（LRU，超出裁掉最老条目）。
const MAX_CHECKPOINTS: usize = 20;
/// Cap on the diff preview's patch text (truncated beyond it; the `changes`
/// list is unaffected).
///
/// Module-private on purpose: the cap is applied where the patch is produced
/// and the `patchTruncated` flag travels with the data, so a patch renderer
/// never needs this constant. Keep it private unless a consumer actually
/// imports it — a speculative `pub` here is how a surface the dead-code
/// cleanups removed comes back without a reader.
const DIFF_PATCH_LIMIT: usize = 512 * 1024;
/// 执行根体积门（对齐底座 snapshot 的 DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT）：
/// 超过 2GB 的目录不做快照——每轮全量 `add -A` 的 IO/CPU 与影子仓库存储都不
/// 划算，该会话如实没有回退入口（设计 §5 降级语义）。
const MAX_WORKSPACE_BYTES_FOR_SNAPSHOT: u64 = 2 * 1024 * 1024 * 1024;
/// 体积估算的条目数上限（超出按超限处理，防止巨型目录树把估算本身变成负担）。
const SIZE_WALK_MAX_ENTRIES: usize = 200_000;
/// 影子仓库存储预算（对齐底座 MAX_SNAPSHOT_SIZE_MB）：LRU 裁条目不裁字节，
/// 大文件项目单靠条目数守不住体积；超出时裁到一半条目并 gc 收敛。
const MAX_SHADOW_REPO_BYTES: u64 = 500 * 1024 * 1024;

/// 影子仓库的 exclude 列表：与 workspace 浏览的忽略目录对齐，避免把依赖/构建
/// 产物纳入快照（git 项目自身的 .gitignore 会被影子仓库自然尊重，无需重复）。
/// `.Pinvou-Icase-Probe-*` 是 `fs_is_case_insensitive` 的探针文件：同根另一
/// 会话的快照可能在探针存活的毫秒窗口内 `add -A` 把它卷进快照，显式排除。
const SHADOW_EXCLUDES: &[&str] = &[
    ".git/",
    ".hg/",
    ".svn/",
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".next/",
    ".cache/",
    "__pycache__/",
    ".venv/",
    "venv/",
    ".Pinvou-Icase-Probe-*",
];

/// 敏感文件模式：秘密实际居住的约定位置（.env.local 系、证书/私钥本体、
/// 含 token 的包管理凭据）。原文不进快照、不进 diff 预览——非 git 执行根
/// （临时会话、未初始化目录）没有 .gitignore 兜底，`add -A` 会把它们快照进
/// 影子 objects。匹配恒为大小写不敏感（exclude 用字符类、purge 用 `:(icase)`、
/// 过滤用 ASCII 折叠），`.ENV`/`ID_RSA` 等变体同样命中。收窄的取舍：
/// .env.example/.env.sample/id_rsa.pub 等常被有意提交的示例/公钥照常进快照、
/// 随回退恢复；.env.production 等约定文件仍会进快照（其内容通常可提交）。
/// 扩展名对齐 file_ingest.rs 的 secret 分类
/// （key/pem/p12/pfx/keystore/jks/gpg/pgp）与 gitleaks 的私钥约定。
/// 代价是这些文件不随回退恢复，属可接受取舍（设计文档已知限制有记录）。
const SECRET_EXCLUDES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.*.local",
    ".npmrc",
    ".netrc",
    ".git-credentials",
    "credentials.json",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.pkcs12",
    "*.keystore",
    "*.jks",
    "*.kdbx",
    "*.gpg",
    "*.pgp",
    "id_rsa",
    "id_ed25519",
    "id_dsa",
    "id_ecdsa",
    // r27 M4 + r28 M3：平台 path_policy / safety_deny_rules 同类凭据（ingest
    // 侧整目录拦截；快照侧按内容 glob 粒度补齐——含斜杠条目经 B2 双腿派生
    // 覆盖嵌套副本；`.tfstate*` 状态文件含明文 secret 变量）。
    ".aws/credentials",
    ".aws/config",
    ".aws/sso/cache/**",
    ".kube/config",
    ".docker/config.json",
    ".gnupg/**",
    ".password-store/**",
    ".ssh/**",
    ".azure/accessTokens.json",
    "application_default_credentials.json",
    "terraform.tfstate*",
    ".my.cnf",
    ".pgpass",
    // r29 minor 6：path-policy 清单余量（快照仅本地，calibrated minor）。
    "*.tfstate*",
    ".pypirc",
    ".envrc",
    "_netrc",
    "secrets.yaml",
    "service-account*.json",
];

/// 把敏感模式转成大小写不敏感的 gitignore 模式：ASCII 字母逐个展开为 `[xX]`
/// 字符类，其余字符（`.`/`_`/数字/`*`）原样保留。info/exclude 的匹配大小写
/// 行为由影子仓库 core.ignorecase 决定（按执行根文件系统探测，见 ensure_repo），
/// 字符类让秘密排除在大小写敏感文件系统上同样命中 `.ENV`/`ID_RSA` 等变体，
/// 而 core.ignorecase 不被强制——`Makefile`/`makefile` alias 语义不受影响
/// （评审 B1/M1 的最终口径）。
fn icase_gitignore_pattern(pattern: &str) -> String {
    pattern
        .chars()
        .map(|character| match character.to_ascii_lowercase() {
            lower @ 'a'..='z' => format!("[{lower}{}]", lower.to_ascii_uppercase()),
            other => other.to_string(),
        })
        .collect()
}

/// r28 B2：info/exclude 与 clean -e 的排除形式派生——gitignore 语义里含
/// 斜杠的模式锚定仓库根，嵌套副本（`sub/.aws/credentials`）不被裸形式命中
/// （clean -fd 会物理删除它、add -A 会暂存它）。返回「裸根 icase 形式 +
/// `**/` 嵌套 icase 形式」双腿（无斜杠模式单腿）；目录内容 glob
/// （`.ssh/**`）两腿同形。
fn icase_ignore_legs(pattern: &str) -> Vec<String> {
    let bare = icase_gitignore_pattern(pattern);
    if pattern.contains('/') {
        vec![bare.clone(), format!("**/{bare}")]
    } else {
        vec![bare]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointKind {
    /// turn 开始时的自动快照。
    Turn,
    /// 恢复前自动打的「回滚点」快照（保证回滚可反悔）。
    PreRestore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointMeta {
    /// 稳定 id（`c<序号>-<纳秒>`），前端回滚/diff 时回传。
    pub id: String,
    /// 第几个用户 turn（1-based）；计数失败时为 None，前端按顺序兜底对齐。
    pub turn: Option<u32>,
    pub kind: CheckpointKind,
    /// Display label. Always present on the wire; write-side values go
    /// through `normalize_checkpoint_label` (trim + 80-char cap counted by
    /// chars so a CJK label never splits). Reads are `serde(default)` for
    /// compatibility: main-era indexes carry no `label` key, and a missing
    /// key would fail the whole index parse into the quarantine branch,
    /// which rebuilds from an empty index — an upgrade that loses history.
    #[serde(default)]
    pub label: String,
    /// 影子仓库中的 commit sha（orphan commit，互不为父子）。
    pub commit: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointIndex {
    version: u32,
    #[serde(default)]
    entries: Vec<CheckpointMeta>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointChange {
    pub path: String,
    /// `added` / `modified` / `deleted` / `renamed` / `copied` / `other`
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointDiff {
    pub checkpoint: CheckpointMeta,
    /// 从快照到当前执行根的变更清单（即「回滚将撤销的变更」）。
    pub changes: Vec<CheckpointChange>,
    /// Unified diff text (possibly truncated; empty when the caller asked to
    /// skip patch generation).
    pub patch: String,
    pub patch_truncated: bool,
}

fn checkpoints_dir(ledger_root: &Path) -> PathBuf {
    ledger_root.join("checkpoints")
}

fn repo_dir(ledger_root: &Path) -> PathBuf {
    checkpoints_dir(ledger_root).join("repo")
}

fn index_path(ledger_root: &Path) -> PathBuf {
    checkpoints_dir(ledger_root).join("index.json")
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

/// 估算执行根体积（跳过 exclude 目录、内嵌的 checkpoint 账本目录与符号链接；
/// 条目数超 SIZE_WALK_MAX_ENTRIES 或读取失败返回 None——按「不在预算内」处理，
/// 宁可跳过快照也不低估；读取失败必须如实记日志，调用方的「超预算」文案
/// 分不清两类原因，单个不可读目录会把该会话快照永久静默禁用——评审 nit）。
/// 临时会话两根相同，影子仓库自身会被反复快照增长，必须排除，否则它会把
/// 自己顶过 2GB 门（评审 nit）。
fn estimate_workspace_bytes(execution_root: &Path, ledger_root: &Path) -> Option<u64> {
    let skip_dirs: std::collections::HashSet<&str> = SHADOW_EXCLUDES
        .iter()
        .map(|line| line.trim_end_matches('/'))
        .collect();
    let ledger_canonical =
        fs::canonicalize(ledger_root).unwrap_or_else(|_| ledger_root.to_path_buf());
    // 影子仓库目录同样不计入预算（评审 r9 minor 4）：非绑定会话的账本根即
    // 遍历根，「跳过账本目录」只对子目录生效，checkpoints/ 会被计入——
    // 快照自身把会话顶过预算线并触发粘性禁用。常量行见 SHADOW_EXCLUDES 的
    // 目录级排除（这里按 canonical 前缀精确跳过，不受目录名匹配限制）。
    let shadow_dir_canonical = fs::canonicalize(&checkpoints_dir(ledger_root))
        .unwrap_or_else(|_| checkpoints_dir(ledger_root));
    let mut total: u64 = 0;
    let mut entries: usize = 0;
    let mut stack = vec![execution_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = match fs::read_dir(&dir) {
            Ok(read) => read,
            // 目录已消失（评审 r10 M4）：入栈后被删是 build/test/watcher 清临时
            // 目录的常态竞态，消失的目录贡献 0 字节而非「不可读」——按 skip 处
            // 理，不再触发粘性禁用把整个会话的回退静默永久关掉。其余错误
            // （EACCES 等）维持 fail-closed。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "[checkpoints] 体积估算读取目录失败（按不在预算内处理，快照跳过）: {}: {error:#}",
                    dir.display()
                );
                return None;
            }
        };
        for entry in read {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "[checkpoints] 体积估算读取目录项失败（按不在预算内处理，快照跳过）: {}: {error:#}",
                        dir.display()
                    );
                    return None;
                }
            };
            entries += 1;
            if entries > SIZE_WALK_MAX_ENTRIES {
                return None;
            }
            // 不跟随符号链接（file_type 自身不触发跟随）。
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                // 同款 NotFound 竞态（评审 r10 M4）：条目消失按 skip。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    eprintln!(
                        "[checkpoints] 体积估算读取文件类型失败（按不在预算内处理，快照跳过）: {}: {error:#}",
                        entry.path().display()
                    );
                    return None;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let path = entry.path();
                let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if canonical == ledger_canonical || canonical == shadow_dir_canonical {
                    continue;
                }
                // r24 B1：跳过集按小写比较——大小写变体构建目录（Dist/）与
                // info/exclude 的 icase 字符类同向，否则预算遍历与排除语义
                // 在大小写敏感文件系统上劈叉。
                if !skip_dirs.contains(
                    entry
                        .file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                ) {
                    stack.push(path);
                }
            } else {
                // 与目录级 fail-closed 对齐（评审 r10 M4）：NotFound（条目刚被删）
                // 跳过；其余 metadata 错误按不可读处理——此前一律静默计 0，方向
                // 与目录级相反。
                match entry.metadata() {
                    Ok(metadata) => {
                        total += metadata.len();
                        if total > MAX_WORKSPACE_BYTES_FOR_SNAPSHOT {
                            return Some(total);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        eprintln!(
                            "[checkpoints] 体积估算读取文件元数据失败（按不在预算内处理，快照跳过）: {}: {error:#}",
                            entry.path().display()
                        );
                        return None;
                    }
                }
            }
        }
    }
    Some(total)
}

/// 正向判定缓存有效期（r19 M7）：窗口内复用上次「在预算内」结论，免每条
/// 消息全树走读（大树上 0.2-3s 的 time-to-first-token 开销）。体积门是成本
/// 闸（2GB 上限保护快照 IO 与影子库膨胀），非正确性闸——窗口内长过限的最坏
/// 后果是多付一次快照成本，且有 2× 预算硬停兜底；120s 平衡「每消息走读」
/// 与「超限形状的感知延迟」。
const BUDGET_OK_TTL_SECONDS: i64 = 120;

/// 执行根是否在快照体积预算内（chat.rs 在 turn 快照前调用；超限的会话如实
/// 没有回退入口，不算错误）。
///
/// 负向粘性判定（评审 r7 M4 + minor 11）：首次「不在预算内」（超限/条目超
/// 上限/任何读取错误按 None 处理）即在账本根落 `budget-disabled` 标记，此后
/// 每次 turn 直接读标记短路——不再重复全树遍历（超限形状此前每条消息都白付
/// 一次 20 万条目级 walk + 逐目录 canonicalize，且只有一条 info 日志、用户
/// 无感知）。首次落标记升级为 `log::warn`。标记只随账本根存续；用户换绑/
/// 清空目录后删除会话即可重置。
///
/// 正向 TTL 缓存（r19 M7）：在预算内的会话此前同样每条消息重走全树——预算
/// 内才是长期稳态。`budget-ok` 标记内容为 Unix 秒时间戳（不依赖文件系统
/// 时间戳精度），过期/损坏按无缓存处理重新走读。
pub fn execution_root_within_snapshot_budget(execution_root: &Path, ledger_root: &Path) -> bool {
    let marker = ledger_root.join("budget-disabled");
    if marker.exists() {
        return false;
    }
    let ok_marker = ledger_root.join("budget-ok");
    if let Ok(bytes) = fs::read(&ok_marker) {
        if let Ok(stamp) = String::from_utf8_lossy(&bytes).trim().parse::<i64>() {
            if now_seconds().saturating_sub(stamp) < BUDGET_OK_TTL_SECONDS {
                return true;
            }
        }
    }
    let within = estimate_workspace_bytes(execution_root, ledger_root)
        .is_some_and(|bytes| bytes <= MAX_WORKSPACE_BYTES_FOR_SNAPSHOT);
    if !within {
        log::warn!(
            "[checkpoints] 执行根不在快照预算内（超限或不可读），本会话的回退入口已停用：{}",
            execution_root.display()
        );
        // 标记写入失败不改变本次判定（本次已拒绝）；只是下次会再走一遍估算。
        // r14 M-A：与其它 ledger 文件同款原子私有写。
        let _ = crate::platform::filesystem::atomic_write_private(&marker, b"");
        let _ = fs::remove_file(&ok_marker);
    } else {
        let _ = crate::platform::filesystem::atomic_write_private(
            &ok_marker,
            now_seconds().to_string().as_bytes(),
        );
    }
    within
}

/// 探测执行根文件系统是否大小写不敏感：写入混合大小写探针文件，再以全大写
/// 形式 stat——能读到同一文件即不敏感。探测失败（目录不可写等）按「敏感」
/// 处理：探测结果只喂给 core.ignorecase（决定 Makefile/makefile alias 等
/// git 原生语义）；秘密的三层防护（exclude/purge/过滤）恒为大小写不敏感，
/// 不依赖探测结果（见 ensure_repo 的 info/exclude 注释）。
fn fs_is_case_insensitive(dir: &Path) -> bool {
    // r33 minor 13：探针名追加纳秒后缀——pid 键名在同根并发会话间会
    // AlreadyExists 冲突、第二个 ensure_repo 得到假阴性。
    let probe = dir.join(format!(
        ".Pinvou-Icase-Probe-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    let insensitive = match fs::File::create_new(&probe)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b""))
    {
        Ok(()) => {
            let other = dir.join(format!(
                ".PINVOU-ICASE-PROBE-{}-{}",
                std::process::id(),
                now_nanos()
            ));
            fs::metadata(&other).is_ok()
        }
        Err(_) => false,
    };
    let _ = fs::remove_file(&probe);
    insensitive
}

/// 影子仓库当前体积（objects + pack 文件总和；读取失败按 0——压力裁剪是
/// best-effort，量不出来就不裁）。
fn shadow_repo_bytes(repo: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                total += metadata.len();
            }
        }
    }
    total
}

/// 已打包体积（`git count-objects -v` 的 `size-pack`，字节）。只读对象库统计，
/// 不做任何打包/回收动作——供存储压力闸判断超额是否为内容性（打包体积本身
/// 超预算 → gc 无法收敛，评审 M5 断路）。读取失败按 0：门保守走旧路径
/// （裁剪 + gc），与改动前行为一致。
fn packed_repo_bytes(repo: &Path, work_tree: &Path) -> u64 {
    let Ok(output) = git_ok(repo, work_tree, &["count-objects", "-v"]) else {
        return 0;
    };
    for line in output.lines() {
        if let Some(kb) = line.strip_prefix("size-pack:") {
            if let Ok(kb) = kb.trim().parse::<u64>() {
                return kb.saturating_mul(1024);
            }
        }
    }
    0
}

/// checkpoint id 校验：命令入口的路径安全闸（id 会拼进 git ref/日志，不进文件路径，
/// 但仍按 SessionRoots 的 validate 惯例收紧字符集，防注入）。
fn validate_checkpoint_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        bail!("Invalid checkpoint id '{id}'");
    }
    Ok(())
}

/// 执行根必须存在且是目录；否则如实报错（项目目录被删、权限不足都走这里）。
fn canonical_execution_root(execution_root: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(execution_root)
        .with_context(|| format!("执行根不可用: {}", execution_root.display()))?;
    if !canonical.is_dir() {
        bail!("执行根不是目录: {}", canonical.display());
    }
    Ok(canonical)
}

/// 影子仓库专用 git 命令：剥离宿主环境的 `GIT_*` 变量——启动 shell 里的
/// `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`/`GIT_OBJECT_DIRECTORY` 会把影子
/// 仓库的内部操作重定向到无关仓库，`GIT_CONFIG_*` 注入会覆盖影子仓库的精心
/// 配置；系统/全局 gitconfig 钉死为空（用户全局的 `core.fsmonitor=true` 会对
/// 执行根拉起守护进程）。`PATH`/`HOME` 等基础环境保留（定位 git 可执行文件
/// 与签名/钩子以外的正常行为需要）。
fn isolated_git_command() -> std::process::Command {
    let mut command = crate::platform::process::HiddenCommand::new("git");
    // Strip via a fixed key list (platform::process::strip_all_git_env); do
    // not scan the process env for GIT_*: in parallel tests, setenv from other
    // threads concurrent with the iteration can miss keys, making the
    // isolation fail intermittently as a whole (root cause of the 2026-09-12
    // flaky family).
    crate::platform::process::strip_all_git_env(&mut command);
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", crate::platform::os::null_device());
    command
}

// 快照 git 子进程的兜底预算：`git add -A` 在大工作区上可能确实很慢，但卡死的
// git（NFS/FUSE 停摆、挂死的 hook）不能无限阻塞回合开始前的检查点路径——
// create_checkpoint 的任何错误都会按既有语义降级为「跳过快照并告警」，
// 不会阻塞发送。注意这是**单次 git 调用**的预算，create_checkpoint 一次会
// 顺序执行多条 git 命令（add/write-tree/commit-tree/update-ref/gc），最坏
// 情况是预算的数倍；git 子进程整体有界，但同一等待路径里的普通文件系统 IO
// （目录大小走查、写 exclude 文件）在存储完全停摆时仍可能阻塞——该残余与
// git 预算正交。
const GIT_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 超时 kill 的 git 来不及清理锁文件，之后每次调用都会「File exists」
/// 快速失败——一次超时会把这个会话的检查点能力永久打没。锁龄超过
/// GIT_COMMAND_TIMEOUT 即可断定持锁者已死（活着的 git 至多持锁一个预算周期，
/// 影子仓库为本应用私目录），清除并让调用方重试一次；更年轻的锁可能是并发
/// 活锁，不动。
///
/// 覆盖三类锁（`git()` 的重试谓词按 stderr 含 `.lock` 触发）：
/// `index.lock`（add/write-tree）、`config.lock`（ensure_repo 每次快照都
/// 幂等校正 `core.ignorecase`）、`refs/**/*.lock`（update-ref，每次内容
/// 变更的回合都会写 `refs/checkpoints/head`）。
fn clear_stale_git_locks(repo: &Path) -> bool {
    clear_stale_git_locks_older_than(repo, GIT_COMMAND_TIMEOUT)
}

/// [`clear_stale_git_locks`] 的可注入阈值变体：`min_age` 生产路径传
/// [`GIT_COMMAND_TIMEOUT`]，测试传更小/零值即可钉住阈值与覆盖面，
/// 无需回拨 mtime。
fn clear_stale_git_locks_older_than(repo: &Path, min_age: std::time::Duration) -> bool {
    let is_stale = |path: &Path| -> bool {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= min_age)
    };
    let mut cleared = false;
    for name in ["index.lock", "config.lock", "HEAD.lock", "packed-refs.lock"] {
        let lock = repo.join(name);
        if is_stale(&lock) && std::fs::remove_file(&lock).is_ok() {
            eprintln!(
                "[checkpoints] cleared stale lock left by a killed git: {}",
                lock.display()
            );
            cleared = true;
        }
    }
    // update-ref 的锁在 refs 子树里（refs/checkpoints/head.lock、按 id 的
    // 逐检查点锁）。refs 层级固定很浅，定深遍历避免意外放大。
    fn sweep_refs(dir: &Path, depth: u8, is_stale: &dyn Fn(&Path) -> bool) -> bool {
        if depth == 0 {
            return false;
        }
        let mut cleared = false;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                cleared |= sweep_refs(&path, depth - 1, is_stale);
            } else if path.extension().is_some_and(|ext| ext == "lock")
                && is_stale(&path)
                && std::fs::remove_file(&path).is_ok()
            {
                eprintln!(
                    "[checkpoints] cleared stale lock left by a killed git: {}",
                    path.display()
                );
                cleared = true;
            }
        }
        cleared
    }
    cleared |= sweep_refs(&repo.join("refs"), 4, &is_stale);
    cleared
}

/// git 因发现残留锁而拒绝执行（退出码非零、stderr 指名某个 .lock 路径）时，
/// 清掉确证为陈旧的锁重试一次。[`git`] 与 [`git_ref`] 共用：被超时 kill 的
/// git 同样会死在 update-ref / 删 ref 的路上——若只给写路径接清扫，
/// drop/invalidate 就会留下永远清不掉的 ref 锁，索引条目已删而 ref 仍在，
/// gc 永远收不回对象，影子仓库随之泄漏。
fn run_git_with_stale_lock_retry(
    run_bounded: impl Fn() -> Result<std::process::Output>,
    repo: &Path,
) -> Result<std::process::Output> {
    let output = run_bounded()?;
    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains(".lock")
        && clear_stale_git_locks(repo)
    {
        return run_bounded();
    }
    Ok(output)
}

/// [`git`] 与 [`git_ref`] 的公共主体：组装 `--git-dir`（及可选 `--work-tree`）
/// 的隔离 git 命令并以带超时方式执行，交 [`run_git_with_stale_lock_retry`]
/// 收尾；`run_error` 保留两条调用链各自的失败文案。
fn git_bounded(
    repo: &Path,
    work_tree: Option<&Path>,
    arguments: &[&str],
    run_error: impl Fn(&str) -> anyhow::Error,
) -> Result<std::process::Output> {
    let run_bounded = || {
        let mut command = isolated_git_command();
        command.arg(format!("--git-dir={}", repo.display()));
        if let Some(work_tree) = work_tree {
            command.arg(format!("--work-tree={}", work_tree.display()));
        }
        // r15 M3：影子仓库位于 agent 可写的工作树内（非绑定布局），其 config 文件可被追加
        // [core] fsmonitor = <任意命令> 等键——文本写入即命令执行，绕过工具审批。所有命令统一钉死这三个执行面：
        // fsmonitor（命令执行）、hooksPath（hook 执行）、attributesFile（任意过滤器）。
        command.arg("-c").arg("core.fsmonitor=false");
        command.arg("-c").arg("core.hooksPath=");
        command.arg("-c").arg("core.attributesFile=");
        command.args(arguments);
        crate::platform::process::output_with_timeout_and_kill_tree(command, GIT_COMMAND_TIMEOUT)
            .map_err(|error| run_error(&error))
    };
    run_git_with_stale_lock_retry(run_bounded, repo)
}

fn git(repo: &Path, work_tree: &Path, arguments: &[&str]) -> Result<std::process::Output> {
    git_bounded(repo, Some(work_tree), arguments, |error| {
        anyhow::anyhow!("failed to run git {}: {error}", arguments.join(" "))
    })
}

fn git_ok(repo: &Path, work_tree: &Path, arguments: &[&str]) -> Result<String> {
    let output = git(repo, work_tree, arguments)?;
    if !output.status.success() {
        bail!(
            "git {} 失败: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Same bounded `git` invocation as [`git_ok`], but the command's stdout is
/// redirected to a temp file via `diff --output` and only the first `cap`
/// bytes (plus one probe byte) are read back into memory — a huge diff costs
/// disk in the git child, never RSS in this process. Returns the output
/// clamped to a `\n` boundary (a `diff --git` section header is therefore
/// either fully present or fully absent, which the secret-path filter
/// downstream relies on to drop a section wholesale) and whether the full
/// output exceeded `cap`.
fn git_diff_capped(
    repo: &Path,
    work_tree: &Path,
    arguments: &[&str],
    cap: usize,
) -> Result<(String, bool)> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // The spill lives beside the shadow repo, not in the shared temp
    // directory. The git child writes the UNFILTERED diff here, so on a crash
    // or SIGKILL between the write and `TempDiffFile`'s drop the leftover is
    // a full plaintext copy of the working tree's changes — including
    // whatever the secret filter would have dropped. Under the ledger it is
    // inside the same private tree as the snapshots themselves and goes away
    // with the session; in `/tmp` it is a world-visible artifact with no
    // reaper and an unbounded lifetime.
    let spill_dir = repo.parent().unwrap_or(repo).to_path_buf();
    let output_path = spill_dir.join(format!(
        ".checkpoint-diff-{}-{nanos}.tmp",
        std::process::id()
    ));
    let _cleanup = TempDiffFile(&output_path);
    // Create the spill file ourselves with private permissions: a
    // child-created file would follow the process umask. `git diff --output`
    // truncates an existing file and keeps its mode, so pre-creating with
    // 0600 is enough.
    crate::platform::filesystem::create_secret_file(&output_path).with_context(|| {
        format!(
            "create private checkpoint diff spill file {}",
            output_path.display()
        )
    })?;
    use std::io::Read as _;
    let mut arguments: Vec<String> = arguments
        .iter()
        .map(|argument| (*argument).to_string())
        .collect();
    arguments.push(format!("--output={}", output_path.display()));
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let output = git(repo, work_tree, &arguments)?;
    if !output.status.success() {
        // Deliberately names no arguments: the argv carries the spill path
        // (and this error reaches the frontend via the command layer), and
        // the same privacy posture that keeps the quarantine path and serde
        // message off stderr applies here.
        bail!(
            "git diff 预览生成失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let file_len = fs::metadata(&output_path)?.len();
    let truncated = file_len > cap as u64;
    let mut file = fs::File::open(&output_path)?;
    let mut buf = Vec::new();
    if truncated {
        (&mut file).take(cap as u64 + 1).read_to_end(&mut buf)?;
        let mut end = buf.len().min(cap);
        while end > 0 && buf[end - 1] != b'\n' {
            end -= 1;
        }
        buf.truncate(end);
    } else {
        file.read_to_end(&mut buf)?;
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}

/// Best-effort removal of the `--output` spill file; the diff preview must
/// not leave a full-diff copy behind in the shared temp directory.
struct TempDiffFile<'a>(&'a Path);

impl Drop for TempDiffFile<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}

/// 迁移/恢复共用的敏感文件 pathspec 全集：字面模式（无通配符，如 .env、
/// id_rsa）在 git pathspec 里只命中仓库根部，自动派生 `**/` 前缀版本覆盖任意
/// 深度。r19 M3：含通配符的模式同样无条件派生——pathspec 匹配锚定整条路径，
/// 字面量前导的 glob（.env.*.local）裸形态只命中根部，`apps/web/.env.*.local`
/// 逃过 purge、被 checkout-index 写回、再被 staged 校验永久击穿（M1/M2 链）。
/// `**/` 前缀对星号前导的 glob（*.pem）只是冗余命中，无语义损失。
/// 恒带 `:(icase)`：与 info/exclude 的字符类模式同向（大小写不敏感），三层
/// 口径一致才不会出现「exclude 放行、purge 错杀」的 B1 链路（评审 M1）。
fn secret_pathspecs() -> Vec<String> {
    let mut patterns: Vec<String> = SECRET_EXCLUDES
        .iter()
        .map(|line| format!(":(icase){line}"))
        .collect();
    for line in SECRET_EXCLUDES {
        patterns.push(format!(":(icase)**/{line}"));
    }
    patterns
}

/// SHADOW_EXCLUDES 的 pathspec 全集（r22 R1）：与 secret_pathspecs 同款
/// icase；根腿 `<dir>/` 命中根级目录，嵌套腿 `**/<dir>/**` 命中任意深度
/// 下的内容——r23 minor 实证 `**/<dir>/` 形状在 `rm -r --cached` 的 pathspec
/// 匹配里是空集（index 里只有文件条目、目录条目不落 index），必须是内容
/// 形状 `**` 结尾；`**/` 前缀可匹配零层目录，根级内容两腿都覆盖。
fn shadow_exclude_pathspecs() -> Vec<String> {
    let mut patterns: Vec<String> = SHADOW_EXCLUDES
        .iter()
        .map(|line| format!(":(icase){line}"))
        .collect();
    for line in SHADOW_EXCLUDES {
        patterns.push(format!(":(icase)**/{line}**"));
    }
    patterns
}

/// 账本路径在执行根内的真实相对位置（r19 M1/M4）。`ledger_prefix`：账本根
/// 位于执行根内时的相对前缀（两根相同 = 空串；在外 = None，绑定会话）。
/// `checkpoint_rel`：checkpoints 目录的相对路径（不在执行根内 = None）。
/// 派生与 ensure_repo 的 info/exclude 腿同源：绑定布局下同名项目内容
/// （ML 项目的 training/checkpoints/）是无辜用户数据，不得进校验/purge/
/// clean 排除集——这正是账本腿必须布局条件化的原因（评审 r19 M1b）。
struct LedgerGuardPaths {
    ledger_prefix: Option<String>,
    checkpoint_rel: Option<String>,
}

fn ledger_guard_paths(ledger_root: &Path, execution_root: &Path) -> LedgerGuardPaths {
    let canonical_exec =
        fs::canonicalize(execution_root).unwrap_or_else(|_| execution_root.to_path_buf());
    let canonical_ledger =
        fs::canonicalize(ledger_root).unwrap_or_else(|_| ledger_root.to_path_buf());
    let ledger_prefix = canonical_ledger
        .strip_prefix(&canonical_exec)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .map(|rel| {
            if rel.is_empty() {
                rel
            } else {
                format!("{rel}/")
            }
        });
    let checkpoint_dir = checkpoints_dir(ledger_root);
    let canonical_checkpoint_dir = fs::canonicalize(&checkpoint_dir).unwrap_or(checkpoint_dir);
    let checkpoint_rel = canonical_checkpoint_dir
        .strip_prefix(&canonical_exec)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|rel| !rel.is_empty());
    LedgerGuardPaths {
        ledger_prefix,
        checkpoint_rel,
    }
}

/// 账本 pathspec 全集（:(icase) pathspec 形态，供 staged 校验与 restore 的
/// rm --cached）。绑定布局（账本在工作树外）返回空集。
fn ledger_guard_pathspecs(guard: &LedgerGuardPaths) -> Vec<String> {
    let mut specs: Vec<String> = Vec::new();
    if let Some(prefix) = &guard.ledger_prefix {
        for name in [
            "pending_rewind.json",
            "budget-disabled",
            "budget-ok",
            ".pinvou3",
        ] {
            specs.push(format!(":(icase){prefix}{name}"));
        }
        specs.push(format!(":(icase){prefix}.pinvou3/**"));
    }
    if let Some(rel) = &guard.checkpoint_rel {
        specs.push(format!(":(icase){rel}"));
        specs.push(format!(":(icase){rel}/**"));
    }
    specs
}

/// 账本文件名的 gitignore 形态（供 restore 的 clean -e 排除），与
/// info/exclude 同款的 icase 字符类——裸字面大小写敏感，.ENV/ID_RSA 变体
/// 可穿透（r19 M5 的 clean 腿同因）。绑定布局返回空集：项目同名目录是
/// 无辜内容，clean 的常规语义不得被排挤。
fn ledger_guard_ignore_patterns(guard: &LedgerGuardPaths) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    if let Some(prefix) = &guard.ledger_prefix {
        for name in [
            "pending_rewind.json",
            "budget-disabled",
            "budget-ok",
            ".pinvou3",
        ] {
            patterns.push(icase_gitignore_pattern(&format!("{prefix}{name}")));
        }
        patterns.push(icase_gitignore_pattern(&format!("{prefix}.pinvou3/**")));
    }
    if let Some(rel) = &guard.checkpoint_rel {
        patterns.push(icase_gitignore_pattern(&format!("{rel}/**")));
    }
    patterns
}

/// 从影子 index 清除敏感文件条目（`rm --cached`，只动影子 index，不碰工作区
/// 文件、不碰用户项目 .git）。返回 git 进程的真实成败（非零退出 = 失败）。
///
/// 必须带 `-f`：`git rm --cached` 的 up-to-date 检查用相对路径 lstat，解析到
/// 调用进程的 cwd 而非影子 work-tree（git 2.55 实测，`--cached` 跳过
/// setup_work_tree）——cwd 恰含同名文件时检查打到无关文件上、purge 全有或全
/// 无地失败。`-f` 与 `--cached` 组合只绕过该检查，工作区文件绝不被碰。
fn purge_secret_patterns_from_index(repo: &Path, work_tree: &Path) -> Result<()> {
    let patterns = secret_pathspecs();
    let mut arguments: Vec<&str> = vec![
        "rm",
        "-r",
        "--cached",
        "--ignore-unmatch",
        "--quiet",
        "-f",
        "--",
    ];
    arguments.extend(patterns.iter().map(String::as_str));
    let output = git(repo, work_tree, &arguments)?;
    if !output.status.success() {
        bail!(
            "git rm --cached 敏感文件失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// r32 MAJOR-3 backstop：icase pathspec 的 rm 每次调用只移除一个大小写
/// 变体（git 2.43 实证：.env 与 .ENV 同存时 `:(icase).env` 仅移除 .ENV）。
/// purge 后按同组 pathspec 列出幸存条目，再逐条精确路径 `rm --cached`
/// （精确路径 rm 无此怪癖），确保任何变体都无法抵达 checkout-index。
fn purge_secret_survivors_exactly(repo: &Path, work_tree: &Path) -> Result<()> {
    let patterns = secret_pathspecs();
    // r33 minor 6：-z（NUL 分隔）——ls-files 的 C 引号输出会把含引号/反斜杠
    // 的幸存路径变成不可解析形态，rm 回扫失败会让 restore 确定性中止。
    let mut list_args: Vec<&str> = vec!["ls-files", "-z", "--"];
    list_args.extend(patterns.iter().map(String::as_str));
    let survivors = git_ok(repo, work_tree, &list_args)?;
    for path in survivors
        .as_bytes()
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let output = git(
            repo,
            work_tree,
            &[
                "rm",
                "-r",
                "--cached",
                "--quiet",
                "-f",
                "--",
                &String::from_utf8_lossy(path),
            ],
        )?;
        if !output.status.success() {
            bail!(
                "git rm --cached 幸存变体失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}

/// 从影子 index 清除被否定行击穿而暂存的 SHADOW_EXCLUDES 条目（r22 R1）。
/// 与秘密腿不同：依赖/构建目录命中不 bail——标准 keepfile 惯用法
/// （`dist/*` + `!dist/.gitkeep`）会把无辜 keepfile 送进 pathspec 命中集，
/// bail 等于让该会话快照永久停摆。这里只 `rm --cached`（只动影子 index、
/// 不碰工作区），把排除语义恢复为「不进快照」而非「拒绝快照」；秘密腿的
/// fail-closed 仍由 validate 的 bail 承担。apply_restore 有意不用本函数：
/// r21 前的旧快照 tree 可能合法含依赖内容，恢复必须保真（见 1652 注）。
fn purge_shadow_excludes_from_index(repo: &Path, work_tree: &Path) -> Result<()> {
    let patterns = shadow_exclude_pathspecs();
    let mut arguments: Vec<&str> = vec![
        "rm",
        "-r",
        "--cached",
        "--ignore-unmatch",
        "--quiet",
        "-f",
        "--",
    ];
    arguments.extend(patterns.iter().map(String::as_str));
    let output = git(repo, work_tree, &arguments)?;
    if !output.status.success() {
        bail!(
            "git rm --cached 影子排除目录失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Crash-leftover reaper for diff spill files: `.checkpoint-diff-<pid>-<nanos>.tmp`
/// holds the UNFILTERED diff until `TempDiffFile` drops it, and a hard kill
/// leaves it behind with no in-process owner. Age-gated far past the 120 s
/// git command timeout so a concurrently running diff (this or another
/// process) is never reaped mid-flight. Best-effort: sweep failures are
/// silent, the next ensure_repo retries.
fn reap_stale_diff_spills(checkpoints_dir: &Path) {
    const STALE_SPILL_AGE: Duration = Duration::from_secs(600);
    let Ok(entries) = fs::read_dir(checkpoints_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(".checkpoint-diff-")
        {
            continue;
        }
        let is_stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_SPILL_AGE);
        if is_stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 初始化影子仓库（幂等）：git-dir 在账本根，work-tree 指向执行根。
/// - `core.autocrlf false`：不受用户全局行尾配置影响，快照/恢复保持原字节；
///   执行根自己的 .gitattributes 仍会被尊重（与用户在项目内看到的行尾一致）。
/// - `core.ignorecase`：按执行根文件系统语义探测设置（`fs_is_case_insensitive`），
///   每次 ensure 幂等校正。它只承载 git 原生大小写语义：无条件强制 true 会在
///   大小写敏感文件系统上触发 git alias 冲突（评审 M1）——`Makefile`/`makefile`
///   共存让 `add -A` 整体 fatal（快照静默全灭）、已跟踪别名的新文件被静默跳过、
///   仅大小写改名会记成空树——探测设置后这些都不触发（大小写不敏感系统上才
///   承受其语义，那里 alias 冲突本来就不可能同时存在两个仅大小写不同的文件）。
/// - `info/exclude`：忽略依赖/构建目录 + checkpoint 目录自身（临时会话两根相同，
///   checkpoint 数据在执行根内，必须排除，否则快照自我递归、clean 会误删账本）；
///   敏感模式写成恒大小写不敏感的字符类形式（`icase_gitignore_pattern`），使
///   exclude / purge / 预览过滤三层在任何文件系统上都命中相同的大小写变体集合，
///   闭合评审 B1 的事故链（`.ENV` 逃过 exclude 却被 icase purge 移出 index、
///   被 `clean -fd` 物理删除）。
fn ensure_repo(ledger_root: &Path, execution_root: &Path) -> Result<PathBuf> {
    reap_stale_diff_spills(&checkpoints_dir(ledger_root));
    let repo = repo_dir(ledger_root);
    let fresh = !repo.join("HEAD").is_file();
    if fresh {
        fs::create_dir_all(&repo)
            .with_context(|| format!("创建 checkpoint 仓库目录失败: {}", repo.display()))?;
        git_ok(&repo, execution_root, &["init"])?;
        git_ok(&repo, execution_root, &["config", "core.autocrlf", "false"])?;
    }
    // config 幂等：按执行根 fs 语义探测（存量仓库同样每次校正）。该配置只承载
    // git 原生大小写语义（Makefile/makefile alias、仅大小写改名）；秘密排除的
    // 大小写不敏感由 info/exclude 的字符类模式保证，不靠此配置（见下）。
    let ignorecase = if fs_is_case_insensitive(execution_root) {
        "true"
    } else {
        "false"
    };
    git_ok(
        &repo,
        execution_root,
        &["config", "core.ignorecase", ignorecase],
    )?;
    // r19 B1：命令执行面三重防御——worktree `.gitattributes` 写 filter/diff/merge/
    // encoding 属性 + 影子 repo config（agent 可写）或 promisor/partialclone/
    // sshCommand/protocol.ext 键可驱动任意命令执行。
    // 1) info/attributes 最高优先级 `* !filter` 等四条钉死 worktree 属性引用
    //    （probe 验证有效：即使 config 里有 filter 定义，`!filter` 属性使其不被调用）。
    // 2) 移除 config 中的 filter/extensions/remote/protocol 危险键（r17 的实现
    //    是死代码：`--name-only --get-regexp` 输出 `filter.pwn.clean`（key 名，
    //    无空格），`find(' ')` 恒 None；修复为截取最后一个点之前的节名）。
    // 3) 未来加固方向：将影子仓库移出 agent 可写根。
    let info_attrs = repo.join("info").join("attributes");
    if let Some(parent) = info_attrs.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // r24 minor 3：与 info/exclude 的 r13-M3 修复同款——非原子写可被并发的
    // git 进程读到半截钉（半行 = 部分属性不生效）。同目录临时文件 + rename。
    let attrs_tmp = info_attrs.with_extension("tmp");
    fs::write(
        &attrs_tmp,
        "* !filter\n* !diff\n* !merge\n* !working-tree-encoding\n",
    )
    .context("写入 info/attributes 临时文件失败")?;
    fs::rename(&attrs_tmp, &info_attrs).context("替换 info/attributes 失败")?;
    // r23 M4：`--name-only --get-regexp` 输出的 section/key 一律小写——旧
    // 正则的大小写敏感 `core\.sshCommand` 永不命中（死代码，与 r17 的「输出
    // 形状击败解析器」同类）；subsection 可含点（`remote.evil.x.url` 实证
    // 逃逸 `[^.]+`），放宽为 `.+`。
    const SCRUB_DANGEROUS_KEYS_RE: &str = r"^(filter\.|diff\.|extensions\.partialclone|remote\..+\.promisor|remote\..+\.url|core\.sshcommand|protocol\.)";
    let dangerous_keys = match git(
        &repo,
        execution_root,
        &[
            "config",
            "--local",
            "--name-only",
            "--get-regexp",
            SCRUB_DANGEROUS_KEYS_RE,
        ],
    ) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        // exit 1 = 无匹配（get-regexp 语义），非失败。
        Ok(output) if output.status.code() == Some(1) => String::new(),
        Ok(output) => {
            // r23 M4：清洗探针失败不再静默吞掉（fail-open 但必须响亮）——
            // 影子 config 本轮保留未清洗的危险键，下轮 ensure_repo 重试。
            log::error!(
                "[checkpoints] dangerous-config scrub probe failed (exit {:?}); unscrubbed keys remain this round: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            String::new()
        }
        Err(error) => {
            log::error!(
                "[checkpoints] dangerous-config scrub probe could not run; unscrubbed keys remain this round: {error:#}"
            );
            String::new()
        }
    };
    for line in dangerous_keys.lines() {
        let key = line.trim();
        if key.is_empty() {
            continue;
        }
        // r19 B1: `--name-only` outputs `filter.pwn.clean`; the section is
        // `filter.pwn` — trim the last dotted component. For top-level keys
        // like `core.sshCommand`, the section is `core`.
        let section = match key.rfind('.') {
            Some(idx) if idx > 0 => {
                let candidate = &key[..idx];
                // The section itself may contain dots (e.g. `filter.pwn`);
                // strip trailing `.clean`/`.smudge`/`.url` etc by finding the
                // section/subsection boundary. Git sections are `a.b` or
                // `a."b c"`; `filter.pwn.clean` → section `filter.pwn`.
                // Check if the candidate still looks like a subsection key
                // (not a known leaf like clean/smudge/required/url/promisor/
                // sshCommand/partialclone/allow).
                const LEAVES: [&str; 11] = [
                    "clean",
                    "smudge",
                    "required",
                    "url",
                    "promisor",
                    // `--name-only --get-regexp` 输出恒小写（r23 M4）：混合
                    // 大小写的 "sshCommand" 永不命中。
                    "sshcommand",
                    "partialclone",
                    "allow",
                    // r20 M1：diff 驱动键族（diff.external 顶层 / diff.<driver>.
                    // command|textconv 子节）。info/attributes 的 `* !diff` 只
                    // unset 属性、把路径路由回 diff.external——挡不住 config，
                    // 必须清洗节本体。
                    "external",
                    "command",
                    "textconv",
                ];
                if let Some(dot2) = candidate.rfind('.') {
                    let possible_leaf = &candidate[dot2 + 1..];
                    if LEAVES.contains(&possible_leaf) {
                        &candidate[..dot2]
                    } else {
                        candidate
                    }
                } else {
                    candidate
                }
            }
            _ => key,
        };
        if !section.is_empty() {
            // r24 minor 1：单段顶层键（core.sshcommand）不得 --remove-section
            // core——那会把 ensure_repo 几行前刚写下的 core.autocrlf /
            // ignorecase / filemode 钉一并抹掉。顶层节精确 --unset-all 去键，
            // 多段节（filter.pwn / remote.evil）仍整节移除。
            if !section.contains('.') {
                let _ = git(
                    &repo,
                    execution_root,
                    &["config", "--local", "--unset-all", key],
                );
            } else {
                let _ = git(
                    &repo,
                    execution_root,
                    &["config", "--local", "--remove-section", section],
                );
            }
        }
    }
    // 排除列表 = 忽略目录（恒大小写字符类，r24 B1）+ 敏感模式（同款字符类）。
    // 三层防护必须同向（评审 B1）：exclude（本文件）让 .ENV 类与大小写变体
    // 构建目录（Dist/）永不进快照、且因「被 ignored」不受 clean -fd 波及；
    // purge pathspec（恒 icase）清掉存量 index 条目；预览过滤（恒 icase）
    // 挡住 legacy 快照里的原文。r24 B1 实证：shadow 腿裸小写时，大小写敏感
    // 文件系统上的 Dist/ 不被 ignore、被 icase purge 移出 index、restore 的
    // clean -fd 物理删除会话前就存在的真实文件且预览不可见——字符类让三层
    // 在任何文件系统上闭合。
    let mut excludes: Vec<String> = SHADOW_EXCLUDES
        .iter()
        .flat_map(|line| icase_ignore_legs(line))
        .chain(
            SECRET_EXCLUDES
                .iter()
                .flat_map(|line| icase_ignore_legs(line)),
        )
        .collect();
    // ledger 与执行根都可能未经 canonicalize（Windows 短名/大小写），统一规范化后
    // 再判断包含关系，确保临时会话（两根相同）的排除规则一定生效。
    let checkpoint_dir = checkpoints_dir(ledger_root);
    let canonical_checkpoint_dir = fs::canonicalize(&checkpoint_dir).unwrap_or(checkpoint_dir);
    if let Ok(relative) = canonical_checkpoint_dir.strip_prefix(execution_root) {
        let mut text = relative.to_string_lossy().replace('\\', "/");
        if !text.is_empty() {
            if !text.ends_with('/') {
                text.push('/');
            }
            excludes.push(text);
        }
    }
    // 账本根侧的运行时小文件（评审 r8 M2）：非绑定会话两根相同，它们落在
    // 工作树内且在 checkpoints/ 排除目录之外——不排除会被 `clean -fd` 删掉
    // （pending_rewind 恰在其协调的恢复步骤中被毁，重试承诺失效）或被旧快照
    // 的 checkout-index 复活。仅当账本根位于执行根内时添加（绑定会话的文件
    // 在工作树外，同名模式不应改变其快照语义）；「被 ignored」同向豁免 clean。
    {
        let canonical_ledger =
            fs::canonicalize(ledger_root).unwrap_or_else(|_| ledger_root.to_path_buf());
        if canonical_ledger.starts_with(execution_root) {
            // .pinvou3/（评审 r9 minor 7）：含完整用户消息文本的会话侧运行时
            // 数据（conversation-attachments.json）按 umask 落盘却被影子 git
            // 收进对象库；transcript 相邻数据本模块一贯给 0600（atomic_write_
            // private）。App 自有目录，排除不触碰绑定项目语义。
            excludes.push(".pinvou3/".to_string());
            excludes.push("pending_rewind.json".to_string());
            excludes.push("budget-disabled".to_string());
            excludes.push("budget-ok".to_string());
        }
    }
    let info_dir = repo.join("info");
    fs::create_dir_all(&info_dir)
        .with_context(|| format!("创建 checkpoint 仓库 info 目录失败: {}", info_dir.display()))?;
    // r13 M3：exclude 的 truncate-then-write 可被并发 add -A 读到半写；unbound
    // 布局下位于 agent 可写的工作树内，同款原子私有写原语。
    crate::platform::filesystem::atomic_write_private(
        &info_dir.join("exclude"),
        (excludes.join("\n") + "\n").as_bytes(),
    )
    .with_context(|| "写入 checkpoint 仓库 exclude 失败".to_string())?;
    // 一次性迁移（存量仓库）：exclude 只挡 untracked 文件，升级前已 add -A 进
    // 影子 index 的敏感文件会继续被跟踪、被后续快照、被 checkout-index 恢复。
    // 按新模式清一次 index（幂等，marker 门控；只有 git 真实成功才写 marker，
    // 非零退出/spawn 失败都留到下次重试，不阻断快照主流程）。历史 commit
    // objects 里的原文不在清理范围，随 LRU 淘汰与 gc 回收。
    let migration_marker = info_dir.join("secret-excludes-v1");
    if !fresh && !migration_marker.exists() {
        match purge_secret_patterns_from_index(&repo, execution_root)
            .and_then(|()| purge_secret_survivors_exactly(&repo, execution_root))
        {
            Ok(()) => {
                fs::write(&migration_marker, "1\n").ok();
            }
            Err(error) => eprintln!(
                "[checkpoints] secret-exclude migration failed (will retry next call): {error:#}"
            ),
        }
    }
    Ok(repo)
}

fn load_index(ledger_root: &Path) -> Result<CheckpointIndex> {
    let path = index_path(ledger_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CheckpointIndex {
                version: 2,
                entries: Vec::new(),
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 checkpoint 索引失败: {}", path.display()));
        }
    };
    match serde_json::from_slice::<CheckpointIndex>(&bytes) {
        Ok(mut index) => {
            // 谓词收窄迁移（评审 r13 M1）：v1 的 Turn 条目号是旧口径
            // （is_user_turn_prompt，步数预算通知计 turn）算的；r10 收窄为
            // counts_as_user_turn 后无迁移则新旧同号碰撞——新边界取旧条目，
            // 「文件+对话」回退实际恢复早 k 轮的文件状态。保守失效：v1 索引
            // 的 Turn 条目整体退役（PreRestore 保留供 undo），该会话的旧边界
            // 诚实降级为仅对话；新 turn 起以 v2 口径从当前计数对齐。
            if index.version < 2 {
                let retired = index
                    .entries
                    .iter()
                    .filter(|e| e.kind == CheckpointKind::Turn)
                    .count();
                if retired > 0 {
                    eprintln!(
                        "[checkpoints] legacy v1 turn numbering: retiring {retired} Turn entries (honest conversation-only degradation); new turns use the narrowed predicate",
                    );
                }
                index.entries.retain(|e| e.kind != CheckpointKind::Turn);
                index.version = 2;
            }
            // r20 M1：commit 字段按对象 id 形状校验（SHA-1 40 / SHA-256 64 位
            // 十六进制）。它被插进 git argv 的 rev 位（rev-parse / read-tree /
            // diff 等）且位于 `--no-textconv`/`--no-ext-diff` 等选项钉之后——
            // git 选项 last-wins，被篡改的 `"--ext-diff"` 值可重新启用外部 diff
            // 驱动（config 清洗只覆盖 diff.* 节，攻击者可改 index 而非 config）。
            // r39 minor 5：语义损坏（合法 JSON、非法对象 id）改走与解析损坏
            // 同款的隔离+重建——此前 bail! 让该会话所有 checkpoint 操作永久
            // 失败（load_index 是全部读写的入口），而解析损坏路径早已证明隔离
            // +重建既保住「不可信 rev 不喂 git」（坏条目随隔离消失）又恢复
            // 快照能力。取证保留在隔离文件里供检查。
            let corrupt_entry = index.entries.iter().any(|entry| {
                let commit = entry.commit.trim();
                let valid_len = commit.len() == 40 || commit.len() == 64;
                !valid_len || !commit.bytes().all(|b| b.is_ascii_hexdigit())
            });
            if corrupt_entry {
                eprintln!(
                    "[checkpoints] checkpoint index entry with a malformed object id quarantined; rebuilt from an empty index"
                );
                return Ok(quarantine_index_and_rebuild(&path));
            }
            Ok(index)
        }
        Err(_parse_error) => {
            // 损坏的 index 不得让该会话的 checkpoint 功能永久失效（临时会话的
            // 账本就在 agent 可见的工作目录内，agent 的工具可能写坏它）：隔离
            // 保留现场后从空索引重建——与 sidecar 的损坏处理同款，共用
            // platform 的纳秒唯一名原语（秒级后缀在同一秒内的二次损坏会覆盖
            // 首次取证）。代价：影子仓库里的历史快照失去索引（不可列不可用；
            // 注意其 refs/checkpoints/* 引用并不被本路径清扫，对象因仍可达
            // 不会被 gc 回收，属已披露的预存在限制），此后快照能力恢复。
            // Neither the quarantine path (it lives inside the session
            // directory) nor the serde message (it can quote the corrupt
            // bytes) may reach stderr — same cleartext-logging surface as the
            // sidecar persist logs. The ledger root in context already
            // identifies the write, and the evidence stays in the quarantined
            // file for inspection.
            Ok(quarantine_index_and_rebuild(&path))
        }
    }
}

/// r39 minor 5：索引损坏（解析失败或语义损坏）的共用隔离+重建腿——隔离保留
/// 现场后返回空索引。隔离失败只记日志（现场未保留，但重建照常），不得让
/// checkpoint 能力因一次写坏而永久失效。
fn quarantine_index_and_rebuild(path: &std::path::Path) -> CheckpointIndex {
    match crate::platform::filesystem::quarantine_corrupt_file(path) {
        Ok(_) => eprintln!(
            "[checkpoints] corrupt checkpoint index quarantined; rebuilt from an empty index"
        ),
        Err(_) => eprintln!(
            "[checkpoints] failed to quarantine the corrupt index (scene not preserved); rebuilt from an empty index"
        ),
    }
    CheckpointIndex {
        version: 2,
        entries: Vec::new(),
    }
}

fn save_index(ledger_root: &Path, index: &CheckpointIndex) -> Result<()> {
    let path = index_path(ledger_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 checkpoint 目录失败: {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(index).context("序列化 checkpoint 索引失败")?;
    // The index lives next to the transcripts and its `label` field carries
    // user-message prefixes, so it must match the transcripts' privacy
    // (0600): private atomic write — a umask-default 0644 would expose every
    // turn's input prefix to other local users. Atomic replace keeps the
    // existing semantics: whatever lands on disk is always a complete index
    // (corruption goes through quarantine).
    crate::platform::filesystem::atomic_write_private(&path, &payload)
        .with_context(|| format!("保存 checkpoint 索引失败: {}", path.display()))?;
    Ok(())
}

/// Snapshot the execution root's current state and record it as a new
/// checkpoint.
///
/// `turn` is the user-turn ordinal this snapshot belongs to (1-based; the UI
/// aligns the entry to that turn boundary). When the content is identical to
/// the previous checkpoint (no changes before this turn) the previous commit
/// is reused instead of creating a redundant object. When done, the index is
/// trimmed to `MAX_CHECKPOINTS` (a private constant) LRU-style.
pub fn create_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    turn: Option<u32>,
    kind: CheckpointKind,
    label: &str,
) -> Result<CheckpointMeta> {
    create_checkpoint_preserving(ledger_root, execution_root, turn, kind, label, &[])
}

/// Display-label normalization before persisting: trims and caps at 80
/// characters (by chars, never bytes — a CJK label is never split mid
/// character). An empty label persists as-is.
fn normalize_checkpoint_label(label: &str) -> String {
    label.trim().chars().take(80).collect()
}

/// `create_checkpoint` 的保留变体：LRU/存储压力淘汰跳过 `preserve` 中的条目。
/// `restore_checkpoint` 打 PreRestore 时以此保住恢复目标——否则目标恰落在被
/// 淘汰的一半里时 ref 被删、commit 被 prune，随后 read-tree 失败，用户请求
/// 的历史被销毁（评审 M2）。
/// 存储压力决策谓词（评审 r8 M3 提取为纯函数以便单测锚定）。
/// 断路：总字节超预算且打包字节也超 → 内容性超额，gc 徒劳，跳过裁剪/gc；
/// 打包字节未超预算 → 超额可经 gc 收敛，走裁剪+gc 老路径。
fn pressure_breaker_engaged(total_bytes: u64, packed_bytes: u64) -> bool {
    total_bytes > MAX_SHADOW_REPO_BYTES as u64 && packed_bytes >= MAX_SHADOW_REPO_BYTES as u64
}

/// 硬停：断路模式下字节回收被跳过、增长只受内容变化约束——仓库字节量
/// （total 与 packed 取大：断路后 gc 不再打包，增量以 loose 对象累积而
/// size-pack 冻结，只看 packed 会让上限在 [budget, 2×budget) 打包窗口内
/// 永不触发，评审 r9 M3）达 2× 预算即拒绝新的 Turn 快照登记（回退路径的
/// PreRestore 豁免：可反悔兜底不可缺席，其频率由用户回退行为约束）。该轮
/// 入口如实降级为仅对话。
fn turn_snapshot_hard_stopped(repo_bytes: u64, packed_bytes: u64, kind: CheckpointKind) -> bool {
    kind == CheckpointKind::Turn
        && repo_bytes.max(packed_bytes) >= (MAX_SHADOW_REPO_BYTES as u64).saturating_mul(2)
}

/// add -A 后的账本路径 staged 校验（评审 r14 M-B）：worktree `.gitignore` 的
/// 否定行（`!checkpoints/`、`!.env`）可击穿 info/exclude 的三层排除——把敏感
/// 文件或账本自身照进影子树。fail-closed：staged 里出现账本路径即中止（不
/// 产出被污染的快照）；敏感文件由既有 purge 处理（迁移/restore 各跑一次，
/// 此处不清——`add -A` 每轮跑会双写）。
/// add -A 后的 staged 校验（评审 r14 M-B → r15 M1/M2 收紧）：worktree
/// .gitignore 的否定行（!checkpoints/ 、!.env 、!.pinvou3/）可击穿
/// info/exclude 的排除——把敏感文件、账本自身或会话侧数据照进影子树。
/// 用 git 自身的 pathspec 匹配（避免手写 gitignore 语义），命中即 bail。
fn validate_no_sensitive_paths_staged(
    ledger_root: &Path,
    repo: &Path,
    work_tree: &Path,
) -> Result<()> {
    // r16 M1 → r17 M1 → r19 M1 收紧：(1) 全部 secret 模式（含嵌入式 glob 如
    // .env.*.local）统一派生 :(icase)**/ 变体——pathspec 锚定整条路径，裸形态
    // 不跨 /；(2) 账本腿从固定 **/<name> 死形状改为按账本真实位置派生的根级
    // 形状（裸 + /**；**/checkpoints 在 git pathspec 匹配空集、**/x/** 只命中
    // 嵌套内容——根级 checkpoints/* 对旧形状不可见），且仅当账本位于执行根内
    // 才启用——绑定项目的 training/checkpoints/ 是无辜内容，无条件账本腿会把
    // 其快照永久误杀（评审 r19 M1a/M1b）；(3) diff 路径接同一校验。
    let mut pathspecs: Vec<String> = Vec::new();
    for pattern in SECRET_EXCLUDES {
        pathspecs.push(format!(":(icase){pattern}"));
        pathspecs.push(format!(":(icase)**/{pattern}"));
    }
    // r22 R1：SHADOW_EXCLUDES 不再进 bail 集——标准 keepfile 惯用法
    // （`dist/*` + `!dist/.gitkeep`）把无辜 keepfile 送进 `:(icase)dist/` 的
    // 命中集，r21 M3 的同款 bail 让该会话快照每回合全废（指数级劣于收益）。
    // 击穿的依赖条目改由 add -A 之后的 purge_shadow_excludes_from_index
    // `rm --cached` 摘除（create 与 diff 两路对称），排除语义恢复为「不进
    // 快照」；秘密与账本腿保持 fail-closed bail（原文入影子 objects / 账本
    // 自毁是不可逆伤害，bail 相称）。
    // ledger 腿本身已是 icase 前缀形状（见 ledger_guard_pathspecs）——r25
    // minor 2 所述「verbatim」与树不符，复核后维持现状：icase 匹配带来的
    // 大小写变体过度保护（无辜同名目录也触发 bail）是 fail-closed 方向、
    // 仅影响未绑定布局的罕见形状，如实记录不改。
    pathspecs.extend(ledger_guard_pathspecs(&ledger_guard_paths(
        ledger_root,
        work_tree,
    )));
    let refs: Vec<&str> = pathspecs.iter().map(|s| s.as_str()).collect();
    let mut args: Vec<&str> = vec!["diff", "--cached", "--name-only", "--"];
    args.extend(refs);
    let staged = git_ok(repo, work_tree, &args)?;
    let first = staged.lines().find(|l| !l.trim().is_empty());
    if let Some(path) = first {
        // r19 M2：bail 前清空影子 index——add -A 已把击穿条目暂存进持久 index，
        // 不清则此后每轮快照/预览的 add -A 重新暂存同一批条目（exclude 管不住
        // 已跟踪文件）、每轮 bail，用户删掉冒犯文件前回退永久停摆且无自愈。
        // read-tree --empty 只动 index：下一轮 add -A 全量重建，无正确性损失。
        let _ = git(repo, work_tree, &["read-tree", "--empty"]);
        bail!(
            "checkpoint staged excluded path {path} detected: worktree .gitignore negation pierced a secret or ledger exclusion; refusing to snapshot until the negation is removed",
        );
    }
    Ok(())
}
fn create_checkpoint_preserving(
    ledger_root: &Path,
    execution_root: &Path,
    turn: Option<u32>,
    kind: CheckpointKind,
    label: &str,
    preserve: &[&str],
) -> Result<CheckpointMeta> {
    let execution_root = canonical_execution_root(execution_root)?;
    let repo = ensure_repo(ledger_root, &execution_root)?;
    // 硬停（评审 r8 M3）：打包体积 ≥ 2× 预算时 Turn 快照拒绝登记——断路模式
    // （跳过裁剪/gc）下影子库只增不减，这里补回字节上限：Err 走「快照失败不
    // 阻断 turn」路径，该轮入口如实降级为仅对话回退。PreRestore 豁免：回退
    // 路径必须保留可反悔兜底，其频率由用户回退行为约束。
    // 只在 Turn 分支花这次 fs 走读 + count-objects（评审 r9 M3：PreRestore 恒
    // 放行，不必为其 spawn）；total 与 packed 取大后进谓词——断路后 gc 不再
    // 打包、增量以 loose 累积，只看 packed 会让上限永不触发。
    let (repo_bytes_at_entry, packed_at_entry) = if kind == CheckpointKind::Turn {
        (
            shadow_repo_bytes(&repo),
            packed_repo_bytes(&repo, &execution_root),
        )
    } else {
        (0, 0)
    };
    if turn_snapshot_hard_stopped(repo_bytes_at_entry, packed_at_entry, kind) {
        bail!(
            "影子仓库已达硬上限（{}MB ≥ 2×{}MB），本轮跳过 Turn 快照",
            repo_bytes_at_entry.max(packed_at_entry) / 1024 / 1024,
            MAX_SHADOW_REPO_BYTES / 1024 / 1024,
        );
    }
    // add -A 全量暂存（影子 index），write-tree 取内容树；与上一条 commit 的树相同
    // 则复用（空 turn 不产生冗余快照内容，但 meta 仍按新 turn 登记，保持对齐）。
    git_ok(&repo, &execution_root, &["add", "-A"])?;
    // r22 R1：被否定行击穿而暂存的依赖/构建条目在此摘除（每回合幂等自愈，
    // 存量被击穿 index 无需迁移 marker），排除语义=「不进快照」而非 bail。
    purge_shadow_excludes_from_index(&repo, &execution_root)?;
    validate_no_sensitive_paths_staged(ledger_root, &repo, &execution_root)?;
    let tree = git_ok(&repo, &execution_root, &["write-tree"])?
        .trim()
        .to_string();
    // r32 MAJOR-4：index RMW 跨度持进程内串行锁（钩子线程的作废写与本快照
    // 的新条目写互斥；跨度内的 LRU 作废不再重复取锁）。守卫到 save 之后
    // 返回时释放。
    // r32 MAJOR-4：index RMW 跨度（load→save）整体持进程内串行锁——钩子线程
    // 的作废写与本快照的新条目写互斥（否则互相丢失更新：作废被复活 / 新条目
    // 被丢）。跨度内的 LRU 作废不再重复取锁；守卫在闭包返回后释放。
    with_index_write_lock(ledger_root, || -> Result<CheckpointMeta> {
        let mut index = load_index(ledger_root)?;
        let previous = index.entries.last().cloned();
        let commit = match previous {
            Some(previous) => {
                let previous_tree = git_ok(
                    &repo,
                    &execution_root,
                    &["rev-parse", &format!("{}^{{tree}}", previous.commit)],
                )?;
                if previous_tree.trim() == tree {
                    previous.commit
                } else {
                    commit_tree(&repo, &execution_root, &tree, kind)?
                }
            }
            None => commit_tree(&repo, &execution_root, &tree, kind)?,
        };
        let meta = CheckpointMeta {
            id: format!("c{}-{}", index.entries.len() + 1, now_nanos()),
            turn,
            kind,
            label: normalize_checkpoint_label(label),
            commit,
            created_at: now_seconds(),
        };
        // 每条 checkpoint 一个 ref：orphan commit 只有被 ref 指着才可达，否则
        // git gc（含下面的 --auto）会把未淘汰的历史快照对象当垃圾清掉。
        git_ok(
            &repo,
            &execution_root,
            &[
                "update-ref",
                &format!("refs/checkpoints/{}", meta.id),
                &meta.commit,
            ],
        )?;
        index.entries.push(meta.clone());
        // 淘汰辅助：把最老的可淘汰条目（跳过 preserve）移出 index 并删其 ref。
        // 返回实际淘汰条数。最新一条（刚登记的 meta）恒保留：压力分支名额滚存时
        // 会把 preserve 挡下的名额落到它头上——刚创建的 PreRestore 被裁掉再被
        // gc --prune=now 物理删除，undo 绑定静默断裂（评审 M3）。
        let mut preserve_with_newest: Vec<&str> = preserve.to_vec();
        preserve_with_newest.push(meta.id.as_str());
        let preserve_set = &preserve_with_newest;
        let evict_oldest = |index: &mut CheckpointIndex, count: usize| -> usize {
            let mut evicted_ids = Vec::new();
            let mut kept = Vec::with_capacity(index.entries.len());
            let mut evictable_left = count;
            for entry in index.entries.drain(..) {
                if evictable_left > 0 && !preserve_set.contains(&entry.id.as_str()) {
                    evicted_ids.push(entry.id);
                    evictable_left -= 1;
                } else {
                    kept.push(entry);
                }
            }
            index.entries = kept;
            for id in &evicted_ids {
                let _ = git(
                    &repo,
                    &execution_root,
                    &["update-ref", "-d", &format!("refs/checkpoints/{id}")],
                );
            }
            evicted_ids.len()
        };
        if index.entries.len() > MAX_CHECKPOINTS {
            let overflow = index.entries.len() - MAX_CHECKPOINTS;
            // 被裁掉的条目删 ref，commit 变为不可达，交给 git 后台回收；
            // 回收失败不影响裁剪语义（索引已裁，对象留到下次 gc）。
            evict_oldest(&mut index, overflow);
            let _ = git(&repo, &execution_root, &["gc", "--auto", "--quiet"]);
        }
        // 存储压力闸（对齐底座 MAX_SNAPSHOT_SIZE_MB）：LRU 裁条目不裁字节，大文件
        // 项目单靠条目数守不住体积。超预算时裁到一半条目 + 主动 gc 收敛（至少保留
        // 最新一条——刚登记的本条；被裁的 undo 绑定由可反悔判定如实收敛）。
        let total_bytes = shadow_repo_bytes(&repo);
        if total_bytes > MAX_SHADOW_REPO_BYTES as u64 && index.entries.len() > 1 {
            // 评审 M5 内容性超额断路：先读打包体积（count-objects -v 的 size-pack，
            // 只读对象库统计、不做任何打包）。打包体积本身已 ≥ 预算 → 超额是内容
            // 性的（工作区大且不可压缩），gc --prune=now 无法收敛——此前该分支每轮
            // 登记都触发「裁半 + 全量 gc」（591MB 实测一次 10s+，且在持有 turn 预约
            // 的发送路径内联执行），条目数 1↔2 振荡、只剩最近一轮可回退，与确认
            // 弹窗「最近 20 个」披露矛盾。跳过裁剪与 gc：条目仍受 MAX_CHECKPOINTS
            // 上限约束；字节上限由 turn_snapshot_hard_stopped 硬停兜底（r8 M3——
            // 打包 ≥ 2× 预算即拒绝新的 Turn 快照，此模式不再是只增不减）。
            if pressure_breaker_engaged(total_bytes, packed_repo_bytes(&repo, &execution_root)) {
                eprintln!(
                    "[checkpoints] shadow repo over {}MB budget is content-driven (packed size ≥ budget); keeping entries, skipping futile gc",
                    MAX_SHADOW_REPO_BYTES / 1024 / 1024,
                );
                save_index(ledger_root, &index)?;
                return Ok(meta);
            }
            let target = (index.entries.len() / 2).max(1);
            let drain_count = index.entries.len() - target;
            let evicted = evict_oldest(&mut index, drain_count);
            let _ = git(&repo, &execution_root, &["gc", "--prune=now", "--quiet"]);
            eprintln!(
                "[checkpoints] shadow repo over {}MB budget, pruned {evicted} oldest entries",
                MAX_SHADOW_REPO_BYTES / 1024 / 1024,
            );
        }
        save_index(ledger_root, &index)?;
        Ok(meta)
    })
}

fn commit_tree(repo: &Path, work_tree: &Path, tree: &str, kind: CheckpointKind) -> Result<String> {
    let message = match kind {
        CheckpointKind::Turn => "pinvou checkpoint",
        CheckpointKind::PreRestore => "pinvou pre-restore checkpoint",
    };
    let commit = git_ok(
        repo,
        work_tree,
        &[
            "-c",
            "user.name=Pinvou",
            "-c",
            "user.email=pinvou@localhost",
            "commit-tree",
            tree,
            "-m",
            message,
        ],
    )?
    .trim()
    .to_string();
    git_ok(
        repo,
        work_tree,
        &["update-ref", "refs/checkpoints/head", &commit],
    )?;
    Ok(commit)
}

/// 列出会话的全部 checkpoint（按创建顺序升序，turn 对齐用）。
pub fn list_checkpoints(ledger_root: &Path) -> Result<Vec<CheckpointMeta>> {
    Ok(load_index(ledger_root)?.entries)
}

pub fn find_checkpoint(ledger_root: &Path, checkpoint_id: &str) -> Result<CheckpointMeta> {
    validate_checkpoint_id(checkpoint_id)?;
    load_index(ledger_root)?
        .entries
        .into_iter()
        .find(|entry| entry.id == checkpoint_id)
        .with_context(|| format!("checkpoint '{checkpoint_id}' 不存在"))
}

/// r39 minor 4：暂记成员校验专用——区分「索引读失败」与「条目确认不存在」。
/// 此前三条成员腿（matching_pending_pre_restore 的 PreRestore、
/// live_pending_rewind 的目标、pending_pre_restore_for_degraded 的绑定）都
/// 用 `find_checkpoint(...).is_err()` 判淘汰：索引文件的瞬态读失败与「已被
/// LRU 淘汰」同判，活暂记在飞时一次读失败就删暂记/丢绑定，孤儿化 P1——
/// 模块自身的 fail-closed 信条（读腿、命令层对账均已实现）没有覆盖到成员腿。
/// 只有「确认不存在」（错误链根部不是索引 IO）返回 false；读失败上抛。
fn checkpoint_entry_present(ledger_root: &Path, checkpoint_id: &str) -> Result<bool> {
    match find_checkpoint(ledger_root, checkpoint_id) {
        Ok(_) => Ok(true),
        Err(error) => {
            let io_rooted = error.root_cause().downcast_ref::<std::io::Error>().is_some()
                || error
                    .root_cause()
                    .downcast_ref::<serde_json::Error>()
                    .is_some();
            if io_rooted {
                Err(error)
            } else {
                Ok(false)
            }
        }
    }
}

/// 待完成回退的暂记（评审 r6 M2）：rewind 的恢复阶段已创建（或复用）PreRestore
/// 但整个回退未走完（恢复或截断中途失败）时，把「目标 checkpoint + keepTurns +
/// PreRestore id」记到账本根 `pending_rewind.json`。同目标的引导重试据此复用
/// 首个 PreRestore——否则重试会对混合/已回退的工作区新拍快照并把 undo 绑定到
/// 错误状态（恢复失败重试绑混合态；截断失败重试绑已回退态，评审 M2 两条腿）。
#[derive(Serialize, Deserialize)]
pub(crate) struct PendingRewind {
    pub(crate) checkpoint_id: String,
    pub(crate) keep_turns: u32,
    pub(crate) pre_restore_id: String,
    /// 写暂记时的会话 transcript revision：重试复用仅当其未变——新轮次/
    /// 编辑/成功回退/撤销都会改变它，届时退回「新建 PreRestore」（对那些
    /// 场景新建才是正确语义：undo 应绑定重试时刻的真实前置状态）。
    pub(crate) transcript_revision: String,
}

fn pending_rewind_path(ledger_root: &Path) -> std::path::PathBuf {
    ledger_root.join("pending_rewind.json")
}

pub fn write_pending_rewind(
    ledger_root: &Path,
    checkpoint_id: &str,
    keep_turns: u32,
    pre_restore_id: &str,
    transcript_revision: &str,
) -> Result<()> {
    // r31 MAJOR-2：把文档不变量落地为真——「pending 在飞期间新回退被拒」。
    // 仅同边界（同目标 + 同 keepTurns + 同 revision，即错误信息引导的同一
    // 重试）允许重写；其它新回退在此如实拒绝。否则 fresh PreRestore 会快照
    // 已半恢复的树成为新绑定，而 P1（真前态唯一副本）既不被暂记引用也不被
    // sidecar 引用，应用内从此不可达（standalone restore 已移除）。
    // r32 MAJOR-1：读腿 fail-closed——真实 IO 错误不得吞成「无暂记」（否则
    // 覆写照常进行，P1 被替换 = 不可恢复孤儿）。
    let existing = match live_pending_rewind(ledger_root) {
        Ok(Some(existing)) => Some(existing),
        Ok(None) => None,
        Err(error) => {
            return Err(
                anyhow::Error::from(error).context("读取回退暂记失败（fail-closed，拒绝覆盖）")
            );
        }
    };
    if let Some(existing) = existing {
        // r32 MAJOR-2：按失败腿分流——revision 前进（truncate 前发生了不经
        // 对账前奏的 transcript 写入：edit_last_turn / compact_now / 定时投递）
        // 时，同边界重试必然 matcher None + 本守卫拒绝 = 死循环；此时正确动作
        // 是桌面端发送一条新消息（快照前奏的 reconcile 重放会自动完成被中断
        // 的回退；headless 无此入口，文案同时给出真实补救，r39 minor 8）。
        // 目标/keepTurns 不同才是真正的「边界冲突」文案。
        let revision_moved = existing.transcript_revision != transcript_revision;
        let same_target =
            existing.checkpoint_id == checkpoint_id && existing.keep_turns == keep_turns;
        if same_target && revision_moved {
            // r39 minor 8：恢复指引按表面分流如实表述——「发送一条新消息自动
            // 对账」只有桌面端存在（快照前奏的 reconcile）；headless CLI 没有
            // 对账入口，真实补救是删除暂记标记。单一文案同时说真。
            bail!(
                "存在未完成的回退暂记（目标 {}，保留 {} 轮），且对话在暂记后发生了变化：\
                 桌面端发送一条新消息即可自动完成被中断的回退，无需重试回退；\
                 无界面环境（CLI）没有发送对账入口，请删除账本根下的 pending_rewind.json 后重试",
                existing.checkpoint_id,
                existing.keep_turns
            );
        }
        if !same_target {
            bail!(
                "存在未完成的回退暂记（目标 {}，保留 {} 轮）：请先按错误信息重试同一回退完成截断，或撤销后重试；本次目标不同，已拒绝覆盖",
                existing.checkpoint_id,
                existing.keep_turns
            );
        }
    }
    let pending = PendingRewind {
        checkpoint_id: checkpoint_id.to_string(),
        keep_turns,
        pre_restore_id: pre_restore_id.to_string(),
        transcript_revision: transcript_revision.to_string(),
    };
    // r14 M-A：崩溃恢复契约文件——原子+私有（0600）+O_EXCL 暂存+rename 抗
    // symlink 植入；unbound 布局下账本位于 agent 可写的工作树内。
    crate::platform::filesystem::atomic_write_private(
        &pending_rewind_path(ledger_root),
        &serde_json::to_vec(&pending).context("序列化 pending_rewind 失败")?,
    )
    .context("写入 pending_rewind 失败")
}

/// 读取匹配「同目标 + 同 keepTurns」的暂记 PreRestore id。陈旧性（该
/// PreRestore 已被成功回退绑定）由调用方核对——本层无会话侧备份记录视图。
/// 损坏的暂记删除后退回「新建 PreRestore」路径。
pub fn matching_pending_pre_restore(
    ledger_root: &Path,
    checkpoint_id: &str,
    keep_turns: u32,
    transcript_revision: &str,
) -> Result<Option<String>> {
    let path = pending_rewind_path(ledger_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        // r27 P2-1：NotFound（无暂记）与真实 IO 错误区分——后者映射 None 会让
        // 引导重试 fresh 一枚 PreRestore 快照已恢复树、孤儿化 P1（暂记协议
        // 要防的正是这个）。传播 Err 由调用方 fail-closed。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::Error::from(error).context("读取 pending rewind 失败")),
    };
    let Ok(pending) = serde_json::from_slice::<PendingRewind>(&bytes) else {
        // 损坏的暂记不阻塞回退：删除后退回「新建 PreRestore」路径。
        let _ = fs::remove_file(&path);
        return Ok(None);
    };
    if pending.checkpoint_id != checkpoint_id
        || pending.keep_turns != keep_turns
        || pending.transcript_revision != transcript_revision
    {
        return Ok(None);
    }
    // 索引成员校验（评审 r7 minor 5；r39 minor 4 fail-closed）：复用目标已被
    // LRU/压力裁剪时按不复用处理并删除陈旧暂记——硬报错会让同目标重试一直
    // 失败直到 revision 变化。索引读失败（瞬态 IO）不再是「淘汰」同义词：
    // 活暂记在飞时删暂记会孤儿化 P1，读失败如实上抛由调用方拒绝本次回退。
    if !checkpoint_entry_present(ledger_root, &pending.pre_restore_id)? {
        let _ = fs::remove_file(&path);
        return Ok(None);
    }
    Ok(Some(pending.pre_restore_id))
}

/// 按 keepTurns + revision 匹配暂记（忽略目标 checkpoint id）——降级回退
/// （无快照目标）的携带场景（评审 r7 minor 4）：此前失败的全量回退已把代码
/// 恢复到目标态并留有暂记，随后的同边界降级回退必须把该 PreRestore 绑进
/// 备份记录，否则记录承诺「代码保持不变」而实际代码已动，undo 只还原对话。
/// 同样做索引成员校验；不删除暂记（由成功后的统一清理收口）。
pub fn pending_pre_restore_for_degraded(
    ledger_root: &Path,
    transcript_revision: &str,
) -> Result<Option<String>> {
    let path = pending_rewind_path(ledger_root);
    // r28 M1：与 matching_pending_pre_restore / live_pending_rewind 同款——
    // 真实 IO 错误映射 None 会让降级回退跳过 PreRestore 绑定（代码已回退而
    // 记录宣称「代码未变」）。
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::Error::from(error).context("读取 pending rewind 失败")),
    };
    let Ok(pending) = serde_json::from_slice::<PendingRewind>(&bytes) else {
        let _ = fs::remove_file(&path);
        return Ok(None);
    };
    // r33 MAJOR-2：绑定判定只看 transcript revision——revision 匹配证明该
    // 暂记属于本次回退上下文（apply 已把工作区带到目标树），此时绑定 P1 是
    // 连贯语义（无论 apply 是否跑过）；keep_turns 不再参与判定——不同边界
    // 的降级回退在崩溃暂记在飞时同样必须绑 P1，否则 P1 被孤儿化、「代码
    // 保持不变」的通知失实。
    if pending.transcript_revision != transcript_revision {
        return Ok(None);
    }
    // r39 minor 4：成员腿 fail-closed——读失败上抛（调用方拒绝），只有确认
    // 不存在才放弃绑定。
    if !checkpoint_entry_present(ledger_root, &pending.pre_restore_id)? {
        return Ok(None);
    }
    Ok(Some(pending.pre_restore_id))
}
/// 清理性质：失败只记日志（陈旧暂记由调用方的已绑定检查兜底，不会把
/// undo 绑到已完成回退的旧快照上）。
pub fn clear_pending_rewind(ledger_root: &Path) {
    if let Err(error) = fs::remove_file(pending_rewind_path(ledger_root)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[checkpoints] 清理 pending_rewind 失败（仅清理未做）: {error:#}");
        }
    }
}

/// r19 M6 崩溃对账读取：任何「活」暂记（存在、可解析、目标仍在索引）。
/// 与 matching_pending_pre_restore 不同——不做 checkpoint/keepTurns/revision
/// 维度匹配：对账要回答的是「协议是否走到一半死了」，任何活暂记都值得
/// 处理。损坏文件沿既有约定隔离（删除 + None）。
/// r26 M2：是否存在「活」暂记的布尔探针——CLI crate 无法触及 pub(crate) 的
/// `PendingRewind` 类型，跨 crate 消费（回退前奏的和解跳过）走这里。
pub fn live_pending_rewind_present(ledger_root: &Path) -> Result<bool> {
    Ok(live_pending_rewind(ledger_root)?.is_some())
}

/// r39 M2：非破坏性暂记窥视——只读解析，无任何删除副作用，也不做索引成员
/// 校验。回退入口的不同边界拒绝必须走无副作用读取：`live_pending_rewind`
/// 的「目标已被淘汰 → 删除暂记」终结臂如果被入口探测复用，被淘汰的崩溃
/// 暂记会在拒绝路径上被顺手清掉，引导重试随之丢失（正是 M2 要堵的静默
/// 覆盖链的前半段）。损坏文件返回 None、不做清理——清理语义仍归
/// live_pending_rewind 的既有约定（快照前奏/对账路径）。
pub fn peek_pending_rewind(ledger_root: &Path) -> Result<Option<PendingRewind>> {
    let path = pending_rewind_path(ledger_root);
    // 读腿 fail-closed 与 live_pending_rewind 同款（r25 minor 1）：真实 IO
    // 错误上抛，由入口按「暂记不可读 → 拒绝本次回退」处理，不得吞成 None
    // 后照常 prepare_restore（那会重新打开 M2 的淘汰窗口）。
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::Error::from(error).context("读取 pending rewind 失败")),
    };
    let Ok(pending) = serde_json::from_slice::<PendingRewind>(&bytes) else {
        return Ok(None);
    };
    Ok(Some(pending))
}

/// r39 M2：回退入口的不同边界判定——活暂记存在且目标 ≠ 本次请求的
/// checkpoint id。跨 crate 布尔包装（CLI crate 无法触及 pub(crate) 的
/// `PendingRewind`），GUI 与 CLI 的入口拒绝共用同一判定。
pub fn pending_rewind_targets_different_checkpoint(
    ledger_root: &Path,
    checkpoint_id: &str,
) -> Result<bool> {
    Ok(peek_pending_rewind(ledger_root)?
        .map(|pending| pending.checkpoint_id != checkpoint_id)
        .unwrap_or(false))
}

pub fn live_pending_rewind(ledger_root: &Path) -> Result<Option<PendingRewind>> {
    let path = pending_rewind_path(ledger_root);
    // r25 minor 1：NotFound（无暂记）与真实 IO 错误必须区分——后者映射为
    // None 会把对账的「暂记不可读 → 阻断快照」分支变成死代码，瞬态 EACCES
    // 后照常快照可能固化混合态。
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::Error::from(error).context("读取 pending rewind 失败")),
    };
    let Ok(pending) = serde_json::from_slice::<PendingRewind>(&bytes) else {
        let _ = fs::remove_file(&path);
        return Ok(None);
    };
    // r39 minor 4：目标成员校验 fail-closed——索引读失败与「已被淘汰」不再
    // 同判。淘汰才清暂记解除快照阻断并如实留痕；瞬态读失败上抛，快照前奏
    // 的 Err 臂照常阻断本轮快照（不得在暂记在飞时因一次读失败删暂记孤儿化
    // P1）。
    if !checkpoint_entry_present(ledger_root, &pending.checkpoint_id)? {
        // 目标已被 LRU 淘汰：该回退永远无法完成——清掉暂记解除快照阻断并
        // 如实留痕（此前静默 None，文件永不清、reconcile 的淘汰分支不可达）。
        eprintln!(
            "[checkpoints] pending rewind target evicted, dropping pending: {}",
            pending.checkpoint_id
        );
        let _ = fs::remove_file(&path);
        return Ok(None);
    }
    Ok(Some(pending))
}

/// 恢复准备段：解析目标 + 创建（或按 `reuse_pre_restore` 复用）PreRestore，
/// **不动工作区**。拆出本段让调用方能在「PreRestore 已存在」与「开始变更
/// 工作区」之间落暂记（评审 r6 M2）。
pub fn prepare_restore(
    ledger_root: &Path,
    execution_root: &Path,
    checkpoint_id: &str,
    reuse_pre_restore: Option<&str>,
) -> Result<(CheckpointMeta, CheckpointMeta)> {
    let meta = find_checkpoint(ledger_root, checkpoint_id)?;
    let execution_root = canonical_execution_root(execution_root)?;
    let undo = match reuse_pre_restore {
        Some(id) => find_checkpoint(ledger_root, id)?,
        None => {
            // r39 M2：崩溃暂记在飞时，索引满员/存储压力的 LRU 淘汰按年龄裁
            // 最老、从不查看 pending_rewind.json——若暂记的目标或其 PreRestore
            // 恰是最老条目，会被本次登记的新 PreRestore 挤出索引并删 ref，
            // live_pending_rewind 随后按「目标缺失」终结暂记、r36-M2 守卫
            // 失明，崩溃边界被静默覆盖（快照被暂记阻断，其目标年龄只增不减，
            // 恰是最容易被裁的位置）。把在飞暂记的两个 id 加入 preserve 集；
            // 窥视读腿 fail-closed（IO 错误上抛中止回退，不打开淘汰窗口）。
            // 无暂记时 preserve 集与旧行为逐字相同。
            let pending_ids = match peek_pending_rewind(ledger_root)? {
                Some(pending) => vec![pending.checkpoint_id, pending.pre_restore_id],
                None => Vec::new(),
            };
            let mut preserve: Vec<&str> = vec![meta.id.as_str()];
            preserve.extend(pending_ids.iter().map(|id| id.as_str()));
            create_checkpoint_preserving(
                ledger_root,
                &execution_root,
                None,
                CheckpointKind::PreRestore,
                // Fixed zh label persisted as ledger fidelity data (byte-identical
                // with the pre-cleanup surface) and printed verbatim by the CLI;
                // localizing it is a consumer-side decision (PR #602).
                &format!("回滚到 {} 前的自动快照", meta.id),
                &preserve,
            )
            .context("回滚前自动快照失败，已中止回退")?
        }
    };
    Ok((meta, undo))
}

/// 恢复执行段：read-tree → 清 index（敏感 + 账本）→ checkout-index → clean，
/// 幂等可重入。
pub fn apply_restore(
    ledger_root: &Path,
    execution_root: &Path,
    meta: &CheckpointMeta,
) -> Result<()> {
    let execution_root = canonical_execution_root(execution_root)?;
    // r23 M3：apply_restore 是唯一不经 ensure_repo 就执行 read-tree /
    // checkout-index 的窗口（update-ref/gc 的 invalidate 路径同样运行在
    // ensure_repo 之外、其执行根参数穿线列为 follow-up——r24 minor 2）。
    // attributes 钉与危险 config 清洗只在 ensure_repo 建立，而崩溃/对账重放
    // 路径直达这里。无竞争的过滤执行窗：被动代理文件写可植入工作区
    // .gitattributes + [filter]/partialClone config，`checkout-index -f -a`
    // 会执行 smudge 过滤器、read-tree 会触发 promisor 懒取链（git 2.43
    // 实证，评审 r23 M3）。顶部刷新卫生（幂等，ms 级）。
    let repo = ensure_repo(ledger_root, &execution_root)?;
    git_ok(&repo, &execution_root, &["read-tree", &meta.commit])
        .context("读取 checkpoint 快照失败")?;
    // read-tree 整体替换 index：迁移前打的旧快照 tree 里可能仍含敏感文件，
    // 不清理就会被 checkout-index 写回工作区并被后续 add -A 重新跟踪（一次性
    // 迁移的效果被一次回滚复活）。恢复前按同一模式清影子 index（只动 index）。
    purge_secret_patterns_from_index(&repo, &execution_root)
        .context("清理快照中的敏感文件条目失败")?;
    // r32 MAJOR-3 backstop：icase 幸存变体精确清除（restore 路径无 validate
    // bail 兜底）。
    purge_secret_survivors_exactly(&repo, &execution_root)
        .context("清理快照中的敏感文件幸存变体失败")?;
    // r19 M4：被击穿的旧快照 tree 含账本路径（checkpoints/index.json、
    // pending_rewind.json、.pinvou3/**——M1 根洞时期的树，或 negate 击穿）
    // 时，checkout-index -f -a 把陈旧账本覆写到活账本上、协议中途自毁。
    // read-tree 之后、checkout-index 之前 rm --cached 同源派生的账本形状
    // （绑定布局 = 空集，同名项目内容不受影响）。
    let ledger_specs = ledger_guard_pathspecs(&ledger_guard_paths(ledger_root, &execution_root));
    if !ledger_specs.is_empty() {
        let mut rm_args: Vec<&str> = vec![
            "rm",
            "-r",
            "--cached",
            "--ignore-unmatch",
            "--quiet",
            "-f",
            "--",
        ];
        rm_args.extend(ledger_specs.iter().map(String::as_str));
        let output = git(&repo, &execution_root, &rm_args)?;
        if !output.status.success() {
            bail!(
                "git rm --cached 账本路径失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    git_ok(&repo, &execution_root, &["checkout-index", "-f", "-a"])
        .context("写回 checkpoint 文件失败")?;
    // r17 M2：clean 的 -e 排除压过 ignore 规则——worktree .gitignore 的
    // 否定行（!.env）可让真实密钥文件在 restore 的 clean -fd 中被物理删除。
    // r19 M5：-e 排除集必须用与 info/exclude 同款的大小写字符类形式——裸字面
    // 是大小写敏感的，.ENV / ID_RSA 变体可穿透。
    // r19 M1：账本腿改为按账本真实位置派生（绑定布局 = 空集——固定
    // checkpoints/** 会把项目无辜同名目录挡在 clean 语义之外，预览与实际
    // 行为背离）。
    let mut clean_args: Vec<String> = vec!["clean".into(), "-fd".into()];
    // r24 B1：shadow 腿与秘密腿同款 icase 字符类——裸小写在大小写敏感文件
    // 系统上挡不住 Dist/ 变体，被 icase purge 移出 index 的真实文件会被
    // clean -fd 物理删除（评审实证，会话前就存在的用户文件）。
    for pattern in SHADOW_EXCLUDES {
        for leg in icase_ignore_legs(pattern) {
            clean_args.push("-e".into());
            clean_args.push(leg);
        }
    }
    for pattern in SECRET_EXCLUDES {
        for leg in icase_ignore_legs(pattern) {
            clean_args.push("-e".into());
            clean_args.push(leg);
        }
    }
    for pattern in ledger_guard_ignore_patterns(&ledger_guard_paths(ledger_root, &execution_root)) {
        clean_args.push("-e".into());
        clean_args.push(pattern);
    }
    let clean_refs: Vec<&str> = clean_args.iter().map(|s| s.as_str()).collect();
    git_ok(&repo, &execution_root, &clean_refs).context("清理快照后新建文件失败")?;
    Ok(())
}

/// `diff --cached --raw` 输出行解析（`:<srcmode> <dstmode> <srcsha> <dstsha> <status>\t<path>`，
/// rename/copy 多一个 `\t<newpath>`）。gitlink（嵌套仓库/submodule，mode 160000）
/// 单独标注——快照不跟踪其内容、restore 不会 materialize 它（评审 M4）。
fn parse_raw_status(text: &str) -> Vec<CheckpointChange> {
    text.lines()
        .filter_map(|line| {
            let (header, paths) = line.split_once('\t')?;
            let fields: Vec<&str> = header.split_whitespace().collect();
            if fields.len() < 5 {
                return None;
            }
            let (src_mode, dst_mode, status) = (fields[0], fields[1], fields[4]);
            let label = if src_mode == "160000" || dst_mode == "160000" {
                "gitlink"
            } else {
                match status.chars().next() {
                    Some('A') => "added",
                    Some('M') => "modified",
                    Some('D') => "deleted",
                    Some('R') => "renamed",
                    Some('C') => "copied",
                    _ => "other",
                }
            };
            // R/C 状态是「旧路径\t新路径」，展示新路径。
            let path = paths.rsplit('\t').next()?.to_string();
            if path.is_empty() {
                return None;
            }
            Some(CheckpointChange {
                path,
                status: label.to_string(),
            })
        })
        .collect()
}

/// 仅支持 `*` 的通配匹配（任意字符序列）。敏感模式均无斜杠，等价于
/// gitignore 对 basename 的匹配语义。
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while t < text.len() {
        if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star_at) = star {
            p = star_at + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// path 是否命中敏感文件模式：模式均无斜杠 → 字面 = basename 精确匹配，
/// 含 `*` 的按 basename 通配（任意深度）。恒按大小写不敏感匹配（ASCII 折叠），
/// 与 info/exclude 的字符类模式、purge 的 `:(icase)` pathspec 三层同向
/// （评审 B1/M1）。
fn secret_path_matches(path: &str) -> bool {
    // r28 B2：含斜杠模式（.aws/credentials 等）按整条路径匹配——basename
    // 匹配对它们恒假；自身注释「模式均无斜杠」自 r27 起失效，一并改写。
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let basename = normalized.rsplit('/').next().unwrap_or(&normalized);
    SECRET_EXCLUDES.iter().any(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        if pattern.contains('/') {
            if pattern.ends_with("/**") {
                // r30 MAJOR-1：目录 glob 的嵌套副本按中路径命中——`==`/前缀
                // 只覆盖根级，`sub/.ssh/agent-env`（basename 不落任何 basename
                // 模式）曾从 legacy 树的 patch 预览泄漏明文。
                let dir = pattern.trim_end_matches("/**");
                return normalized == dir
                    || normalized.starts_with(&format!("{dir}/"))
                    || normalized.contains(&format!("/{dir}/"));
            }
            // 根级副本整路径相等；嵌套副本按「/ 边界 + 后缀」命中。
            normalized == pattern || normalized.ends_with(&format!("/{pattern}"))
        } else if pattern.contains('*') {
            wildcard_match(&pattern, basename)
        } else {
            basename == pattern
        }
    })
}

/// Secret check for a `--raw` status path. `parse_raw_status` hands the field
/// through verbatim, and with `core.quotepath=false` git still C-quotes paths
/// containing quotes/backslashes/tabs (`"my \"key.pem"`), whose trailing
/// quote defeats a plain suffix match — while the patch filter (which parses
/// the quoted shapes) drops that section, leaving the two preview surfaces
/// inconsistent. Unquote before matching so both agree. Only the common
/// quote/backslash escapes are handled: the secret patterns are plain ASCII
/// names, so an exotic escape can at worst over-retain a list entry — never
/// leak a patch section (that filter is independent and conservative).
fn raw_status_path_is_secret(path: &str) -> bool {
    if secret_path_matches(path) {
        return true;
    }
    let unquoted = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(path);
    secret_path_matches(&unquoted.replace("\\\"", "\""))
}

/// Whether a `diff --git` section header points at a secret file. For the
/// C-quoted form (git emits `diff --git "a/x" "b/x"` when the path contains
/// quotes/tabs/non-ASCII) the a/ and b/ paths are extracted by quoted
/// segment; unquoted paths may contain spaces (plain spaces do not trigger
/// C-quoting), so both paths are split at the LAST ` b/` occurrence (same
/// trick git consumers use). A mis-split errs conservative — over-removing
/// rather than leaking — except for exotic quoted-binary shapes, disclosed.
/// Binary-file sections have no ---/+++ lines, so the header parse is their
/// only predicate; the token scan is the final fallback.
fn diff_section_is_secret(header: &str) -> bool {
    if let Some(rest) = header.strip_prefix("diff --git \"") {
        // C-quoted shape: extract the a/ and b/ paths by quoted segment
        // (the paths may contain spaces).
        if let Some(path) = rest.strip_prefix("a/").and_then(|s| s.split('"').next()) {
            if secret_path_matches(path) {
                return true;
            }
        }
        if let Some(path) = rest
            .split("\" \"")
            .nth(1)
            .and_then(|s| s.strip_prefix("b/"))
            .and_then(|s| s.split('"').next())
        {
            if secret_path_matches(path) {
                return true;
            }
        }
    }
    if let Some(rest) = header.strip_prefix("diff --git a/") {
        if let Some(split) = rest.rfind(" b/") {
            let old_path = &rest[..split];
            let new_path = &rest[split + " b/".len()..];
            if secret_path_matches(old_path) || secret_path_matches(new_path) {
                return true;
            }
        }
    }
    header
        .split_whitespace()
        .map(|token| token.trim_matches('"'))
        .filter_map(|token| {
            token
                .strip_prefix("a/")
                .or_else(|| token.strip_prefix("b/"))
        })
        .any(|token| secret_path_matches(token))
}

/// Path predicate for `--- a/<path>` / `+++ b/<path>` marker lines (the
/// rest of the line IS the path, spaces tolerated; a deleted file's marker
/// line carries a trailing tab pad, stripped first; when the path contains
/// tabs/quotes git C-quotes it as `--- "a/x"` — strip the outer quotes
/// before matching; /dev/null honestly never matches).
fn marker_line_is_secret(line: &str) -> bool {
    let path = line
        .strip_prefix("--- a/")
        .or_else(|| line.strip_prefix("+++ b/"))
        .or_else(|| line.strip_prefix("--- \"a/"))
        .or_else(|| line.strip_prefix("+++ \"b/"));
    match path {
        Some(path) => secret_path_matches(path.trim_end().trim_matches('"')),
        None => false,
    }
}

/// Drops every whole per-file diff section whose path matches a secret
/// pattern from a unified-diff text: old snapshots taken before a purge may
/// still carry secret plaintext in their trees (the purge only clears the
/// index), and the preview must not carry the plaintext into the UI.
/// Sections are buffered then judged (a header OR marker-line match drops
/// the whole section; binary sections rely purely on the header parse).
/// Lines are kept verbatim, terminators included: `str::lines` would strip a
/// trailing `\r`, silently turning a CRLF project's diff into an LF one that
/// no longer applies, and would append a newline to a patch that ended
/// without one.
fn filter_secret_paths_from_patch(patch: &str) -> String {
    let mut out = String::with_capacity(patch.len());
    let mut section: Vec<&str> = Vec::new();
    let mut section_secret = false;
    let flush = |out: &mut String, section: &mut Vec<&str>, secret: &mut bool| {
        if !*secret {
            for line in section.drain(..) {
                out.push_str(line);
            }
        } else {
            section.clear();
        }
        *secret = false;
    };
    for raw_line in patch.split_inclusive('\n') {
        // Judge on the content, emit the bytes: the predicates below match
        // path prefixes and would not see through a trailing `\r\n`.
        let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
        if line.starts_with("diff --git ") {
            flush(&mut out, &mut section, &mut section_secret);
            section_secret = diff_section_is_secret(line);
        } else if marker_line_is_secret(line) {
            section_secret = true;
        }
        section.push(raw_line);
    }
    flush(&mut out, &mut section, &mut section_secret);
    out
}

/// 快照与当前执行根的差异预览（即「回滚将撤销的变更」），供 UI 确认前展示。
///
/// `include_patch` gates the unified diff: the `changes` list is always
/// computed, but the patch costs a full `git diff` whose generation is
/// unbounded (only the read-back is capped), and the desktop preview renders
/// only the list — it must pass `false`. The patch surface exists for the
/// CLI. When `false`, `patch` is empty and `patch_truncated` is `false`.
pub fn diff_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    checkpoint_id: &str,
    include_patch: bool,
) -> Result<CheckpointDiff> {
    let meta = find_checkpoint(ledger_root, checkpoint_id)?;
    let execution_root = canonical_execution_root(execution_root)?;
    let repo = ensure_repo(ledger_root, &execution_root)?;
    // add -A 让影子 index 反映当前执行根，diff --cached <commit> 即「快照→现在」。
    git_ok(&repo, &execution_root, &["add", "-A"])?;
    // r22 R1：与 create 路对称——击穿条目先摘除再校验，diff 预览不再把依赖
    // 目录误报为「回滚将删除」，也不把击穿 index 留给后续快照。
    purge_shadow_excludes_from_index(&repo, &execution_root)?;
    // r17 M1(c)：diff 路径的 add -A 同样需要 staged 校验——被否定的 index
    // 持久留存会让后续每个快照的校验都 bail（无自愈）。
    validate_no_sensitive_paths_staged(ledger_root, &repo, &execution_root)?;
    let raw_status = git_ok(
        &repo,
        &execution_root,
        // --raw 带文件模式（gitlink 160000 = 嵌套仓库/submodule，清单中如实
        // 标注）；-M 开启 rename 检测；core.quotepath=false：非 ASCII 文件名
        // 原样输出（默认会把中文路径转成八进制转义，确认弹窗不可读）。
        &[
            "-c",
            "core.quotepath=false",
            "diff",
            "--cached",
            "--raw",
            "-M",
            "--no-color",
            &meta.commit,
        ],
    )?;
    // legacy 快照（迁移前打的）tree 里可能仍含秘密原文：清单剔除命中敏感模式
    // 的条目，预览不谎称「回滚将删除 .env」（restore 实际保留工作区现有同名
    // 文件）。过滤恒大小写不敏感，与 exclude/purge 三层同向。
    let changes: Vec<CheckpointChange> = parse_raw_status(&raw_status)
        .into_iter()
        .filter(|change| !raw_status_path_is_secret(&change.path))
        .collect();
    let (patch, patch_truncated) = if !include_patch {
        // The desktop preview renders only the `changes` list. Generating the
        // unified diff here cost a full `git diff` per preview open — main
        // carried an explicit 「unified diff 不再生成」 decision for exactly
        // that reason — and nothing in the webview reads `patch`.
        (String::new(), false)
    } else {
        let (raw_patch, mut patch_truncated) = git_diff_capped(
            &repo,
            &execution_root,
            // 与 --raw 同开 -M + quotepath：changes 清单标 renamed 时 patch
            // 也是 rename 形态，中文路径在两处都是原文。
            &[
                "-c",
                "core.quotepath=false",
                "-c",
                "diff.noprefix=false",
                "-c",
                "diff.mnemonicPrefix=false",
                // The secret-path filter parses `diff --git a/<p> b/<p>` and the
                // `--- a/` / `+++ b/` markers, so the prefixes are part of its
                // contract, not cosmetics. They are configurable
                // (diff.noprefix, diff.srcPrefix, diff.dstPrefix,
                // diff.mnemonicPrefix), and `isolated_git_command` only
                // neutralizes the system and global config — the shadow repo's
                // OWN config still applies, and for an unbound session that repo
                // sits inside the directory the agent's own tools can write
                // (see the ledger note on `restore_checkpoint`). A single
                // `diff.noprefix = true` line there makes every section
                // unparseable to the filter and passes `.env` plaintext straight
                // into the preview. Pin them on the command line, where repo
                // config cannot reach.
                "diff",
                "--cached",
                "-M",
                "--no-color",
                // Same threat model, second escape hatch: `--no-ext-diff` closes
                // `diff.external` / `GIT_EXTERNAL_DIFF`, but textconv is a
                // separate switch and defaults to ON. `[diff "x"] textconv = <cmd>`
                // in the shadow repo's own config plus one `.gitattributes` line
                // makes git run an arbitrary command per file and substitute its
                // stdout as the diff body — arbitrary content reaching the preview
                // under a path the filter permits, and arbitrary execution in this
                // process's group.
                "--no-textconv",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                &meta.commit,
            ],
            DIFF_PATCH_LIMIT,
        )?;
        // Legacy snapshots (taken before the purge) may still carry secret
        // plaintext in their trees: whole sections matching a secret pattern
        // are dropped so the preview cannot leak it. The filter is always
        // case-insensitive, aligned with the exclude/purge layers. raw_patch
        // is already clamped to a line boundary, so a section header either
        // fully participates in the filter's judgment or is absent entirely —
        // truncation cannot leave half a secret-path header behind.
        let mut patch = filter_secret_paths_from_patch(&raw_patch);
        // Truncation signals through `patch_truncated` alone: consumers pin the
        // flag (and the byte cap), not a locale-specific tail appended to the
        // data — an embedded CJK string here would leak into CLI output and
        // bypass the trilingual i18n surface. The flag reports the raw `git diff`
        // output exceeding the cap (the filter only removes content, so a capped
        // read implies the caller is not seeing the full diff). The clamp is a
        // belt-and-braces for `from_utf8_lossy` expansion (one invalid byte
        // becomes three replacement bytes): it can only move the end backwards
        // to the previous line boundary, so a section header stays whole — and
        // when it does fire, content was dropped without the raw read ever
        // crossing the cap, so the flag must flip too.
        if patch.len() > DIFF_PATCH_LIMIT {
            let bytes = patch.as_bytes();
            let mut end = DIFF_PATCH_LIMIT;
            while end > 0 && bytes[end - 1] != b'\n' {
                end -= 1;
            }
            patch.truncate(end);
            patch_truncated = true;
        }
        (patch, patch_truncated)
    };
    Ok(CheckpointDiff {
        checkpoint: meta,
        changes,
        patch,
        patch_truncated,
    })
}

/// 回滚执行根到指定 checkpoint。恢复前先自动打一个「回滚点」快照（失败则中止，
/// 保证回滚永远可反悔），返回该回滚点供前端提示。
///
/// 写操作只落在执行根内：read-tree/checkout-index/clean 都以执行根为 work-tree，
/// 且不删 ignored 文件（不用 -x，node_modules 等不受波及）。
/// **双写者分歧披露（评审 r14 M-C）**：本函数是单段组合（prepare→apply），
/// 不走 GUI 命令层的 pending-rewind 协议。pinvou-cli 的 code 子命令直接调用
/// 本函数：GUI 回退在 apply 中途失败后改用 CLI 完成恢复，CLI 会对混合工作树
/// 新拍 PreRestore 并绑定之——GUI 的 undo 语义保证在 CLI 路径不成立。
/// 协议下沉到本函数或 CLI 改走拆分序列是跟进项。
pub fn restore_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    checkpoint_id: &str,
) -> Result<CheckpointMeta> {
    // 单段组合（undo 流程用：undo 的恢复 SHOULD 对当前（回退后）状态新拍
    // PreRestore 以支持「反悔可再反悔」，因此不走暂记/复用路径）。
    // preserve 传入恢复目标：PreRestore 触发的 LRU/存储压力淘汰不得把目标
    // 本身裁掉（ref 删除 + commit prune 会让随后的 read-tree 失败，用户请求
    // 的历史被销毁，评审 M2）——见 prepare_restore 内 create_checkpoint_preserving。
    let (meta, undo) = prepare_restore(ledger_root, execution_root, checkpoint_id, None)?;
    apply_restore(ledger_root, execution_root, &meta)?;
    Ok(undo)
}

/// 只操作 refs 的 git 调用（update-ref/gc 不需要 work-tree）。
/// 与写路径共用陈旧锁清扫重试：被超时 kill 的 git 会留下
/// `refs/checkpoints/*.lock`，无清扫时删 ref 永久失败，ref 与其对象泄漏。
fn git_ref(repo: &Path, arguments: &[&str]) -> Result<std::process::Output> {
    git_bounded(repo, None, arguments, |error| {
        anyhow::anyhow!("执行 git {} 失败: {error}", arguments.join(" "))
    })
}

/// 回退后作废被截对话分支的 Turn checkpoint（设计审阅 P0 修复）。
///
/// turn 序号是消息序列里的相对位置：`rewind_to_turn` 恢复 + 截断后，用户重新
/// 创作的新一轮会复用 turn 编号；若 index 里仍留着被截分支 `turn > keep_turns`
/// 的旧 Turn 快照，对齐规则（`resolve_rewind_plan` 与前端 first-wins）会把新
/// 分支的对齐锚到旧快照上，再次回退恢复出被遗弃分支的代码状态，历史错乱。
///
/// 本函数从 index 移除 `kind == Turn && turn > keep_turns` 的条目并删其
/// `refs/checkpoints/<id>` ref（照抄 LRU 淘汰段的 `update-ref -d` + 末尾
/// `gc --auto` 模式），保留原有条目顺序；PreRestore 条目（turn=None）不动——
/// 被截分支的代码状态已由本次 rewind 强制的 PreRestore 快照兜底，不丢反悔能力。
/// 返回作废条数；无符合条件条目时幂等返回 0（不触碰磁盘）。
/// r32 MAJOR-4：index.json 的进程内读改写串行锁——messages-shrunk 钩子线程
/// （compaction 作废）与 turn 快照的 load→save 跨度互斥，否则两条 RMW 链
/// 互相丢失更新（作废被复活 / 新条目被丢）。进程内；跨进程（CLI）锁列
/// follow-up（既有披露）。按账本根分桶，不同会话不互斥。
fn with_index_write_lock<R>(ledger_root: &Path, f: impl FnOnce() -> R) -> R {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let key = ledger_root.to_string_lossy().to_ascii_lowercase();
    let bucket = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = bucket
        .lock()
        // r39 minor 5：与注册表锁同款 poison 恢复——持锁闭包 panic 会毒化桶锁，
        // 此前 `let _guard = bucket.lock();` 绑定的是 Result，Err（毒化）臂
        // 等于放弃串行化静默放行，进程内 index RMW 从此互斥失效。
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f()
}

pub fn invalidate_turn_checkpoints_after(ledger_root: &Path, keep_turns: u32) -> Result<usize> {
    // r32 MAJOR-4：作废写与快照 RMW 跨度串行（钩子线程亦经此函数入锁）。
    with_index_write_lock(ledger_root, || {
        invalidate_entries_if(ledger_root, |entry| {
            entry.kind == CheckpointKind::Turn && entry.turn.is_some_and(|turn| turn > keep_turns)
        })
    })
}

/// r39 minor 1：压缩钩子的火时刻截止变体。钩子在自己的线程上异步执行，与
/// 新 turn 的快照登记竞速——纯 `turn > kept` 谓词会把「钩子触发后、线程执行
/// 前」合法登记的 kept+1 轮快照一并作废（该轮入口永久降级为仅对话，且无
/// 自愈）。截止取钩子触发时刻（persist 锁内捕获，先于线程 spawn）：只有
/// 创建于火时刻之前的 Turn 快照属被压缩截断的分支。与
/// invalidate_stale_turn_checkpoints 同款秒级精度约定（<=，同秒条目按被截
/// 分支处理是安全方向：误删只降级入口，漏删会重新引入 first-wins 劫持）。
pub fn invalidate_turn_checkpoints_after_created_until(
    ledger_root: &Path,
    keep_turns: u32,
    created_until: i64,
) -> Result<usize> {
    with_index_write_lock(ledger_root, || {
        invalidate_entries_if(ledger_root, |entry| {
            entry.kind == CheckpointKind::Turn
                && entry.turn.is_some_and(|turn| turn > keep_turns)
                && entry.created_at <= created_until
        })
    })
}

/// 陈旧快照和解（评审 finding：作废失败/崩溃残留）。`invalidate_turn_checkpoints_after`
/// 是清理性质，失败只记日志；残留的旧分支快照随后会在 first-wins 下抢占同号
/// turn 的对齐锚。sidecar 回退备份先于截断落盘，是可靠的和解凭据：对某次回退
/// （keep_turns，截断时刻），`turn > keep_turns` 且**创建于截断时刻之前**的
/// Turn 快照必属被截分支——回退后新分支的同号快照创建于截断之后，不受波及。
/// created_at 与截断时刻同为秒级：同秒创建的条目无法区分归属，按被截分支
/// 处理（安全方向，见谓词处注释）。rewind 预检按备份记录逐条调用本函数，
/// 让作废最终一致、自愈。
///
/// `created_before` 为秒级时间戳（备份记录 `rewound_at` 的解析结果）。
pub fn invalidate_stale_turn_checkpoints(
    ledger_root: &Path,
    keep_turns: u32,
    created_before: i64,
) -> Result<usize> {
    // r33 MAJOR-1：与 create/钩子作废同锁——否则 rewind 前奏的陈旧和解与
    // 在飞钩子作废互相丢失更新（作废被复活 → first-wins 劫持回归）。
    with_index_write_lock(ledger_root, || {
        invalidate_entries_if(ledger_root, |entry| {
            entry.kind == CheckpointKind::Turn
                && entry.turn.is_some_and(|turn| turn > keep_turns)
                // <= 故意不收紧为 <：秒级精度下同秒条目无法区分归属，按被截分支
                // 处理是安全方向——误删新分支锚点只让后续回退到该轮降级为仅对话，
                // 漏删被截锚点会重新引入 first-wins 劫持，且该截止随记录固定、
                // 不会被后续和解自动修正。
                && entry.created_at <= created_before
        })
    })
}

/// 按谓词作废条目：从 index 移除命中项并删其 `refs/checkpoints/<id>` ref
/// （照抄 LRU 淘汰段的 `update-ref -d` + 末尾 `gc --auto` 模式），保留原有
/// 条目顺序。返回作废条数；无命中时幂等返回 0（不触碰磁盘）。
fn invalidate_entries_if(
    ledger_root: &Path,
    matches: impl Fn(&CheckpointMeta) -> bool,
) -> Result<usize> {
    let mut index = load_index(ledger_root)?;
    // partition 稳定保序：kept 即「原顺序去掉被作废条目」。
    let (kept, invalidated): (Vec<CheckpointMeta>, Vec<CheckpointMeta>) =
        index.entries.into_iter().partition(|entry| !matches(entry));
    if invalidated.is_empty() {
        return Ok(0);
    }
    // 影子仓库不存在（如从未成功快照却有残留索引）时 refs 无从删起，索引照裁。
    let repo = repo_dir(ledger_root);
    if repo.join("HEAD").is_file() {
        for entry in &invalidated {
            let _ = git_ref(
                &repo,
                &[
                    "update-ref",
                    "-d",
                    &format!("refs/checkpoints/{}", entry.id),
                ],
            );
        }
        let _ = git_ref(&repo, &["gc", "--auto", "--quiet"]);
    }
    index.entries = kept;
    save_index(ledger_root, &index)?;
    Ok(invalidated.len())
}

/// 按 id 移除单条 checkpoint（index 条目 + ref），返回是否真的移除了一条。
/// 用于发送失败后作废「未成活」的 Turn 快照（chat.rs send_error 路径）——按 id
/// 精确删除，覆盖 turn 序号为 None（计数失败兜底）的情形；幂等，不存在即 false。
pub fn drop_checkpoint(ledger_root: &Path, checkpoint_id: &str) -> Result<bool> {
    validate_checkpoint_id(checkpoint_id)?;
    // r33 MAJOR-1：drop 的 RMW 同锁串行（对钩子/create 皆然）。
    with_index_write_lock(ledger_root, || {
        Ok(invalidate_entries_if(ledger_root, |e| e.id == checkpoint_id)? > 0)
    })
}

/// 测试专用 git 可用性探测，供本模块测试与命令层测试
/// （`app/commands/checkpoints.rs`）共用：无 git 的环境里相关用例跳过而非报错。
#[cfg(test)]
pub(crate) fn git_available() -> bool {
    let available = crate::platform::process::HiddenCommand::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    // A silent skip would let security/parity regressions pass a git-less CI
    // lane invisibly; say so when the coverage is skipped.
    if !available {
        eprintln!("[checkpoints] skipping git-dependent test coverage: git is unavailable");
    }
    available
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::code_checkpoints::turns::is_step_budget_notification;
    use deepseek_tui::models::{ContentBlock, Message, Role};

    /// Upgrade regression: index entries written before `CheckpointMeta.label`
    /// was restored carry no `label` key. The field must deserialize with its
    /// default instead of failing the whole index parse — a failed parse hits
    /// the quarantine branch, which rebuilds from an empty index and leaves
    /// every shadow-repo snapshot unlistable (user-visible history loss on
    /// upgrade). Pure JSON: no git fixture needed.
    #[test]
    fn main_era_index_without_label_key_still_loads() {
        let dir = TestDir::new("label-upgrade");
        let path = index_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
  "version": 1,
  "entries": [
    {"id": "c1-1758000000000000000", "turn": 1, "kind": "turn",
     "commit": "0123456789abcdef0123456789abcdef01234567", "createdAt": 1758000000},
    {"id": "c2-1758000000000000001", "turn": 2, "kind": "preRestore",
     "label": "legacy value", "commit": "fedcba9876543210fedcba9876543210fedcba98",
     "createdAt": 1758000060}
  ]
}"#,
        )
        .unwrap();
        let index = load_index(dir.path()).expect("a main-era index must deserialize");
        // r13 M1 迁移：v1 的 Turn 条目被保守退役（旧口径号不可信），PreRestore 保留。
        assert_eq!(
            index.entries.len(),
            1,
            "the v1 Turn entry is retired by the predicate migration; only PreRestore survives"
        );
        assert_eq!(
            index.entries[0].label, "legacy value",
            "a present label must still be read"
        );
        assert_eq!(index.version, 2, "the index is stamped to v2 on load");
        // No quarantine file may appear next to the index.
        let corrupt: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter_map(|entry| {
                let p = entry.path();
                p.to_str()
                    .is_some_and(|s| s.contains(".corrupt-"))
                    .then_some(p)
            })
            .collect();
        assert!(
            corrupt.is_empty(),
            "the index must not be quarantined: {corrupt:?}"
        );
    }

    /// Round-12 review: the label chain has no end-to-end pin — nothing
    /// asserted that a caller-supplied label actually reaches the ledger
    /// through normalize. The expected value is computed inline (trim +
    /// first 80 chars) instead of via `normalize_checkpoint_label`, so a
    /// silently dropped or un-normalized label both go red.
    #[test]
    fn checkpoint_label_lands_in_the_ledger_normalized() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("label-e2e-ledger");
        let exec = TestDir::new("label-e2e-exec");
        exec.write("a.txt", "v1\n");
        let raw = format!("  {} tail", "深".repeat(100));
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            &raw,
        )
        .expect("checkpoint");
        let listed = list_checkpoints(ledger.path()).expect("list");
        assert_eq!(listed.len(), 1);
        let expected: String = raw.trim().chars().take(80).collect();
        assert_eq!(
            listed[0].label, expected,
            "label must be trimmed and capped at 80 chars"
        );
        assert_eq!(listed[0].label.chars().count(), 80);
        assert!(
            listed[0].label.ends_with('深'),
            "CJK must not be split mid-character"
        );
    }

    #[test]
    fn normalize_checkpoint_label_trims_and_caps_by_chars() {
        assert_eq!(normalize_checkpoint_label("  x  "), "x");
        assert_eq!(normalize_checkpoint_label(""), "");
        let cjk: String = "深".repeat(120);
        let normalized = normalize_checkpoint_label(&cjk);
        assert_eq!(normalized.chars().count(), 80);
        assert!(
            normalized.ends_with('深'),
            "truncation must not split a character"
        );
    }

    /// 存储压力决策谓词（评审 r8 M3：断路与硬停是 r7 引擎改动的零锚定区）。
    #[test]
    fn pressure_predicates() {
        const CAP: u64 = MAX_SHADOW_REPO_BYTES as u64;
        // 断路：总字节超且打包也超才触发；打包未超走老路径。
        assert!(pressure_breaker_engaged(CAP + 1, CAP));
        assert!(pressure_breaker_engaged(CAP + 1, CAP + 1));
        assert!(
            !pressure_breaker_engaged(CAP + 1, CAP - 1),
            "打包未超预算 → gc 可收敛，不断路"
        );
        assert!(
            !pressure_breaker_engaged(CAP, CAP * 3),
            "总字节未超预算不触发压力分支"
        );
        // 硬停：Turn 且仓库字节量（total 与 packed 取大）≥ 2× 预算；PreRestore
        // 豁免（可反悔兜底不可缺席）。
        // r9 M3：total 与 packed 取大——断路后 gc 不再打包、loose 无界累积
        // （size-pack 冻结在 [budget, 2×budget) 窗口）也必须触发硬停。
        assert!(turn_snapshot_hard_stopped(CAP * 2, 0, CheckpointKind::Turn));
        assert!(turn_snapshot_hard_stopped(0, CAP * 2, CheckpointKind::Turn));
        assert!(turn_snapshot_hard_stopped(
            CAP,
            CAP * 3,
            CheckpointKind::Turn
        ));
        assert!(!turn_snapshot_hard_stopped(
            CAP,
            CAP * 2 - 1,
            CheckpointKind::Turn
        ));
        assert!(
            !turn_snapshot_hard_stopped(CAP * 9, CAP * 9, CheckpointKind::PreRestore),
            "PreRestore 豁免硬停"
        );
    }

    /// 非绑定布局的预算遍历跳过影子目录自身（评审 r10 M5：r9 minor-4 修复
    /// 此前零测试引用，删除跳过逻辑可全绿通过——快照自身把会话顶过预算线
    /// 并触发粘性禁用的回归必须变红）。
    #[test]
    fn budget_walk_excludes_shadow_dir_in_unbound_layout() {
        let dir = TestDir::new("budget-walk-unbound");
        let root = dir.path().to_path_buf();
        dir.write(
            "a.txt", "v1
",
        );
        // 真实登记两个快照：影子仓库（含对象文件）落在 root/checkpoints/ 内。
        create_checkpoint(&root, &root, Some(1), CheckpointKind::Turn, "t1").expect("first");
        dir.write(
            "a.txt", "v2
",
        );
        create_checkpoint(&root, &root, Some(2), CheckpointKind::Turn, "t2").expect("second");
        assert!(
            root.join("checkpoints").join("repo").exists(),
            "前置：影子仓库确实位于遍历根内"
        );
        // 估算只计入工作区文件（a.txt），不含 checkpoints/ ——没有跳过的话，
        // 对象文件会计入 total（测试体量小，断言「远小于」以兼容平台差异）。
        let estimate = estimate_workspace_bytes(&root, &root).expect("estimate must succeed");
        let shadow_bytes = shadow_repo_bytes(&repo_dir(&root));
        assert!(
            shadow_bytes > 0,
            "前置：影子仓库确有字节可计入（否则断言无效）"
        );
        assert!(
            estimate < shadow_bytes,
            "估算不得计入影子目录自身（estimate={estimate}, shadow={shadow_bytes}）"
        );
    }
    /// 粘性超预算标记（评审 r8 M3 锚定）：标记存在即短路（不触估算路径——
    /// 用不存在的执行根验证：无标记时会走估算并落标记，有标记时直接 false）。
    #[test]
    fn budget_disabled_marker_short_circuits() {
        let ledger = TestDir::new("budget-marker-ledger");
        // r10 M4 后「目录不存在」按 skip 计 0 字节（不再触发 fail-closed），改用
        // 「执行根是个文件」制造 NotADirectory——这条错误路径维持 fail-closed。
        let not_a_dir = ledger.path().join("exec-is-a-file");
        fs::write(&not_a_dir, b"").unwrap();
        // 无标记：估算失败（NotADirectory）按不在预算内 → false，且落下标记。
        assert!(!execution_root_within_snapshot_budget(
            &not_a_dir,
            ledger.path()
        ));
        assert!(
            ledger.path().join("budget-disabled").exists(),
            "首次不在预算内必须落下粘性标记"
        );
        // 有标记：短路返回 false（此刻执行根存在与否、可读与否都无关）。
        assert!(!execution_root_within_snapshot_budget(
            &not_a_dir,
            ledger.path()
        ));
    }
    /// pending_rewind 暂记生命周期（评审 r7 M3：r6-M2 修复此前零测试锚定，
    /// 删除复用机制可全绿通过）。覆盖：往返命中、目标/keepTurns/revision 任一
    /// 不匹配即拒绝、损坏暂记隔离删除、复用目标被裁剪后按不复用处理并清理、
    /// clear 后不复用。
    #[test]
    fn pending_rewind_roundtrip_and_guards() {
        let ledger = TestDir::new("pending-roundtrip-ledger");
        let exec = TestDir::new("pending-roundtrip-exec");
        exec.write(
            "a.txt", "v1
",
        );
        let target = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "target",
        )
        .expect("target checkpoint");
        // 暂记指向的 PreRestore 必须真实存在于索引（成员校验会拒绝幽灵 id——
        // 幽灵/被裁剪场景由 evicted 测试单独覆盖）。
        let pre_restore = create_checkpoint(
            ledger.path(),
            exec.path(),
            None,
            CheckpointKind::PreRestore,
            "undo",
        )
        .expect("pre-restore checkpoint");

        // 往返：同目标 + keepTurns + revision → Some(id)。
        write_pending_rewind(ledger.path(), &target.id, 1, &pre_restore.id, "rev-a")
            .expect("write pending");
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), &target.id, 1, "rev-a").expect("match"),
            Some(pre_restore.id.clone()),
            "匹配三元组必须命中暂记的 PreRestore id"
        );

        // 任一维度不匹配：目标、keepTurns、revision。
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), "other-target", 1, "rev-a").expect("match"),
            None,
            "目标不同不得复用"
        );
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), &target.id, 2, "rev-a").expect("match"),
            None,
            "keepTurns 不同不得复用"
        );
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), &target.id, 1, "rev-b").expect("match"),
            None,
            "transcript revision 变化（新轮次/编辑/成功回退）不得复用"
        );

        // clear 后不复用（成功回退/撤销的收口）。
        clear_pending_rewind(ledger.path());
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), &target.id, 1, "rev-a").expect("match"),
            None,
            "clear 之后不得复用"
        );
    }

    /// r8 M2 排除的端到端锚定（评审 r9 M4：删除排除代码可全绿通过）：非绑定
    /// 会话布局（ledger == exec root，单目录两根）下，pending_rewind.json 与
    /// budget-disabled 必须被影子仓库排除——restore 的 clean -fd 不得删除
    /// 它们、git ls-files 不得跟踪它们。
    #[test]
    fn runtime_sidecar_files_survive_restore_in_unbound_layout() {
        let dir = TestDir::new("sidecar-unbound");
        let ledger = dir.path().to_path_buf();
        let exec = dir.path().to_path_buf();
        dir.write(
            "a.txt", "v1
",
        );
        // 先登记两个快照（内容不同），再落运行时文件。
        let first = create_checkpoint(&ledger, &exec, Some(1), CheckpointKind::Turn, "t1")
            .expect("first checkpoint");
        dir.write(
            "a.txt", "v2
",
        );
        let _second = create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2")
            .expect("second checkpoint");
        write_pending_rewind(&ledger, &first.id, 1, "pre-1", "rev-x").expect("write pending");
        fs::write(ledger.join("budget-disabled"), b"").unwrap();

        // 排除生效：两文件从未被跟踪。
        let repo = repo_dir(&ledger);
        let ls = git_ok(&repo, &exec, &["ls-files"]).expect("ls-files");
        assert!(
            !ls.lines()
                .any(|l| l == "pending_rewind.json" || l == "budget-disabled"),
            "运行时 sidecar 不得进入影子 index: {ls}"
        );

        // 恢复到第一个快照：clean -fd 之后两文件仍在且内容未动。
        restore_checkpoint(&ledger, &exec, &first.id).expect("restore");
        let expected_pending = format!(
            "{{\"checkpoint_id\":\"{}\",\"keep_turns\":1,\"pre_restore_id\":\"pre-1\",\"transcript_revision\":\"rev-x\"}}",
            first.id
        );
        assert_eq!(
            fs::read_to_string(ledger.join("pending_rewind.json")).unwrap(),
            expected_pending,
            "pending_rewind.json 必须在 restore 后原样存活（r9 M4 锚定）"
        );
        assert!(
            ledger.join("budget-disabled").exists(),
            "budget-disabled 必须在 restore 后存活"
        );
    }

    /// r19 M1a + M2（unbound 布局）：根级账本击穿必须被校验命中（旧
    /// `**/checkpoints` 死形状对根级 checkpoints/* 不可见），且 bail 后影子
    /// index 已被清空——移除冒犯文件后下一轮快照自愈（旧代码条目粘死在
    /// 持久 index，每轮 add -A 重新暂存、每轮 bail）。
    #[test]
    fn root_ledger_pierce_bails_then_self_heals_after_offending_file_removed() {
        if !git_available() {
            return;
        }
        let dir = TestDir::new("ledger-pierce-unbound");
        let ledger = dir.path().to_path_buf();
        let exec = dir.path().to_path_buf();
        dir.write("a.txt", "v1\n");
        dir.write(".gitignore", "!checkpoints/\n");
        let pierced = create_checkpoint(&ledger, &exec, Some(1), CheckpointKind::Turn, "t1");
        assert!(
            pierced.is_err(),
            "根级 checkpoints 击穿必须 bail: {pierced:?}"
        );

        // 自愈：撤掉否定行后同一会话的下一轮快照必须成功。
        dir.write(".gitignore", "a.txt\n");
        create_checkpoint(&ledger, &exec, Some(2), CheckpointKind::Turn, "t2")
            .expect("移除冒犯文件后快照必须自愈（r19 M2 read-tree --empty）");
        let repo = repo_dir(&ledger);
        let tracked = git_ok(&repo, &exec, &["ls-files"]).unwrap();
        assert!(
            !tracked.lines().any(|l| l.starts_with("checkpoints/")),
            "影子仓库自身不得被跟踪: {tracked}"
        );
    }

    /// r19 M1b（bound 布局）：账本腿必须布局条件化——项目内的
    /// training/checkpoints/ 与同名 pending_rewind.json 是无辜用户内容，
    /// 快照照常收录（旧代码的固定账本腿让这类项目永久 bail、回退停摆）；
    /// clean -e 亦不再排挤同名目录：快照后新建的该目录文件按 restore 的
    /// 常规语义清理（预览与实际行为一致）。
    #[test]
    fn bound_project_same_named_dirs_are_innocent() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("innocent-bound-ledger");
        let exec = TestDir::new("innocent-bound-exec");
        exec.write("training/checkpoints/model.bin", "weights\n");
        exec.write("pending_rewind.json", "{\"project\":\"artifact\"}\n");
        exec.write("src/main.rs", "fn main() {}\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .expect("无辜同名项目内容不得让快照 bail（r19 M1b）");
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            tracked
                .lines()
                .any(|l| l == "training/checkpoints/model.bin"),
            "项目同名目录内容应照常跟踪: {tracked}"
        );
        assert!(
            tracked.lines().any(|l| l == "pending_rewind.json"),
            "项目同名文件应照常跟踪: {tracked}"
        );

        // 快照后新建的无辜同名目录文件：restore 的 clean -fd 按常规语义清理。
        exec.write("training/checkpoints/late.bin", "late\n");
        restore_checkpoint(ledger.path(), exec.path(), &first.id).expect("restore");
        assert!(
            !exec.path().join("training/checkpoints/late.bin").exists(),
            "clean -e 不得再排挤项目同名目录（预览承诺与实际行为一致）"
        );
        assert!(
            exec.path().join("training/checkpoints/model.bin").exists(),
            "快照内文件照常写回"
        );
    }

    /// r19 M3：字面量前导 glob（.env.*.local）的 purge 必须派生 `**/` 变体
    /// ——裸形态锚定整条路径，`apps/web/.env.development.local` 逃过清理、
    /// 被 checkout-index 写回、再永久击穿 staged 校验。直接对影子 index
    /// 注入两个深度的条目（等价迁移前已跟踪的存量），跑 purge 断言全清。
    #[test]
    fn purge_covers_nested_literal_leading_glob_secrets() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("nested-glob-ledger");
        let exec = TestDir::new("nested-glob-exec");
        exec.write("a.txt", "v1\n");
        exec.write(".env.production.local", "SECRET=1\n");
        exec.write("apps/web/.env.development.local", "SECRET=2\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        let repo = repo_dir(ledger.path());
        // 绕过 exclude 强制暂存（等价迁移前存量跟踪）。
        git_ok(
            &repo,
            exec.path(),
            &[
                "add",
                "-f",
                "--",
                ".env.production.local",
                "apps/web/.env.development.local",
            ],
        )
        .unwrap();
        purge_secret_patterns_from_index(&repo, exec.path()).expect("purge");
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|l| l.ends_with(".env.production.local")
                    || l.ends_with(".env.development.local")),
            "任意深度的 .env.*.local 都必须被 purge 清出 index: {tracked}"
        );
    }

    /// r19 M4：含账本路径的陈旧快照 tree（M1 根洞时期的 pierced tree）经
    /// restore 不得把陈旧账本覆写到活账本上——apply 的 rm --cached 腿在
    /// checkout-index 之前清掉账本条目（协议中途自毁的直达链）。
    #[test]
    fn restore_never_overwrites_live_ledger_from_stale_pierced_tree() {
        if !git_available() {
            return;
        }
        let dir = TestDir::new("stale-tree-unbound");
        let ledger = dir.path().to_path_buf();
        let exec = dir.path().to_path_buf();
        dir.write("a.txt", "v1\n");
        let real = create_checkpoint(&ledger, &exec, Some(1), CheckpointKind::Turn, "t1")
            .expect("base checkpoint");

        // 手工构造含账本路径的 pierced tree（等价旧根洞时期的快照）。
        let repo = repo_dir(&ledger);
        git_ok(&repo, &exec, &["read-tree", &real.commit]).unwrap();
        let stale_file = exec.join("stale-ledger-probe.txt");
        fs::write(&stale_file, "STALE-LEDGER\n").unwrap();
        let blob = git_ok(
            &repo,
            &exec,
            &["hash-object", "-w", &stale_file.to_string_lossy()],
        )
        .unwrap()
        .trim()
        .to_string();
        let _ = fs::remove_file(&stale_file);
        for staged_path in ["checkpoints/index.json", "pending_rewind.json"] {
            git_ok(
                &repo,
                &exec,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{staged_path}"),
                ],
            )
            .unwrap();
        }
        let pierced_tree = git_ok(&repo, &exec, &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let pierced_commit = git_ok(
            &repo,
            &exec,
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &pierced_tree,
                "-m",
                "pierced",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        let mut index = load_index(&ledger).unwrap();
        let pierced_meta = CheckpointMeta {
            id: format!("c-pierced-{}", now_nanos()),
            turn: Some(1),
            kind: CheckpointKind::Turn,
            label: "pierced".to_string(),
            commit: pierced_commit.clone(),
            created_at: now_seconds(),
        };
        index.entries.push(pierced_meta.clone());
        save_index(&ledger, &index).unwrap();

        // 活账本真身。
        let live_pending = format!(
            "{{\"checkpoint_id\":\"{}\",\"keep_turns\":1,\"pre_restore_id\":\"pre-1\",\"transcript_revision\":\"rev-live\"}}",
            real.id
        );
        fs::write(ledger.join("pending_rewind.json"), &live_pending).unwrap();
        let live_index_bytes = fs::read(ledger.join("checkpoints/index.json")).unwrap();

        apply_restore(&ledger, &exec, &pierced_meta).expect("apply");

        assert_eq!(
            fs::read_to_string(ledger.join("pending_rewind.json")).unwrap(),
            live_pending,
            "陈旧 tree 里的 pending_rewind.json 不得覆写活暂记（r19 M4）"
        );
        assert_eq!(
            fs::read(ledger.join("checkpoints/index.json")).unwrap(),
            live_index_bytes,
            "陈旧 tree 里的 checkpoints/index.json 不得覆写活账本（r19 M4）"
        );
    }

    /// r30 MAJOR-3 pin：r27-M4/r28-M3/r29-minor-6 扩充条目跨形状覆盖——
    /// purge pathspec 双腿（根级 + `**/` 嵌套）逐条命中；patch 过滤三形状
    /// （目录 glob 中路径、basename 通配、plain basename）命中。删除扩充
    /// 条目或破坏双腿派生必须变红。
    #[test]
    fn expanded_secret_entries_cover_all_shapes() {
        let specs = secret_pathspecs();
        for entry in [
            ".aws/sso/cache/**",
            ".gnupg/**",
            ".password-store/**",
            ".ssh/**",
            "terraform.tfstate*",
            ".pypirc",
            ".envrc",
            "_netrc",
            "secrets.yaml",
            "service-account*.json",
        ] {
            let lower = entry.to_ascii_lowercase();
            assert!(
                specs.iter().any(|s| s == &format!(":(icase){lower}")),
                "purge 根腿缺失: {entry}"
            );
            assert!(
                specs.iter().any(|s| s == &format!(":(icase)**/{lower}")),
                "purge 嵌套腿缺失: {entry}"
            );
        }
        assert_eq!(
            SECRET_EXCLUDES
                .iter()
                .filter(|entry| **entry == ".git-credentials")
                .count(),
            1,
            ".git-credentials 不得重复（r30 MAJOR-3 去重后保持单条）"
        );
        assert!(
            secret_path_matches("x/.aws/sso/cache/token"),
            "目录 glob 中路径命中"
        );
        assert!(
            secret_path_matches("terraform.tfstate.backup"),
            "basename 通配命中"
        );
        assert!(
            secret_path_matches("deploy/.envrc"),
            "plain basename 嵌套命中"
        );
        assert!(!secret_path_matches("src/main.rs"), "非秘密路径不误报");
    }

    /// r31 MAJOR-2 行为 pin：活暂记在飞时，非同边界的新回退写入被拒绝
    /// （文档不变量「pending 在飞期间新回退被拒」落地为真）；同边界重试
    /// （同目标/同 keepTurns/同 revision）照常写。
    #[test]
    fn write_pending_rewind_refuses_a_mismatched_live_pending() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("pending-refuse-ledger");
        let exec = TestDir::new("pending-refuse-exec");
        exec.write(
            "a.txt", "v1
",
        );
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        write_pending_rewind(ledger.path(), &first.id, 1, "p1", "rev-a").unwrap();
        // 不同边界（不同目标）→ 拒绝且原暂记原样保留。
        let refused = write_pending_rewind(ledger.path(), "c-other", 2, "p2", "rev-a");
        assert!(refused.is_err(), "不同边界的新回退必须被拒绝");
        let pending = live_pending_rewind(ledger.path()).unwrap().unwrap();
        assert_eq!(pending.checkpoint_id, first.id);
        assert_eq!(pending.pre_restore_id, "p1");
        // r32 MAJOR-2：同目标 + revision 前进（暂记后发生了不经对账前奏的
        // transcript 写入）→ 按失败腿分流的专属文案（指引发送新消息自动完成，
        // 而非注定被再次拒绝的「重试同一回退」）。
        let revision_moved = write_pending_rewind(
            ledger.path(),
            &first.id,
            1,
            "p1",
            "rev-a-but-transcript-moved",
        );
        let revision_moved_message = revision_moved.err().map(|error| error.to_string());
        assert!(
            revision_moved_message.as_deref().is_some_and(|message| {
                message.contains("自动完成被中断的回退") && message.contains("pending_rewind.json")
            }),
            "同目标 + revision 前进必须携带自动完成指引文案，且为无界面环境给出真实补救（r39 minor 8）"
        );
        // 原暂记保持（未被覆盖）。
        let pending = live_pending_rewind(ledger.path()).unwrap().unwrap();
        assert_eq!(pending.checkpoint_id, first.id);
        assert_eq!(pending.pre_restore_id, "p1");
        // 同边界重试 → 照常重写。
        write_pending_rewind(ledger.path(), &first.id, 1, "p1", "rev-a").unwrap();
        let pending = live_pending_rewind(ledger.path()).unwrap().unwrap();
        assert_eq!(pending.pre_restore_id, "p1");
    }

    /// r31 MAJOR-1 行为 pin：主匹配器（matching_pending_pre_restore，GUI 与
    /// CLI 回退复用共用）的 IO fail-closed——pending 路径被目录占用（读取 =
    /// 非 NotFound 的真实 IO 错误）必须 Err，不得映射为「无暂记」（否则
    /// 引导重试 fresh PreRestore 快照已恢复树、孤儿化 P1）。
    #[test]
    fn matching_pending_pre_restore_fails_closed_on_io_errors() {
        let ledger = TestDir::new("matcher-io-ledger");
        let pending_path = ledger.path().join("pending_rewind.json");
        fs::create_dir_all(&pending_path).unwrap();
        let result = matching_pending_pre_restore(ledger.path(), "c1", 1, "rev");
        assert!(
            result.is_err(),
            "真实 IO 错误必须 fail-closed（r31 MAJOR-1 变异实证：映射 None 时全绿）"
        );
    }

    /// r30 MAJOR-2 行为 pin：降级匹配器的 IO fail-closed——pending 路径被
    /// 目录占用（读取 = 非 NotFound 的真实 IO 错误）必须 Err，不得映射为
    /// 「无暂记」（否则降级回退跳过 PreRestore 绑定而代码已回退）。
    #[test]
    fn pending_pre_restore_for_degraded_fails_closed_on_io_errors() {
        let ledger = TestDir::new("degraded-io-ledger");
        let pending_path = ledger.path().join("pending_rewind.json");
        fs::create_dir_all(&pending_path).unwrap();
        let result = pending_pre_restore_for_degraded(ledger.path(), "rev");
        assert!(
            result.is_err(),
            "真实 IO 错误必须 fail-closed（r30 MAJOR-2 变异实证：映射 None 时全绿）"
        );
    }

    /// 降级回退携带查询（评审 r9 M4 零锚定区）：keepTurns+revision 匹配命中；
    /// ghost/被裁剪目标与维度不匹配按 None 处理。
    #[test]
    fn pending_pre_restore_for_degraded_matches_and_guards() {
        let ledger = TestDir::new("degraded-matcher-ledger");
        let exec = TestDir::new("degraded-matcher-exec");
        exec.write(
            "a.txt", "v1
",
        );
        let pre_restore = create_checkpoint(
            ledger.path(),
            exec.path(),
            None,
            CheckpointKind::PreRestore,
            "undo",
        )
        .expect("pre-restore");
        write_pending_rewind(ledger.path(), "target-1", 2, &pre_restore.id, "rev-9")
            .expect("write pending");

        assert_eq!(
            pending_pre_restore_for_degraded(ledger.path(), "rev-9").expect("degraded match"),
            Some(pre_restore.id.clone()),
            "同 keepTurns+revision（忽略目标 id）必须命中",
        );
        assert_eq!(
            pending_pre_restore_for_degraded(ledger.path(), "rev-9").expect("degraded match"),
            Some(pre_restore.id.clone()),
            "r33 MAJOR-2：revision 匹配即绑定（keepTurns 不再参与判定）",
        );
        assert_eq!(
            pending_pre_restore_for_degraded(ledger.path(), "rev-other").expect("degraded match"),
            None,
            "revision 不同不携带",
        );
    }
    /// prepare_restore 复用腿（评审 r14 M-E ②）：同目标 + reuse id → 跳过新拍
    /// PreRestore，返回的 undo 即被复用条目——这是崩溃恢复（同目标重试）的核心。
    #[test]
    fn prepare_restore_reuse_skips_new_pre_restore() {
        let dir = TestDir::new("prepare-reuse");
        let ledger = dir.path().to_path_buf();
        let exec = dir.path().to_path_buf();
        dir.write(
            "a.txt", "v1
",
        );
        let target =
            create_checkpoint(&ledger, &exec, Some(1), CheckpointKind::Turn, "t").expect("target");
        let pre = create_checkpoint(&ledger, &exec, None, CheckpointKind::PreRestore, "undo")
            .expect("pre-restore");
        let count_before = load_index(&ledger).unwrap().entries.len();
        let (meta, undo) =
            prepare_restore(&ledger, &exec, &target.id, Some(&pre.id)).expect("prepare with reuse");
        assert_eq!(undo.id, pre.id, "复用时返回被复用的 PreRestore");
        assert_eq!(meta.id, target.id, "目标解析正确");
        let count_after = load_index(&ledger).unwrap().entries.len();
        assert_eq!(
            count_before, count_after,
            "复用路径不得新增 index 条目（跳过新拍快照）"
        );
    }

    /// is_step_budget_notification 行为夹具（评审 r14 M-E ③）：两条真实前缀
    /// 命中、lookalike 用户消息不误吞、assistant 角色不命中。
    #[test]
    fn step_budget_notification_predicate_fixtures() {
        let notice = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Step budget soft landing: you have used about 80% of your 200 step budget."
                    .into(),
                cache_control: None,
            }],
        };
        assert!(
            is_step_budget_notification(&notice),
            "soft landing 通知必须命中"
        );
        let notice2 = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Your model-step budget was exhausted (limit: 200).".into(),
                cache_control: None,
            }],
        };
        assert!(
            is_step_budget_notification(&notice2),
            "exhaustion 通知必须命中"
        );
        let lookalike = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text:
                    "Step budget soft landing: what does this mean? I was asking about the weather."
                        .into(),
                cache_control: None,
            }],
        };
        // 当前谓词按前缀匹配，lookalike 同样命中——这是已知取舍（fail-closed
        // 方向：不计数不渲染），夹具钉住当前行为防止意外改变。
        assert!(
            is_step_budget_notification(&lookalike),
            "前缀 lookalike 同命中（fail-closed 方向的已知取舍）"
        );
        let assistant = Message {
            role: deepseek_tui::models::Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "Step budget soft landing:".into(),
                cache_control: None,
            }],
        };
        assert!(
            !is_step_budget_notification(&assistant),
            "assistant 角色不命中"
        );
    }
    /// 损坏的暂记文件被隔离删除，退回「新建 PreRestore」路径（不阻塞回退）。
    #[test]
    fn pending_rewind_corrupt_file_is_quarantined() {
        let ledger = TestDir::new("pending-corrupt-ledger");
        fs::write(ledger.path().join("pending_rewind.json"), "{not json").unwrap();
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), "any", 0, "rev").expect("match"),
            None,
            "损坏暂记按不匹配处理"
        );
        assert!(
            !ledger.path().join("pending_rewind.json").exists(),
            "损坏暂记必须被删除（隔离）"
        );
    }

    /// 复用目标被裁剪（不在索引）时按不复用处理并清理暂记（r7 minor 5）——
    /// 不硬报错卡死同目标重试。
    #[test]
    fn pending_rewind_evicted_reuse_target_is_ignored() {
        let ledger = TestDir::new("pending-evicted-ledger");
        let exec = TestDir::new("pending-evicted-exec");
        exec.write(
            "a.txt", "v1
",
        );
        let checkpoint = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "label",
        )
        .expect("checkpoint");
        // 暂记指向一个不存在的 id（模拟被 LRU/压力裁剪）。
        write_pending_rewind(ledger.path(), "ghost-id", 1, "c-999", "rev-a")
            .expect("write pending");
        let _ = &checkpoint;
        assert_eq!(
            matching_pending_pre_restore(ledger.path(), "ghost-id", 1, "rev-a").expect("match"),
            None,
            "复用目标不在索引时必须退回新建路径"
        );
        assert!(
            !ledger.path().join("pending_rewind.json").exists(),
            "指向已裁剪目标的暂记必须被清理"
        );
    }
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pinvou3-checkpoint-{label}-{}-{}",
                std::process::id(),
                now_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Write-then-rename over the target: the staging file is created
            // while the target still exists, so its inode differs from the
            // target's — and thus from the stat entry git cached at the
            // previous `add`, provided each path is written at most once
            // between index refreshes (every test here does). Rewriting in
            // place or via remove-then-write is not safe: ext4 hands the
            // just-freed inode right back, so a same-length rewrite landing
            // inside one timestamp tick can match the cached entry exactly
            // (same ino/mtime/size) and git trusts it without re-reading —
            // `add -A` keeps the stale blob and diff/restore assertions fail
            // intermittently (2026-09-18 flake, recurred on Linux in #576's
            // CI).
            let mut staging = path.clone().into_os_string();
            staging.push(format!(".pinvou3-new-{}", now_nanos()));
            let staging = PathBuf::from(staging);
            fs::write(&staging, content).unwrap();
            fs::rename(&staging, &path).unwrap();
        }

        fn read(&self, relative: &str) -> Option<String> {
            fs::read_to_string(self.0.join(relative)).ok()
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 敏感文件 exclude 语义：.env（任意深度，gitignore 语义的 exclude 文件
    /// 本就在任意深度生效）/私钥本体不进快照；.env.example、id_rsa.pub 等
    /// 示例/公钥照常进快照。匹配恒大小写不敏感（info/exclude 字符类模式），
    /// 大小写敏感文件系统上 `.ENV`/`ID_RSA`/`SERVER.KEY` 等变体同样命中。
    /// 本测试锚定 exclude 集合的回归。
    #[test]
    fn secret_files_are_excluded_but_examples_and_public_keys_are_tracked() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("secret-ledger");
        let exec = TestDir::new("secret-exec");
        exec.write(".env", "SECRET=1\n");
        exec.write("sub/.env", "SECRET=2\n");
        exec.write(".env.local", "SECRET=3\n");
        exec.write(".env.example", "EXAMPLE=\n");
        exec.write("id_rsa", "PRIVATE\n");
        exec.write("id_rsa.pub", "PUBLIC\n");
        exec.write("SERVER.KEY", "SECRET=4\n");
        exec.write("src/a.rs", "a\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();

        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        let is_tracked = |path: &str| tracked.lines().any(|line| line == path);
        assert!(is_tracked(".env.example"), "示例文件应照常进快照");
        assert!(is_tracked("id_rsa.pub"), "公钥应照常进快照");
        assert!(is_tracked("src/a.rs"));
        assert!(!is_tracked(".env"), ".env 不得进快照");
        assert!(!is_tracked("sub/.env"), "任意深度的 .env 不得进快照");
        assert!(!is_tracked(".env.local"));
        assert!(!is_tracked("id_rsa"), "私钥本体不得进快照");
        assert!(
            !tracked.lines().any(|line| line.eq_ignore_ascii_case(".env")
                || line.eq_ignore_ascii_case("server.key")
                || line.eq_ignore_ascii_case("id_rsa")),
            "大小写变体的秘密文件不得进快照: {tracked}"
        );
    }

    /// 残留锁清扫必须覆盖三类锁（index/config/refs 子树），且只删 `.lock`：
    /// config 与 refs 本体是仓库数据，绝不能被清扫误伤。零阈值变体用于钉住
    /// 覆盖面，无需回拨 mtime。
    #[test]
    fn stale_lock_sweep_covers_index_config_and_refs_locks() {
        let ledger = TestDir::new("lock-sweep-ledger");
        let repo = ledger.path().join("checkpoints");
        for rel in [
            "index.lock",
            "config.lock",
            "refs/checkpoints/head.lock",
            "refs/checkpoints/deadbeef.lock",
        ] {
            let path = repo.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"stale").unwrap();
        }
        // 仓库本体数据，清扫不得触碰。
        for rel in ["config", "index", "refs/checkpoints/head"] {
            let path = repo.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"data").unwrap();
        }

        assert!(clear_stale_git_locks_older_than(
            &repo,
            std::time::Duration::ZERO
        ));
        for rel in [
            "index.lock",
            "config.lock",
            "refs/checkpoints/head.lock",
            "refs/checkpoints/deadbeef.lock",
        ] {
            assert!(!repo.join(rel).exists(), "{rel} 应被清扫");
        }
        for rel in ["config", "index", "refs/checkpoints/head"] {
            assert!(repo.join(rel).exists(), "{rel} 是仓库数据，不得被清扫");
        }
        // 再跑一遍：无可清锁，返回 false 且不误删数据。
        assert!(!clear_stale_git_locks_older_than(
            &repo,
            std::time::Duration::ZERO
        ));
        assert!(repo.join("refs/checkpoints/head").exists());
    }

    /// 安全方向：年轻的锁（可能是并发活锁或刚启动的 git）即使让 git 调用
    /// 失败，也绝不能被清除——`git()` 的重试只在锁被确证陈旧时发生。
    #[test]
    fn young_lock_is_never_cleared_and_retry_is_declined() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("young-lock-ledger");
        let exec = TestDir::new("young-lock-exec");
        exec.write("a.rs", "v1\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();

        let repo = repo_dir(ledger.path());
        let lock = repo.join("index.lock");
        fs::write(&lock, b"held by a live git").unwrap();

        let error = git_ok(&repo, exec.path(), &["add", "-A"]).unwrap_err();
        assert!(
            format!("{error:#}").contains("失败"),
            "持锁时 git 调用必须失败: {error:#}"
        );
        assert!(lock.exists(), "年轻锁不得被清除");
    }

    /// `git_ref`（drop/invalidate 路径）与写路径同样享有陈旧锁清扫：
    /// 被超时 kill 的 update-ref 会留下 `refs/checkpoints/<id>.lock`，
    /// 无清扫时删 ref 永久失败而索引条目已删，ref 与对象随之泄漏。
    /// 锁龄用 `File::set_times` 回拨，不依赖 sleep。
    #[test]
    fn git_ref_clears_stale_ref_lock_and_retries() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("ref-lock-ledger");
        let exec = TestDir::new("ref-lock-exec");
        exec.write("a.rs", "v1\n");
        let checkpoint = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();

        let repo = repo_dir(ledger.path());
        let ref_name = format!("refs/checkpoints/{}", checkpoint.id);
        let lock = repo.join(format!("{ref_name}.lock"));
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, b"left by a killed git").unwrap();
        // 锁龄回拨到两个预算之外，跨过「确证陈旧」阈值。
        let stale = std::time::SystemTime::now() - 2 * GIT_COMMAND_TIMEOUT;
        let file = fs::File::options().write(true).open(&lock).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(stale))
            .unwrap();
        drop(file);

        git_ref(&repo, &["update-ref", "-d", &ref_name]).expect("陈旧 ref 锁应被清扫并重试成功");
        assert!(!lock.exists(), "陈旧 ref 锁应被清扫");
        // ref 确实已删：loose ref 文件随 update-ref -d 消失。
        assert!(
            !repo.join(&ref_name).exists(),
            "ref 文件应已随成功的 update-ref -d 删除"
        );
    }

    /// M3 回归：中文（非 ASCII）文件名在 diff 预览中原样上屏，不被
    /// core.quotepath 转成八进制转义。
    #[test]
    fn diff_preview_shows_non_ascii_paths_verbatim() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("quotepath-ledger");
        let exec = TestDir::new("quotepath-exec");
        exec.write("文档/需求.md", "v1\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        exec.write("文档/需求.md", "v2\n");
        exec.write("新建文件.rs", "fn main() {}\n");

        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id, true).unwrap();
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "文档/需求.md"),
            "中文路径必须原样出现在清单: {:?}",
            diff.changes
        );
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "新建文件.rs")
        );
        // The patch side carries the same pin: without the patch call's own
        // `-c core.quotepath=false`, the header would render octal escapes,
        // which both garbles the preview and blinds the secret filter's
        // section-header parse.
        assert!(
            diff.patch.contains("文档/需求.md"),
            "non-ASCII paths must reach the patch verbatim: {:?}",
            &diff.patch[..diff.patch.len().min(400)]
        );
        assert!(
            !diff.patch.contains("\\346"),
            "the patch must not carry octal-escaped paths: {:?}",
            &diff.patch[..diff.patch.len().min(400)]
        );
    }

    /// B1 回归（评审 Blocker）：大写命名的秘密文件（`.ENV`）在大小写敏感
    /// 文件系统上曾逃过 exclude 被快照跟踪，restore 的 icase purge 把它移出
    /// index 后被 `clean -fd` 物理删除。info/exclude 改用恒大小写不敏感的
    /// 字符类模式后（core.ignorecase 仍按文件系统探测，不强制 true）：`.ENV`
    /// 永不进快照、被 ignored 而不受 clean 波及——用户在 turn 间对它的修改
    /// 原样保留，restore 后仍然存在且内容不变。
    #[test]
    fn uppercase_secret_survives_restore_untouched() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("icase-ledger");
        let exec = TestDir::new("icase-exec");
        exec.write("ok.txt", "v1\n");
        exec.write(".ENV", "SECRET=before\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();

        // turn 1 同时改了普通文件和大写秘密文件。
        exec.write("ok.txt", "v2\n");
        exec.write(".ENV", "SECRET=after\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t2",
        )
        .unwrap();

        // 大写秘密从未进快照（exclude 的字符类模式恒大小写不敏感）。
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|line| line.eq_ignore_ascii_case(".env"))
        );

        // 回退到 turn 1：普通文件恢复，.ENV 存活且保持用户最新内容（未被
        // checkout-index 覆盖、未被 clean 删除）。
        restore_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
        assert_eq!(exec.read("ok.txt").as_deref(), Some("v1\n"));
        assert_eq!(
            exec.read(".ENV").as_deref(),
            Some("SECRET=after\n"),
            "大写秘密文件必须在 restore 后存活且内容不变"
        );
    }

    /// r24 B1 回归：大小写变体构建目录（Dist/，用户配置只排除小写 dist/）
    /// 在大小写敏感文件系统上曾被「info/exclude 裸小写不命中 + icase purge
    /// 移出 index + clean -fd 无 shadow -e 腿」三层错位物理删除，且预览不
    /// 可见。修复后：icase 字符类三层同向——Dist/ 被忽略（不进快照）且
    /// restore 后存活、内容不变；普通文件照常回退。
    #[test]
    fn case_variant_shadow_exclude_dir_survives_restore() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("distcase-ledger");
        let exec = TestDir::new("distcase-exec");
        exec.write(".gitignore", "dist/\n");
        exec.write("Dist/assets.bin", "user asset\n");
        exec.write("ok.txt", "v1\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        exec.write("ok.txt", "v2\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t2",
        )
        .unwrap();

        // 大小写变体目录不进快照（exclude 恒 icase）。
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|line| line.eq_ignore_ascii_case("Dist/assets.bin")),
            "大小写变体构建目录不得进快照: {tracked}"
        );

        // 回退到 turn 1：Dist/ 必须存活（此前被 clean -fd 物理删除）。
        restore_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
        assert_eq!(exec.read("ok.txt").as_deref(), Some("v1\n"));
        assert_eq!(
            exec.read("Dist/assets.bin").as_deref(),
            Some("user asset\n"),
            "大小写变体构建目录必须在 restore 后存活（r24 B1 数据丢失）"
        );
    }

    /// r28 B2 回归：嵌套凭据副本（`services/api/.aws/credentials`，含斜杠
    /// 模式在 gitignore 语义锚定根）曾被「info/exclude 裸形式不命中 → add -A
    /// 暂存 → icase purge 移出 → clean -fd 物理删除」与「staged 校验每轮
    /// bail」两条链打掉。修复后四层同向：不进快照、restore 存活、patch
    /// 预览按整路径命中（slash-aware secret_path_matches）。
    #[test]
    fn nested_credential_copies_survive_restore() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("nestedcred-ledger");
        let exec = TestDir::new("nestedcred-exec");
        exec.write("services/api/.aws/credentials", "AKIAEXAMPLE\n");
        exec.write("ok.txt", "v1\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .expect("嵌套凭据副本不得使快照 bail（r28 B2 前每轮必死）");
        exec.write("ok.txt", "v2\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t2",
        )
        .unwrap();

        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|line| line.contains(".aws/credentials")),
            "嵌套凭据副本不得进快照: {tracked}"
        );
        assert!(
            secret_path_matches("services/api/.aws/credentials"),
            "patch 预览过滤必须按整路径命中含斜杠模式（r28 B2）"
        );

        restore_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
        assert_eq!(exec.read("ok.txt").as_deref(), Some("v1\n"));
        assert_eq!(
            exec.read("services/api/.aws/credentials").as_deref(),
            Some("AKIAEXAMPLE\n"),
            "嵌套凭据副本必须在 restore 后存活（r28 B2 数据丢失）"
        );
    }

    /// 存量仓库一次性迁移：升级前被强制跟踪的敏感文件（含子目录深度）被
    /// 清出影子 index，普通文件不受影响，迁移成功后写 marker。
    /// 覆盖迁移主战场：index 里的秘密是旧内容、磁盘上是新内容（用户上次
    /// 快照后改过）——带 -f 的 rm --cached 必须照样成功（其 up-to-date 检查
    /// 不带 -f 时会把 lstat 打到进程 cwd 上，成败取决于启动目录）。
    #[test]
    fn migration_purges_previously_tracked_secrets_at_any_depth() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("migrate-ledger");
        let exec = TestDir::new("migrate-exec");
        exec.write(".env", "SECRET=1\n");
        exec.write("sub/.env", "SECRET=2\n");
        exec.write("ok.txt", "ok\n");
        // 模拟升级前的存量仓库：手工 init + 强制跟踪敏感文件（绕过 exclude）。
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(
            &repo,
            exec.path(),
            &["add", "-f", ".env", "sub/.env", "ok.txt"],
        )
        .unwrap();
        // 迁移主战场：快照后用户改了秘密文件，index 与工作区内容不同。
        exec.write(".env", "SECRET=changed\n");
        exec.write("sub/.env", "SECRET=changed-too\n");

        ensure_repo(ledger.path(), exec.path()).unwrap();
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|line| line == ".env" || line == "sub/.env")
        );
        assert!(tracked.lines().any(|line| line == "ok.txt"));
        assert!(repo.join("info").join("secret-excludes-v1").is_file());
        // 工作区文件绝不被迁移触碰。
        assert_eq!(exec.read(".env").as_deref(), Some("SECRET=changed\n"));
    }

    /// 迁移前打的旧快照 tree 里含 .env：read-tree 会把它装回 index（复活），
    /// restore 必须在写回工作区前清掉——影子 index 无 .env，工作区现有 .env
    /// 不被删除/覆盖，普通文件正常恢复。
    #[test]
    fn restore_does_not_resurrect_secret_entries_from_old_snapshot() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("resurrect-ledger");
        let exec = TestDir::new("resurrect-exec");
        exec.write("ok.txt", "v1\n");
        exec.write(".env", "SECRET=old\n");
        // 手工构造「迁移前」的快照 commit（强制跟踪 .env）并登记进 checkpoint 索引。
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(&repo, exec.path(), &["config", "core.autocrlf", "false"]).unwrap();
        git_ok(&repo, exec.path(), &["add", "-f", "ok.txt", ".env"]).unwrap();
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &tree,
                "-m",
                "legacy snapshot",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        let meta = CheckpointMeta {
            id: "c1-1".into(),
            turn: Some(1),
            kind: CheckpointKind::Turn,
            label: String::new(),
            commit: commit.clone(),
            created_at: 0,
        };
        git_ok(
            &repo,
            exec.path(),
            &[
                "update-ref",
                &format!("refs/checkpoints/{}", meta.id),
                &commit,
            ],
        )
        .unwrap();
        save_index(
            ledger.path(),
            &CheckpointIndex {
                version: 2,
                entries: vec![meta],
            },
        )
        .unwrap();

        // 用户当前的 .env（与快照内容不同）：restore 不得删除/覆盖它。
        exec.write(".env", "SECRET=current\n");
        restore_checkpoint(ledger.path(), exec.path(), "c1-1").unwrap();
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked.lines().any(|line| line == ".env"),
            "复活进 index 的 .env 必须被清除"
        );
        assert_eq!(
            exec.read(".env").as_deref(),
            Some("SECRET=current\n"),
            "工作区现有 .env 不得被恢复流程触碰"
        );
        assert_eq!(exec.read("ok.txt").as_deref(), Some("v1\n"));
    }

    /// 评审 M1 回归：含空格路径的秘密文件（`my dir/.env`）——git 原样输出
    /// 不引号包裹，段头 token 解析不到它，必须由 ---/+++ 行兜底整段剔除。
    #[test]
    fn diff_preview_filters_secret_paths_with_spaces() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("spacefilter-ledger");
        let exec = TestDir::new("spacefilter-exec");
        exec.write("ok.txt", "v1\n");
        exec.write("my dir/.env", "SECRET=old\n");
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(&repo, exec.path(), &["config", "core.autocrlf", "false"]).unwrap();
        git_ok(&repo, exec.path(), &["add", "-f", "ok.txt", "my dir/.env"]).unwrap();
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &tree,
                "-m",
                "legacy snapshot",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        git_ok(
            &repo,
            exec.path(),
            &["update-ref", "refs/checkpoints/c1-1", &commit],
        )
        .unwrap();
        save_index(
            ledger.path(),
            &CheckpointIndex {
                version: 2,
                entries: vec![CheckpointMeta {
                    id: "c1-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: String::new(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();

        exec.write("ok.txt", "v2\n");
        exec.write("my dir/.env", "SECRET=current\n");
        let diff = diff_checkpoint(ledger.path(), exec.path(), "c1-1", true).unwrap();
        assert!(
            diff.changes
                .iter()
                .all(|change| !change.path.ends_with(".env")),
            "含空格的秘密路径不得出现在清单: {:?}",
            diff.changes
        );
        // The patch must not carry the secret section either (the ---/+++
        // strip covers paths containing spaces).
        assert!(
            !diff.patch.contains("SECRET") && !diff.patch.contains(".env"),
            "secret section with spaces must not enter the patch: {:?}",
            diff.patch
        );
    }

    /// A binary file's diff section has no ---/+++ marker lines, so the
    /// header parse is its only predicate; and a pure-space path does not
    /// trigger git's C-quoting (`diff --git a/my key.pem b/my key.pem` is
    /// emitted verbatim), so the header must split at the LAST ` b/` to
    /// recover both full paths for `*.pem`-style patterns — a whitespace
    /// token scan would sever `key.pem` from its prefix and miss, leaking
    /// the secret path into the patch via the Binary line.
    #[test]
    fn binary_secret_section_with_spaced_path_is_dropped() {
        let patch = concat!(
            "diff --git a/ok.txt b/ok.txt\n",
            "index 1111111..2222222 100644\n",
            "--- a/ok.txt\n",
            "+++ b/ok.txt\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/my key.pem b/my key.pem\n",
            "Binary files a/my key.pem and b/my key.pem differ\n",
        );
        let filtered = filter_secret_paths_from_patch(patch);
        assert!(
            filtered.contains("diff --git a/ok.txt b/ok.txt"),
            "the non-secret section must survive: {filtered:?}"
        );
        assert!(
            !filtered.contains("key.pem"),
            "the binary secret section must be dropped wholesale: {filtered:?}"
        );
    }

    /// The C-quoted header shape (`diff --git "a/x" "b/x"`, emitted when a
    /// path contains quotes or tabs — plain spaces do NOT trigger quoting) is
    /// the only predicate a binary section has, and the first line of defense
    /// for text sections. The quoted branch must extract whole path segments,
    /// not split on whitespace.
    #[test]
    fn cquoted_secret_sections_are_dropped() {
        let quoted = "diff --git \"a/.env\" \"b/.env\"\n-SECRET=1\n";
        assert_eq!(
            filter_secret_paths_from_patch(quoted),
            "",
            "a C-quoted secret section with content must be dropped"
        );
        // Spaces inside a quoted path.
        let spaced = concat!(
            "diff --git \"a/my key.pem\" \"b/my key.pem\"\n",
            "Binary files a/my key.pem and b/my key.pem differ\n",
        );
        assert_eq!(
            filter_secret_paths_from_patch(spaced),
            "",
            "a C-quoted binary secret section must be dropped"
        );
        // A non-secret section around it must survive untouched.
        let mut mixed = String::from(
            "diff --git a/ok.txt b/ok.txt\n--- a/ok.txt\n+++ b/ok.txt\n@@ -1 +1 @@\n-a\n+b\n",
        );
        mixed.push_str("diff --git \"a/.env\" \"b/.env\"\n-SECRET=1\n");
        let filtered = filter_secret_paths_from_patch(&mixed);
        assert!(
            filtered.contains("ok.txt"),
            "clean section lost: {filtered:?}"
        );
        assert!(
            !filtered.contains(".env") && !filtered.contains("SECRET"),
            "quoted secret section survived: {filtered:?}"
        );
    }

    /// The marker predicates must stand alone. Synthetic shape (git would not
    /// emit a clean header with quoted markers): pins that the
    /// `--- "a/` / `+++ "b/` strips work without any help from the header
    /// parse, and that a space-bearing secret path matches through the
    /// unquoted marker form too.
    #[test]
    fn marker_lines_drop_the_section_without_header_help() {
        let quoted_markers = concat!(
            "diff --git a/placeholder b/placeholder\n",
            "--- \"a/.env\"\n+++ \"b/.env\"\n@@ -1 +1 @@\n-SECRET=1\n",
        );
        assert_eq!(
            filter_secret_paths_from_patch(quoted_markers),
            "",
            "quoted marker lines must drop the section on their own"
        );
        let spaced_marker = concat!(
            "diff --git a/placeholder b/placeholder\n",
            "--- a/my dir/.env\n+++ b/my dir/.env\n@@ -1 +1 @@\n-SECRET=1\n",
        );
        assert_eq!(
            filter_secret_paths_from_patch(spaced_marker),
            "",
            "space-bearing marker paths must drop the section on their own"
        );
    }

    /// A removed line whose CONTENT starts with `-- a/x` is emitted by git as
    /// `-` + content = `--- a/x`, indistinguishable from a real marker; the
    /// same holds for added `++ b/x` lines. The filter errs conservative and
    /// drops the whole (innocent) section. Pin that intent: a silent preview
    /// loss must stay a deliberate, tested trade-off, never an accident.
    #[test]
    fn content_lines_shaped_like_markers_drop_the_section_conservatively() {
        let patch = concat!(
            "diff --git a/docs.md b/docs.md\n",
            "--- a/docs.md\n",
            "+++ b/docs.md\n",
            "@@ -1,2 +1,2 @@\n",
            " context\n",
            "--- a/.env\n",
            "+++ b/.env\n",
        );
        assert_eq!(
            filter_secret_paths_from_patch(patch),
            "",
            "marker-shaped content lines must drop the section (conservative)"
        );
    }

    /// 评审 M2 回归：restore 打 PreRestore 触发 LRU 淘汰时不得淘汰恢复目标
    /// 本身（目标恰在淘汰区时 ref 被删 + commit 被 prune → read-tree 失败且
    /// 用户请求的历史被销毁）。
    #[test]
    fn restore_preserves_target_from_pre_restore_eviction() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("preserve-ledger");
        let exec = TestDir::new("preserve-exec");
        exec.write("a.txt", "0\n");
        let target = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        // 恰好打满 LRU 上限（target 是最老条目但尚未被淘汰）；restore 的
        // PreRestore 是第 MAX+1 条，溢出淘汰的第一顺位就是 target。
        for turn in 2..=MAX_CHECKPOINTS {
            exec.write("a.txt", &format!("{turn}\n"));
            create_checkpoint(
                ledger.path(),
                exec.path(),
                Some(turn as u32),
                CheckpointKind::Turn,
                "t",
            )
            .unwrap();
        }
        // restore 目标是最老条目：PreRestore 会触发淘汰，但 preserve 保住 target。
        let undo = restore_checkpoint(ledger.path(), exec.path(), &target.id).unwrap();
        assert_eq!(undo.kind, CheckpointKind::PreRestore);
        assert_eq!(exec.read("a.txt").as_deref(), Some("0\n"));
        // 目标条目仍在 index（未被自己的 PreRestore 淘汰）。
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert!(
            listed.iter().any(|entry| entry.id == target.id),
            "恢复目标不得被 PreRestore 的淘汰挤出: {listed:?}"
        );
    }

    /// r39 M2 回归：崩溃暂记在飞时，prepare_restore 的 PreRestore 登记不得
    /// 在索引满员淘汰中裁掉暂记的目标或其 PreRestore（淘汰按年龄、从不查看
    /// pending_rewind.json；暂记被「目标缺失」终结后 r36-M2 守卫失明，崩溃
    /// 边界被静默覆盖）。preserve 名额必须落到非暂记条目头上。
    #[test]
    fn prepare_restore_preserves_a_live_pendings_entries_at_index_full() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("pending-preserve-ledger");
        let exec = TestDir::new("pending-preserve-exec");
        exec.write("a.txt", "0\n");
        let oldest = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        let mut ids = vec![oldest.id.clone()];
        // 只填到 MAX-1：bootstrap 的 PreRestore 登记占满最后一格而不触发淘汰，
        // 让「满员淘汰」精确发生在暂记在飞后的那次不同边界 prepare 上。
        for turn in 2..MAX_CHECKPOINTS {
            exec.write("a.txt", &format!("{turn}\n"));
            ids.push(
                create_checkpoint(
                    ledger.path(),
                    exec.path(),
                    Some(turn as u32),
                    CheckpointKind::Turn,
                    "t",
                )
                .unwrap()
                .id,
            );
        }
        // 引导重试路径先把暂记立起来：索引 MAX-1 满时为最老条目登记
        // PreRestore（占满最后一格、不触发淘汰），再写暂记文件——此刻索引
        // 满员、暂记目标 = 最老条目（淘汰第一顺位）。
        let (_, pending_pre) = prepare_restore(ledger.path(), exec.path(), &oldest.id, None)
            .unwrap();
        write_pending_rewind(ledger.path(), &oldest.id, 0, &pending_pre.id, "rev-1").unwrap();
        assert!(live_pending_rewind(ledger.path()).unwrap().is_some());
        // 不同边界的 prepare_restore（目标 = 最新 Turn）：PreRestore 登记触发
        // MAX+1 溢出淘汰——暂记的两个 id 必须被 preserve 保住，名额落到次老
        // 条目头上。
        let newest = ids.last().unwrap().clone();
        let second_oldest = ids[1].clone();
        let (_, _) = prepare_restore(ledger.path(), exec.path(), &newest, None).unwrap();
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert!(
            listed.iter().any(|entry| entry.id == oldest.id),
            "在飞暂记的目标不得被他人 PreRestore 的淘汰挤出: {listed:?}"
        );
        assert!(
            listed.iter().any(|entry| entry.id == pending_pre.id),
            "在飞暂记的 PreRestore 不得被淘汰挤出: {listed:?}"
        );
        assert!(
            !listed.iter().any(|entry| entry.id == second_oldest),
            "淘汰名额必须落到非暂记条目（次老）头上: {listed:?}"
        );
        // 暂记必须仍然存活且成员校验通过（live_pending_rewind 不走终结臂）。
        let live = live_pending_rewind(ledger.path()).unwrap();
        assert_eq!(live.as_ref().map(|pending| pending.checkpoint_id.as_str()), Some(oldest.id.as_str()));
    }

    /// r39 M2 回归：入口窥视无副作用——不同边界判定为 true / 同边界为 false
    /// 都不得删除或改写暂记文件（拒绝路径顺手清暂记 = 引导重试丢失）。
    #[test]
    fn pending_boundary_probe_is_non_destructive() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("peek-ledger");
        let exec = TestDir::new("peek-exec");
        exec.write("a.txt", "a\n");
        let t1 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        let (_, pre) = prepare_restore(ledger.path(), exec.path(), &t1.id, None).unwrap();
        write_pending_rewind(ledger.path(), &t1.id, 0, &pre.id, "rev-1").unwrap();
        let pending_path = pending_rewind_path(ledger.path());
        let before = fs::read(&pending_path).unwrap();
        // 同边界 → false；不同边界 → true。两次窥视后文件逐字节不变。
        assert!(!pending_rewind_targets_different_checkpoint(ledger.path(), &t1.id).unwrap());
        assert!(pending_rewind_targets_different_checkpoint(ledger.path(), "c-other").unwrap());
        assert_eq!(fs::read(&pending_path).unwrap(), before);
        // 窥视对「目标已被淘汰」的暂记同样无副作用（终结臂只属于
        // live_pending_rewind——快照前奏/对账的清理约定）。
        drop_checkpoint(ledger.path(), &t1.id).unwrap();
        assert!(peek_pending_rewind(ledger.path()).unwrap().is_some());
        assert_eq!(fs::read(&pending_path).unwrap(), before);
    }

    /// 评审 M3 回归：损坏的 index.json 被隔离为 index.json.corrupt 并从空索引
    /// 重建——该会话的 checkpoint 能力恢复（此前所有操作永久失败）。
    #[test]
    fn corrupt_index_is_quarantined_and_rebuilt_empty() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("corrupt-ledger");
        let exec = TestDir::new("corrupt-exec");
        exec.write("a.txt", "a\n");
        fs::create_dir_all(checkpoints_dir(ledger.path())).unwrap();
        fs::write(index_path(ledger.path()), b"{ not valid json !!!").unwrap();

        let index = load_index(ledger.path()).expect("quarantined load");
        assert!(index.entries.is_empty());
        // 隔离文件带时间戳后缀（二次损坏不覆盖首次取证）。
        let quarantined = fs::read_dir(checkpoints_dir(ledger.path()))
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("index.json.corrupt-")
            });
        assert!(quarantined);
        assert!(!index_path(ledger.path()).exists());
        // 快照能力恢复。
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        assert_eq!(list_checkpoints(ledger.path()).unwrap().len(), 1);
    }

    /// 评审 M4 回归：嵌套 git 仓库在 changes 清单中标注为 gitlink（快照不跟踪
    /// 其内容、restore 不 materialize——UI 必须如实区分，不能暗示可回退）。
    #[test]
    fn nested_git_repo_is_labeled_gitlink_in_changes() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("gitlink-ledger");
        let exec = TestDir::new("gitlink-exec");
        exec.write("ok.txt", "v1\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        // turn 1：在执行根内 clone 出一个嵌套仓库（git 以 gitlink 记录；
        // 需要真实 .git 子目录 + commit——--git-dir 指向目录本身只会产生普通文件，
        // unborn HEAD 的空仓库也不会进 index）。
        exec.write("ok.txt", "v2\n");
        let nested = exec.path().join("vendor/lib");
        let nested_git = nested.join(".git");
        fs::create_dir_all(&nested).unwrap();
        git_ok(&nested_git, &nested, &["init"]).unwrap();
        git_ok(&nested_git, &nested, &["config", "user.name", "T"]).unwrap();
        git_ok(&nested_git, &nested, &["config", "user.email", "t@t"]).unwrap();
        exec.write("vendor/lib/README.md", "lib\n");
        git_ok(&nested_git, &nested, &["add", "README.md"]).unwrap();
        git_ok(&nested_git, &nested, &["commit", "-m", "init"]).unwrap();

        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id, true).unwrap();
        let gitlink = diff
            .changes
            .iter()
            .find(|change| change.path == "vendor/lib")
            .expect("nested repo in changes");
        assert_eq!(gitlink.status, "gitlink");
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "ok.txt" && change.status == "modified")
        );
    }

    /// 对迁移前的 legacy 快照做 diff 预览（快照树里含 .env）：变更列表不得
    /// 泄露敏感条目或密钥明文；普通文件变更正常渲染。
    #[test]
    fn diff_preview_filters_secret_paths_from_legacy_snapshot() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("difffilter-ledger");
        let exec = TestDir::new("difffilter-exec");
        exec.write("ok.txt", "v1\n");
        exec.write(".env", "SECRET=old\n");
        // 同 restore 测试：手工构造含秘密的 legacy 快照并登记。
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(&repo, exec.path(), &["config", "core.autocrlf", "false"]).unwrap();
        git_ok(&repo, exec.path(), &["add", "-f", "ok.txt", ".env"]).unwrap();
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &tree,
                "-m",
                "legacy snapshot",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        git_ok(
            &repo,
            exec.path(),
            &["update-ref", "refs/checkpoints/c1-1", &commit],
        )
        .unwrap();
        save_index(
            ledger.path(),
            &CheckpointIndex {
                version: 2,
                entries: vec![CheckpointMeta {
                    id: "c1-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: String::new(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();

        // 当前状态：ok.txt 改了，.env 也改了（purge 后不在 index，快照→当前
        // 的原始 diff 会显示 D .env）。
        exec.write("ok.txt", "v2\n");
        exec.write(".env", "SECRET=current\n");
        let diff = diff_checkpoint(ledger.path(), exec.path(), "c1-1", true).unwrap();
        assert!(
            diff.changes.iter().all(|change| change.path != ".env"),
            "敏感条目不得出现在变更清单: {:?}",
            diff.changes
        );
        assert!(diff.changes.iter().any(|change| change.path == "ok.txt"));
        // The patch renders straight to the CLI: the secret section (path
        // and content) must be stripped wholesale, normal files kept.
        assert!(
            !diff.patch.contains("SECRET"),
            "secret content must not enter the patch: {:?}",
            diff.patch
        );
        assert!(
            !diff.patch.contains(".env"),
            "secret path must not enter the patch: {:?}",
            diff.patch
        );
        assert!(
            diff.patch.contains("ok.txt"),
            "normal file diffs must survive in the patch: {:?}",
            diff.patch
        );
    }

    /// r30 MAJOR-1 回归：`.ssh/**` 目录 glob 曾只按根级命中——legacy 树里
    /// 的嵌套副本（`sub/.ssh/agent-env`，basename 不落任何 basename 模式）
    /// 从 patch 预览泄漏明文、清单误报「将删除」。修复后按中路径命中。
    #[test]
    fn diff_preview_filters_nested_dir_glob_secrets_from_legacy_snapshot() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("dirglob-ledger");
        let exec = TestDir::new("dirglob-exec");
        exec.write(
            "ok.txt", "v1
",
        );
        exec.write(
            "sub/.ssh/agent-env",
            "SECRET MATERIAL 1234
",
        );
        exec.write(
            "sub/.ssh/authorized_keys",
            "ssh-ed25519 AAAA
",
        );
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(&repo, exec.path(), &["config", "core.autocrlf", "false"]).unwrap();
        git_ok(
            &repo,
            exec.path(),
            &[
                "add",
                "-f",
                "ok.txt",
                "sub/.ssh/agent-env",
                "sub/.ssh/authorized_keys",
            ],
        )
        .unwrap();
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &tree,
                "-m",
                "legacy snapshot",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        git_ok(
            &repo,
            exec.path(),
            &["update-ref", "refs/checkpoints/c1-1", &commit],
        )
        .unwrap();
        save_index(
            ledger.path(),
            &CheckpointIndex {
                version: 2,
                entries: vec![CheckpointMeta {
                    id: "c1-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: String::new(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();

        // 当前状态：ok.txt 改了，嵌套 .ssh 内容也变了（purge 后不在 index）。
        exec.write(
            "ok.txt", "v2
",
        );
        exec.write(
            "sub/.ssh/agent-env",
            "SECRET MATERIAL 5678
",
        );
        let diff = diff_checkpoint(ledger.path(), exec.path(), "c1-1", true).unwrap();
        assert!(
            diff.changes
                .iter()
                .all(|change| !change.path.contains(".ssh/")),
            "嵌套 .ssh 条目不得出现在变更清单（中路径命中）: {:?}",
            diff.changes
        );
        assert!(diff.changes.iter().any(|change| change.path == "ok.txt"));
        assert!(
            !diff.patch.contains("SECRET MATERIAL"),
            "嵌套 .ssh 明文不得进入 patch: {:?}",
            diff.patch
        );
        assert!(
            !diff.patch.contains(".ssh/"),
            "嵌套 .ssh 路径不得进入 patch: {:?}",
            diff.patch
        );
        assert!(
            diff.patch.contains("ok.txt"),
            "normal file diffs must survive in the patch: {:?}",
            diff.patch
        );
    }

    /// The secret filter parses `a/` / `b/` path prefixes, and those prefixes
    /// are configurable. `isolated_git_command` neutralizes the system and
    /// global config but not the shadow repo's own, which for an unbound
    /// session lives inside the directory the agent's tools can write. A
    /// single `diff.noprefix = true` there used to make every section
    /// unparseable and pass `.env` plaintext straight into the preview.
    #[test]
    fn diff_preview_filters_secrets_even_when_the_repo_config_drops_diff_prefixes() {
        // git_available() itself logs the skip; no inline copy needed.
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("noprefix-ledger");
        let exec = TestDir::new("noprefix-exec");
        exec.write("ok.txt", "v1\n");
        exec.write(".env", "SECRET=old\n");
        let repo = repo_dir(ledger.path());
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, exec.path(), &["init"]).unwrap();
        git_ok(&repo, exec.path(), &["config", "core.autocrlf", "false"]).unwrap();
        // The hostile bit: repo-local config that strips the prefixes.
        git_ok(&repo, exec.path(), &["config", "diff.noprefix", "true"]).unwrap();
        git_ok(&repo, exec.path(), &["add", "-f", "ok.txt", ".env"]).unwrap();
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .unwrap()
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &[
                "-c",
                "user.name=Pinvou",
                "-c",
                "user.email=pinvou@localhost",
                "commit-tree",
                &tree,
                "-m",
                "legacy snapshot",
            ],
        )
        .unwrap()
        .trim()
        .to_string();
        git_ok(
            &repo,
            exec.path(),
            &["update-ref", "refs/checkpoints/np-1", &commit],
        )
        .unwrap();
        save_index(
            ledger.path(),
            &CheckpointIndex {
                version: 2,
                entries: vec![CheckpointMeta {
                    id: "np-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: String::new(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();
        exec.write("ok.txt", "v2\n");
        exec.write(".env", "SECRET=current\n");
        let diff = diff_checkpoint(ledger.path(), exec.path(), "np-1", true).unwrap();
        assert!(
            !diff.patch.contains("SECRET"),
            "repo-local diff.noprefix must not defeat the secret filter: {:?}",
            diff.patch
        );
        assert!(
            !diff.patch.contains(".env"),
            "repo-local diff.noprefix must not leak the secret path: {:?}",
            diff.patch
        );
        assert!(
            diff.patch.contains("ok.txt"),
            "the non-secret section must still be rendered: {:?}",
            diff.patch
        );
    }

    /// The filter rewrites the patch line by line, so it must not silently
    /// normalize line endings: a CRLF project's diff that comes back LF-only
    /// no longer applies, and a patch without a trailing newline must not
    /// grow one.
    #[test]
    fn secret_filter_preserves_line_endings_verbatim() {
        let crlf = "diff --git a/ok.txt b/ok.txt\r\n--- a/ok.txt\r\n+++ b/ok.txt\r\n-a\r\n+b\r\n";
        assert_eq!(
            filter_secret_paths_from_patch(crlf),
            crlf,
            "CR bytes must survive the filter"
        );
        let no_trailing_newline = "diff --git a/ok.txt b/ok.txt\n-a\n+b";
        assert_eq!(
            filter_secret_paths_from_patch(no_trailing_newline),
            no_trailing_newline,
            "the filter must not append a terminator the input did not have"
        );
        // Still filters, with CRLF input.
        let secret = "diff --git a/.env b/.env\r\n-SECRET=1\r\n";
        assert_eq!(filter_secret_paths_from_patch(secret), "");
    }

    /// The checkpoint index carries the first 80 characters of user messages
    /// as labels, so it must be created private and STAY private when
    /// rewritten over a world-readable file from an older build.
    #[test]
    fn checkpoint_index_is_written_private() {
        use crate::platform::test_support::{loosen_to_world_readable_for_test, permission_bits};
        let ledger = TestDir::new("index-perms");
        let index = CheckpointIndex {
            version: 2,
            entries: Vec::new(),
        };
        save_index(ledger.path(), &index).unwrap();
        let path = index_path(ledger.path());
        let Some(mode) = permission_bits(&path) else {
            return; // the platform has no POSIX mode bits
        };
        assert_eq!(mode, 0o600, "a fresh checkpoint index must be private");

        // An index left behind by a build that used fs::File::create.
        assert!(loosen_to_world_readable_for_test(&path));
        save_index(ledger.path(), &index).unwrap();
        assert_eq!(
            permission_bits(&path),
            Some(0o600),
            "a rewritten index must be tightened to private"
        );
    }

    /// Patch output beyond `DIFF_PATCH_LIMIT` must truncate and set the flag,
    /// and truncation happens after the secret filter (the dropped tail can
    /// never be unfiltered content).
    #[test]
    fn diff_patch_truncates_to_limit_with_flag() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("patchtrunc-ledger");
        let exec = TestDir::new("patchtrunc-exec");
        exec.write("big.txt", "v1\n");
        let target = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        exec.write(
            "big.txt",
            &format!("v2\n{}\n", "x".repeat(DIFF_PATCH_LIMIT)),
        );
        let diff = diff_checkpoint(ledger.path(), exec.path(), &target.id, true).unwrap();
        assert!(
            diff.patch_truncated,
            "a patch over the cap must carry the truncation flag: len={}",
            diff.patch.len()
        );
        // Truncation semantics: the body is clamped to DIFF_PATCH_LIMIT and
        // the notice travels only through the `patch_truncated` flag — no
        // (single-language) tail is embedded in the data.
        assert!(
            !diff.patch.contains("已截断"),
            "the patch data must not embed a notice tail: {:?}",
            &diff.patch[diff.patch.len().saturating_sub(64)..]
        );
        assert!(
            diff.patch.len() <= DIFF_PATCH_LIMIT,
            "the patch must be clamped to DIFF_PATCH_LIMIT: len={}",
            diff.patch.len()
        );
        // Truncation lands on a line boundary (the `--output` file is read
        // back line-clamped): the secret filter depends on whole
        // `diff --git` headers to drop sections wholesale — a half header
        // could both miss a section and leak fragments to the consumer.
        assert!(
            diff.patch.ends_with('\n') || diff.patch.is_empty(),
            "the truncated patch must end on a whole line: {:?}",
            &diff.patch[diff.patch.len().saturating_sub(64)..]
        );
        assert!(
            diff.patch.contains("big.txt"),
            "the retained head must still carry the file header: {:?}",
            &diff.patch[..diff.patch.len().min(200)]
        );
        // The spill file must not outlive the call: `.checkpoint-diff-*.tmp`
        // holds the UNFILTERED diff (secret sections included), so a leftover
        // is plaintext residue beside the shadow repo. Guards both the
        // TempDiffFile drop and the spill's placement under the ledger (a
        // regression to a shared temp dir would also fail the read-dir here).
        let spills: Vec<String> = fs::read_dir(ledger.path().join("checkpoints"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".checkpoint-diff-"))
            .collect();
        assert!(
            spills.is_empty(),
            "diff spill files must be reaped after the call: {spills:?}"
        );
    }

    /// The post-filter clamp exists for `from_utf8_lossy` expansion: a raw
    /// diff under the cap whose bytes are invalid UTF-8 grows threefold when
    /// read back as a String and can cross the cap after filtering. The flag
    /// must flip even though the raw read never truncated.
    #[test]
    fn lossy_expansion_crossing_the_cap_flips_the_truncated_flag() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("lossy-ledger");
        let exec = TestDir::new("lossy-exec");
        exec.write("blob.txt", "");
        let target = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        // Lone continuation bytes (0x80..0x9F): invalid UTF-8, no NUL anywhere
        // (so git still emits a TEXT diff), ~200 KB raw — under the cap.
        let bytes: Vec<u8> = (0..200_000usize)
            .map(|i| 0x80u8 + (i % 0x20) as u8)
            .collect();
        fs::write(exec.path().join("blob.txt"), &bytes).unwrap();
        let diff = diff_checkpoint(ledger.path(), exec.path(), &target.id, true).unwrap();
        assert!(
            diff.patch_truncated,
            "lossy expansion past the cap must set the flag: len={}",
            diff.patch.len()
        );
        assert!(
            diff.patch.len() <= DIFF_PATCH_LIMIT,
            "the post-filter clamp must hold: len={}",
            diff.patch.len()
        );
        assert!(
            diff.patch.ends_with('\n'),
            "the clamp must end on a line boundary"
        );
    }

    /// `--raw` C-quotes paths containing quotes/tabs even with
    /// `core.quotepath=false`; the trailing quote defeats a plain suffix
    /// match, so the changes list would keep an entry the patch filter drops.
    #[test]
    fn raw_status_quoted_secret_paths_still_match() {
        assert!(raw_status_path_is_secret(".env"));
        assert!(raw_status_path_is_secret("my key.pem"));
        // The C-quoted form as `--raw` emits it, outer quotes and all.
        assert!(raw_status_path_is_secret("\"my \\\"key.pem\""));
        assert!(!raw_status_path_is_secret("ok.txt"));
        assert!(!raw_status_path_is_secret("\"quoted but clean.txt\""));
    }

    /// A spill file left by a hard kill must be reaped by the next
    /// ensure_repo once it is older than the safety window; a recent one (a
    /// live diff in this or another process) must be left alone.
    #[test]
    fn stale_diff_spill_files_are_reaped() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("spill-reap-ledger");
        let exec = TestDir::new("spill-reap-exec");
        exec.write("a.txt", "v1\n");
        let dir = ledger.path().join("checkpoints");
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join(".checkpoint-diff-1-100.tmp");
        fs::write(&stale, b"stale unfiltered diff").unwrap();
        let fresh = dir.join(".checkpoint-diff-1-200.tmp");
        fs::write(&fresh, b"live diff").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        assert!(!stale.exists(), "a stale spill must be reaped");
        assert!(fresh.exists(), "a recent spill must be kept");
    }

    /// 按 id 作废「未成活」快照：条目与 ref 都移除；不存在幂等 false；
    /// turn 序号为 None 的条目同样可按 id 删。
    #[test]
    fn drop_checkpoint_removes_entry_by_id() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("drop-ledger");
        let exec = TestDir::new("drop-exec");
        exec.write("a.txt", "0\n");
        let kept = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        let unsent = create_checkpoint(
            ledger.path(),
            exec.path(),
            None,
            CheckpointKind::Turn,
            "unsent",
        )
        .unwrap();

        assert!(drop_checkpoint(ledger.path(), &unsent.id).unwrap());
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, kept.id);
        // ref 已删；幂等：再删返回 false。
        let repo = repo_dir(ledger.path());
        let ref_exists = git(
            &repo,
            exec.path(),
            &[
                "show-ref",
                "--verify",
                &format!("refs/checkpoints/{}", unsent.id),
            ],
        )
        .map(|output| output.status.success())
        .unwrap_or(false);
        assert!(!ref_exists);
        assert!(!drop_checkpoint(ledger.path(), &unsent.id).unwrap());
        // 非法 id 如实报错。
        assert!(drop_checkpoint(ledger.path(), "../escape").is_err());
    }

    #[test]
    fn rejects_invalid_checkpoint_ids() {
        assert!(validate_checkpoint_id("c1-123").is_ok());
        assert!(validate_checkpoint_id("").is_err());
        assert!(validate_checkpoint_id("../escape").is_err());
        assert!(validate_checkpoint_id("a/b").is_err());
        assert!(validate_checkpoint_id("a b").is_err());
        assert!(validate_checkpoint_id(&"x".repeat(65)).is_err());
    }

    #[test]
    fn errors_honestly_when_execution_root_missing() {
        let ledger = TestDir::new("missing-ledger");
        let missing = TestDir::new("missing-exec");
        let ghost = missing.path().join("ghost");
        let result =
            create_checkpoint(ledger.path(), &ghost, Some(1), CheckpointKind::Turn, "test");
        assert!(result.is_err());
        assert!(list_checkpoints(ledger.path()).unwrap().is_empty());
    }

    #[test]
    fn create_list_and_restore_round_trip() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("round-ledger");
        let exec = TestDir::new("round-exec");
        exec.write("src/main.rs", "fn main() {}\n");
        exec.write("README.md", "v1\n");

        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "第一轮消息",
        )
        .unwrap();
        assert_eq!(first.turn, Some(1));

        // turn 1 的改动：修改既有文件、新建文件、子目录文件。
        exec.write("src/main.rs", "fn main() { println!(\"hi\"); }\n");
        exec.write("src/new.rs", "pub fn added() {}\n");
        std::fs::remove_file(exec.path().join("README.md")).unwrap();

        let second = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "第二轮消息",
        )
        .unwrap();

        let listed = list_checkpoints(ledger.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, first.id);
        assert_eq!(listed[1].id, second.id);

        // diff 预览：快照（第一轮前）→ 当前 = 3 个文件的变更。
        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id, true).unwrap();
        assert_eq!(diff.checkpoint.id, first.id);
        assert_eq!(diff.changes.len(), 3);
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "src/new.rs" && change.status == "added")
        );
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "README.md" && change.status == "deleted")
        );
        assert!(
            diff.changes
                .iter()
                .any(|change| change.path == "src/main.rs" && change.status == "modified")
        );
        // The patch must carry the round trip too: a fresh file's diff body is
        // what the CLI renderer prints, not just the status line.
        assert!(
            diff.patch.contains("src/new.rs") && diff.patch.contains("pub fn added()"),
            "added files must appear in the patch with content: {:?}",
            &diff.patch[..diff.patch.len().min(400)]
        );

        // 回滚到第一轮前：文件内容还原、新建文件被删除；返回回滚点可反悔。
        let undo = restore_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
        assert_eq!(undo.kind, CheckpointKind::PreRestore);
        assert_eq!(exec.read("src/main.rs").as_deref(), Some("fn main() {}\n"));
        assert_eq!(exec.read("README.md").as_deref(), Some("v1\n"));
        assert!(exec.read("src/new.rs").is_none());

        // 反悔：回滚到回滚点，turn 1 的改动回来。
        restore_checkpoint(ledger.path(), exec.path(), &undo.id).unwrap();
        assert_eq!(
            exec.read("src/main.rs").as_deref(),
            Some("fn main() { println!(\"hi\"); }\n")
        );
        assert_eq!(
            exec.read("src/new.rs").as_deref(),
            Some("pub fn added() {}\n")
        );
        assert!(exec.read("README.md").is_none());

        // 未知 checkpoint 如实报错。
        assert!(restore_checkpoint(ledger.path(), exec.path(), "c999-1").is_err());
    }

    #[test]
    fn empty_turn_reuses_previous_commit_but_keeps_alignment() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("empty-ledger");
        let exec = TestDir::new("empty-exec");
        exec.write("a.txt", "a\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        // 无变更的 turn：复用同一 commit，但 meta 仍登记（turn 对齐不漂移）。
        let second = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t2",
        )
        .unwrap();
        assert_eq!(first.commit, second.commit);
        assert_ne!(first.id, second.id);
        assert_eq!(list_checkpoints(ledger.path()).unwrap().len(), 2);
    }

    #[test]
    fn lru_keeps_only_newest_entries() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("lru-ledger");
        let exec = TestDir::new("lru-exec");
        exec.write("a.txt", "0\n");
        for turn in 1..=(MAX_CHECKPOINTS + 3) {
            exec.write("a.txt", &format!("{turn}\n"));
            create_checkpoint(
                ledger.path(),
                exec.path(),
                Some(turn as u32),
                CheckpointKind::Turn,
                "t",
            )
            .unwrap();
        }
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert_eq!(listed.len(), MAX_CHECKPOINTS);
        // 最老的 3 条已被裁掉，剩余保持创建顺序。
        assert_eq!(listed[0].turn, Some(4));
        assert_eq!(
            listed.last().unwrap().turn,
            Some((MAX_CHECKPOINTS + 3) as u32)
        );
        // 裁剪后恢复仍可用（最新 checkpoint 的对象可达）。
        exec.write("a.txt", "dirty\n");
        let newest = listed.last().unwrap().id.clone();
        restore_checkpoint(ledger.path(), exec.path(), &newest).unwrap();
        assert_eq!(
            exec.read("a.txt").as_deref(),
            Some(format!("{}\n", MAX_CHECKPOINTS + 3).as_str())
        );
    }

    #[test]
    fn ledger_inside_execution_root_is_excluded_from_snapshot_and_clean() {
        if !git_available() {
            return;
        }
        // 临时代码会话两根相同：checkpoint 数据在执行根内，必须被排除，
        // 否则快照自我递归、restore 的 clean 会误删账本。
        let root = TestDir::new("same-root");
        let ledger = root.path().join("ledger-nested");
        fs::create_dir_all(&ledger).unwrap();
        root.write("code.txt", "v1\n");
        create_checkpoint(&ledger, root.path(), Some(1), CheckpointKind::Turn, "t1").unwrap();
        root.write("code.txt", "v2\n");
        let undo = restore_checkpoint(
            &ledger,
            root.path(),
            &list_checkpoints(&ledger).unwrap()[0].id.clone(),
        )
        .unwrap();
        let _ = undo;
        assert_eq!(root.read("code.txt").as_deref(), Some("v1\n"));
        // checkpoint 数据未被 clean 删除，索引与仓库仍在。
        assert!(index_path(&ledger).is_file());
        assert!(repo_dir(&ledger).join("HEAD").is_file());
        assert_eq!(list_checkpoints(&ledger).unwrap().len(), 2);
    }

    #[test]
    fn ignored_directories_are_not_tracked() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("ignore-ledger");
        let exec = TestDir::new("ignore-exec");
        exec.write("src/a.rs", "a\n");
        exec.write("node_modules/pkg/index.js", "dep\n");
        // fs_is_case_insensitive 的探针文件：并发同根会话的 add -A 不得卷入。
        exec.write(".Pinvou-Icase-Probe-1234", "");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t",
        )
        .unwrap();
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked
                .lines()
                .any(|line| line.starts_with(".Pinvou-Icase-Probe-")),
            "探针文件不得进快照: {tracked}"
        );
        // node_modules 内的变化不产生新 commit（被 exclude，不进入快照）。
        exec.write("node_modules/pkg/index.js", "dep2\n");
        let second = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t",
        )
        .unwrap();
        assert_eq!(first.commit, second.commit);
        // restore 不删除 turn 中新建的 ignored 文件（已知限制，protect node_modules）。
        exec.write("node_modules/pkg/new.js", "new dep\n");
        restore_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
        assert!(exec.read("node_modules/pkg/new.js").is_some());
    }

    #[test]
    fn keepfile_idiom_does_not_kill_snapshots() {
        if !git_available() {
            return;
        }
        // r22 R1 回归 pin：标准 keepfile 惯用法（`dist/*` + `!dist/.gitkeep`）
        // 曾被 r21 M3 的 `:(icase)dist/` staged bail 每回合判死（warn-only、
        // index 被清、会话静默失去回退）。快照必须存活，且 keepfile 不进
        // 快照——排除语义是「不进」而非「拒绝」。
        let ledger = TestDir::new("keepfile-ledger");
        let exec = TestDir::new("keepfile-exec");
        exec.write(".gitignore", "dist/*\n!dist/.gitkeep\n");
        exec.write("dist/.gitkeep", "");
        exec.write("src/a.rs", "a\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t",
        )
        .expect("keepfile 惯用法不得使快照 bail");
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked.lines().any(|line| line.starts_with("dist/")),
            "keepfile 不得进快照: {tracked}"
        );
        exec.write("src/a.rs", "a2\n");
        let second = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t",
        )
        .expect("后续回合快照必须继续存活");
        assert_ne!(first.commit, second.commit);
    }

    #[test]
    fn pierced_shadow_exclude_is_purged_not_fatal() {
        if !git_available() {
            return;
        }
        // r21 M3 的缺失 pin（r22 R1 补）：`!node_modules/` 否定行击穿
        // info/exclude 后 add -A 照收依赖条目——必须被 rm --cached 摘除而非
        // bail；快照存活、依赖内容不进树、第二回合自愈复用同一树。
        // r23 minor：嵌套依赖（`packages/app/node_modules/...`）由
        // `**/<dir>/**` 嵌套腿摘除——旧的 `**/<dir>/` 形状在 rm --cached 的
        // pathspec 匹配里是空集（probe 实证），此处一并钉住。
        let ledger = TestDir::new("pierce-ledger");
        let exec = TestDir::new("pierce-exec");
        exec.write(".gitignore", "!node_modules/\n");
        exec.write("node_modules/pkg/index.js", "dep\n");
        exec.write("packages/app/node_modules/lib/index.js", "nested dep\n");
        exec.write("src/a.rs", "a\n");
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t",
        )
        .expect("击穿的依赖条目必须被 purge 而非 bail");
        let repo = repo_dir(ledger.path());
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).unwrap();
        assert!(
            !tracked.lines().any(|line| line.contains("node_modules/")),
            "击穿条目（根级与嵌套）不得留在影子 index/树中: {tracked}"
        );
        exec.write("node_modules/pkg/index.js", "dep2\n");
        let second = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t",
        )
        .unwrap();
        assert_eq!(first.commit, second.commit);
    }

    /// r39 minor 5(c)：purge_secret_survivors_exactly 的存在性 fixture——
    /// `.env` 与 `.ENV` 在影子 index 中共存时，icase pathspec 的单次
    /// `rm --cached` 只移除一个大小写变体（git 2.43 实证），幸存变体由精确
    /// 路径回扫清除；此前删除两条 purge 调用全部测试仍绿（评审变异实证）。
    /// 构造「排除规则生效前」的存量旧树（plumbing 直填两个变体——cacheinfo
    /// 不落盘，case-insensitive 文件系统上 index 也能同时持有两者；`.ENV`
    /// 的 blob 内容取自探测临时文件，blob 与来源文件名无关），apply_restore
    /// 的 read-tree 把两个变体同时带入 index：删掉幸存者回扫腿，`.ENV` 将
    /// 抵达 checkout-index 被写回工作区（必须变红）。
    #[test]
    fn apply_restore_purges_both_icase_secret_variants_from_a_legacy_tree() {
        if !git_available() {
            return;
        }
        const LEGACY_ID: &str = "c-legacy-icase";
        let ledger = TestDir::new("icase-purge-ledger");
        let exec = TestDir::new("icase-purge-exec");
        exec.write(".env", "SECRET=one\n");
        exec.write("env_upper_probe.tmp", "SECRET=two\n");
        exec.write("a.txt", "a\n");
        let _bootstrap =
            create_checkpoint(ledger.path(), exec.path(), Some(1), CheckpointKind::Turn, "t1")
                .expect("bootstrap checkpoint");
        let repo = repo_dir(ledger.path());
        // git() 只设 --git-dir/--work-tree，进程 cwd 不在 exec——hash-object
        // 的读取路径按绝对路径传。
        let abs = |name: &str| {
            exec.path()
                .join(name)
                .to_str()
                .expect("utf-8 test path")
                .to_string()
        };
        let blob_env = git_ok(&repo, exec.path(), &["hash-object", "-w", "--", &abs(".env")])
            .expect("hash .env")
            .trim()
            .to_string();
        let blob_env_upper = git_ok(
            &repo,
            exec.path(),
            &["hash-object", "-w", "--", &abs("env_upper_probe.tmp")],
        )
        .expect("hash upper probe")
        .trim()
        .to_string();
        let blob_a = git_ok(&repo, exec.path(), &["hash-object", "-w", "--", &abs("a.txt")])
            .expect("hash a.txt")
            .trim()
            .to_string();
        for (blob, path) in [
            (&blob_env, ".env"),
            (&blob_env_upper, ".ENV"),
            (&blob_a, "a.txt"),
        ] {
            git_ok(
                &repo,
                exec.path(),
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{path}"),
                ],
            )
            .unwrap_or_else(|error| panic!("cacheinfo {path}: {error}"));
        }
        let tree = git_ok(&repo, exec.path(), &["write-tree"])
            .expect("legacy tree")
            .trim()
            .to_string();
        let commit = git_ok(
            &repo,
            exec.path(),
            &["commit-tree", &tree, "-m", "legacy snapshot with secret variants"],
        )
        .expect("legacy commit")
        .trim()
        .to_string();
        git_ok(
            &repo,
            exec.path(),
            &["update-ref", &format!("refs/checkpoints/{LEGACY_ID}"), &commit],
        )
        .expect("ref legacy snapshot");
        let mut index = load_index(ledger.path()).unwrap();
        index.entries.push(CheckpointMeta {
            id: LEGACY_ID.to_string(),
            turn: Some(2),
            kind: CheckpointKind::Turn,
            label: "legacy".to_string(),
            commit,
            created_at: now_seconds(),
        });
        save_index(ledger.path(), &index).unwrap();

        // 恢复到存量旧树：read-tree 带入两个变体 → 两级 purge 清场 →
        // checkout-index 只落 a.txt。
        restore_checkpoint(ledger.path(), exec.path(), LEGACY_ID).expect("restore legacy tree");
        let tracked = git_ok(&repo, exec.path(), &["ls-files"]).expect("ls-files");
        assert!(
            !tracked
                .lines()
                .any(|line| line.eq_ignore_ascii_case(".env")),
            "存量树的两个大小写变体都不得在 restore 后留在影子 index（幸存者回扫腿）: {tracked}"
        );
    }

    /// P0 修复：回退后作废旧分支 Turn 快照——只移除 turn > keep_turns 的 Turn
    /// 条目并删其 ref；PreRestore 保留、原顺序保留、幂等。
    #[test]
    fn invalidate_turn_checkpoints_after_removes_only_abandoned_branch() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("inval-ledger");
        let exec = TestDir::new("inval-exec");
        exec.write("a.txt", "0\n");
        let t1 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        exec.write("a.txt", "1\n");
        let t2 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "t2",
        )
        .unwrap();
        exec.write("a.txt", "2\n");
        let t3 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(3),
            CheckpointKind::Turn,
            "t3",
        )
        .unwrap();
        let pre = create_checkpoint(
            ledger.path(),
            exec.path(),
            None,
            CheckpointKind::PreRestore,
            "回滚点",
        )
        .unwrap();

        // 回退到第 1 轮：turn 2/3 的 Turn 快照作废，turn 1 与 PreRestore 保留。
        let removed = invalidate_turn_checkpoints_after(ledger.path(), 1).unwrap();
        assert_eq!(removed, 2);
        let listed = list_checkpoints(ledger.path()).unwrap();
        let ids: Vec<&str> = listed.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![t1.id.as_str(), pre.id.as_str()],
            "保序且 PreRestore 不动"
        );

        // 被作废条目的 ref 已删，保留条目的 ref 仍在。
        let repo = repo_dir(ledger.path());
        let ref_exists = |id: &str| {
            git(
                &repo,
                exec.path(),
                &["show-ref", "--verify", &format!("refs/checkpoints/{id}")],
            )
            .map(|output| output.status.success())
            .unwrap_or(false)
        };
        assert!(!ref_exists(&t2.id));
        assert!(!ref_exists(&t3.id));
        assert!(ref_exists(&t1.id));
        assert!(ref_exists(&pre.id));

        // 幂等：再调一次返回 0，索引不变。
        assert_eq!(
            invalidate_turn_checkpoints_after(ledger.path(), 1).unwrap(),
            0
        );
        assert_eq!(list_checkpoints(ledger.path()).unwrap().len(), 2);

        // keep_turns=0：Turn 全部作废，PreRestore 仍保留。
        let removed = invalidate_turn_checkpoints_after(ledger.path(), 0).unwrap();
        assert_eq!(removed, 1);
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, CheckpointKind::PreRestore);
    }

    /// 陈旧快照和解（评审 finding）：只作废「turn > keep_turns 且创建于截断
    /// 时刻之前」的 Turn 条目；回退后新分支的同号快照（创建于截断后）保留；
    /// 幂等。
    #[test]
    fn invalidate_stale_turn_checkpoints_respects_creation_cutoff() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("stale-ledger");
        let exec = TestDir::new("stale-exec");
        exec.write("a.txt", "0\n");
        let t1 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        exec.write("a.txt", "1\n");
        let old_t2 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "old t2",
        )
        .unwrap();
        // 截断时刻。created_at 是秒级：睡过一秒边界，保证与前后快照可区分。
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let cutoff = now_seconds();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // 回退后的新分支同号快照（创建于截断之后）。
        exec.write("a.txt", "1-new\n");
        let new_t2 = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(2),
            CheckpointKind::Turn,
            "new t2",
        )
        .unwrap();

        let removed = invalidate_stale_turn_checkpoints(ledger.path(), 1, cutoff).unwrap();
        assert_eq!(removed, 1);
        let listed = list_checkpoints(ledger.path()).unwrap();
        let ids: Vec<&str> = listed.iter().map(|entry| entry.id.as_str()).collect();
        assert!(ids.contains(&t1.id.as_str()), "turn <= keep 的不动");
        assert!(
            !ids.contains(&old_t2.id.as_str()),
            "被弃分支的旧快照必须作废"
        );
        assert!(ids.contains(&new_t2.id.as_str()), "新分支同号快照必须保留");
        // 幂等：再次和解无条目可删。
        assert_eq!(
            invalidate_stale_turn_checkpoints(ledger.path(), 1, cutoff).unwrap(),
            0
        );
    }

    /// GIT_* environment isolation (review nit + issue #492): host GIT_DIR /
    /// GIT_WORK_TREE / GIT_INDEX_FILE / GIT_OBJECT_DIRECTORY must not redirect
    /// the shadow repo's internal operations to unrelated locations. Asserts
    /// the strip contract of `isolated_git_command` directly via
    /// `Command::get_envs` instead of mutating the process-global environment:
    /// process-level `set_var` used to race with concurrent git-subprocess
    /// tests, causing intermittent full-suite failures (issue #492).
    #[test]
    fn git_subprocess_ignores_host_git_environment() {
        if !git_available() {
            return;
        }
        let ledger = TestDir::new("env-ledger");
        let exec = TestDir::new("env-exec");
        exec.write("a.txt", "0\n");
        create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap();
        let repo = repo_dir(ledger.path());

        // Assert the strip contract directly via `Command::get_envs` without
        // mutating the process-global environment: process-level `set_var`
        // races with concurrent git-subprocess tests (the intermittent
        // full-suite failures of issue #492). Full coverage of the strip key
        // lists is anchored by the platform::process tests.
        let command = isolated_git_command();
        let env: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> = command.get_envs().collect();
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
            // The shadow repo's commit identity is provided explicitly via -c
            // by the caller; host identity variables must not leak either.
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            assert!(
                matches!(
                    env.iter()
                        .find(|(name, _)| name == &std::ffi::OsStr::new(key)),
                    Some((_, None))
                ),
                "{key} must be stripped via env_remove"
            );
        }
        assert_eq!(
            env.iter()
                .find(|(name, _)| name == &std::ffi::OsStr::new("GIT_CONFIG_GLOBAL"))
                .and_then(|(_, value)| *value),
            Some(std::ffi::OsStr::new(crate::platform::os::null_device())),
            "GIT_CONFIG_GLOBAL must be redirected to the null device"
        );
        assert_eq!(
            env.iter()
                .find(|(name, _)| name == &std::ffi::OsStr::new("GIT_CONFIG_NOSYSTEM"))
                .and_then(|(_, value)| *value),
            Some(std::ffi::OsStr::new("1")),
            "GIT_CONFIG_NOSYSTEM must be pinned"
        );
        // `get_envs()` only contains explicitly set/removed entries (inherited
        // variables never appear), so this actually constrains that no non-
        // GIT_* entry is explicitly added or removed.
        assert!(
            env.iter()
                .all(|(name, _)| name.as_encoded_bytes().starts_with(b"GIT_")),
            "no non-GIT_* entry may be explicitly added or removed"
        );

        // Smoke check: with the host environment in place, shadow-repo git
        // operations still succeed and a.txt stays tracked. Regression
        // protection is carried by the get_envs contract assertions above;
        // this test no longer mutates the process environment, so the old
        // set_var-era assertion pointing at a bogus directory is dropped.
        let output = isolated_git_command()
            .arg(format!("--git-dir={}", repo.display()))
            .arg(format!("--work-tree={}", exec.path().display()))
            .arg("ls-files")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tracked = String::from_utf8_lossy(&output.stdout);
        assert!(tracked.lines().any(|line| line == "a.txt"));
    }
}
