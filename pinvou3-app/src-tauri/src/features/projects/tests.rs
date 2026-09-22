//! ProjectStore 行为测试。全部走 `from_paths` + 临时目录,不触进程全局
//! `PINVOU3_HOME`。
// architecture-guard: allow-target-cfg -- the rebind alias regression needs a real directory symlink to emulate macOS /var → /private/var ancestor resolution; symlink creation is only available unprivileged on unix, so the test is cfg(unix)-gated and no platform behavior leaks into shared code.

use std::collections::HashSet;
use std::path::PathBuf;

use super::ProjectStore;

fn store_in(temp: &tempfile::TempDir) -> ProjectStore {
    ProjectStore::from_paths(temp.path().join("projects.json"))
}

/// 平台中立的不存在绝对路径:锚在 std::env::temp_dir() 下,Windows 上同样
/// 满足 is_absolute()(此前硬编码 "/pinvou3-projects-test-root" 会让全部
/// create 用例在 Windows 上被绝对性校验拒掉,套件只能跑 Ubuntu)。
/// 路径刻意不存在:canonicalize 失败走词法绝对化,身份键仍可判定。
fn abs(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join("pinvou3-projects-test-root")
        .join(name)
}

/// 断言用的期望形态:与 store 的 root_display 同一约定。PathBuf 相等是词法
/// 比较,拿原始 `canonicalize()`(Windows 带 `\\?\` verbatim 前缀)或原始
/// 写法(macOS 的 /var 与 canonical 的 /private/var 错位)直接对比会在
/// 非 Linux 平台假失败(评审 #447-D2)。
fn display(path: &std::path::Path) -> PathBuf {
    super::store::root_display(path)
}

fn create(store: &ProjectStore, name: &str, roots: &[PathBuf]) -> super::Project {
    store
        .create_project(name.to_string(), roots.to_vec())
        .expect("create project")
}

#[test]
fn boot_without_file_starts_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    assert!(store.list().is_empty());
    assert!(!temp.path().join("projects.json").exists());
    assert!(!temp.path().join("projects.json.tmp").exists());
}

#[test]
fn create_assigns_sequential_positions_and_generated_ids() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let first = create(&store, "前端", &[abs("web")]);
    let second = create(&store, "后端", &[abs("api")]);

    assert!(first.id.starts_with("prj-"));
    assert_ne!(first.id, second.id);
    let listed = store.list();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, first.id);
    assert_eq!(listed[0].position, 0);
    assert_eq!(listed[1].id, second.id);
    assert_eq!(listed[1].position, 1);
}

#[test]
fn create_rejects_blank_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let error = store
        .create_project("   ".to_string(), vec![])
        .expect_err("blank name rejected");
    assert!(error.to_string().contains("name must not be empty"));
    assert!(store.list().is_empty());
}

#[test]
fn create_rejects_relative_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let error = store
        .create_project("相对路径".to_string(), vec![PathBuf::from("repo")])
        .expect_err("relative root rejected");
    assert!(error.to_string().contains("must be absolute"));
}

#[test]
fn create_rejects_duplicate_and_nested_roots_within_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let duplicate = store
        .create_project("重复".to_string(), vec![abs("a"), abs("a")])
        .expect_err("duplicate root rejected");
    assert!(duplicate.to_string().contains("duplicate project root"));

    let nested = store
        .create_project("嵌套".to_string(), vec![abs("b"), abs("b").join("sub")])
        .expect_err("nested roots rejected");
    assert!(nested.to_string().contains("must not nest"));
}

#[test]
fn create_allows_overlap_with_other_projects() {
    // §9.9 legalizes cross-project root overlap: a folder may be referenced by
    // several projects (auto-materialization and the browse channel both rely
    // on it). The intra-set no-nesting invariant is what still holds.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    create(&store, "已有项目", &[abs("work")]);

    store
        .create_project("同路径".to_string(), vec![abs("work")])
        .expect("same root across projects is legal");

    store
        .create_project("子路径".to_string(), vec![abs("work").join("sub")])
        .expect("nested root across projects is legal");

    create(&store, "无关", &[abs("other")]);
    store
        .create_project(
            "父路径".to_string(),
            vec![abs("work").parent().unwrap().to_path_buf()],
        )
        .expect("ancestor root across projects is legal");

    // 组内不嵌套不变量仍然成立:同一个项目的 roots 不得互相嵌套。
    let nested = store
        .create_project(
            "组内嵌套".to_string(),
            vec![abs("work"), abs("work").join("sub")],
        )
        .expect_err("intra-set nesting rejected");
    assert!(nested.to_string().contains("must not nest"), "{nested}");
}

#[test]
fn canonicalized_real_dirs_still_reject_intra_set_nesting() {
    // Canonicalization must keep working with the overlap legalization: two
    // real, symlink-resolved directories that nest inside ONE project are
    // still rejected (the cross-project direction is legal since §9.9).
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path().join("repo");
    let child = parent.join("sub");
    std::fs::create_dir_all(&child).expect("create dirs");

    let store = store_in(&temp);
    store
        .create_project("父".to_string(), vec![parent.clone()])
        .expect("first project");
    store
        .create_project("子".to_string(), vec![child.clone()])
        .expect("cross-project overlap is legal");

    let error = store
        .create_project("组内".to_string(), vec![parent, child])
        .expect_err("intra-set nesting rejected");
    assert!(error.to_string().contains("must not nest"), "{error}");
}

#[test]
fn update_renames_and_replaces_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "旧名", &[abs("old")]);

    let updated = store
        .update_project(
            &project.id,
            Some("新名".to_string()),
            Some(vec![abs("new")]),
        )
        .expect("update project");
    assert_eq!(updated.name, "新名");
    assert_eq!(updated.roots, vec![display(&abs("new"))]);
    assert_eq!(store.get(&project.id).unwrap().name, "新名");

    // None = 保持不变;空 roots 合法(项目退化为纯标签)。
    let kept = store
        .update_project(&project.id, None, None)
        .expect("no-op update");
    assert_eq!(kept.roots, vec![display(&abs("new"))]);
    let emptied = store
        .update_project(&project.id, None, Some(vec![]))
        .expect("clear roots");
    assert!(emptied.roots.is_empty());

    let unknown = store
        .update_project("prj-does-not-exist", None, None)
        .expect_err("unknown project rejected");
    assert!(unknown.to_string().contains("project not found"));

    let blank = store
        .update_project(&project.id, Some("  ".to_string()), None)
        .expect_err("blank name rejected");
    assert!(blank.to_string().contains("name must not be empty"));
}

