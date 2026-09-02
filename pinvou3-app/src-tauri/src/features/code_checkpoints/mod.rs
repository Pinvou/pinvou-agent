//! 代码会话 checkpoint：每轮用户消息（turn）开始时对执行根做快照，支持回滚。
//!
//! 移植自 fork 分支 `qiuYliangM/feat-full-code-mode` 提交 `32b5fdf9e` 的
//! `code_sessions/checkpoints.rs`（设计文档 `docs/code-mode-改动随对话回退-设计.md`
//! §3），砍掉 ACP 钩子、仅保留品悟原生 code 车道。与 feat 分支的差异：
//! - 模块落位改为 `features/code_checkpoints`（main 无 `code_sessions` 拆分）；
//! - turn 计数口径修正：feat 分支按 `role == "user"` 计数会把 tool_result 计入，
//!   改用 [`turns`] 中与 fork `8cc61b609` `is_user_turn_prompt` 同口径的谓词。
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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) mod turns;

pub(crate) use turns::count_user_turns;

/// 每会话保留的 checkpoint 上限（LRU，超出裁掉最老条目）。
const MAX_CHECKPOINTS: usize = 20;
/// diff 预览的 patch 文本上限（超出截断，changes 清单不受影响）。
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
    /// 展示标签（用户消息摘要或「回滚前自动快照」）。
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
    /// unified diff 文本（可能截断）。
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
/// 宁可跳过快照也不低估）。临时会话两根相同，影子仓库自身会被反复快照增长，
/// 必须排除，否则它会把自己顶过 2GB 门（评审 nit）。
fn estimate_workspace_bytes(execution_root: &Path, ledger_root: &Path) -> Option<u64> {
    let skip_dirs: std::collections::HashSet<&str> = SHADOW_EXCLUDES
        .iter()
        .map(|line| line.trim_end_matches('/'))
        .collect();
    let ledger_canonical =
        fs::canonicalize(ledger_root).unwrap_or_else(|_| ledger_root.to_path_buf());
    let mut total: u64 = 0;
    let mut entries: usize = 0;
    let mut stack = vec![execution_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).ok()?;
        for entry in read {
            let entry = entry.ok()?;
            entries += 1;
            if entries > SIZE_WALK_MAX_ENTRIES {
                return None;
            }
            // 不跟随符号链接（file_type 自身不触发跟随）。
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let path = entry.path();
                let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if canonical == ledger_canonical {
                    continue;
                }
                if !skip_dirs.contains(entry.file_name().to_string_lossy().as_ref()) {
                    stack.push(path);
                }
            } else if let Ok(metadata) = entry.metadata() {
                total += metadata.len();
                if total > MAX_WORKSPACE_BYTES_FOR_SNAPSHOT {
                    return Some(total);
                }
            }
        }
    }
    Some(total)
}

/// 执行根是否在快照体积预算内（chat.rs 在 turn 快照前调用；超限的会话如实
/// 没有回退入口，不算错误）。
pub fn execution_root_within_snapshot_budget(execution_root: &Path, ledger_root: &Path) -> bool {
    estimate_workspace_bytes(execution_root, ledger_root)
        .is_some_and(|bytes| bytes <= MAX_WORKSPACE_BYTES_FOR_SNAPSHOT)
}

