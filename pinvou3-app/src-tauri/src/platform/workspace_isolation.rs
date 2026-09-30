//! Workspace isolation primitives for session fork
//! (`docs/fork-session-plan.md` §4.2, OS adapter row).
//!
//! Fork copies a session into a new session; a root the user chose to isolate
//! gets a private working copy on disk. The method is picked automatically
//! and is deliberately not a user choice (plan D5): a git work tree when the
//! root lives inside one (cheap, git-native comparisons) with uncommitted
//! state synced on top (a bare `worktree add` would silently drop it), and a
//! plain recursive directory copy otherwise, including the git-missing /
//! old-git / mid-operation degradation paths. Both methods stamp the copy
//! with the `.pinvou-fork-workspace.json` marker so a future storage manager
//! can recognize fork copies; worktrees also add the marker to their
//! per-worktree `info/exclude` so it never pollutes `git status`.
//!
//! Failure contract (plan §8.3): every public entry point leaves no partial
//! target behind — a mid-copy failure rolls the whole root back, and the
//! error names the concrete path that failed.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Marker file written inside every isolated copy (plan §3.4). Contents are
/// metadata only (creation time + source path, both already local user data).
pub(crate) const FORK_WORKSPACE_MARKER_FILE: &str = ".pinvou-fork-workspace.json";

/// Single git invocation budget. `worktree add` on a large repository can be
/// slow, but a wedged git (NFS stall, hung hook) must not block a fork
/// forever — same reasoning as `code_checkpoints::GIT_COMMAND_TIMEOUT`, with
/// a larger ceiling because worktree materialization also writes objects.
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(180);

/// How a root was isolated. Drives rollback (`remove_isolated_root`) and the
/// fork report; the choice itself stays invisible to the user (plan D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IsolationMethod {
    GitWorktree,
    DirectoryCopy,
}

/// Cumulative copy progress handed to the fork progress callback. Counters
/// only ever grow (test §6.1 #14 pins monotonicity).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CopyProgress {
    pub files_copied: u64,
    pub bytes_copied: u64,
}

/// A successfully isolated root, kept by the caller for whole-fork rollback
/// when a later root fails (plan §6.1 #10).
#[derive(Debug, Clone)]
pub(crate) struct IsolatedRoot {
    /// The new copy path (canonicalized form used for binding).
    pub path: PathBuf,
    pub method: IsolationMethod,
}

/// Git program reference. A string (not a `Path`) so tests can point at a
/// nonexistent program and pin the git-missing degradation (§6.1 #12).
#[derive(Debug, Clone)]
pub(crate) struct GitProgram<'a> {
    pub program: &'a str,
}

impl GitProgram<'_> {
    /// Whether the git binary can be executed at all. Drives the
    /// copy fallback; never fatal on its own.
    pub(crate) fn available(&self) -> bool {
        let mut command = user_git_command(self.program);
        command.arg("--version");
        crate::platform::process::output_with_timeout_and_kill_tree(command, GIT_COMMAND_TIMEOUT)
            .is_ok_and(|output| output.status.success())
    }
}

/// Build a git command against the USER's repository: only strip the
/// host-shell override variables (`GIT_DIR`/`GIT_WORK_TREE`/… exported by the
/// launching shell would redirect operations to an unrelated repo); the
/// user's own gitconfig must keep steering behavior (autocrlf, hooks), so the
/// shadow-repo hardening (`strip_all_git_env` + pinned empty config) is
/// deliberately NOT applied here.
fn user_git_command(program: &str) -> std::process::Command {
    let mut command = crate::platform::process::HiddenCommand::new(program);
    crate::platform::process::strip_git_override_env(&mut command);
    command
}

fn run_git(program: &str, cwd: Option<&Path>, args: &[&str]) -> Result<std::process::Output> {
    let mut command = user_git_command(program);
    if let Some(dir) = cwd {
        command.arg("-C").arg(dir);
    }
    command.args(args);
    crate::platform::process::output_with_timeout_and_kill_tree(command, GIT_COMMAND_TIMEOUT)
        .map_err(|error| anyhow::anyhow!("run git {}: {error}", args.join(" ")))
}

