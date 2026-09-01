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

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub(crate) mod turns;

pub(crate) use turns::count_user_turns;

/// 每会话保留的 checkpoint 上限（LRU，超出裁掉最老条目）。
const MAX_CHECKPOINTS: usize = 20;
/// diff 预览的 patch 文本上限（超出截断，changes 清单不受影响）。
const DIFF_PATCH_LIMIT: usize = 512 * 1024;

/// 影子仓库的 exclude 列表：与 workspace 浏览的忽略目录对齐，避免把依赖/构建
/// 产物纳入快照（git 项目自身的 .gitignore 会被影子仓库自然尊重，无需重复）。
/// 另排除常见敏感文件：非 git 执行根（临时会话、未初始化目录）没有 .gitignore
/// 兜底，`add -A` 会把 .env/私钥原文快照进影子 objects 并随 diff 进入 UI 链路；
/// 代价是这些文件不随回退恢复，属可接受取舍（设计文档已知限制有记录）。
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

/// 敏感文件模式：秘密实际居住的约定位置（.env.local 系、证书/私钥本体）。
/// 原文不进快照、不进 diff 预览——非 git 执行根（临时会话、未初始化目录）没有
/// .gitignore 兜底，`add -A` 会把它们快照进影子 objects。收窄的取舍：
/// .env.example/.env.sample/id_rsa.pub 等常被有意提交的示例/公钥照常进快照、
/// 随回退恢复；.env.production 等约定文件仍会进快照（其内容通常可提交）。
/// 代价是这些文件不随回退恢复，属可接受取舍（设计文档已知限制有记录）。
const SECRET_EXCLUDES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.*.local",
    "*.pem",
    "*.key",
    "*.p12",
    "*.keystore",
    "id_rsa",
    "id_ed25519",
];

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