/// 探测执行根文件系统是否大小写不敏感：写入混合大小写探针文件，再以全大写
/// 形式 stat——能读到同一文件即不敏感。探测失败（目录不可写等）按「敏感」
/// 处理：探测结果只喂给 core.ignorecase（决定 Makefile/makefile alias 等
/// git 原生语义）；秘密的三层防护（exclude/purge/过滤）恒为大小写不敏感，
/// 不依赖探测结果（见 ensure_repo 的 info/exclude 注释）。
fn fs_is_case_insensitive(dir: &Path) -> bool {
    let probe = dir.join(format!(".Pinvou-Icase-Probe-{}", std::process::id()));
    let insensitive = match fs::write(&probe, b"") {
        Ok(()) => {
            let other = dir.join(format!(".PINVOU-ICASE-PROBE-{}", std::process::id()));
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

fn git(repo: &Path, work_tree: &Path, arguments: &[&str]) -> Result<std::process::Output> {
    let output = crate::platform::process::HiddenCommand::new("git")
        .arg(format!("--git-dir={}", repo.display()))
        .arg(format!("--work-tree={}", work_tree.display()))
        .args(arguments)
        .output()
        .with_context(|| format!("执行 git {} 失败（Git 不可用？）", arguments.join(" ")))?;
    Ok(output)
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

/// 迁移/恢复共用的敏感文件 pathspec 全集：字面模式（无通配符，如 .env、
/// id_rsa）在 git pathspec 里只命中仓库根部，自动派生 `**/` 前缀版本覆盖任意
/// 深度；含通配符的模式（*.pem 等）本身跨 `/` 匹配，无需派生。gitignore 语义
/// 的 exclude 本就任意深度生效，此差异只影响 `rm --cached` 这类 pathspec 调用。
/// 恒带 `:(icase)`：与 info/exclude 的字符类模式同向（大小写不敏感），三层
/// 口径一致才不会出现「exclude 放行、purge 错杀」的 B1 链路（评审 M1）。
fn secret_pathspecs() -> Vec<String> {
    let mut patterns: Vec<String> = SECRET_EXCLUDES
        .iter()
        .map(|line| format!(":(icase){line}"))
        .collect();
    for line in SECRET_EXCLUDES {
        if !line.contains(['*', '?', '[']) {
            patterns.push(format!(":(icase)**/{line}"));
        }
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
    // 排除列表 = 忽略目录（原样）+ 敏感模式（恒大小写不敏感的字符类形式）。
    // 三层防护必须同向（评审 B1）：exclude（本文件）让 .ENV 类变体永不进
    // 快照、且因「被 ignored」不受 clean -fd 波及；purge pathspec（恒 icase）
    // 清掉存量 index 条目；预览过滤（恒 icase）挡住 legacy 快照里的原文。
    // 若 exclude 放行而 purge icase，restore 会把移出 index 的真实文件交给
    // clean 物理删除——这就是 B1 事故链，字符类模式让它在任何文件系统上闭合。
    let mut excludes: Vec<String> = SHADOW_EXCLUDES
        .iter()
        .map(|line| line.to_string())
        .chain(
            SECRET_EXCLUDES
                .iter()
                .map(|line| icase_gitignore_pattern(line)),
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
    let info_dir = repo.join("info");
    fs::create_dir_all(&info_dir)
        .with_context(|| format!("创建 checkpoint 仓库 info 目录失败: {}", info_dir.display()))?;
    fs::write(info_dir.join("exclude"), excludes.join("\n") + "\n")
        .with_context(|| "写入 checkpoint 仓库 exclude 失败".to_string())?;
    // 一次性迁移（存量仓库）：exclude 只挡 untracked 文件，升级前已 add -A 进
    // 影子 index 的敏感文件会继续被跟踪、被后续快照、被 checkout-index 恢复。
    // 按新模式清一次 index（幂等，marker 门控；只有 git 真实成功才写 marker，
    // 非零退出/spawn 失败都留到下次重试，不阻断快照主流程）。历史 commit
    // objects 里的原文不在清理范围，随 LRU 淘汰与 gc 回收。
    let migration_marker = info_dir.join("secret-excludes-v1");
    if !fresh && !migration_marker.exists() {
        match purge_secret_patterns_from_index(&repo, execution_root) {
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
                version: 1,
                entries: Vec::new(),
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 checkpoint 索引失败: {}", path.display()));
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(index) => Ok(index),
        Err(parse_error) => {
            // 损坏的 index 不得让该会话的 checkpoint 功能永久失效（临时会话的
            // 账本就在 agent 可见的工作目录内，agent 的工具可能写坏它）：隔离
            // 保留现场（带时间戳，二次损坏不覆盖首次取证）后从空索引重建——与
            // sidecar `_rewound_turns.json` 的损坏处理同款。代价：影子仓库里的
            // 历史快照失去索引（不可列不可用，对象随 gc 回收），此后快照能力恢复。
            let quarantine = path.with_extension(format!(
                "json.corrupt-{}",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            ));
            eprintln!(
                "[checkpoints] checkpoint 索引损坏，隔离为 {} 后从空索引重建: {parse_error:#}",
                quarantine.display()
            );
            if let Err(error) = fs::rename(&path, &quarantine) {
                eprintln!("[checkpoints] 隔离损坏索引失败: {error:#}");
            }
            Ok(CheckpointIndex {
                version: 1,
                entries: Vec::new(),
            })
        }
    }
}

fn save_index(ledger_root: &Path, index: &CheckpointIndex) -> Result<()> {
    let path = index_path(ledger_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 checkpoint 目录失败: {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(index).context("序列化 checkpoint 索引失败")?;
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("创建 checkpoint 索引失败: {}", temporary.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("写入 checkpoint 索引失败: {}", temporary.display()))?;
        file.sync_all().ok();
    }
    fs::rename(&temporary, &path)
        .with_context(|| format!("保存 checkpoint 索引失败: {}", path.display()))?;
    Ok(())
}

/// 快照执行根当前状态并登记为新的 checkpoint。
///
/// `turn` 为该快照对应的用户 turn 序号（1-based，UI 按它把入口对齐到 turn 边界）；
/// 内容与上一条 checkpoint 相同（本轮之前无任何变更）时复用上一条 commit，不产生
/// 冗余对象。完成后按 LRU 裁剪到 [`MAX_CHECKPOINTS`]。
pub fn create_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    turn: Option<u32>,
    kind: CheckpointKind,
    label: &str,
) -> Result<CheckpointMeta> {
    create_checkpoint_preserving(ledger_root, execution_root, turn, kind, label, &[])
}

/// `create_checkpoint` 的保留变体：LRU/存储压力淘汰跳过 `preserve` 中的条目。
/// `restore_checkpoint` 打 PreRestore 时以此保住恢复目标——否则目标恰落在被
/// 淘汰的一半里时 ref 被删、commit 被 prune，随后 read-tree 失败，用户请求
/// 的历史被销毁（评审 M2）。
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
    // add -A 全量暂存（影子 index），write-tree 取内容树；与上一条 commit 的树相同
    // 则复用（空 turn 不产生冗余快照内容，但 meta 仍按新 turn 登记，保持对齐）。
    git_ok(&repo, &execution_root, &["add", "-A"])?;
    let tree = git_ok(&repo, &execution_root, &["write-tree"])?
        .trim()
        .to_string();
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
        label: label.chars().take(60).collect(),
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
    if shadow_repo_bytes(&repo) > MAX_SHADOW_REPO_BYTES && index.entries.len() > 1 {
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

fn find_checkpoint(ledger_root: &Path, checkpoint_id: &str) -> Result<CheckpointMeta> {
    validate_checkpoint_id(checkpoint_id)?;
    load_index(ledger_root)?
        .entries
        .into_iter()
        .find(|entry| entry.id == checkpoint_id)
        .with_context(|| format!("checkpoint '{checkpoint_id}' 不存在"))
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
    let basename = path.rsplit('/').next().unwrap_or(path);
    let basename = basename.to_ascii_lowercase();
    SECRET_EXCLUDES.iter().any(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        if pattern.contains('*') {
            wildcard_match(&pattern, &basename)
        } else {
            basename == pattern
        }
    })
}

/// diff --git 段头是否指向敏感文件。常规按空白切 token、剥 a//b/ 前缀与引号；
/// 含空格/tab 的路径（git C-quoting 输出 `diff --git "a/x y" "b/x y"`）按引号
/// 段解析；仍解析不了时由 ---/+++ 行兜底（见 filter_secret_paths_from_patch）。
fn diff_section_is_secret(header: &str) -> bool {
    if let Some(rest) = header.strip_prefix("diff --git \"") {
        // C-quoted 形式：按引号段提取 a/ 与 b/ 路径（路径可含空格）。
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

/// `--- a/<path>` / `+++ b/<path>` 行的路径判定（整行剩余部分即路径，容忍空格；
/// 删除文件的 marker 行尾部带 tab 填充，先剥掉；路径含 tab/引号时 git 用
/// C-quoting 输出 `--- "a/x"`，剥掉外层引号再判定；/dev/null 如实不命中）。
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

/// 从 unified diff 文本剔除命中敏感文件模式的整段文件 diff：迁移前打的旧
/// 快照 tree 里可能仍有秘密原文（purge 只清 index），预览不得把原文带进 UI。
/// 按段缓冲后判定（header 或 ---/+++ 行任一命中即整段剔除）——含空格路径
/// 无法从段头 token 解析，必须看到 ---/+++ 行才能判定（评审 M1）。
fn filter_secret_paths_from_patch(patch: &str) -> String {
    let mut out = String::with_capacity(patch.len());
    let mut section: Vec<&str> = Vec::new();
    let mut section_secret = false;
    let flush = |out: &mut String, section: &mut Vec<&str>, secret: &mut bool| {
        if !*secret {
            for line in section.drain(..) {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            section.clear();
        }
        *secret = false;
    };
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut out, &mut section, &mut section_secret);
            section_secret = diff_section_is_secret(line);
        } else if marker_line_is_secret(line) {
            section_secret = true;
        }
        section.push(line);
    }
    flush(&mut out, &mut section, &mut section_secret);
    out
}

/// 快照与当前执行根的差异预览（即「回滚将撤销的变更」），供 UI 确认前展示。
pub fn diff_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    checkpoint_id: &str,
) -> Result<CheckpointDiff> {
    let meta = find_checkpoint(ledger_root, checkpoint_id)?;
    let execution_root = canonical_execution_root(execution_root)?;
    let repo = ensure_repo(ledger_root, &execution_root)?;
    // add -A 让影子 index 反映当前执行根，diff --cached <commit> 即「快照→现在」。
    git_ok(&repo, &execution_root, &["add", "-A"])?;
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
    let raw_patch = git_ok(
        &repo,
        &execution_root,
        // 与 --raw 同开 -M + quotepath：changes 清单标 renamed 时 patch
        // 也是 rename 形态，中文路径在两处都是原文。
        &[
            "-c",
            "core.quotepath=false",
            "diff",
            "--cached",
            "-M",
            "--no-color",
            "--no-ext-diff",
            &meta.commit,
        ],
    )?;
    // legacy 快照（迁移前打的）tree 里可能仍含秘密原文：清单与 patch 都剔除
    // 命中敏感模式的条目，预览既不带原文上屏，也不谎称「回滚将删除 .env」
    // （restore 实际保留工作区现有同名文件）。过滤恒大小写不敏感，与
    // exclude/purge 三层同向。
    let changes: Vec<CheckpointChange> = parse_raw_status(&raw_status)
        .into_iter()
        .filter(|change| !secret_path_matches(&change.path))
        .collect();
    let mut patch = filter_secret_paths_from_patch(&raw_patch);
    let patch_truncated = patch.len() > DIFF_PATCH_LIMIT;
    if patch_truncated {
        let mut end = DIFF_PATCH_LIMIT;
        while !patch.is_char_boundary(end) {
            end -= 1;
        }
        patch.truncate(end);
        patch.push_str("\n\n…差异过大，已截断");
    }
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
pub fn restore_checkpoint(
    ledger_root: &Path,
    execution_root: &Path,
    checkpoint_id: &str,
) -> Result<CheckpointMeta> {
    let meta = find_checkpoint(ledger_root, checkpoint_id)?;
    let execution_root = canonical_execution_root(execution_root)?;
    // 回滚点快照失败必须中止：没有可反悔的兜底就不能动用户文件。
    // preserve 传入恢复目标：PreRestore 触发的 LRU/存储压力淘汰不得把目标
    // 本身裁掉（ref 删除 + commit prune 会让随后的 read-tree 失败，用户请求
    // 的历史被销毁，评审 M2）。
    let undo = create_checkpoint_preserving(
        ledger_root,
        &execution_root,
        None,
        CheckpointKind::PreRestore,
        &format!("回滚到 {} 前的自动快照", meta.id),
        &[&meta.id],
    )
    .context("回滚前自动快照失败，已中止回滚")?;
    let repo = repo_dir(ledger_root);
    git_ok(&repo, &execution_root, &["read-tree", &meta.commit])
        .context("读取 checkpoint 快照失败")?;
    // read-tree 整体替换 index：迁移前打的旧快照 tree 里可能仍含敏感文件，
    // 不清理就会被 checkout-index 写回工作区并被后续 add -A 重新跟踪（一次性
    // 迁移的效果被一次回滚复活）。恢复前按同一模式清影子 index（只动 index）。
    purge_secret_patterns_from_index(&repo, &execution_root)
        .context("清理快照中的敏感文件条目失败")?;
    git_ok(&repo, &execution_root, &["checkout-index", "-f", "-a"])
        .context("写回 checkpoint 文件失败")?;
    git_ok(&repo, &execution_root, &["clean", "-fd"]).context("清理快照后新建文件失败")?;
    Ok(undo)
}

/// 只操作 refs 的 git 调用（update-ref/gc 不需要 work-tree）。
fn git_ref(repo: &Path, arguments: &[&str]) -> Result<std::process::Output> {
    let output = crate::platform::process::HiddenCommand::new("git")
        .arg(format!("--git-dir={}", repo.display()))
        .args(arguments)
        .output()
        .with_context(|| format!("执行 git {} 失败（Git 不可用？）", arguments.join(" ")))?;
    Ok(output)
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
pub fn invalidate_turn_checkpoints_after(ledger_root: &Path, keep_turns: u32) -> Result<usize> {
    let mut index = load_index(ledger_root)?;
    // partition 稳定保序：kept 即「原顺序去掉被作废条目」。
    let (kept, invalidated): (Vec<CheckpointMeta>, Vec<CheckpointMeta>) =
        index.entries.into_iter().partition(|entry| {
            !(entry.kind == CheckpointKind::Turn
                && entry.turn.is_some_and(|turn| turn > keep_turns))
        });
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
    let mut index = load_index(ledger_root)?;
    let before = index.entries.len();
    index.entries.retain(|entry| entry.id != checkpoint_id);
    if index.entries.len() == before {
        return Ok(false);
    }
    let repo = repo_dir(ledger_root);
    if repo.join("HEAD").is_file() {
        let _ = git_ref(
            &repo,
            &[
                "update-ref",
                "-d",
                &format!("refs/checkpoints/{checkpoint_id}"),
            ],
        );
        let _ = git_ref(&repo, &["gc", "--auto", "--quiet"]);
    }
    save_index(ledger_root, &index)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            fs::write(path, content).unwrap();
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

    fn git_available() -> bool {
        crate::platform::process::HiddenCommand::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
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

        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
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
        assert!(diff.patch.contains("文档/需求.md"));
        assert!(!diff.patch.contains("\\346"), "不得出现八进制转义");
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
            label: "legacy".into(),
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
                version: 1,
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
                version: 1,
                entries: vec![CheckpointMeta {
                    id: "c1-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: "legacy".into(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();

        exec.write("ok.txt", "v2\n");
        exec.write("my dir/.env", "SECRET=current\n");
        let diff = diff_checkpoint(ledger.path(), exec.path(), "c1-1").unwrap();
        assert!(
            diff.changes
                .iter()
                .all(|change| !change.path.ends_with(".env")),
            "含空格的秘密路径不得出现在清单: {:?}",
            diff.changes
        );
        assert!(
            !diff.patch.contains("SECRET=old"),
            "patch 不得带秘密原文:\n{}",
            diff.patch
        );
        assert!(
            !diff.patch.contains("my dir/.env"),
            "含空格秘密段必须整段剔除:\n{}",
            diff.patch
        );
        assert!(diff.patch.contains("ok.txt"));
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

        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
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

    /// legacy 快照（迁移前打的，tree 含 .env）的 diff 预览：清单与 patch 都
    /// 不得带敏感条目/秘密原文上屏；普通文件变更照常展示。
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
                version: 1,
                entries: vec![CheckpointMeta {
                    id: "c1-1".into(),
                    turn: Some(1),
                    kind: CheckpointKind::Turn,
                    label: "legacy".into(),
                    commit,
                    created_at: 0,
                }],
            },
        )
        .unwrap();

        // 当前状态：ok.txt 改了，.env 也改了（purge 后不在 index，快照→当前
        // 的原始 diff 会显示 D .env 且 patch 带 SECRET=old 原文）。
        exec.write("ok.txt", "v2\n");
        exec.write(".env", "SECRET=current\n");
        let diff = diff_checkpoint(ledger.path(), exec.path(), "c1-1").unwrap();
        assert!(
            diff.changes.iter().all(|change| change.path != ".env"),
            "敏感条目不得出现在变更清单: {:?}",
            diff.changes
        );
        assert!(diff.changes.iter().any(|change| change.path == "ok.txt"));
        assert!(!diff.patch.contains("SECRET=old"), "patch 不得带秘密原文");
        assert!(!diff.patch.contains("SECRET=current"));
        assert!(diff.patch.contains("ok.txt"));
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
        let diff = diff_checkpoint(ledger.path(), exec.path(), &first.id).unwrap();
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
        assert!(diff.patch.contains("src/new.rs"));

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
        let first = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t",
        )
        .unwrap();
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
}