fn run_git_checked(program: &str, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let output = run_git(program, cwd, args)?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `root` is inside a git work tree (and git works at all). A bare
/// repository answers false — the copy path handles that shape honestly
/// instead of half-cloning it.
pub(crate) fn is_git_work_tree(git: &GitProgram<'_>, root: &Path) -> bool {
    run_git_checked(
        git.program,
        Some(root),
        &["rev-parse", "--is-inside-work-tree"],
    )
    .ok()
    .is_some_and(|stdout| stdout.trim() == "true")
}

/// Isolate `source_root` into a fresh copy at `target_path`. `session_id` is
/// the NEW session's id and only feeds naming conventions (directory suffix
/// uses its first 4 chars, the worktree branch its first 8 — plan §4.4).
///
/// The target must not exist. On success returns the canonical target path
/// and the method used; on failure nothing is left on disk and the error
/// names the failing path.
pub(crate) fn isolate_workspace_root(
    git: &GitProgram<'_>,
    source_root: &Path,
    target_path: &Path,
    session_id: &str,
    progress: &dyn Fn(CopyProgress),
) -> Result<IsolatedRoot> {
    if !source_root.is_dir() {
        bail!(
            "cannot isolate workspace root: source is not a directory: {}",
            source_root.display()
        );
    }
    if target_path.exists() {
        bail!(
            "cannot isolate workspace root: target already exists: {}",
            target_path.display()
        );
    }
    // Disk-space precheck (plan §8.3): the copy's partition must have room
    // for the source's on-disk size before anything is written. Upper-bound
    // estimate — a worktree copy is smaller, failing early beats failing
    // mid-copy.
    ensure_free_space_for_copy(source_root, target_path)?;

    let method = if git.available() && is_git_work_tree(git, source_root) {
        IsolationMethod::GitWorktree
    } else {
        IsolationMethod::DirectoryCopy
    };
    match method {
        IsolationMethod::GitWorktree => {
            match isolate_via_worktree(git, source_root, target_path, session_id, progress) {
                Ok(()) => {}
                Err(worktree_error) => {
                    // Degradation contract (plan §8.3): an anomalous worktree
                    // path (old git lacking flags, unappliable patch, …) falls
                    // back to a whole-directory copy after removing whatever
                    // the attempt created.
                    eprintln!(
                        "[workspace-isolation] worktree isolation failed for {}, falling back to directory copy: {worktree_error:#}",
                        source_root.display()
                    );
                    remove_worktree_best_effort(git, source_root, target_path);
                    copy_root_fresh(source_root, target_path, progress).with_context(|| {
                        format!(
                            "isolate {} by copy after worktree failure",
                            source_root.display()
                        )
                    })?;
                }
            }
        }
        IsolationMethod::DirectoryCopy => {
            copy_root_fresh(source_root, target_path, progress)
                .with_context(|| format!("isolate {} by copy", source_root.display()))?;
        }
    }

    write_fork_marker(target_path, source_root)
        .with_context(|| format!("write fork marker into {}", target_path.display()))?;
    if method == IsolationMethod::GitWorktree {
        exclude_marker_in_worktree(git, target_path).with_context(|| {
            format!("exclude fork marker in worktree {}", target_path.display())
        })?;
    }

    let canonical = target_path
        .canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());
    Ok(IsolatedRoot {
        path: crate::platform::os::platform_compat_path(&canonical.to_string_lossy()),
        method,
    })
}

