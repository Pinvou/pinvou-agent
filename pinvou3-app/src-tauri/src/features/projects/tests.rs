//! ProjectStore 行为测试。全部走 `from_paths` + 临时目录,不触进程全局
//! `PINVOU3_HOME`。

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
fn create_rejects_overlap_with_other_projects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    create(&store, "已有项目", &[abs("work")]);

    let same = store
        .create_project("同路径".to_string(), vec![abs("work")])
        .expect_err("same root across projects rejected");
    assert!(same.to_string().contains("overlaps project '已有项目'"));

    let nested = store
        .create_project("子路径".to_string(), vec![abs("work").join("sub")])
        .expect_err("nested root across projects rejected");
    assert!(nested.to_string().contains("overlaps project"));

    // 无关路径不受影响;父路径方向同样拦截。
    create(&store, "无关", &[abs("other")]);
    let parent = store
        .create_project(
            "父路径".to_string(),
            vec![abs("work").parent().unwrap().to_path_buf()],
        )
        .expect_err("parent root across projects rejected");
    assert!(parent.to_string().contains("overlaps project"));
}

#[test]
fn canonicalized_real_dirs_catch_overlap_across_projects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path().join("repo");
    let child = parent.join("sub");
    std::fs::create_dir_all(&child).expect("create dirs");

    let store = store_in(&temp);
    create(&store, "父", std::slice::from_ref(&parent));
    let error = store
        .create_project("子".to_string(), vec![child])
        .expect_err("canonical overlap rejected");
    assert!(error.to_string().contains("overlaps project"));
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
    assert_eq!(updated.roots, vec![abs("new")]);
    assert_eq!(store.get(&project.id).unwrap().name, "新名");

    // None = 保持不变;空 roots 合法(项目退化为纯标签)。
    let kept = store
        .update_project(&project.id, None, None)
        .expect("no-op update");
    assert_eq!(kept.roots, vec![abs("new")]);
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
fn delete_expels_all_members_to_ungrouped() {
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

    // 删除时命令层枚举的自动归组成员(s5:无归属条目)一并传入。
    let report = store
        .delete_project(&project.id, &["s5".to_string()])
        .expect("delete project");
    let mut affected = report.affected_session_ids;
    affected.sort();
    assert_eq!(affected, vec!["s1", "s2", "s5"]);

    // 显式成员与自动成员一律写成显式移出:留在未分组,不随该文件夹下一次
    // 自动物化复活。
    assert_eq!(store.assignment_of("s1"), Some(None));
    assert_eq!(store.assignment_of("s2"), Some(None));
    assert_eq!(store.assignment_of("s5"), Some(None));
    // 既有条目不改写:s3 的移出条目保留,s4 的显式归属幸存。
    assert_eq!(store.assignment_of("s3"), Some(None));
    assert_eq!(store.assignment_of("s4"), Some(Some(other.id.clone())));
    assert!(store.get(&project.id).is_none());

    // 幸存项目删除(无成员)后归属表仍有移出条目,文件保留;条目全部退场
    // (会话删除钩子)后才回落空状态删文件。
    store.delete_project(&other.id, &[]).expect("delete other");
    assert!(temp.path().join("projects.json").exists());
    for session_id in ["s1", "s2", "s3", "s4", "s5"] {
        store.forget_session(session_id);
    }
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
    let canonical = workspace.canonicalize().expect("canonicalize");

    let store = store_in(&temp);
    let project = create(&store, "目标", &[abs("elsewhere")]);
    let other = create(&store, "他人领地", std::slice::from_ref(&foreign));

    // 顺带加 root:归并与加目录一次落盘。
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&workspace))
        .expect("move with workspace root");
    assert_eq!(outcome.project_id, Some(project.id.clone()));
    assert_eq!(outcome.added_root, Some(canonical.clone()));
    assert!(store.get(&project.id).unwrap().roots.contains(&canonical));

    // 已被现有 root 覆盖时幂等跳过,不重复添加。
    let again = store
        .move_session_to_project("s2", Some(&project.id), Some(&workspace.join("deep")))
        .expect("covered workspace skips add");
    assert_eq!(again.added_root, None);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 2);

    // 落在他人领地内的目录必须拦截,且不得污染两个项目。
    let conflict = store
        .move_session_to_project("s3", Some(&project.id), Some(&foreign.join("deeper")))
        .map(|_| ())
        .expect_err("cross-project overlap rejected");
    assert!(conflict.to_string().contains("overlaps project '他人领地'"));
    assert_eq!(store.assignment_of("s3"), None);
    assert_eq!(store.get(&other.id).unwrap().roots.len(), 1);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 2);
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
    assert!(roots.contains(&to.canonicalize().unwrap()));
    assert!(roots.contains(&abs("untouched")), "prefix 外的 root 不动");
    assert_eq!(store.get(&other.id).unwrap().roots, vec![abs("elsewhere")]);

    // 幂等:from 前缀已无命中,再跑为空操作。
    assert!(store.rebind_roots(&from, &to).unwrap().is_empty());
    assert_eq!(store.get(&project.id).unwrap().roots, roots);
}

