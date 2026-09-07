//! ProjectStore 行为测试。全部走 `from_paths` + 临时目录,不触进程全局
//! `PINVOU3_HOME`。

use std::collections::HashSet;
use std::path::PathBuf;

use super::ProjectStore;

fn store_in(temp: &tempfile::TempDir) -> ProjectStore {
    ProjectStore::from_paths(temp.path().join("projects.json"))
}

/// 不真实存在的绝对路径:canonicalize 失败走词法绝对化,键仍可判定。
fn abs(name: &str) -> PathBuf {
    PathBuf::from("/pinvou3-projects-test-root").join(name)
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
    create(&store, "父", &[parent.clone()]);
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

    let report = store.delete_project(&project.id).expect("delete project");
    let mut affected = report.affected_session_ids;
    affected.sort();
    assert_eq!(affected, vec!["s1", "s2"]);

    assert_eq!(store.assignment_of("s1"), None);
    assert_eq!(store.assignment_of("s2"), None);
    // s3 的显式移出条目保留,不被删除项目连带清理。
    assert_eq!(store.assignment_of("s3"), Some(None));
    assert_eq!(store.assignment_of("s4"), Some(Some(other.id.clone())));
    assert!(store.get(&project.id).is_none());

    // 全部项目删除 + 归属清空后,空状态不留文件。
    store
        .move_session_to_project("s3", None, None)
        .expect("re-move s3");
    store.delete_project(&other.id).expect("delete other");
    store.forget_session("s3");
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
    let other = create(&store, "他人领地", &[foreign.clone()]);

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
fn newer_schema_version_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    std::fs::write(
        &path,
        r#"{"schema_version": 99, "projects": [], "assignments": {}}"#,
    )
    .expect("write future schema");

    let store = ProjectStore::from_paths(path);
    assert!(store.list().is_empty(), "future schema degrades to empty");
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
