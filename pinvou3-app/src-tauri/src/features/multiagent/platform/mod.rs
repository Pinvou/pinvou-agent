//! 多智能体运行工作区的平台适配：把工作区初始化成 git 仓库。
//!
//! 底座派发并行子任务前会做 cwd 校验（`subagent/mod.rs` 的 git 根定位）：
//! 工作区自身不是仓库时它会**向上逐级找**，一路走到用户主目录、再扫描其
//! 直接子目录——扫出多个无关仓库就以 "Multiple git repositories found"
//! 拒绝 spawn。真机上一次运行的 4 个并行调研子任务因此全军覆没。
//! 工作区本身是仓库时，定位在第 0 层直接命中，永远不会走到那步。
//!
//! 用 `git init` 而非手写 `.git` 骨架，并创建一个空的初始提交：底座给可写
//! 子任务创建 git worktree 时必须有可检出的 HEAD。初始化与提交任一步失败都
//! 向调用方返回错误，不能留下一个看似可用、实际派不出子任务的运行。

use std::path::Path;
use std::process::{Command, Output};

const INITIAL_COMMIT_AUTHOR_NAME: &str = "Pinvou Agent";
const INITIAL_COMMIT_AUTHOR_EMAIL: &str = "pinvou-agent@localhost";

fn git_output(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", INITIAL_COMMIT_AUTHOR_NAME)
        .env("GIT_AUTHOR_EMAIL", INITIAL_COMMIT_AUTHOR_EMAIL)
        .env("GIT_COMMITTER_NAME", INITIAL_COMMIT_AUTHOR_NAME)
        .env("GIT_COMMITTER_EMAIL", INITIAL_COMMIT_AUTHOR_EMAIL);
    suppress_console(&mut command);
    command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            // 缺 git 是普通用户可自助的情况（Windows 常见）：给人话与出路，
            // 不甩原始 NotFound。
            return "本机未安装 git：多智能体的并行隔离（git worktree）需要它，\
                    可前往 设置 → 依赖体检 一键安装后重试。"
                .to_string();
        }
        format!("无法执行 git {}: {err}", args.join(" "))
    })
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let output = git_output(dir, args)?;
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(format!(
        "git {} 失败（{}）{}",
        args.join(" "),
        output.status,
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

/// 把 `dir` 初始化成带首个提交的 git 仓库，并把**已有文件**收进首个提交。
///
/// 初始提交使用仅对该命令生效的 identity，不读取或写入用户的 `user.name` /
/// `user.email`。如果目录已经有可检出的 HEAD，则保留它，不额外制造提交。
/// 首提交必须含现有文件：worktree 隔离的子智能体只检出 HEAD，空提交会让
/// 工作区里已有的输入文件在 worktree 里不可见。
pub(crate) fn ensure_git_repository(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("工作区不存在或不是目录: {}", dir.display()));
    }

    // 快路径：已是带 HEAD 的仓库就只做一次校验（.git 判断零 spawn）。
    // Windows 进程启动贵，重复开关每次都跑 git init 是点击延迟的主要来源。
    if dir.join(".git").exists() {
        let head = git_output(dir, &["rev-parse", "--verify", "HEAD"])?;
        if head.status.success() {
            // 旧版建的仓库没有排除规则，这里补上（零 git spawn）。
            ensure_state_excluded(dir)?;
            return Ok(());
        }
    } else {
        run_git(dir, &["init", "-q"])?;
    }

    ensure_state_excluded(dir)?;
    run_git(dir, &["add", "-A"])?;
    commit_quietly(dir, "chore: 初始化多智能体工作区")?;

    run_git(dir, &["rev-parse", "--verify", "HEAD"])?;
    Ok(())
}

/// 把工作区当前内容快照进 git（幂等：无变化直接返回）。
///
/// 多智能体会话每轮发送前调用：底座给 `worktree=true` 的子智能体检出的是
/// HEAD，上一轮之后新增/修改的输入文件不快照就在 worktree 里不可见。目录
/// 还不是仓库时顺带初始化（老会话在本修复之前开的开关也能自愈）。
pub(crate) fn snapshot_workspace(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("工作区不存在或不是目录: {}", dir.display()));
    }
    ensure_git_repository(dir)?;
    let status = run_git(dir, &["status", "--porcelain"])?;
    if String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return Ok(());
    }
    run_git(dir, &["add", "-A"])?;
    commit_quietly(dir, "chore: 快照工作区（供 worktree 子智能体检出）")
}