#[test]
fn delete_unassigns_sessions_but_keeps_explicit_move_out() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "待删", &[abs("x")]);
    let other = create(&store, "幸存", &[abs("y")]);

    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign s1");
    store
        .move_session_to_project("s2", Some(&project.id), None)
        .expect("assign s2");
    // s3 显式移出:语义是"不进任何项目",与目标项目存亡无关。
    store
        .move_session_to_project("s3", Some(&project.id), None)
        .expect("assign s3");
    store
        .move_session_to_project("s3", None, None)
        .expect("move s3 out");
    store
        .move_session_to_project("s4", Some(&other.id), None)
        .expect("assign s4");

    let report = store
        .delete_project(&project.id, &[])
        .expect("delete project");
    let mut affected = report.affected_session_ids;
    affected.sort();
    assert_eq!(affected, vec!["s1", "s2"]);

    // 删除把成员写成显式移出(tombstone),而不是清空条目:否则该文件夹下一次
    // 自动物化会把它们重新收编,删除结果死而复生。
    assert_eq!(store.assignment_of("s1"), Some(None));
    assert_eq!(store.assignment_of("s2"), Some(None));
    // s3 已有的显式移出条目保留(不被删除项目连带改写)。
    assert_eq!(store.assignment_of("s3"), Some(None));
    assert_eq!(store.assignment_of("s4"), Some(Some(other.id.clone())));
    assert!(store.get(&project.id).is_none());

    // 仅剩 tombstone 时仍要保留文件(否则重启后删除过的文件夹会被重新物化);
    // 真正无项目、无归属、无排除表时,空状态不留文件。
    store.delete_project(&other.id, &[]).expect("delete other");
    store.forget_session("s1");
    store.forget_session("s2");
    store.forget_session("s3");
    store.forget_session("s4");
    assert!(!temp.path().join("projects.json").exists());
}

#[test]
fn move_assigns_explicitly_and_unassign_blocks_auto_revival() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "目标", &[]);

    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign");
    assert_eq!(store.assignment_of("s1"), Some(Some(project.id.clone())));
    assert_eq!(store.assigned_session_ids(&project.id), vec!["s1"]);

    // 显式移出 = 条目存在且为 None(区别于"无条目走自动归组")。
    store
        .move_session_to_project("s1", None, None)
        .expect("explicit move out");
    assert_eq!(store.assignment_of("s1"), Some(None));
    assert!(store.assigned_session_ids(&project.id).is_empty());

    let unknown = store
        .move_session_to_project("s1", Some("prj-nope"), None)
        .expect_err("unknown project rejected");
    assert!(unknown.to_string().contains("project not found"));

    let orphan_root = store
        .move_session_to_project("s1", None, Some(&abs("nowhere")))
        .expect_err("add_workspace_root without project rejected");
    assert!(
        orphan_root
            .to_string()
            .contains("requires a target project")
    );
}

#[test]
fn move_add_workspace_root_atomically_and_idempotently() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    // 他人领地与 workspace 互不重叠,避免测试自触发跨项目重叠规则。
    let foreign = temp.path().join("foreign").join("nested");
    std::fs::create_dir_all(&foreign).expect("create foreign dirs");
    let canonical = display(&workspace);

    let store = store_in(&temp);
    let project = create(&store, "目标", &[abs("elsewhere")]);
    let other = create(&store, "他人领地", std::slice::from_ref(&foreign));

    // 顺带加 root:归并与加目录一次落盘。
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&workspace))
        .expect("move with workspace root");
    assert_eq!(outcome.added_root, Some(canonical.clone()));
    assert!(store.get(&project.id).unwrap().roots.contains(&canonical));

    // 已被现有 root 覆盖时幂等跳过,不重复添加。
    let again = store
        .move_session_to_project("s2", Some(&project.id), Some(&workspace.join("deep")))
        .expect("covered workspace skips add");
    assert_eq!(again.added_root, None);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 2);

    // 落在他人领地内的目录现在合法(§9.9):归属与根一并落盘,两个项目各自
    // 保留自己的 roots。
    store
        .move_session_to_project("s3", Some(&project.id), Some(&foreign.join("deeper")))
        .map(|_| ())
        .expect("cross-project overlap is legal");
    assert_eq!(store.assignment_of("s3"), Some(Some(project.id.clone())));
    assert_eq!(store.get(&other.id).unwrap().roots.len(), 1);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 3);
}

#[test]
fn covered_workspace_skip_survives_symlinked_ancestor() {
    // Review #464 MAJOR 3 (same shape as macOS /var→/private/var): roots are
    // canonicalized on insertion, but when the workspace path under the
    // covered check does not exist, the old purely lexical fallback kept the
    // symlink form, so the identity key was no longer nested and the path was
    // re-added as uncovered. Reproduce on any platform with a symlinked
    // ancestor. Branch on the std::env::consts::OS constant instead of cfg
    // syntax: platform conditional compilation must not appear outside the
    // adapter layer (architecture-guard); Windows directory symlinks require
    // admin/developer mode, so the mechanism is covered by unix/macOS.
    if std::env::consts::OS == "windows" {
        return;
    }
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("real").join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let link = temp.path().join("link");
    let status = std::process::Command::new("ln")
        .arg("-s")
        .arg(temp.path().join("real"))
        .arg(&link)
        .status()
        .expect("spawn ln");
    assert!(status.success(), "ln -s must succeed on unix-likes");

    let store = store_in(&temp);
    let project = create(
        &store,
        "目标",
        std::slice::from_ref(&link.join("workspace")),
    );

    // A nonexistent nested path written through the symlinked ancestor: the
    // covered check must hit the existing root.
    let covered = link.join("workspace").join("deep");
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&covered))
        .expect("move with covered workspace");
    assert_eq!(
        outcome.added_root, None,
        "symlink 形态不得绕过 covered 跳过"
    );
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 1);
}

#[test]
fn nonexistent_leaf_resolves_into_existing_ancestors_territory() {
    // 评审 #471 Major 回归锁:macOS 默认 TMPDIR 位于 /var 下(→ /private/var),
    // 不存在的叶子必须经最深已存在祖先 canonicalize,与已存在路径键入同一
    // 键域,否则 covered-skip 与跨项目重叠拒绝双双失明。
    let temp = tempfile::tempdir().expect("tempdir");
    let base = temp.path().join("base");
    std::fs::create_dir_all(&base).expect("create base");

    let leaf = base.join("not-yet").join("deep");
    assert_eq!(
        display(&leaf),
        display(&base).join("not-yet").join("deep"),
        "non-existent leaf keys into its parent's territory"
    );
    // 同一份临时根下的两条不存在路径键域一致(重叠校验可判定)。
    let dangling = abs("ghost").join("deep");
    assert_eq!(display(&dangling), display(&abs("ghost")).join("deep"));
}

#[test]
fn persist_roundtrip_preserves_state_on_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    let project_id = {
        let store = ProjectStore::from_paths(path.clone());
        let project = create(&store, "持久化", &[abs("persist")]);
        store
            .move_session_to_project("s1", Some(&project.id), None)
            .expect("assign");
        project.id
    };

    let reopened = ProjectStore::from_paths(path.clone());
    assert_eq!(reopened.list().len(), 1);
    assert_eq!(reopened.list()[0].id, project_id);
    assert_eq!(reopened.assignment_of("s1"), Some(Some(project_id.clone())));

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read persisted file"))
            .expect("parse persisted file");
    assert_eq!(raw["schema_version"], 1);
    assert!(
        !path.with_extension("json.tmp").exists(),
        "tmp file cleaned up"
    );
}