/// Remove a root isolated by a previous successful
/// [`isolate_workspace_root`] call (whole-fork rollback, plan §6.1 #10).
/// Worktrees go through `git worktree remove --force` so the main repo's
/// admin metadata goes too; a plain `remove_dir_all` is the fallback for both
/// methods when git refuses. Best-effort: residual cleanup failures are
/// logged, not fatal — the caller is already unwinding a failure.
pub(crate) fn remove_isolated_root(
    git: &GitProgram<'_>,
    source_root: &Path,
    isolated: &IsolatedRoot,
) {
    if isolated.method == IsolationMethod::GitWorktree {
        remove_worktree_best_effort(git, source_root, &isolated.path);
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(&isolated.path) {
        if error.kind() != ErrorKind::NotFound {
            eprintln!(
                "[workspace-isolation] rollback removal of {} failed: {error:#}",
                isolated.path.display()
            );
        }
    }
}

fn remove_worktree_best_effort(git: &GitProgram<'_>, source_root: &Path, target: &Path) {
    let removed = run_git_checked(
        git.program,
        Some(source_root),
        &["worktree", "remove", "--force", &target.to_string_lossy()],
    )
    .is_ok();
    if !removed {
        // The admin entry can outlive the directory (and vice versa); prune
        // the stale registration, then drop the directory if it survived.
        let _ = run_git_checked(git.program, Some(source_root), &["worktree", "prune"]);
        let _ = std::fs::remove_dir_all(target);
    } else {
        let _ = run_git_checked(git.program, Some(source_root), &["worktree", "prune"]);
    }
}

/// Fresh worktree carrying the source's uncommitted state:
/// 1. `worktree add -b pinvou-fork/<id8> <target> HEAD` (HEAD parity is the
///    acceptance contract, §7 #9);
/// 2. `git diff --binary HEAD` from the source applied in the target
///    (tracked modifications, staged or not; `--binary` keeps binary edits);
/// 3. untracked-not-ignored files copied verbatim (`ls-files --others
///    --exclude-standard` — ignored build output is regenerable by design).
/// Submodules appear as empty directories: git-native worktree behavior,
/// documented in the plan and not special-cased (§8.3).
fn isolate_via_worktree(
    git: &GitProgram<'_>,
    source_root: &Path,
    target_path: &Path,
    session_id: &str,
    progress: &dyn Fn(CopyProgress),
) -> Result<()> {
    let branch = format!(
        "pinvou-fork/{}",
        session_id.chars().take(8).collect::<String>()
    );
    run_git_checked(
        git.program,
        Some(source_root),
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &target_path.to_string_lossy(),
            "HEAD",
        ],
    )
    .with_context(|| format!("create worktree at {}", target_path.display()))?;

    let patch = run_git_checked(
        git.program,
        Some(source_root),
        &["diff", "--binary", "HEAD"],
    )
    .context("capture uncommitted changes")?;
    if !patch.trim().is_empty() {
        apply_patch_in_worktree(git, target_path, patch.as_bytes())?;
    }
    copy_untracked_files(git, source_root, target_path, progress)
}

/// Apply the source's uncommitted diff inside the fresh worktree. The patch
/// travels through a temp file (the bounded process runner pins stdin to
/// null by design); it is removed afterwards either way.
fn apply_patch_in_worktree(git: &GitProgram<'_>, target_path: &Path, patch: &[u8]) -> Result<()> {
    let patch_path = std::env::temp_dir().join(format!(
        "pinvou3-fork-patch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| -> Result<()> {
        std::fs::write(&patch_path, patch)
            .with_context(|| format!("stage patch file {}", patch_path.display()))?;
        run_git_checked(
            git.program,
            Some(target_path),
            &[
                "apply",
                "--whitespace=nowarn",
                &patch_path.to_string_lossy(),
            ],
        )
        .with_context(|| format!("apply uncommitted changes in {}", target_path.display()))?;
        Ok(())
    })();
    let cleanup = std::fs::remove_file(&patch_path);
    result.inspect_err(|_| {
        if let Err(error) = cleanup {
            eprintln!("[workspace-isolation] remove patch file failed: {error:#}");
        }
    })
}