/// 让快照永不收录底座运行时状态（`.codewhale/state/`：worker ledger 与
/// 子智能体完整对话）。不排除的话，每轮快照都把私有执行记录提交进本地
/// git 历史：仓库随 transcript 线性膨胀、worktree 子智能体检出 HEAD 就能
/// 看到前面子智能体的完整对话、删除表面文件也删不掉历史（复核 P1）。
/// 规则写在 `.git/info/exclude`：linked worktree 共享、不进提交、也不在
/// 工作区里多出一个用户可见文件；`.codewhale/agents/` 专家名册必须照常
/// 入库——worktree 子智能体靠检出 HEAD 拿到它。
///
/// 迁移标记（exclude 里的注释行，git 忽略注释）：规则行只证明"以后不再
/// 收录"，不证明"历史跟踪已清"——中间版本写过规则但没做停跟踪的仓库，
/// 光看规则行会漏迁移（复核点名）。写入顺序有讲究：规则可以先落（提前
/// 持久化无害），标记必须等 `git rm --cached` **成功后**才落——反过来会
/// 在清理失败时留下假成功标记、下次直接早退（复核点名）。v1 标记正是在
/// 清理前写的，换 v2 代号让潜在的假标记失效、重走一次幂等迁移。
const STATE_MIGRATED_MARKER: &str = "# pinvou: codewhale-state untracked v2";

fn append_exclude_line(exclude: &Path, content: &mut String, line: &str) -> Result<(), String> {
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(line);
    content.push('\n');
    std::fs::write(exclude, content.as_bytes())
        .map_err(|e| format!("写 .git/info/exclude 失败: {e}"))
}

fn ensure_state_excluded(dir: &Path) -> Result<(), String> {
    let exclude = dir.join(".git").join("info").join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let has_rule = existing
        .lines()
        .any(|line| line.trim() == "/.codewhale/state/");
    let migrated = existing
        .lines()
        .any(|line| line.trim() == STATE_MIGRATED_MARKER);
    if has_rule && migrated {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 .git/info 失败: {e}"))?;
    }
    let mut content = existing;
    if !has_rule {
        append_exclude_line(&exclude, &mut content, "/.codewhale/state/")?;
    }
    // 一次性停跟踪：曾被早期快照收录、当前"干净"的 state 不出现在
    // porcelain（tracked-but-clean），只能在迁移时点清理；暂存的删除由随后
    // 的初始提交/快照一并入账。既有历史是否清理是单独决策，不静默重写。
    run_git(
        dir,
        &[
            "rm",
            "-r",
            "-q",
            "--cached",
            "--ignore-unmatch",
            ".codewhale/state",
        ],
    )?;
    if !migrated {
        append_exclude_line(&exclude, &mut content, STATE_MIGRATED_MARKER)?;
    }
    Ok(())
}

/// 带临时 identity 的静默提交（`--allow-empty` 兜底空目录初始化）。
fn commit_quietly(dir: &Path, message: &str) -> Result<(), String> {
    run_git(
        dir,
        &[
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=.git/pinvou-no-hooks",
            "commit",
            "--allow-empty",
            "--no-verify",
            "-q",
            "-m",
            message,
        ],
    )
    .map(|_| ())
}