#[test]
fn corrupt_file_boots_empty_without_panicking() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    std::fs::write(&path, "not json at all").expect("write corrupt file");

    let store = ProjectStore::from_paths(path);
    assert!(store.list().is_empty());
    // 损坏后的首次变更自愈覆盖。
    create(&store, "自愈", &[]);
    assert_eq!(store.list().len(), 1);
}

#[test]
fn newer_schema_version_is_rejected_and_never_overwritten() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    std::fs::write(
        &path,
        r#"{"schema_version": 99, "projects": [], "assignments": {}}"#,
    )
    .expect("write future schema");

    let store = ProjectStore::from_paths(path.clone());
    assert!(store.list().is_empty(), "future schema degrades to empty");

    // 降级进程拒绝一切写入:空状态 + 首次变更不得把新结构文件降级覆盖。
    let error = store
        .create_project("降级写".to_string(), vec![])
        .expect_err("writes refused after newer-schema load");
    assert!(error.to_string().contains("refusing to overwrite"));
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read preserved file"))
            .expect("parse preserved file");
    assert_eq!(raw["schema_version"], 99, "file content untouched");
}

#[test]
fn rebind_roots_rewrites_prefix_and_stays_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("from");
    let to = temp.path().join("moved");
    std::fs::create_dir_all(&to).expect("create to dir");

    let project = create(&store, "搬家", &[from.clone(), abs("untouched")]);
    let other = create(&store, "无关", &[abs("elsewhere")]);

    let affected = store.rebind_roots(&from, &to).expect("rebind roots");
    assert_eq!(affected, vec![project.id.clone()]);
    let roots = store.get(&project.id).unwrap().roots;
    assert!(roots.contains(&display(&to)));
    // Expected forms go through display(): on Windows the 8.3 short name of
    // env::temp_dir is expanded by ancestor canonicalization, so raw abs()
    // and the stored form differ lexically (review #463 windows CI failed
    // twice on this).
    assert!(
        roots.contains(&display(&abs("untouched"))),
        "prefix 外的 root 不动"
    );
    assert_eq!(
        store.get(&other.id).unwrap().roots,
        vec![display(&abs("elsewhere"))]
    );

    // Idempotent: nothing matches the from prefix anymore, rerun is a no-op.
    assert!(store.rebind_roots(&from, &to).unwrap().is_empty());
    assert_eq!(store.get(&project.id).unwrap().roots, roots);
}

/// Round-8 review M2: rebind_roots must persist BEFORE committing the
/// in-memory candidate. The store's final `projects.json` path is replaced
/// by a NON-EMPTY directory — atomic_write's rename onto it fails on every
/// platform (empty dirs would be silently replaced on POSIX) — so the
/// persist fails deterministically after the candidate is built, and the
/// memory must still hold `from`, so a retry (after clearing the obstacle)
/// converges instead of false-succeeding. Mirrors the codex lane's
/// `rebind_prefix_rolls_back_memory_when_index_persist_fails`.
#[test]
fn rebind_roots_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("from");
    let to = temp.path().join("moved");
    std::fs::create_dir_all(&to).expect("create to dir");

    let project = create(&store, "搬家", std::slice::from_ref(&from));
    let before = store.get(&project.id).unwrap();

    // Replace the store's final path with a non-empty directory: rename
    // onto it fails on every platform.
    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    let error = store
        .rebind_roots(&from, &to)
        .expect_err("persist failure surfaces as an error");
    assert!(
        !matches!(
            error,
            crate::features::projects::RebindRootsError::Overlap(_)
        ),
        "a persist failure must not be classified as an overlap conflict (round-8 M3)"
    );
    assert_eq!(
        store.get(&project.id).unwrap(),
        before,
        "memory must roll back to the on-disk state on persist failure"
    );

    // After clearing the obstacle a retry converges: the from-root is still
    // there to be moved.
    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let affected = store.rebind_roots(&from, &to).expect("retry rebind");
    assert_eq!(affected, vec![project.id.clone()]);
    assert!(
        store
            .get(&project.id)
            .unwrap()
            .roots
            .contains(&display(&to))
    );
}

#[cfg(unix)]
#[test]
fn rebind_roots_cuts_suffix_by_resolved_form_for_alias_callers() {
    // round-6/7 B1: an alias caller passes a `from` whose spelling differs
    // from the stored root (/alias vs the resolved /real). What this test pins
    // is the MATCH domain — the stored root is written in resolved display
    // form, so a raw-fold match would find nothing and return an empty report.
    // It does NOT exercise the suffix arithmetic: raw and resolved spellings
    // here have the same component count, so a cut by either count lands on
    // the same suffix. The depth half is pinned separately by
    // `rebind_roots_via_symlink_alias_cuts_suffix_by_resolved_depth` below,
    // where the resolved form is genuinely one component deeper.
    // cfg(unix)-gated under the file-top allow-target-cfg exception:
    // std::os::unix::fs::symlink does not exist on Windows.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let real = temp.path().join("real");
    let alias = temp.path().join("alias");
    std::fs::create_dir_all(&real).expect("create real dir");
    std::os::unix::fs::symlink(&real, &alias).expect("symlink");

    // The project root is stored in display form (create resolves the symlink
    // via root_display) while `from` is passed in raw alias form — exactly the
    // alias-caller scenario.
    let from = alias.clone();
    let to = temp.path().join("moved");
    std::fs::create_dir_all(&to).expect("create to dir");
    let project = create(&store, "Alias", std::slice::from_ref(&from));

    let affected = store.rebind_roots(&from, &to).expect("rebind roots");
    assert_eq!(affected, vec![project.id.clone()]);
    // The rewritten root must equal display(&to) exactly, with no leftover
    // alias component.
    let roots = store.get(&project.id).unwrap().roots;
    assert_eq!(roots, vec![display(&to)]);
}

#[test]
fn begin_rebind_serializes_and_releases_on_drop() {
    // round-7 m7: the gate's check-and-set and Drop release had zero coverage.
    let store =
        ProjectStore::from_paths(std::env::temp_dir().join("pinvou3-rebind-gate-test.json"));
    let _gate = store.begin_rebind().expect("first acquire wins");
    let error = store
        .begin_rebind()
        .expect_err("second acquire must be rejected");
    assert!(error.starts_with("REBIND_IN_PROGRESS:"));
    drop(_gate);
    store.begin_rebind().expect("gate released by Drop");
}