#[test]
fn rebind_roots_rejects_overlap_and_keeps_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("from2");
    let occupied = temp.path().join("occupied");
    std::fs::create_dir_all(&occupied).expect("create occupied dir");

    let project = create(&store, "待搬", std::slice::from_ref(&from));
    create(&store, "已有领地", std::slice::from_ref(&occupied));

    let before = store.get(&project.id).unwrap();
    let error = store
        .rebind_roots(&from, &occupied)
        .expect_err("overlap after rebind rejected");
    assert!(error.to_string().contains("overlap"));
    // 报错回滚:内存态未变(未落盘)。
    assert_eq!(store.get(&project.id).unwrap(), before);
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
    assert_eq!(
        child_outcome.added_root,
        Some(child.canonicalize().expect("canon child"))
    );
    let parent_outcome = store
        .move_session_to_project("s2", Some(&project.id), Some(&parent))
        .expect("move with ancestor root");
    assert_eq!(
        parent_outcome.added_root,
        Some(parent.canonicalize().expect("canon parent"))
    );
    let roots = store.get(&project.id).expect("project").roots;
    assert_eq!(roots.len(), 1, "descendant collapsed into the ancestor");
    assert_eq!(roots[0], parent.canonicalize().expect("canon parent"));

    // 反方向保持幂等:现有 root 是祖先时,子目录工作目录不重复添加。
    let nested_again = store
        .move_session_to_project("s3", Some(&project.id), Some(&child))
        .expect("covered workspace skips add");
    assert_eq!(nested_again.added_root, None);
    assert_eq!(store.get(&project.id).expect("project").roots.len(), 1);
}

#[test]
fn root_keys_fold_case_only_on_windows() {
    // Windows 大小写不敏感:同一(不存在的)目录的两种大小写/分隔符写法必须
    // 折叠为同一 root,否则重叠校验对 `C:\Work` vs `c:\work` 失明;
    // Unix 文件系统大小写敏感,两种写法是两个互不重叠的 root,都允许创建。
    // 用 std::env::consts::OS 常量分支而非 cfg 语法:平台条件编译不得出现在
    // 适配层外(architecture-guard rust_target_cfg_outside_adapter)。
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    if std::env::consts::OS == "windows" {
        let upper = abs("CaseProbe").to_string_lossy().replace('/', "\\");
        let lower = abs("caseprobe").to_string_lossy().replace('/', "\\");
        assert_ne!(upper, lower);

        create(&store, "大写", &[PathBuf::from(&upper)]);
        let error = store
            .create_project("小写".to_string(), vec![PathBuf::from(&lower)])
            .expect_err("case-folded duplicate root rejected");
        assert!(error.to_string().contains("overlaps project"));
    } else {
        create(&store, "大写", &[abs("CaseProbe")]);
        create(&store, "小写", &[abs("caseprobe")]);
        assert_eq!(store.list().len(), 2);
    }
}

// ── 文件夹项目自动物化(ensure)──────────────────────────────────────────────