/// Copy the source's untracked files into the worktree, preserving relative
/// paths. Progress flows through the same callback as directory copies so the
/// fork progress events stay uniform across methods.
fn copy_untracked_files(
    git: &GitProgram<'_>,
    source_root: &Path,
    target_path: &Path,
    progress: &dyn Fn(CopyProgress),
) -> Result<()> {
    let listing = run_git_checked(
        git.program,
        Some(source_root),
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .context("list untracked files")?;
    let mut state = CopyProgress::default();
    for relative in listing.split('\0') {
        if relative.is_empty() {
            continue;
        }
        let from = source_root.join(relative);
        let to = target_path.join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create untracked parent dir {}", parent.display()))?;
        }
        let bytes = std::fs::copy(&from, &to).with_context(|| {
            format!("copy untracked file {} -> {}", from.display(), to.display())
        })?;
        state.files_copied += 1;
        state.bytes_copied += bytes;
        progress(state);
    }
    Ok(())
}

/// Whole-directory copy with progress. Recursion only descends REAL
/// directories (`DirEntry::file_type` does not follow links), so link cycles
/// cannot loop the walker; a symlink entry is handled by `fs::copy`, which
/// follows file links (preserving the pointed-at content) and fails on
/// dangling links or links-to-directories — per the plan's fail-closed rule
/// the whole root then rolls back with this path in the error (§8.3).
pub(crate) fn copy_dir_recursive(
    source: &Path,
    target: &Path,
    progress: &dyn Fn(CopyProgress),
) -> Result<()> {
    let mut state = CopyProgress::default();
    copy_dir_recursive_inner(source, target, &mut state, progress, 0)
}

/// Per-copy recursion depth cap: real-directory recursion cannot cycle, but a
/// hostile/deep tree must not exhaust the stack. Deep enough for any real
/// project layout.
const MAX_COPY_DEPTH: u8 = 64;

fn copy_dir_recursive_inner(
    source: &Path,
    target: &Path,
    state: &mut CopyProgress,
    progress: &dyn Fn(CopyProgress),
    depth: u8,
) -> Result<()> {
    if depth > MAX_COPY_DEPTH {
        bail!(
            "copy recursion too deep under {}: maximum depth {MAX_COPY_DEPTH} exceeded",
            source.display()
        );
    }
    std::fs::create_dir_all(target)
        .with_context(|| format!("create directory {}", target.display()))?;
    let entries = std::fs::read_dir(source)
        .with_context(|| format!("read directory {}", source.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("scan directory {}", source.display()))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("inspect {}", from.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive_inner(&from, &to, state, progress, depth + 1)?;
        } else {
            let bytes = std::fs::copy(&from, &to)
                .with_context(|| format!("copy file {} -> {}", from.display(), to.display()))?;
            state.files_copied += 1;
            state.bytes_copied += bytes;
            progress(*state);
        }
    }
    Ok(())
}

/// Copy wrapper used by the two public isolation paths: on failure remove the
/// partial target so no half copy survives (plan §6.1 #14).
fn copy_root_fresh(source: &Path, target: &Path, progress: &dyn Fn(CopyProgress)) -> Result<()> {
    match copy_dir_recursive(source, target, progress) {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_dir_all(target) {
                if cleanup.kind() != ErrorKind::NotFound {
                    eprintln!(
                        "[workspace-isolation] rollback partial copy at {} failed: {cleanup:#}",
                        target.display()
                    );
                }
            }
            Err(error)
        }
    }
}

/// Write `.pinvou-fork-workspace.json` with creation time and source path —
/// the storage-manager recognition contract (plan D7). Timestamps and the
/// source path are local user data; nothing else is recorded.
fn write_fork_marker(target: &Path, source_root: &Path) -> Result<()> {
    let marker = serde_json::json!({
        "version": 1u32,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "source_root": source_root.display().to_string(),
    });
    let payload = serde_json::to_vec_pretty(&marker).context("serialize fork marker")?;
    crate::platform::filesystem::atomic_write(&target.join(FORK_WORKSPACE_MARKER_FILE), &payload)
        .with_context(|| {
            format!(
                "write {}",
                target.join(FORK_WORKSPACE_MARKER_FILE).display()
            )
        })
}