#[test]
fn rebind_fence_excludes_writers_and_rebinds_in_both_directions() {
    // review #464 round-6 finding 6: the root-accepting writers must not commit
    // into an in-flight rebind, and a rebind must not start while a writer
    // holds the fence — one flag, both directions.
    let store =
        ProjectStore::from_paths(std::env::temp_dir().join("pinvou3-rebind-fence-test.json"));
    let fence = store.rebind_fence().expect("first writer wins the fence");
    assert!(
        store.begin_rebind().is_err(),
        "a rebind must not start while a fenced writer is committing"
    );
    assert!(
        store
            .rebind_fence()
            .expect_err("second writer must be rejected")
            .starts_with("REBIND_IN_PROGRESS:"),
        "writers are mutually exclusive under the same marker the frontend maps"
    );
    drop(fence);
    store
        .rebind_fence()
        .expect("fence released by Drop, so error paths cannot close it forever");
    let gate = store
        .begin_rebind()
        .expect("rebind after the fence is released");
    assert!(
        store.rebind_fence().is_err(),
        "a writer must not commit while the rebind holds the gate"
    );
    drop(gate);
    store
        .rebind_fence()
        .expect("fence available again after the rebind");
}

#[test]
fn rebind_roots_legalizes_cross_project_overlap_and_keeps_intra_set_rule() {
    // Section-9.9: a rebind may land a root on (or nested under) another
    // project's territory — cross-project overlap is legal, so plan and
    // commit both succeed there and the surviving project is untouched. What
    // the commit still rejects is the intra-set no-nesting invariant, and the
    // rollback contract is unchanged for it.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("from2");
    let occupied = temp.path().join("occupied");
    std::fs::create_dir_all(&occupied).expect("create occupied dir");

    // Cross-project: `from` translates onto `occupied`, which another project
    // holds — plan and commit agree it is legal.
    let project = create(&store, "待搬", std::slice::from_ref(&from));
    let other = create(&store, "已有领地", std::slice::from_ref(&occupied));
    assert_eq!(
        store.plan_rebind_roots(&from, &occupied).expect("plan"),
        vec![project.id.clone()],
        "cross-project overlap is legal in the pre-flight too"
    );
    assert_eq!(
        store.rebind_roots(&from, &occupied).expect("commit"),
        vec![project.id.clone()]
    );
    // Stored roots are display forms: compare through the same projection
    // (raw temp paths can carry Windows 8.3 short-name segments).
    assert_eq!(
        store.get(&project.id).unwrap().roots,
        vec![display(&occupied)]
    );
    assert_eq!(
        store.get(&other.id).unwrap().roots,
        vec![display(&occupied)],
        "the surviving project is untouched"
    );

    // Intra-set: a translation that would nest the translated root against a
    // sibling root of the SAME project is rejected by plan and commit alike,
    // with the memory state rolled back to the on-disk state.
    let nested_from = abs("nest-from");
    let to = temp.path().join("nest-to");
    std::fs::create_dir_all(to.join("sub")).expect("create target dirs");
    let both = create(&store, "组内嵌套", &[nested_from.clone(), to.join("sub")]);
    let before = store.get(&both.id).unwrap();
    let error = store
        .plan_rebind_roots(&nested_from, &to)
        .expect_err("intra-set nesting rejected in the pre-flight");
    assert!(error.to_string().contains("nest"));
    let error = store
        .rebind_roots(&nested_from, &to)
        .expect_err("intra-set nesting rejected at commit");
    assert!(error.to_string().contains("nest"));
    assert_eq!(
        store.get(&both.id).unwrap(),
        before,
        "commit rollback leaves memory identical to disk"
    );
}

/// Pre-flight for the reordered rebind (review #463 round-8 M3): the session
/// lanes now run before the project roots, so a root rewrite that cannot
/// succeed has to be rejected before anything is written. `plan_rebind_roots`
/// reports exactly what `rebind_roots` would commit and touches nothing.
#[test]
fn plan_rebind_roots_previews_without_mutating() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("plan-from");
    let to = temp.path().join("plan-to");
    std::fs::create_dir_all(to.join("sub")).expect("create target dirs");

    // Conflict: the translation would nest `to` against the project's own
    // `to/sub` sibling root — the one failure mode the commit still has. The
    // plan must fail without touching state…
    let project = create(&store, "To be moved", &[from.clone(), to.join("sub")]);
    let before = store.get(&project.id).unwrap();
    let error = store
        .plan_rebind_roots(&from, &to)
        .expect_err("intra-set nesting rejected in the pre-flight");
    assert!(error.to_string().contains("nest"));
    assert_eq!(store.get(&project.id).unwrap(), before);

    // …and on a clean target it must report the same set `rebind_roots`
    // commits, still without writing.
    let clean_from = abs("plan-clean-from");
    let clean_to = temp.path().join("plan-clean-to");
    std::fs::create_dir_all(&clean_to).expect("create clean target dir");
    let lone = create(&store, "Lone holder", std::slice::from_ref(&clean_from));
    assert_eq!(
        store
            .plan_rebind_roots(&clean_from, &clean_to)
            .expect("plan"),
        vec![lone.id.clone()]
    );
    assert_eq!(
        store.get(&project.id).unwrap(),
        before,
        "planning must not persist the rewrite"
    );
    // The plan is a pure preview: rerunning it on the untouched state reports
    // the same set again.
    assert_eq!(
        store
            .plan_rebind_roots(&clean_from, &clean_to)
            .expect("plan again"),
        vec![lone.id.clone()]
    );
    // The commit lands exactly what the plan previewed; afterwards nothing
    // matches `clean_from` any more, matching the retry contract.
    assert!(
        store
            .rebind_roots(&clean_from, &clean_to)
            .expect("commit")
            .len()
            == 1
    );
    assert!(
        store
            .plan_rebind_roots(&clean_from, &clean_to)
            .expect("nothing left under clean_from")
            .is_empty()
    );
}

#[test]
fn ensure_folder_roots_materializes_reuses_and_honors_exclusions() {
    // §3 client-driven auto-materialization: per-root outcomes, anchored
    // reuse (§9.9), exclusion skip without an outcome, and per-root failure
    // isolation.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let folder = abs("browse-folder");

    // Created: named after the directory basename, anchored at the folder.
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect("ensure");
    match outcomes.as_slice() {
        [super::EnsureFolderOutcome::Created { project }] => {
            assert_eq!(project.name, "browse-folder");
            assert_eq!(project.origin.as_deref(), Some("folder"));
            assert_eq!(project.roots, vec![display(&folder)]);
        }
        other => panic!("expected Created, got {other:?}"),
    }

    // Anchored reuse (and idempotency): the same folder is Covered, never a
    // duplicate project.
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect("ensure again");
    assert!(
        matches!(
            outcomes.as_slice(),
            [super::EnsureFolderOutcome::Covered { .. }]
        ),
        "second run must be Covered, got {outcomes:?}"
    );
    assert_eq!(store.list().len(), 1);

    // A manual project that merely REFERENCES a folder does not anchor it
    // (§9.9): the browse channel materializes its own same-named project —
    // legal cross-project overlap.
    let referenced = abs("referenced");
    create(&store, "手动引用", std::slice::from_ref(&referenced));
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&referenced))
        .expect("ensure referenced");
    assert!(
        matches!(
            outcomes.as_slice(),
            [super::EnsureFolderOutcome::Created { .. }]
        ),
        "referencing does not anchor; got {outcomes:?}"
    );

    // Exclusion (§3): a banned folder is skipped WITHOUT an outcome — the
    // browse channel can tell exclusion apart from failure.
    store.set_never_materialize(&folder, true).expect("exclude");
    let outcomes = store
        .ensure_folder_roots(&[folder.clone(), abs("fresh-tail")])
        .expect("ensure with excluded root");
    assert_eq!(outcomes.len(), 1, "excluded roots produce no outcome");
    assert!(matches!(
        &outcomes[0],
        super::EnsureFolderOutcome::Created { .. }
    ));

    // Failure isolation: a relative path fails alone and never blocks the
    // rest of the batch.
    let outcomes = store
        .ensure_folder_roots(&[PathBuf::from("relative/path"), abs("after-failure")])
        .expect("ensure batch");
    assert_eq!(outcomes.len(), 2);
    assert!(matches!(
        &outcomes[0],
        super::EnsureFolderOutcome::Failed { .. }
    ));
    assert!(matches!(
        &outcomes[1],
        super::EnsureFolderOutcome::Created { .. }
    ));
}