/// Windows 上不带这个标志会闪一个控制台窗口。
#[cfg(windows)]
fn suppress_console(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn suppress_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pinvou3-multiagent-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn command_output(command: &mut Command, context: &str) -> Output {
        suppress_console(command);
        let output = command.output().unwrap_or_else(|err| {
            panic!("{context}: 无法执行 git: {err}");
        });
        assert!(
            output.status.success(),
            "{context}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    #[test]
    fn initialized_workspace_has_head_and_supports_worktree_add() {
        let root = unique_temp_dir("git-worktree");
        let repo = root.join("repo");
        let linked = root.join("linked");
        std::fs::create_dir_all(&repo).expect("create repo dir");

        std::fs::write(repo.join("input.md"), "既有输入文件").expect("seed file");
        ensure_git_repository(&repo).expect("initialize repository");

        // 首提交必须带上已有文件：worktree 子智能体只看得到 HEAD。
        let tracked = run_git(&repo, &["ls-tree", "--name-only", "HEAD"]).expect("ls HEAD");
        assert!(
            String::from_utf8_lossy(&tracked.stdout).contains("input.md"),
            "初始提交必须包含工作区既有文件"
        );

        // 快照：新文件 → 新提交；无变化 → 幂等不加提交。
        std::fs::write(repo.join("later.md"), "后写入").expect("write later");
        snapshot_workspace(&repo).expect("snapshot");
        let tracked = run_git(&repo, &["ls-tree", "--name-only", "HEAD"]).expect("ls HEAD2");
        assert!(String::from_utf8_lossy(&tracked.stdout).contains("later.md"));
        let count_before = run_git(&repo, &["rev-list", "--count", "HEAD"]).expect("count");
        snapshot_workspace(&repo).expect("idempotent snapshot");
        let count_after = run_git(&repo, &["rev-list", "--count", "HEAD"]).expect("count2");
        assert_eq!(
            String::from_utf8_lossy(&count_before.stdout).trim(),
            String::from_utf8_lossy(&count_after.stdout).trim(),
            "无变化的快照不得制造空提交"
        );

        // 运行时状态不得进快照：worker ledger/transcript 是私有执行记录。
        let state_dir = repo.join(".codewhale").join("state");
        std::fs::create_dir_all(&state_dir).expect("create state dir");
        std::fs::write(state_dir.join("subagents.v1.json"), "{}").expect("write ledger");
        let agents_dir = repo.join(".codewhale").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("create agents dir");
        std::fs::write(agents_dir.join("exp-t.toml"), "name = \"t\"").expect("write roster");
        snapshot_workspace(&repo).expect("snapshot with state present");
        let tracked = run_git(&repo, &["ls-files"]).expect("ls-files");
        let tracked = String::from_utf8_lossy(&tracked.stdout).to_string();
        assert!(
            tracked.contains("exp-t.toml"),
            "专家名册必须入库供 worktree 检出"
        );
        assert!(
            !tracked.contains("subagents.v1.json"),
            "worker ledger/transcript 不得进入快照"
        );

        // 历史遗留自愈：模拟真正的旧版仓库——exclude 规则尚未落地、state
        // 已被跟踪且**内容干净**（tracked-but-clean 不出现在 porcelain，
        // 复核点名的漏网形态）。
        std::fs::write(repo.join(".git").join("info").join("exclude"), "").expect("wipe exclude");
        run_git(&repo, &["add", "-f", ".codewhale/state/subagents.v1.json"]).expect("force add");
        commit_quietly(&repo, "legacy: state was once tracked").expect("legacy commit");
        snapshot_workspace(&repo).expect("snapshot migrates clean tracked state");
        let after = run_git(&repo, &["ls-files"]).expect("ls-files after");
        assert!(
            !String::from_utf8_lossy(&after.stdout).contains("subagents.v1.json"),
            "干净但被跟踪的旧 state 也必须被迁出 index"
        );

        // 更隐蔽的旧形态（复核点名）：中间版本已写入规则行、但没做过停跟踪
        // ——规则已存在时也必须凭"迁移标记缺失"完成清理。
        run_git(&repo, &["add", "-f", ".codewhale/state/subagents.v1.json"]).expect("re-force add");
        commit_quietly(&repo, "legacy: rule present but still tracked").expect("legacy commit 2");
        std::fs::write(
            repo.join(".git").join("info").join("exclude"),
            "/.codewhale/state/\n",
        )
        .expect("rule-only exclude");
        snapshot_workspace(&repo).expect("snapshot migrates rule-present repo");
        let after2 = run_git(&repo, &["ls-files"]).expect("ls-files after2");
        assert!(
            !String::from_utf8_lossy(&after2.stdout).contains("subagents.v1.json"),
            "规则已存在但仍被跟踪的旧仓库也必须被迁出"
        );

        // v1 标记时代把标记写在清理之前，可能存在"标记在、清理没做成"的
        // 假成功仓库：v2 换代号让旧标记失效，重走幂等迁移（复核点名）。
        run_git(&repo, &["add", "-f", ".codewhale/state/subagents.v1.json"]).expect("v1-era add");
        commit_quietly(&repo, "legacy: v1 marker false success").expect("legacy commit 3");
        std::fs::write(
            repo.join(".git").join("info").join("exclude"),
            "/.codewhale/state/\n# pinvou: codewhale-state untracked v1\n",
        )
        .expect("v1 exclude");
        snapshot_workspace(&repo).expect("snapshot migrates v1-marked repo");
        let after3 = run_git(&repo, &["ls-files"]).expect("ls-files after3");
        assert!(
            !String::from_utf8_lossy(&after3.stdout).contains("subagents.v1.json"),
            "v1 假标记不得挡住重新迁移"
        );

        let head = run_git(&repo, &["rev-parse", "--verify", "HEAD"]).expect("verify HEAD");
        assert!(
            !String::from_utf8_lossy(&head.stdout).trim().is_empty(),
            "初始化后必须有可检出的 HEAD"
        );

        let author = run_git(&repo, &["show", "-s", "--format=%an <%ae>", "HEAD"])
            .expect("read initial author");
        assert_eq!(
            String::from_utf8_lossy(&author.stdout).trim(),
            format!("{INITIAL_COMMIT_AUTHOR_NAME} <{INITIAL_COMMIT_AUTHOR_EMAIL}>")
        );

        // identity 只属于初始提交命令，不能污染仓库配置，更不能依赖用户全局配置。
        let local_name =
            git_output(&repo, &["config", "--local", "--get", "user.name"]).expect("read config");
        assert!(
            !local_name.status.success(),
            "临时 identity 不应写进 .git/config"
        );

        let mut add = Command::new("git");
        add.arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach", "-q"])
            .arg(&linked)
            .arg("HEAD");
        command_output(&mut add, "git worktree add");
        assert!(
            linked.join(".git").is_file(),
            "linked worktree 应由 .git 文件指回主仓"
        );

        let mut linked_head = Command::new("git");
        linked_head
            .arg("-C")
            .arg(&linked)
            .args(["rev-parse", "--verify", "HEAD"]);
        command_output(&mut linked_head, "verify linked worktree HEAD");

        let mut remove = Command::new("git");
        remove
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "remove", "--force"])
            .arg(&linked);
        command_output(&mut remove, "git worktree remove");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