/// Append the marker file name to the WORKTREE's own `info/exclude` (local
/// exclusion, never committed, does not touch the user's project ignores).
/// `--git-path` resolves the per-worktree admin dir; a linked worktree's
/// `.git` is a file, so joining by hand would be wrong.
fn exclude_marker_in_worktree(git: &GitProgram<'_>, target: &Path) -> Result<()> {
    let exclude_path = run_git_checked(
        git.program,
        Some(target),
        &["rev-parse", "--git-path", "info/exclude"],
    )
    .context("resolve worktree exclude file")?;
    let exclude_path = PathBuf::from(exclude_path.trim());
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == FORK_WORKSPACE_MARKER_FILE)
    {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(FORK_WORKSPACE_MARKER_FILE);
    updated.push('\n');
    std::fs::write(&exclude_path, updated)
        .with_context(|| format!("update {}", exclude_path.display()))
}

/// Sum the source root's on-disk file sizes (metadata only — same walker
/// family as the copy; symlink entries count as their entry metadata).
pub(crate) fn directory_size(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("read directory {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("scan directory {}", dir.display()))?;
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else if let Ok(metadata) = entry.metadata() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

/// Fail when the target's partition cannot hold the source's size (plan
/// §8.3). Uses the target's PARENT as the probe anchor (the target itself
/// does not exist yet).
fn ensure_free_space_for_copy(source_root: &Path, target_path: &Path) -> Result<()> {
    let needed = directory_size(source_root)?;
    let probe = target_path.parent().unwrap_or(target_path);
    ensure_free_space(needed, source_root, probe)
}

/// The probe core, split out so the failure branch stays testable without
/// fabricating a multi-terabyte source tree.
fn ensure_free_space(needed: u64, source_root: &Path, probe: &Path) -> Result<()> {
    let Some(available) = available_disk_space(probe) else {
        // Availability probing is unsupported here (never blocks a fork on a
        // probe gap); a genuine out-of-space condition still surfaces at copy
        // time with the concrete failing path.
        return Ok(());
    };
    if available < needed {
        bail!(
            "not enough disk space to isolate {}: needs approximately {} bytes, {} available on {}",
            source_root.display(),
            needed,
            available,
            probe.display()
        );
    }
    Ok(())
}

/// Free bytes on the partition containing `path`, when the platform exposes a
/// probe. Unix uses statvfs; Windows uses GetDiskFreeSpaceExW.
fn available_disk_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let c_path = CString::new(path.as_os_str().to_string_lossy().as_bytes().to_vec()).ok()?;
        // SAFETY: an all-zero bit pattern is a valid `statvfs` (a POD struct
        // of integers); it is fully overwritten by the call below on success.
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_path` is a valid NUL-terminated path string owned for the
        // duration of the call, and `stats` is a valid, aligned out-pointer of
        // the exact type statvfs expects. The call touches no Rust references.
        let status = unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) };
        if status != 0 {
            return None;
        }
        let free = stats.f_bavail as u64;
        let block = stats.f_bsize as u64;
        Some(free.saturating_mul(block))
    }
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_available: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        // SAFETY: `wide` is NUL-terminated and owned for the call; the three
        // out-pointers are valid ULARGE_INTEGER slots of the declared type.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_available,
                &mut total,
                &mut total_free,
            )
        };
        (ok != 0).then_some(free_available)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-isolation-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create unique dir");
        dir
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    fn git() -> GitProgram<'static> {
        GitProgram { program: "git" }
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create repo dir");
        let git = git();
        run_git_checked(git.program, Some(dir), &["init"]).expect("git init");
        run_git_checked(
            git.program,
            Some(dir),
            &["config", "user.email", "fork@test.local"],
        )
        .expect("git config email");
        run_git_checked(
            git.program,
            Some(dir),
            &["config", "user.name", "Fork Test"],
        )
        .expect("git config name");
        write(&dir.join("committed.txt"), "committed\n");
        run_git_checked(git.program, Some(dir), &["add", "-A"]).expect("git add");
        run_git_checked(git.program, Some(dir), &["commit", "-m", "init"]).expect("git commit");
    }

    /// §6.1 #11: a dirty work tree forks into a worktree that carries both
    /// the uncommitted modification and the untracked file, with HEAD parity.
    #[test]
    fn worktree_creation_carries_uncommitted_and_untracked() {
        let home = unique_dir("dirty-worktree");
        let source = home.join("repo");
        init_repo(&source);
        write(
            &source.join("committed.txt"),
            "committed + uncommitted edit\n",
        );
        write(&source.join("untracked.txt"), "untracked\n");
        write(
            &source.join("nested").join("deep.txt"),
            "nested untracked\n",
        );

        let target = home.join("repo-fork-a1b2");
        let outcome = isolate_workspace_root(&git(), &source, &target, "a1b2c3d4e5f6", &|_| {})
            .expect("isolate");

        assert_eq!(outcome.method, IsolationMethod::GitWorktree);
        assert!(target.join("committed.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(target.join("committed.txt")).expect("read modified"),
            "committed + uncommitted edit\n",
            "uncommitted modification must exist in the new worktree"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("untracked.txt")).expect("read untracked"),
            "untracked\n",
            "untracked file must exist in the new worktree"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("nested").join("deep.txt"))
                .expect("read nested untracked"),
            "nested untracked\n"
        );
        // HEAD parity + branch naming convention.
        let source_head = run_git_checked(git().program, Some(&source), &["rev-parse", "HEAD"])
            .expect("source HEAD");
        let target_head = run_git_checked(git().program, Some(&target), &["rev-parse", "HEAD"])
            .expect("target HEAD");
        assert_eq!(source_head.trim(), target_head.trim());
        let branch = run_git_checked(
            git().program,
            Some(&target),
            &["rev-parse", "--abbrev-ref", "HEAD"],
        )
        .expect("target branch");
        assert_eq!(branch.trim(), "pinvou-fork/a1b2c3d4");
        // The source stays untouched: its work tree still has the dirty state.
        assert_eq!(
            std::fs::read_to_string(source.join("committed.txt")).expect("source untouched"),
            "committed + uncommitted edit\n"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// §6.1 #12: when git cannot run, isolation degrades to a directory copy
    /// with equivalent content.
    #[test]
    fn worktree_fallback_to_copy_when_git_missing() {
        let home = unique_dir("git-missing");
        let source = home.join("plain");
        write(&source.join("a.txt"), "a\n");
        write(&source.join("sub").join("b.txt"), "b\n");

        let missing_git = GitProgram {
            program: "pinvou3-missing-git-for-test",
        };
        let target = home.join("plain-fork-0000");
        let outcome = isolate_workspace_root(&missing_git, &source, &target, "00001111", &|_| {})
            .expect("isolate with git missing");
        assert_eq!(outcome.method, IsolationMethod::DirectoryCopy);
        assert_eq!(
            std::fs::read_to_string(target.join("a.txt")).expect("copied a"),
            "a\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("sub").join("b.txt")).expect("copied b"),
            "b\n"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// §6.1 #14: the progress callback observes monotonically growing
    /// counters, and a mid-copy failure leaves no partial directory behind.
    #[test]
    fn copy_dir_reports_progress_and_aborts_cleanly() {
        let home = unique_dir("progress");
        let source = home.join("src");
        write(&source.join("one.txt"), "11");
        write(&source.join("two.txt"), "22");

        let seen = std::cell::RefCell::new(Vec::new());
        copy_dir_recursive(&source, &home.join("dst"), &|progress| {
            seen.borrow_mut().push(progress);
        })
        .expect("copy");
        let seen = seen.into_inner();
        assert!(!seen.is_empty(), "progress must fire for copied files");
        for pair in seen.windows(2) {
            assert!(
                pair[0].files_copied <= pair[1].files_copied
                    && pair[0].bytes_copied <= pair[1].bytes_copied,
                "progress counters must be monotonic: {pair:?}"
            );
        }

        // Failure injection: an existing FILE at the target path makes the
        // nested create_dir_all fail mid-copy — no partial tree may survive.
        let blocked = home.join("blocked");
        write(&blocked, "occupies the target location");
        let result = copy_dir_recursive(&source, &blocked, &|_| {});
        assert!(result.is_err(), "copy onto a file must fail");
        assert!(
            !blocked.join("one.txt").exists(),
            "no partial copy may survive a failed isolation"
        );
        assert_eq!(
            std::fs::read_to_string(&blocked).expect("blocker intact"),
            "occupies the target location"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// §6.1 #15: the marker file exists in every copy, and in a worktree the
    /// per-worktree `info/exclude` lists it.
    #[test]
    fn fork_marker_file_written_and_excluded_in_worktree() {
        let home = unique_dir("marker");
        let source = home.join("repo");
        init_repo(&source);
        let target = home.join("repo-fork-beef");
        isolate_workspace_root(&git(), &source, &target, "beef00001111", &|_| {}).expect("isolate");

        let marker = target.join(FORK_WORKSPACE_MARKER_FILE);
        assert!(marker.is_file(), "marker must exist in the worktree");
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).expect("read marker"))
                .expect("parse marker");
        assert_eq!(payload["source_root"], source.display().to_string());

        let exclude = run_git_checked(
            git().program,
            Some(&target),
            &["rev-parse", "--git-path", "info/exclude"],
        )
        .expect("resolve exclude");
        let exclude = PathBuf::from(exclude.trim());
        let contents = std::fs::read_to_string(&exclude).expect("read exclude");
        assert!(
            contents
                .lines()
                .any(|line| line.trim() == FORK_WORKSPACE_MARKER_FILE),
            "worktree exclude must list the marker: {contents}"
        );
        // The source repo's own status must stay clean of the marker (the
        // marker lives in the worktree, excluded there).
        let status = run_git_checked(git().program, Some(&target), &["status", "--porcelain"])
            .expect("status");
        assert!(
            !status.contains(FORK_WORKSPACE_MARKER_FILE),
            "marker must not pollute git status: {status}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A pre-existing target is refused before any byte is copied.
    #[test]
    fn existing_target_is_refused() {
        let home = unique_dir("target-exists");
        let source = home.join("src");
        write(&source.join("a.txt"), "a\n");
        let target = home.join("out");
        write(&target.join("keep.txt"), "keep\n");
        let result = isolate_workspace_root(&git(), &source, &target, "aaaabbbb", &|_| {});
        assert!(result.is_err(), "an existing target must be refused");
        assert_eq!(
            std::fs::read_to_string(target.join("keep.txt")).expect("target untouched"),
            "keep\n"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Removing an isolated worktree cleans both the directory and the main
    /// repo's worktree registration (whole-fork rollback support).
    #[test]
    fn remove_isolated_worktree_cleans_registration() {
        let home = unique_dir("remove-worktree");
        let source = home.join("repo");
        init_repo(&source);
        let target = home.join("repo-fork-c0ff");
        let isolated = isolate_workspace_root(&git(), &source, &target, "c0ffee001111", &|_| {})
            .expect("isolate");
        remove_isolated_root(&git(), &source, &isolated);
        assert!(!target.exists(), "worktree directory must be removed");
        let listing = run_git_checked(git().program, Some(&source), &["worktree", "list"])
            .expect("list worktrees");
        assert!(
            !listing.contains("repo-fork-c0ff"),
            "worktree registration must be pruned: {listing}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The disk-space precheck fails when the requirement exceeds the
    /// partition's availability, and names the source in the error.
    #[test]
    fn insufficient_disk_space_fails_before_copy() {
        let home = unique_dir("disk-space");
        let source = home.join("src");
        write(&source.join("a.txt"), "a\n");
        let Some(available) = available_disk_space(&home) else {
            // Platform without a probe: the check degrades to Ok — pinned by
            // the companion test below.
            return;
        };
        let needed = available.saturating_add(1);
        let result = ensure_free_space(needed, &source, &home);
        let error = result.expect_err("a needed size above availability must fail");
        assert!(
            error.to_string().contains("not enough disk space"),
            "error must state the space shortage: {error:#}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