#[test]
fn set_never_materialize_is_idempotent_and_only_gates_future_ensure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let folder = abs("excluded-folder");
    // Materialize via ensure (not a manual create): anchored reuse in the
    // revoked step requires an origin=folder project.
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect("ensure");
    assert!(matches!(
        outcomes.as_slice(),
        [super::EnsureFolderOutcome::Created { .. }]
    ));

    // Idempotent registration; the list is exposed for the manage panel.
    let registered = store.set_never_materialize(&folder, true).expect("set");
    assert_eq!(registered.len(), 1);
    assert_eq!(
        store
            .set_never_materialize(&folder, true)
            .expect("set again"),
        registered,
        "re-adding must not duplicate the entry"
    );
    // Existing projects and their assignments are untouched: the list gates
    // only FUTURE auto-materialization.
    assert_eq!(store.list().len(), 1);

    // Revocation re-opens the folder; a fresh ensure reuses the surviving
    // project (anchored) instead of materializing a second one.
    store.set_never_materialize(&folder, false).expect("revoke");
    assert!(store.never_materialize_roots().is_empty());
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect("ensure");
    assert!(matches!(
        outcomes.as_slice(),
        [super::EnsureFolderOutcome::Covered { .. }]
    ));
    assert_eq!(store.list().len(), 1);
}

#[test]
fn keychain_for_workspace_keeps_cwd_primary_and_project_order() {
    // §6: the primary slot is always the session's own cwd (display form);
    // additional roots are the caller's roots VERBATIM, in order, minus the
    // identity equal to the cwd. The store hands over stored DISPLAY forms,
    // so the test must too: on Windows hosts whose temp path carries an 8.3
    // short-name segment (GitHub runners: RUNNER~1), a raw spelling and the
    // display form fold to DIFFERENT identity keys and the cwd root would
    // survive into the keychain as a bogus additional root.
    let cwd = abs("k-cwd");
    let roots = vec![display(&abs("k-b")), display(&cwd), display(&abs("k-a"))];
    let keychain = ProjectStore::keychain_for_workspace(&cwd, &roots);
    assert_eq!(keychain.len(), 3);
    assert_eq!(keychain[0], display(&cwd));
    assert_eq!(keychain[1], roots[0]);
    assert_eq!(keychain[2], roots[2]);
    // Empty project roots → single-root semantics (cwd only).
    assert_eq!(
        ProjectStore::keychain_for_workspace(&cwd, &[]),
        vec![display(&cwd)]
    );
}

#[test]
fn forget_session_and_retain_sessions_prune_orphans() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "项目", &[]);
    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign s1");
    store
        .move_session_to_project("s2", None, None)
        .expect("explicit move out s2");

    assert!(store.forget_session("s1"));
    assert_eq!(store.assignment_of("s1"), None);
    assert!(!store.forget_session("unknown"), "unknown id is a no-op");

    // 启动对账:只保留仍存在的会话条目。
    store
        .move_session_to_project("s3", Some(&project.id), None)
        .expect("assign s3");
    let existing: HashSet<String> = ["s3".to_string()].into_iter().collect();
    let pruned = store.retain_sessions(&existing);
    assert_eq!(pruned, 1, "s2 的显式移出条目被剔除");
    assert_eq!(store.assignment_of("s2"), None);
    assert_eq!(store.assignment_of("s3"), Some(Some(project.id)));
}

#[test]
fn move_workspace_ancestor_collapses_descendant_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path().join("repo");
    let child = parent.join("sub");
    std::fs::create_dir_all(&child).expect("create dirs");

    let store = store_in(&temp);
    let project = create(&store, "项目", &[]);

    // 先挂子目录,再把父目录作为工作目录移入:祖先收编后代,组内不得出现嵌套。
    let child_outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&child))
        .expect("move with child root");
    assert_eq!(child_outcome.added_root, Some(display(&child)));
    let parent_outcome = store
        .move_session_to_project("s2", Some(&project.id), Some(&parent))
        .expect("move with ancestor root");
    assert_eq!(parent_outcome.added_root, Some(display(&parent)));
    let roots = store.get(&project.id).expect("project").roots;
    assert_eq!(roots.len(), 1, "descendant collapsed into the ancestor");
    assert_eq!(roots[0], display(&parent));

    // 反方向保持幂等:现有 root 是祖先时,子目录工作目录不重复添加。
    let nested_again = store
        .move_session_to_project("s3", Some(&project.id), Some(&child))
        .expect("covered workspace skips add");
    assert_eq!(nested_again.added_root, None);
    assert_eq!(store.get(&project.id).expect("project").roots.len(), 1);
}

#[test]
fn root_keys_fold_case_only_on_windows() {
    // Windows folds case/separators: two spellings of the same (absent)
    // directory are one identity root. Since the section-9.9 overlap
    // legalization a folded identity shared ACROSS projects is legal, so the
    // fold contract is locked by the intra-set duplicate rejection instead:
    // the same two spellings inside one create call are one root twice. Unix
    // filesystems are case-sensitive: the two spellings are distinct roots,
    // legal even in one root set.
    // Branches on the std::env::consts::OS constant rather than cfg syntax:
    // platform conditional compilation must not appear outside the adapter
    // layer (architecture-guard rust_target_cfg_outside_adapter).
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    if std::env::consts::OS == "windows" {
        let upper = abs("CaseProbe").to_string_lossy().replace('/', "\\");
        let lower = abs("caseprobe").to_string_lossy().replace('/', "\\");
        assert_ne!(upper, lower);

        // Cross-project identity equality is legal since section-9.9
        // (anchored coverage and the browse channel both rely on it).
        create(&store, "大写", &[PathBuf::from(&upper)]);
        create(&store, "小写", &[PathBuf::from(&lower)]);
        assert_eq!(store.list().len(), 2);

        // The fold itself is what the intra-set duplicate check exercises.
        let error = store
            .create_project(
                "组内重复".to_string(),
                vec![PathBuf::from(&upper), PathBuf::from(&lower)],
            )
            .expect_err("case-folded intra-set duplicate rejected");
        assert!(error.to_string().contains("duplicate project root"));
    } else {
        create(&store, "大写", &[abs("CaseProbe")]);
        create(&store, "小写", &[abs("caseprobe")]);
        assert_eq!(store.list().len(), 2);
    }
}