/// 初始化影子仓库（幂等）：git-dir 在账本根，work-tree 指向执行根。
/// - `core.autocrlf false`：不受用户全局行尾配置影响，快照/恢复保持原字节；
///   执行根自己的 .gitattributes 仍会被尊重（与用户在项目内看到的行尾一致）。
/// - `info/exclude`：忽略依赖/构建目录 + checkpoint 目录自身（临时会话两根相同，
///   checkpoint 数据在执行根内，必须排除，否则快照自我递归、clean 会误删账本）。
fn ensure_repo(ledger_root: &Path, execution_root: &Path) -> Result<PathBuf> {
    let repo = repo_dir(ledger_root);
    let fresh = !repo.join("HEAD").is_file();
    if fresh {
        fs::create_dir_all(&repo)
            .with_context(|| format!("创建 checkpoint 仓库目录失败: {}", repo.display()))?;
        git_ok(&repo, execution_root, &["init"])?;
        git_ok(&repo, execution_root, &["config", "core.autocrlf", "false"])?;
    }
    let mut excludes: Vec<String> = SHADOW_EXCLUDES
        .iter()
        .chain(SECRET_EXCLUDES.iter())
        .map(|line| line.to_string())
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
    // 按新模式清一次 index（幂等，marker 门控只跑一次；best-effort——失败留到
    // 下次重试，不阻断快照主流程）。历史 commit objects 里的原文不在清理范围，
    // 随 LRU 淘汰与 gc 回收。
    let migration_marker = info_dir.join("secret-excludes-v1");
    if !fresh && !migration_marker.exists() {
        let mut arguments = vec!["rm", "-r", "--cached", "--ignore-unmatch", "--quiet", "--"];
        arguments.extend(SECRET_EXCLUDES.iter().copied());
        if let Err(error) = git(&repo, execution_root, &arguments) {
            eprintln!("[checkpoints] secret-exclude migration failed (will retry next call): {error:#}");
        } else {
            fs::write(&migration_marker, "1\n").ok();
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
            })
        }
        Err(error) => {
            return Err(error).with_context(|| format!("读取 checkpoint 索引失败: {}", path.display()))
        }
    };
    let index: CheckpointIndex =
        serde_json::from_slice(&bytes).context("解析 checkpoint 索引失败")?;
    Ok(index)
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
    if index.entries.len() > MAX_CHECKPOINTS {
        let overflow = index.entries.len() - MAX_CHECKPOINTS;
        let evicted: Vec<CheckpointMeta> = index.entries.drain(..overflow).collect();
        // 被裁掉的条目删 ref，commit 变为不可达，交给 git 后台回收；
        // 回收失败不影响裁剪语义（索引已裁，对象留到下次 gc）。
        for entry in &evicted {
            let _ = git(
                &repo,
                &execution_root,
                &["update-ref", "-d", &format!("refs/checkpoints/{}", entry.id)],
            );
        }
        let _ = git(&repo, &execution_root, &["gc", "--auto", "--quiet"]);
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
    git_ok(repo, work_tree, &["update-ref", "refs/checkpoints/head", &commit])?;
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

fn parse_name_status(text: &str) -> Vec<CheckpointChange> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?;
            let label = match status.chars().next() {
                Some('A') => "added",
                Some('M') => "modified",
                Some('D') => "deleted",
                Some('R') => "renamed",
                Some('C') => "copied",
                _ => "other",
            };
            // R/C 状态是「旧路径\t新路径」，展示新路径。
            let path = parts.last()?.to_string();
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
    let name_status = git_ok(
        &repo,
        &execution_root,
        // -M 开启 rename 检测，parse_name_status 的 renamed 分支才不是死代码。
        &["diff", "--cached", "--name-status", "-M", "--no-color", &meta.commit],
    )?;
    let mut patch = git_ok(
        &repo,
        &execution_root,
        // 与 name-status 同开 -M：changes 清单标 renamed 时 patch 也是 rename
        // 形态，不出现清单/补丁口径不一致。
        &[
            "diff",
            "--cached",
            "-M",
            "--no-color",
            "--no-ext-diff",
            &meta.commit,
        ],
    )?;
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
        changes: parse_name_status(&name_status),
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
    let undo = create_checkpoint(
        ledger_root,
        &execution_root,
        None,
        CheckpointKind::PreRestore,
        &format!("回滚到 {} 前的自动快照", meta.id),
    )
    .context("回滚前自动快照失败，已中止回滚")?;
    let repo = repo_dir(ledger_root);
    git_ok(&repo, &execution_root, &["read-tree", &meta.commit])
        .context("读取 checkpoint 快照失败")?;
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
                &["update-ref", "-d", &format!("refs/checkpoints/{}", entry.id)],
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
            &["update-ref", "-d", &format!("refs/checkpoints/{checkpoint_id}")],
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
        let kept = create_checkpoint(ledger.path(), exec.path(), Some(1), CheckpointKind::Turn, "t1")
            .unwrap();
        let unsent = create_checkpoint(ledger.path(), exec.path(), None, CheckpointKind::Turn, "unsent")
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
            &["show-ref", "--verify", &format!("refs/checkpoints/{}", unsent.id)],
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
        let result = create_checkpoint(
            ledger.path(),
            &ghost,
            Some(1),
            CheckpointKind::Turn,
            "test",
        );
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
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "src/new.rs" && change.status == "added"));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "README.md" && change.status == "deleted"));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.path == "src/main.rs" && change.status == "modified"));
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
        assert_eq!(exec.read("src/new.rs").as_deref(), Some("pub fn added() {}\n"));
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
        let first = create_checkpoint(ledger.path(), exec.path(), Some(1), CheckpointKind::Turn, "t1")
            .unwrap();
        // 无变更的 turn：复用同一 commit，但 meta 仍登记（turn 对齐不漂移）。
        let second = create_checkpoint(ledger.path(), exec.path(), Some(2), CheckpointKind::Turn, "t2")
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
        assert_eq!(listed.last().unwrap().turn, Some((MAX_CHECKPOINTS + 3) as u32));
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
        let undo = restore_checkpoint(&ledger, root.path(), &list_checkpoints(&ledger).unwrap()[0].id.clone())
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
        let first = create_checkpoint(ledger.path(), exec.path(), Some(1), CheckpointKind::Turn, "t")
            .unwrap();
        // node_modules 内的变化不产生新 commit（被 exclude，不进入快照）。
        exec.write("node_modules/pkg/index.js", "dep2\n");
        let second = create_checkpoint(ledger.path(), exec.path(), Some(2), CheckpointKind::Turn, "t")
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
        let t1 = create_checkpoint(ledger.path(), exec.path(), Some(1), CheckpointKind::Turn, "t1")
            .unwrap();
        exec.write("a.txt", "1\n");
        let t2 = create_checkpoint(ledger.path(), exec.path(), Some(2), CheckpointKind::Turn, "t2")
            .unwrap();
        exec.write("a.txt", "2\n");
        let t3 = create_checkpoint(ledger.path(), exec.path(), Some(3), CheckpointKind::Turn, "t3")
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
        assert_eq!(ids, vec![t1.id.as_str(), pre.id.as_str()], "保序且 PreRestore 不动");

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
        assert_eq!(invalidate_turn_checkpoints_after(ledger.path(), 1).unwrap(), 0);
        assert_eq!(list_checkpoints(ledger.path()).unwrap().len(), 2);

        // keep_turns=0：Turn 全部作废，PreRestore 仍保留。
        let removed = invalidate_turn_checkpoints_after(ledger.path(), 0).unwrap();
        assert_eq!(removed, 1);
        let listed = list_checkpoints(ledger.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, CheckpointKind::PreRestore);
    }
}