fn ensure(store: &ProjectStore, roots: &[PathBuf]) -> Vec<super::EnsureFolderOutcome> {
    store.ensure_folder_roots(roots).expect("ensure folder roots")
}

#[test]
fn ensure_creates_basename_named_folder_projects_idempotently() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    let outcomes = ensure(&store, &[abs("web"), abs("api")]);
    assert!(matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "web" && project.origin.as_deref() == Some("folder")));
    assert!(matches!(&outcomes[1], super::EnsureFolderOutcome::Created { project }
        if project.name == "api"));
    assert_eq!(store.list().len(), 2);

    // 幂等:同批根重放 → 全部 Covered,不新建。
    let replay = ensure(&store, &[abs("web"), abs("api")]);
    let ids: Vec<&str> = replay
        .iter()
        .filter_map(|o| match o {
            super::EnsureFolderOutcome::Covered { project_id } => Some(project_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "重放全为 Covered: {replay:?}");
    assert_eq!(store.list().len(), 2);

    // 既有手工项目已覆盖的文件夹同样只复用,不产生第二个项目。
    let manual = create(&store, "手工", &[abs("manual/root")]);
    let covered = ensure(&store, &[abs("manual/root/sub")]);
    assert!(matches!(&covered[0], super::EnsureFolderOutcome::Covered { project_id }
        if *project_id == manual.id));
    assert_eq!(store.list().len(), 3);
}

#[test]
fn ensure_input_dedupes_and_reports_relative_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    let outcomes = ensure(&store, &[abs("web"), abs("web"), PathBuf::from("relative/x")]);
    assert_eq!(outcomes.len(), 2, "重复根折叠为一项: {outcomes:?}");
    assert!(matches!(outcomes[0], super::EnsureFolderOutcome::Created { .. }));
    assert!(matches!(&outcomes[1], super::EnsureFolderOutcome::Failed { reason }
        if reason.contains("absolute")));
    assert_eq!(store.list().len(), 1);
}

#[test]
fn ensure_conflicts_report_per_root_without_blocking_the_batch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // 既有项目占据 abs("nest/child"):为祖先目录 abs("nest") 建文件夹项目会
    // 与之嵌套,该根 Failed,同批其它根照常创建。
    create(&store, "深根", &[abs("nest/child")]);

    let outcomes = ensure(&store, &[abs("nest"), abs("clean")]);
    assert!(matches!(&outcomes[0], super::EnsureFolderOutcome::Failed { reason }
        if reason.contains("overlaps")));
    assert!(matches!(outcomes[1], super::EnsureFolderOutcome::Created { .. }));
    assert_eq!(store.list().len(), 2);
}

#[test]
fn ensure_recreates_folder_project_after_delete_for_new_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    // 首次物化 → 删除(成员写成显式移出)。
    ensure(&store, &[abs("web")]);
    let created = store.list()[0].clone();
    store
        .delete_project(&created.id, &["s-old".to_string()])
        .expect("delete");
    assert_eq!(store.assignment_of("s-old"), Some(None), "成员留未分组");

    // 无墓碑:同一文件夹再次 ensure 即重建(前端由"新会话"驱动触发;旧会话
    // 的移出条目压住 tier-②,重建项目只收新会话)。
    let recreate = ensure(&store, &[abs("web")]);
    assert!(matches!(&recreate[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "web" && project.origin.as_deref() == Some("folder")));
    // 移出条目跨删除保留:旧会话不因重建复活。
    assert_eq!(store.assignment_of("s-old"), Some(None));
}

#[test]
fn origin_and_expelled_assignments_persist_across_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    {
        let store = store_in(&temp);
        ensure(&store, &[abs("web"), abs("api")]);
        let web = store
            .list()
            .iter()
            .find(|project| project.name == "web")
            .cloned()
            .expect("web project");
        store
            .delete_project(&web.id, &["s-old".to_string()])
            .expect("delete");
    }
    let reopened = store_in(&temp);
    let projects = reopened.list();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].origin.as_deref(), Some("folder"));
    assert_eq!(
        reopened.assignment_of("s-old"),
        Some(None),
        "移出条目跨进程存活:旧会话不随重建复活"
    );
}