#[test]
fn rebind_roots_translates_the_remembered_primary_folder() {
    // §9.2 + review #484 round-3 M1: the remembered primary folder moves
    // with its directory — stranding a `/from`-spelled memory would break
    // "new conversation" on every later project-row entry with a
    // nonexistent-path rejection, minted by the very command that repairs
    // broken links. A primary OUTSIDE the prefix stays.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("mem-from");
    let outside_primary = abs("mem-outside");
    let to = temp.path().join("mem-to");
    std::fs::create_dir_all(&to).expect("create to");

    let project = create(&store, "搬家", std::slice::from_ref(&from));
    store
        .set_last_primary_root(&project.id, &from)
        .expect("remember primary under from");
    let other = create(&store, "别的项目", std::slice::from_ref(&outside_primary));
    store
        .set_last_primary_root(&other.id, &outside_primary)
        .expect("remember primary outside from");

    store
        .rebind_roots(&from, &to)
        .expect("rebind translates the primary");
    let moved = store.get(&project.id).unwrap();
    assert_eq!(moved.roots, vec![display(&to)]);
    assert_eq!(
        moved.last_primary_root.as_deref(),
        Some(display(&to).as_path()),
        "the remembered primary is translated, not stranded"
    );
    let untouched = store.get(&other.id).unwrap();
    assert_eq!(
        untouched.last_primary_root.as_deref(),
        Some(display(&outside_primary).as_path()),
        "a primary outside the prefix stays"
    );
}

#[test]
fn move_ancestor_absorb_demotes_a_covered_primary() {
    // Review #484 round-3 M1: when add_workspace_root absorbs covered
    // descendants, a remembered primary that was one of them must be
    // demoted (the channel falls back to the first roots entry) instead of
    // stranding a path the project no longer claims.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let parent = temp.path().join("ancestor");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).expect("create dirs");

    let project = create(&store, "目标", std::slice::from_ref(&child));
    store
        .set_last_primary_root(&project.id, &child)
        .expect("remember the child as primary");

    // Adding the workspace = the parent: the covered child is absorbed and
    // the remembered primary demoted. The store layer writes assignments
    // without loading sessions (the command layer owns the existence check),
    // so a synthetic id is enough here.
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&parent))
        .expect("move with ancestor add");
    assert_eq!(outcome.added_root, Some(display(&parent)));
    let updated = store.get(&project.id).unwrap();
    assert_eq!(updated.roots, vec![display(&parent)]);
    assert_eq!(
        updated.last_primary_root, None,
        "the covered primary is demoted, not stranded"
    );
}

#[test]
fn rebind_roots_display_form_preserves_nested_suffix() {
    // Display-form `from` (the documented IPC contract): a root nested one
    // level below `from` moves to `to`/suffix with its original casing kept.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("nested-from");
    let to = temp.path().join("nested-to");
    std::fs::create_dir_all(&to).expect("create to dir");

    let project = create(&store, "嵌套搬家", &[from.join("Sub")]);
    let affected = store.rebind_roots(&from, &to).expect("rebind nested root");
    assert_eq!(affected, vec![project.id.clone()]);
    assert_eq!(
        store.get(&project.id).unwrap().roots,
        vec![display(&to).join("Sub")],
        "nested suffix survives with original casing"
    );

    // A `from` that matches no stored root is an explicit no-op, not a
    // partial rewrite: state stays untouched and nothing is persisted.
    let missing = abs("never-stored");
    assert!(
        store.rebind_roots(&missing, &to).unwrap().is_empty(),
        "unknown from must be an empty Ok (idempotent retry contract)"
    );
    assert_eq!(store.list().len(), 1);
}

#[cfg(unix)]
#[test]
fn rebind_roots_via_symlink_alias_cuts_suffix_by_resolved_depth() {
    // review #463 B1 regression: `from` reaches the store through a symlinked
    // ancestor (macOS /var → /private/var). The alias resolves one component
    // DEEPER than its raw spelling; matching runs in the resolved domain, so
    // cutting the suffix by the raw argument's component count would keep one
    // component too many and rewrite the root to <to>/proj/sub instead of
    // <to>/sub (while the codex/session lanes, matching lexically, would not
    // match at all — a cross-store half-migration).
    let temp = tempfile::tempdir().expect("tempdir");
    let deep = temp.path().join("real").join("deep");
    std::fs::create_dir_all(&deep).expect("create deep dir");
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&deep, &alias).expect("create symlink");
    // Vanished leaf below the alias: resolution goes through the deepest
    // existing ancestor (the symlink), landing one level deeper than the
    // raw form.
    let from = alias.join("proj");
    let to = temp.path().join("moved");
    std::fs::create_dir_all(&to).expect("create to dir");

    let store = store_in(&temp);
    // Stored root in canonical (real) form, nested one level below `from`.
    let project = create(&store, "别名", &[deep.join("proj").join("sub")]);

    let affected = store.rebind_roots(&from, &to).expect("rebind via alias");
    assert_eq!(affected, vec![project.id.clone()]);
    assert_eq!(
        store.get(&project.id).unwrap().roots,
        vec![display(&to).join("sub")],
        "suffix cut by resolved depth: no extra component survives"
    );
}

#[test]
fn begin_rebind_rejects_concurrent_rebind_and_releases_on_drop() {
    // Minor 10 fence: check-and-set is atomic; the RAII token's Drop is the
    // only release point, so error paths cannot leave a permanently closed
    // gate.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let gate = store.begin_rebind().expect("first gate acquired");
    let rejected = store
        .begin_rebind()
        .err()
        .expect("second rebind rejected while the gate is held");
    assert!(
        rejected.starts_with("REBIND_IN_PROGRESS"),
        "typed marker follows the stable-prefix convention"
    );
    drop(gate);
    let _gate = store.begin_rebind().expect("gate released on drop");
}

/// §9.9 backend authority (review #484 round-6): tier-② multi-hit resolution
/// must follow the (position, id) order the store loads in — the frontend
/// `matchProjectByPath` pins the same rule, and `align_session_to_project`
/// resolves through this store method. The fixture lists projects in an order
/// that disagrees with both rules, so a resolution trusting file order alone
/// cannot pass.
#[test]
fn resolve_session_project_multi_hit_follows_position_then_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = abs("overlap-ws");
    // The store hands pure functions stored display forms; the fixture must
    // spell roots the same way. A raw temp spelling folds to a different
    // identity key than the ancestor-resolved display form on hosts whose
    // temp path carries an 8.3 short-name segment (GitHub Windows runners:
    // RUNNER~1) or a symlinked /var (macOS), so the tier-2 lookup misses and
    // resolve returns None there while passing on Linux.
    let root = display(&workspace).to_string_lossy().to_string();
    let now = "2026-09-01T00:00:00Z";
    let project = |id: &str, position: i64| {
        serde_json::json!({
            "id": id,
            "name": id,
            "roots": [root],
            "position": position,
            "created_at": now,
            "updated_at": now,
        })
    };
    // File order: highest position first, then the id-order loser of the
    // position-0 tie — trusting file order picks prj-b-high; skipping the id
    // tiebreak leaves the position-0 pair ambiguous.
    let file = serde_json::json!({
        "schema_version": 1,
        "projects": [
            project("prj-b-high", 1),
            project("prj-zz", 0),
            project("prj-aa", 0),
        ],
        "assignments": {},
        "never_materialize_roots": [],
    });
    std::fs::write(
        temp.path().join("projects.json"),
        serde_json::to_vec(&file).expect("serialize fixture"),
    )
    .expect("write fixture");
    let store = store_in(&temp);

    let resolved = store
        .resolve_session_project("s1", &workspace)
        .expect("multi-hit workspace resolves");
    assert_eq!(
        resolved.id, "prj-aa",
        "smallest position wins; the position tie breaks by id order"
    );

    // tier-① beats tier-②: an explicit assignment overrides the position rule.
    let assigned = serde_json::json!({
        "schema_version": 1,
        "projects": file["projects"].clone(),
        "assignments": { "s1": "prj-b-high" },
        "never_materialize_roots": [],
    });
    std::fs::write(
        temp.path().join("projects.json"),
        serde_json::to_vec(&assigned).expect("serialize fixture"),
    )
    .expect("rewrite fixture");
    let store = store_in(&temp);
    let resolved = store
        .resolve_session_project("s1", &workspace)
        .expect("explicit assignment resolves");
    assert_eq!(
        resolved.id, "prj-b-high",
        "explicit assignment beats the position rule"
    );
}

#[test]
fn update_project_and_expel_writes_tombstones_and_keeps_existing_entries() {
    // §4 root-removal semantics: expel ids WITHOUT an assignment entry become
    // explicit move-outs (None) in the same persist as the roots replacement;
    // ids with existing entries (explicit Some / already moved out) stay
    // untouched — tier-① wins over the expulsion enumeration.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "项目", &[abs("u-old"), abs("u-keep")]);
    let other = create(&store, "他处", &[abs("u-elsewhere")]);
    store
        .move_session_to_project("s-keep", Some(&other.id), None)
        .expect("assign elsewhere");

    let updated = store
        .update_project_and_expel(
            &project.id,
            Some("改名".to_string()),
            vec![abs("u-keep")],
            &[
                "s-auto".to_string(),
                "s-keep".to_string(),
                "s-auto".to_string(), // duplicates in the enumeration are harmless
            ],
        )
        .expect("update and expel");
    assert_eq!(updated.name, "改名");
    assert_eq!(updated.roots, vec![display(&abs("u-keep"))]);
    assert_eq!(
        store.assignment_of("s-auto"),
        Some(None),
        "entry-less expel id becomes a tombstone"
    );
    assert_eq!(
        store.assignment_of("s-keep"),
        Some(Some(other.id.clone())),
        "an existing explicit assignment is not rewritten"
    );
}

/// review #484 M3: update_project_and_expel persists BEFORE committing the
/// in-memory state. A persist failure must leave memory identical to disk so
/// an in-process retry recomputes the same removed set and expels — the
/// previous commit-first order let the retry see the NEW roots in memory,
/// compute removed=[], and skip the expulsion forever. Same obstruction idiom
/// as rebind_roots_rolls_back_memory_when_persist_fails.
#[test]
fn update_project_and_expel_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "搬家", &[abs("e-old")]);
    let before = store.get(&project.id).unwrap();

    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    let error = store
        .update_project_and_expel(
            &project.id,
            Some("新名".to_string()),
            vec![abs("e-new")],
            &["s1".to_string()],
        )
        .expect_err("persist failure surfaces as an error");
    assert!(!error.to_string().is_empty());
    assert_eq!(
        store.get(&project.id).unwrap(),
        before,
        "roots AND name roll back to the on-disk state"
    );
    assert_eq!(
        store.assignment_of("s1"),
        None,
        "the expel tombstone is not committed either"
    );

    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let updated = store
        .update_project_and_expel(
            &project.id,
            Some("新名".to_string()),
            vec![abs("e-new")],
            &["s1".to_string()],
        )
        .expect("retry persists");
    assert_eq!(updated.roots, vec![display(&abs("e-new"))]);
    assert_eq!(store.assignment_of("s1"), Some(None), "the retry expels");
}

#[test]
fn delete_project_expel_ids_become_tombstones_and_existing_entries_survive() {
    // The expel_session_ids path of delete_project (previously untested — all
    // callers in the suite passed &[]): command-enumerated auto members under
    // the deleted project's roots are written as explicit move-outs so the
    // folder's next materialization cannot revive them; ids that already have
    // an entry (explicit elsewhere / already moved out) are skipped.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "待删", &[abs("d-x")]);
    let other = create(&store, "幸存", &[abs("d-y")]);
    store
        .move_session_to_project("s-member", Some(&project.id), None)
        .expect("assign member");
    store
        .move_session_to_project("s-elsewhere", Some(&other.id), None)
        .expect("assign elsewhere");
    store
        .move_session_to_project("s-out", None, None)
        .expect("explicit move out");

    let report = store
        .delete_project(
            &project.id,
            &[
                "s-auto".to_string(),      // entry-less auto member: tombstone
                "s-elsewhere".to_string(), // explicit elsewhere: skipped
                "s-out".to_string(),       // already moved out: skipped
                "s-member".to_string(),    // already an explicit member: no dup
            ],
        )
        .expect("delete");
    let mut affected = report.affected_session_ids;
    affected.sort();
    assert_eq!(
        affected,
        vec!["s-auto".to_string(), "s-member".to_string()],
        "explicit members plus entry-less expel ids, no duplicates"
    );
    assert_eq!(store.assignment_of("s-auto"), Some(None));
    assert_eq!(store.assignment_of("s-member"), Some(None));
    assert_eq!(
        store.assignment_of("s-elsewhere"),
        Some(Some(other.id.clone())),
        "explicit assignment to a surviving project is untouched"
    );
    assert_eq!(
        store.assignment_of("s-out"),
        Some(None),
        "pre-existing move-out stays (and was not double-reported)"
    );
}

/// review #484 round-5 M3: delete_project persists BEFORE committing the
/// in-memory state. A persist failure must leave memory identical to disk —
/// the project still listed, no tombstones written — so an in-process retry
/// succeeds; the previous commit-first order dropped the project from memory
/// and the retry failed with "project not found". Same obstruction idiom as
/// update_project_and_expel_rolls_back_memory_when_persist_fails.
#[test]
fn delete_project_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "待删", &[abs("pd-x")]);
    store
        .move_session_to_project("s-member", Some(&project.id), None)
        .expect("assign member");

    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    store
        .delete_project(&project.id, &["s-auto".to_string()])
        .expect_err("persist failure surfaces as an error");
    assert!(
        store.get(&project.id).is_some(),
        "memory rolls back: the project is still there"
    );
    assert_eq!(
        store.assignment_of("s-member"),
        Some(Some(project.id.clone())),
        "explicit membership is untouched"
    );
    assert_eq!(
        store.assignment_of("s-auto"),
        None,
        "no expel tombstone is committed"
    );

    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let report = store
        .delete_project(&project.id, &["s-auto".to_string()])
        .expect("retry deletes");
    assert!(store.get(&project.id).is_none());
    assert!(
        report
            .affected_session_ids
            .contains(&"s-member".to_string())
    );
    assert!(report.affected_session_ids.contains(&"s-auto".to_string()));
    assert_eq!(store.assignment_of("s-member"), Some(None));
    assert_eq!(store.assignment_of("s-auto"), Some(None));
}

/// review #484 round-8 m5: the never-materialize exclusion table moves with
/// its directories. A rebind used to translate project roots and the
/// remembered primary only, so a moved excluded folder re-materialized under
/// the NEW path on the next ensure — the ghost the user excluded came back,
/// minted by the very command that repairs broken links.
#[test]
fn rebind_roots_translates_never_materialize_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("rbx-excl-from");
    let to = temp.path().join("rbx-excl-moved");
    let nested = display(&from).join("keep-out");
    store
        .set_never_materialize(&nested, true)
        .expect("exclude nested folder");
    std::fs::create_dir_all(&to).expect("create to dir");

    let project = create(&store, "搬家排除", std::slice::from_ref(&from));
    let affected = store.rebind_roots(&from, &to).expect("rebind");
    assert_eq!(affected, vec![project.id.clone()]);

    // The table exposes folded identity keys, not display paths (Windows
    // folds case/separators): build the expectation through the same
    // platform folding, or the assertion only holds on POSIX.
    assert_eq!(
        store.never_materialize_roots(),
        vec![crate::platform::os::filesystem_path_identity_key(
            &display(&to).join("keep-out").to_string_lossy(),
        )],
        "the exclusion key must translate onto the new path"
    );

    // And the translated key still gates: ensure at the new path skips
    // silently instead of materializing the excluded folder's project.
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&to.join("keep-out")))
        .expect("ensure at the new path");
    assert!(
        outcomes.is_empty(),
        "the moved exclusion must still suppress ensure, got {outcomes:?}"
    );
    assert_eq!(
        store.list().len(),
        1,
        "no project materialized for the excluded folder; the rebind project remains"
    );
}

#[test]
fn load_dedupes_never_materialize_roots() {
    // review #484 n4: StoreState documents the exclusion table as deduplicated
    // (writes dedupe via set_never_materialize), but a hand-edited file could
    // carry duplicates; load re-pins the invariant.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"projects":[],"assignments":{},"never_materialize_roots":["/a","/b","/a","/c","/b"]}"#,
    )
    .expect("write file with duplicate keys");
    let store = ProjectStore::from_paths(path);
    assert_eq!(
        store.never_materialize_roots(),
        vec!["/a".to_string(), "/b".to_string(), "/c".to_string()],
        "duplicates collapse on load, first occurrence wins, order kept"
    );
}

/// review #484 round-8 M1: set_never_materialize persists BEFORE committing
/// the in-memory state. Its idempotent early-return reads that same memory,
/// so a commit-first persist failure would make every in-process retry
/// return `Ok` over a disk still missing the entry — the exclusion existed
/// only in memory and silently vanished on restart. Same obstruction idiom
/// as update_project_and_expel_rolls_back_memory_when_persist_fails.
#[test]
fn set_never_materialize_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let folder = abs("nm-fail-folder");
    // 一次成功落盘,让 store 文件存在(阻塞手段要把最终路径换成目录)。
    create(&store, "占位", &[abs("nm-fail-anchor")]);

    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    store
        .set_never_materialize(&folder, true)
        .expect_err("persist failure surfaces as an error");
    assert!(
        store.never_materialize_roots().is_empty(),
        "memory must roll back to the on-disk (empty) exclusion table"
    );

    // The retry must actually write, not false-succeed through the
    // idempotent early-return over advanced memory.
    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let registered = store
        .set_never_materialize(&folder, true)
        .expect("retry persists");
    assert_eq!(registered.len(), 1);
    assert_eq!(store.never_materialize_roots(), registered);
}

/// review #484 round-8 M1: set_last_primary_root persists BEFORE committing
/// the in-memory state; the idempotent early-return reads the same memory, so
/// a commit-first persist failure let the retry return the remembered root
/// from memory without ever writing it — lost on restart. Same obstruction
/// idiom as update_project_and_expel_rolls_back_memory_when_persist_fails.
#[test]
fn set_last_primary_root_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let root = abs("primary-fail-root");
    let project = create(&store, "记忆", std::slice::from_ref(&root));

    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    store
        .set_last_primary_root(&project.id, &root)
        .expect_err("persist failure surfaces as an error");
    assert_eq!(
        store.get(&project.id).unwrap().last_primary_root,
        None,
        "memory must roll back: no remembered primary in memory either"
    );

    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let updated = store
        .set_last_primary_root(&project.id, &root)
        .expect("retry persists");
    assert_eq!(updated.last_primary_root, Some(display(&root)));
    assert_eq!(
        store.get(&project.id).unwrap().last_primary_root,
        Some(display(&root))
    );
}

/// review #484 round-8 M1: ensure_folder_roots persists BEFORE committing
/// the batch. The previous order pushed created projects into live memory
/// before the write, so a failed persist let the retry hit the
/// anchored-reuse branch and report `Covered` for a project that existed
/// only in memory — a session's tier-① assignment then pointed at a project
/// id that vanished on restart, blocking tier-② re-adoption. Same
/// obstruction idiom as update_project_and_expel_rolls_back_memory_when_persist_fails.
#[test]
fn ensure_folder_roots_rolls_back_memory_when_persist_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let folder = abs("ensure-fail-folder");
    // 一次成功落盘,让 store 文件存在(阻塞手段要把最终路径换成目录)。
    create(&store, "占位", &[abs("ensure-fail-anchor")]);

    let store_path = temp.path().join("projects.json");
    std::fs::remove_file(&store_path).expect("remove store file");
    std::fs::create_dir_all(&store_path).expect("recreate as dir");
    std::fs::write(store_path.join("obstruction"), b"x").expect("make dir non-empty");

    store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect_err("persist failure surfaces as an error");
    assert_eq!(
        store.list().len(),
        1,
        "memory must roll back: only the pre-existing anchor project remains"
    );

    // The retry re-materializes from the on-disk state and converges:
    // it must report Created again — a Covered here would mean the previous
    // batch leaked into memory (the defect this test pins).
    std::fs::remove_dir_all(&store_path).expect("clear obstruction");
    let outcomes = store
        .ensure_folder_roots(std::slice::from_ref(&folder))
        .expect("retry persists");
    let super::EnsureFolderOutcome::Created { project } = &outcomes[0] else {
        panic!("retry must re-create from rolled-back memory, got {outcomes:?}");
    };
    assert_eq!(store.list().len(), 2);
    assert!(store.list().iter().any(|entry| entry.id == project.id));
}
