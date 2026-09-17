//! ProjectStore behavior tests. Everything goes through `from_paths` + temp
//! directories, never touching the process-global `PINVOU3_HOME`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::ProjectStore;

fn store_in(temp: &tempfile::TempDir) -> ProjectStore {
    ProjectStore::from_paths(temp.path().join("projects.json"))
}

/// Platform-neutral nonexistent absolute path: anchored under
/// std::env::temp_dir() so it also satisfies is_absolute() on Windows (the
/// previously hardcoded "/pinvou3-projects-test-root" got every create case
/// rejected by the absoluteness check on Windows, so the suite could only run
/// on Ubuntu). The path deliberately does not exist: canonicalize fails and
/// falls back to lexical absolutization, and identity keys remain decidable.
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
    let first = create(&store, "web", &[abs("web")]);
    let second = create(&store, "api", &[abs("api")]);

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
        .create_project("relative".to_string(), vec![PathBuf::from("repo")])
        .expect_err("relative root rejected");
    assert!(error.to_string().contains("must be absolute"));
}

#[test]
fn create_rejects_duplicate_and_nested_roots_within_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let duplicate = store
        .create_project("dup".to_string(), vec![abs("a"), abs("a")])
        .expect_err("duplicate root rejected");
    assert!(duplicate.to_string().contains("duplicate project root"));

    let nested = store
        .create_project("nested".to_string(), vec![abs("b"), abs("b").join("sub")])
        .expect_err("nested roots rejected");
    assert!(nested.to_string().contains("must not nest"));
}

#[test]
fn create_allows_overlap_across_projects() {
    // §9.9 root-overlap legalization: the same physical folder (or mutually
    // nested directories) may be referenced by multiple projects;
    // auto-grouping ambiguity is decided by the frontend via position (the
    // earliest one adopts), explicit assignment still wins. The backend no
    // longer enforces cross-project exclusion.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let first = create(&store, "existing", &[abs("work")]);

    let same = create(&store, "same-path", &[abs("work")]);
    let nested = create(&store, "sub-path", &[abs("work").join("sub")]);
    let parent = create(
        &store,
        "parent-path",
        &[abs("work").parent().unwrap().to_path_buf()],
    );
    create(&store, "unrelated", &[abs("other")]);

    let projects = store.list();
    assert_eq!(projects.len(), 5, "all coexist: {projects:?}");
    // Positions increase in order; the frontend tiebreak (when folders hit
    // multiple projects, the smallest position adopts) depends on this order.
    let positions: Vec<i64> = projects.iter().map(|project| project.position).collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(positions, sorted);
    assert_eq!(store.get(&first.id).unwrap().roots, vec![abs("work")]);
    assert_eq!(store.get(&same.id).unwrap().roots, vec![abs("work")]);
    assert_eq!(nested.roots, vec![abs("work").join("sub")]);
    assert_eq!(parent.roots, vec![abs("work").parent().unwrap()]);
}

#[test]
fn canonicalized_real_dirs_coexist_across_projects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path().join("repo");
    let child = parent.join("sub");
    std::fs::create_dir_all(&child).expect("create dirs");

    let store = store_in(&temp);
    create(&store, "parent", std::slice::from_ref(&parent));
    // Canonicalized before being stored as the display form; cross-project
    // nesting is no longer rejected.
    let child_project = create(&store, "child", std::slice::from_ref(&child));
    assert_eq!(
        child_project.roots,
        vec![child.canonicalize().unwrap()],
        "real directory canonicalized into the store"
    );
    assert_eq!(store.list().len(), 2);
}

#[test]
fn update_renames_and_replaces_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "old-name", &[abs("old")]);

    let updated = store
        .update_project(
            &project.id,
            Some("new-name".to_string()),
            Some(vec![abs("new")]),
        )
        .expect("update project");
    assert_eq!(updated.name, "new-name");
    assert_eq!(updated.roots, vec![abs("new")]);
    assert_eq!(store.get(&project.id).unwrap().name, "new-name");

    // None = keep as-is; empty roots are legal (the project degenerates into
    // a pure label).
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
    let project = create(&store, "to-delete", &[abs("x")]);
    let other = create(&store, "survivor", &[abs("y")]);

    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign s1");
    store
        .move_session_to_project("s2", Some(&project.id), None)
        .expect("assign s2");
    // s3 explicitly moved out: the semantic is "in no project", independent
    // of the target project's existence.
    store
        .move_session_to_project("s3", Some(&project.id), None)
        .expect("assign s3");
    store
        .move_session_to_project("s3", None, None)
        .expect("move s3 out");
    store
        .move_session_to_project("s4", Some(&other.id), None)
        .expect("assign s4");

    // The command layer's enumerated auto-grouped members (s5: no assignment
    // entry) are passed in at deletion time.
    let report = store
        .delete_project(&project.id, &["s5".to_string()])
        .expect("delete project");
    let mut affected = report.affected_session_ids;
    affected.sort();
    assert_eq!(affected, vec!["s1", "s2", "s5"]);

    // Explicit and auto members alike are written as explicit move-outs: they
    // stay in Ungrouped and do not revive with the folder's next
    // auto-materialization.
    assert_eq!(store.assignment_of("s1"), Some(None));
    assert_eq!(store.assignment_of("s2"), Some(None));
    assert_eq!(store.assignment_of("s5"), Some(None));
    // Existing entries are not rewritten: s3's move-out entry survives, s4's
    // explicit assignment survives.
    assert_eq!(store.assignment_of("s3"), Some(None));
    assert_eq!(store.assignment_of("s4"), Some(Some(other.id.clone())));
    assert!(store.get(&project.id).is_none());

    // After deleting the surviving (memberless) project the assignment table
    // still has move-out entries, so the file stays; only when every entry
    // has left (session delete hook) does it fall back to the empty state
    // and remove the file.
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
    let project = create(&store, "target", &[]);

    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign");
    assert_eq!(store.assignment_of("s1"), Some(Some(project.id.clone())));
    assert_eq!(store.assigned_session_ids(&project.id), vec!["s1"]);

    // Explicit move-out = entry exists and is None (as opposed to "no entry,
    // auto-grouping applies").
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
    // Foreign territory does not overlap the workspace, so the test does not
    // trip its own cross-project overlap rule.
    let foreign = temp.path().join("foreign").join("nested");
    std::fs::create_dir_all(&foreign).expect("create foreign dirs");
    let canonical = workspace.canonicalize().expect("canonicalize");

    let store = store_in(&temp);
    let project = create(&store, "target", &[abs("elsewhere")]);
    let other = create(&store, "foreign-territory", std::slice::from_ref(&foreign));

    // Adopt-on-move: assignment merge and folder add persist in one write.
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&workspace))
        .expect("move with workspace root");
    assert_eq!(outcome.project_id, Some(project.id.clone()));
    assert_eq!(outcome.added_root, Some(canonical.clone()));
    assert!(store.get(&project.id).unwrap().roots.contains(&canonical));

    // Idempotent skip when already covered by an existing root; not added
    // again.
    let again = store
        .move_session_to_project("s2", Some(&project.id), Some(&workspace.join("deep")))
        .expect("covered workspace skips add");
    assert_eq!(again.added_root, None);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 2);

    // A directory inside foreign territory is legal since §9.9 (overlap
    // legalization); both sides hold their own reference.
    let shared = store
        .move_session_to_project("s3", Some(&project.id), Some(&foreign.join("deeper")))
        .expect("cross-project overlap legal");
    // "deeper" does not exist: the stored form = nearest existing ancestor
    // canonicalized, with the missing suffix re-appended.
    assert_eq!(
        shared.added_root,
        Some(foreign.canonicalize().unwrap().join("deeper"))
    );
    assert!(
        store
            .get(&other.id)
            .unwrap()
            .roots
            .contains(&foreign.canonicalize().unwrap())
    );
    assert_eq!(store.assignment_of("s3"), Some(Some(project.id.clone())));
    assert_eq!(store.get(&other.id).unwrap().roots.len(), 1);
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 3);
}

#[test]
fn persist_roundtrip_preserves_state_on_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("projects.json");
    let project_id = {
        let store = ProjectStore::from_paths(path.clone());
        let project = create(&store, "persisted", &[abs("persist")]);
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
    // The first mutation after the corrupt read self-heals by overwriting.
    create(&store, "self-heal", &[]);
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

    // A downgraded process refuses every write: the empty state plus the
    // first mutation must not downgrade-overwrite the newer file.
    let error = store
        .create_project("downgrade-write".to_string(), vec![])
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

    let project = create(&store, "moved", &[from.clone(), abs("untouched")]);
    let other = create(&store, "unrelated", &[abs("elsewhere")]);

    let affected = store.rebind_roots(&from, &to).expect("rebind roots");
    assert_eq!(affected, vec![project.id.clone()]);
    let roots = store.get(&project.id).unwrap().roots;
    assert!(roots.contains(&to.canonicalize().unwrap()));
    assert!(
        roots.contains(&abs("untouched")),
        "roots outside the prefix untouched"
    );
    assert_eq!(store.get(&other.id).unwrap().roots, vec![abs("elsewhere")]);

    // Idempotent: no `from`-prefix hits left, so a rerun is a no-op.
    assert!(store.rebind_roots(&from, &to).unwrap().is_empty());
    assert_eq!(store.get(&project.id).unwrap().roots, roots);
}

#[test]
fn rebind_roots_allows_overlap_with_other_projects() {
    // Landing on another project's territory no longer errors (§9.9
    // cross-project overlap is legal); the rewrite persists as usual.
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let from = abs("from2");
    let occupied = temp.path().join("occupied");
    std::fs::create_dir_all(&occupied).expect("create occupied dir");

    let project = create(&store, "to-move", std::slice::from_ref(&from));
    let holder = create(
        &store,
        "existing-territory",
        std::slice::from_ref(&occupied),
    );

    let affected = store
        .rebind_roots(&from, &occupied)
        .expect("overlap after rebind is legal");
    assert_eq!(affected, vec![project.id.clone()]);
    assert_eq!(
        store.get(&project.id).unwrap().roots,
        vec![occupied.canonicalize().unwrap()]
    );
    assert_eq!(
        store.get(&holder.id).unwrap().roots,
        vec![occupied.canonicalize().unwrap()],
        "both sides hold the same directory"
    );
}

#[test]
fn forget_session_and_retain_sessions_prune_orphans() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "project", &[]);
    store
        .move_session_to_project("s1", Some(&project.id), None)
        .expect("assign s1");
    store
        .move_session_to_project("s2", None, None)
        .expect("explicit move out s2");

    assert!(store.forget_session("s1"));
    assert_eq!(store.assignment_of("s1"), None);
    assert!(!store.forget_session("unknown"), "unknown id is a no-op");

    // Boot reconciliation: keep only entries whose sessions still exist.
    store
        .move_session_to_project("s3", Some(&project.id), None)
        .expect("assign s3");
    let existing: HashSet<String> = ["s3".to_string()].into_iter().collect();
    let pruned = store.retain_sessions(&existing);
    assert_eq!(pruned, 1, "s2's explicit move-out entry is pruned");
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
    let project = create(&store, "project", &[]);

    // Attach the child first, then move a session whose workspace is the
    // parent: the ancestor adopts the descendant; the set must not end up
    // nested.
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

    // The other direction stays idempotent: when an existing root is the
    // ancestor, a child-directory workspace is not added again.
    let nested_again = store
        .move_session_to_project("s3", Some(&project.id), Some(&child))
        .expect("covered workspace skips add");
    assert_eq!(nested_again.added_root, None);
    assert_eq!(store.get(&project.id).expect("project").roots.len(), 1);
}

#[test]
fn root_keys_fold_case_only_on_windows() {
    // Windows is case-insensitive: two spellings of the same (nonexistent)
    // directory differing in case/separators must fold into the same root key.
    // Cross-project overlap is legal since the §9.9 (2026-09-11) ruling, so
    // the fold's remaining enforcement point is the same-set duplicate check
    // in validate_roots; the two projects below coexist by design.
    // Uses a std::env::consts::OS constant branch instead of cfg syntax:
    // platform conditional compilation must not appear outside the adapter
    // layer (architecture-guard rust_target_cfg_outside_adapter).
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    if std::env::consts::OS == "windows" {
        let backslash_upper = abs("CaseProbe").to_string_lossy().replace('/', "\\");
        // Same directory in forward-slash + lowercase spelling: only the
        // folded identity key sees the two as one root.
        let forward_lower = backslash_upper
            .replace('\\', "/")
            .replace("CaseProbe", "caseprobe");
        assert_ne!(backslash_upper, forward_lower);

        create(&store, "upper", &[PathBuf::from(&backslash_upper)]);
        create(&store, "lower", &[PathBuf::from(&forward_lower)]);
        assert_eq!(
            store.list().len(),
            2,
            "cross-project overlap is legal since §9.9"
        );
        let error = store
            .create_project(
                "dup".to_string(),
                vec![
                    PathBuf::from(&backslash_upper),
                    PathBuf::from(&forward_lower),
                ],
            )
            .expect_err("case/separator-folded same-set duplicate root rejected");
        assert!(error.to_string().contains("duplicate project root"));
    } else {
        create(&store, "upper", &[abs("CaseProbe")]);
        create(&store, "lower", &[abs("caseprobe")]);
        assert_eq!(store.list().len(), 2);
    }
}

// ── Folder-project auto-materialization (ensure) ────────────────────────────

fn ensure(store: &ProjectStore, roots: &[PathBuf]) -> Vec<super::EnsureFolderOutcome> {
    store
        .ensure_folder_roots(roots)
        .expect("ensure folder roots")
}

#[test]
fn ensure_creates_basename_named_folder_projects_idempotently() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    let outcomes = ensure(&store, &[abs("web"), abs("api")]);
    assert!(
        matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "web" && project.origin.as_deref() == Some("folder"))
    );
    assert!(
        matches!(&outcomes[1], super::EnsureFolderOutcome::Created { project }
        if project.name == "api")
    );
    assert_eq!(store.list().len(), 2);

    // Idempotent: replaying the same batch of roots → all Covered, nothing
    // created.
    let replay = ensure(&store, &[abs("web"), abs("api")]);
    let ids: Vec<&str> = replay
        .iter()
        .filter_map(|o| match o {
            super::EnsureFolderOutcome::Covered { project_id } => Some(project_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "replay is all Covered: {replay:?}");
    assert_eq!(store.list().len(), 2);

    // A folder referenced by a manual project does not count as anchored
    // coverage (§9.9): the browse channel always creates a same-named
    // materialized project; overlap coexists legally (see
    // ensure_anchor_reuse_only_for_folder_anchored_projects for details).
    let manual = create(&store, "manual", &[abs("manual/root")]);
    let covered = ensure(&store, &[abs("manual/root")]);
    assert!(
        matches!(&covered[0], super::EnsureFolderOutcome::Created { project }
        if project.origin.as_deref() == Some("folder"))
    );
    assert_eq!(manual.roots.len(), 1, "manual project unaffected");
    assert_eq!(store.list().len(), 4);
}

#[test]
fn ensure_input_dedupes_and_reports_relative_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    let outcomes = ensure(
        &store,
        &[abs("web"), abs("web"), PathBuf::from("relative/x")],
    );
    assert_eq!(
        outcomes.len(),
        2,
        "duplicate roots fold into one: {outcomes:?}"
    );
    assert!(matches!(
        outcomes[0],
        super::EnsureFolderOutcome::Created { .. }
    ));
    assert!(
        matches!(&outcomes[1], super::EnsureFolderOutcome::Failed { reason }
        if reason.contains("absolute"))
    );
    assert_eq!(store.list().len(), 1);
}

#[test]
fn ensure_overlap_with_other_project_does_not_block_the_batch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // An existing project occupies abs("nest/child"): creating a folder
    // project for the ancestor abs("nest") proceeds normally after the
    // overlap legalization (a manual project's reference is not "anchored at
    // that folder", see B4 anchored reuse), and the other roots in the batch
    // are unaffected.
    create(&store, "deep-root", &[abs("nest/child")]);

    let outcomes = ensure(&store, &[abs("nest"), abs("clean")]);
    assert!(matches!(
        outcomes[0],
        super::EnsureFolderOutcome::Created { .. }
    ));
    assert!(matches!(
        outcomes[1],
        super::EnsureFolderOutcome::Created { .. }
    ));
    assert_eq!(store.list().len(), 3);
}

#[test]
fn ensure_recreates_folder_project_after_delete_for_new_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    // First materialization → delete (members written as explicit move-outs).
    ensure(&store, &[abs("web")]);
    let created = store.list()[0].clone();
    store
        .delete_project(&created.id, &["s-old".to_string()])
        .expect("delete");
    assert_eq!(
        store.assignment_of("s-old"),
        Some(None),
        "member stays in Ungrouped"
    );

    // No tombstone: ensuring the same folder again recreates it (the frontend
    // triggers this driven by "new sessions"; old sessions' move-out entries
    // suppress tier-②, so the recreated project only picks up new sessions).
    let recreate = ensure(&store, &[abs("web")]);
    assert!(
        matches!(&recreate[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "web" && project.origin.as_deref() == Some("folder"))
    );
    // The move-out entry survives deletion: old sessions do not revive with
    // the recreation.
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
        "move-out entry survives across processes: old sessions do not revive with recreation"
    );
}

// ── Root-overlap legalization (§9.9): manual decisions and
//    auto-materialization coexist, neither yields to the other ───────────────

#[test]
fn manual_add_root_coexists_with_folder_project_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // The folder-auto project occupies abs("web"); the user moves a session
    // from that folder into a manual project and picks "add folder" → overlap
    // is legal: the target gains the root, the auto project stays as-is.
    ensure(&store, &[abs("web")]);
    let folder_project = store.list()[0].clone();
    let target = create(&store, "target", &[abs("other")]);

    let outcome = store
        .move_session_to_project("s1", Some(&target.id), Some(&abs("web")))
        .expect("move with add root");
    assert_eq!(outcome.added_root, Some(abs("web")));
    assert_eq!(
        store.get(&target.id).expect("target").roots,
        vec![abs("other"), abs("web")]
    );
    assert_eq!(
        store
            .get(&folder_project.id)
            .expect("auto project kept")
            .roots,
        vec![abs("web")],
        "the yielding mechanism is retired: the auto project is not stripped"
    );
    assert_eq!(store.assignment_of("s1"), Some(Some(target.id.clone())));
}

#[test]
fn manual_add_root_allowed_against_manual_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let holder = create(&store, "holder", &[abs("web")]);
    let target = create(&store, "target", &[abs("other")]);

    let outcome = store
        .move_session_to_project("s1", Some(&target.id), Some(&abs("web")))
        .expect("sharing a root between manual projects is equally allowed");
    assert_eq!(outcome.added_root, Some(abs("web")));
    assert_eq!(
        store.get(&holder.id).expect("holder").roots,
        vec![abs("web")]
    );
    assert_eq!(store.get(&target.id).expect("target").roots.len(), 2);
    assert_eq!(store.assignment_of("s1"), Some(Some(target.id.clone())));
}

#[test]
fn manual_add_root_keeps_ancestor_folder_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // The auto project occupies the parent abs("web"); adding the child
    // abs("web/sub") to a manual project: nesting in either direction across
    // projects is legal, both sides keep their roots.
    ensure(&store, &[abs("web")]);
    let folder_project = store.list()[0].clone();
    let target = create(&store, "target", &[abs("other")]);

    store
        .move_session_to_project("s1", Some(&target.id), Some(&abs("web/sub")))
        .expect("move with add sub root");
    assert_eq!(
        store.get(&target.id).expect("target").roots,
        vec![abs("other"), abs("web/sub")]
    );
    assert_eq!(
        store
            .get(&folder_project.id)
            .expect("auto project kept")
            .roots,
        vec![abs("web")]
    );
}

#[test]
fn manual_create_and_update_coexist_with_folder_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // Creating a manual project that directly carries a root referenced by an
    // auto project (the directory view's "convert to project" path): overlap
    // is legal, both projects coexist.
    ensure(&store, &[abs("web")]);
    let created = create(&store, "converted", &[abs("web")]);
    assert_eq!(created.roots, vec![abs("web")]);
    assert_eq!(
        store.list().len(),
        2,
        "the auto project no longer yields and exits"
    );

    // Same for update changing roots: referencing another auto project's
    // folder in the new roots is legal.
    ensure(&store, &[abs("api")]);
    let updated = store
        .update_project(&created.id, None, Some(vec![abs("web"), abs("api")]))
        .expect("update roots");
    assert_eq!(updated.roots, vec![abs("web"), abs("api")]);
    assert_eq!(store.list().len(), 3);
}

#[test]
fn ensure_creates_ancestor_despite_nested_existing_root() {
    // After the overlap legalization: an auto project references a child
    // directory, and ensure of the parent creates as usual (the anchored
    // tightening of covered-reuse is in B4; this locks "no more Failed on
    // nesting").
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    ensure(&store, &[abs("nest/child")]);
    let outcomes = ensure(&store, &[abs("nest")]);
    assert!(
        matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "nest")
    );
    assert_eq!(store.list().len(), 2, "both folder projects coexist");
    assert_eq!(store.list()[0].roots.len(), 1, "existing root unchanged");
}

#[test]
fn covered_workspace_skip_survives_symlinked_ancestor() {
    // Review #464 MAJOR 3 (same shape as macOS /var→/private/var): roots are
    // canonicalized when stored, but a workspace path under coverage review
    // that does not exist used to fall back to a purely lexical form that
    // kept the symlink shape, so identity keys no longer nested and the path
    // was treated as uncovered and added twice. Reproduced on any platform
    // with a symlinked ancestor. Uses a std::env::consts::OS constant branch
    // instead of cfg syntax: platform conditional compilation must not appear
    // outside the adapter layer (architecture-guard); directory symlinks on
    // Windows need admin/developer mode, so unix/macOS cover this mechanism.
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
        "target",
        std::slice::from_ref(&link.join("workspace")),
    );

    // A nonexistent nested path spelled through a symlinked ancestor: the
    // covered check must hit the existing root.
    let covered = link.join("workspace").join("deep");
    let outcome = store
        .move_session_to_project("s1", Some(&project.id), Some(&covered))
        .expect("move with covered workspace");
    assert_eq!(
        outcome.added_root, None,
        "the symlink spelling must not bypass the covered skip"
    );
    assert_eq!(store.get(&project.id).unwrap().roots.len(), 1);
}

// ── Member expulsion on root removal (§4) ───────────────────────────────────

#[test]
fn removed_roots_semantics() {
    let old = vec![abs("keep"), abs("drop")];
    assert_eq!(
        super::removed_roots(&old, &[abs("keep")]),
        vec![abs("drop")],
        "an old root not covered by the new set is removed"
    );
    // New root is an ancestor of the old root: sessions under the old root
    // are still covered by the project, so it does not count as removed.
    let parent = abs("keep").parent().unwrap().to_path_buf();
    assert!(super::removed_roots(&[abs("keep")], &[parent]).is_empty());
    // New root is a descendant of the old root: the old root no longer covers
    // all sessions under it, so it counts as removed.
    assert_eq!(
        super::removed_roots(&[abs("keep")], &[abs("keep/sub")]),
        vec![abs("keep")]
    );
    // An empty new set = everything removed.
    assert_eq!(super::removed_roots(&old, &[]), old);
}

#[test]
fn expel_unassigned_sessions_writes_move_out_only_for_entryless() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "target", &[abs("web")]);
    let elsewhere = create(&store, "elsewhere", &[abs("other")]);
    // s-explicit: explicitly assigned to this project; s-elsewhere:
    // explicitly assigned to another project; s-out: already explicitly moved
    // out; s-auto: no entry (auto-grouped member).
    store
        .move_session_to_project("s-explicit", Some(&project.id), None)
        .expect("explicit member");
    store
        .move_session_to_project("s-elsewhere", Some(&elsewhere.id), None)
        .expect("explicit elsewhere");
    store
        .move_session_to_project("s-out", None, None)
        .expect("explicit move-out");

    let expelled = store
        .expel_unassigned_sessions(&[
            "s-explicit".to_string(),
            "s-elsewhere".to_string(),
            "s-out".to_string(),
            "s-auto".to_string(),
        ])
        .expect("expel");
    assert_eq!(
        expelled, 1,
        "only the entry-less session gets a move-out written"
    );
    assert_eq!(store.assignment_of("s-auto"), Some(None));
    assert_eq!(
        store.assignment_of("s-explicit"),
        Some(Some(project.id.clone())),
        "explicit assignment to this project untouched (root removal does not expel explicit members)"
    );
    assert_eq!(
        store.assignment_of("s-elsewhere"),
        Some(Some(elsewhere.id.clone()))
    );
    assert_eq!(
        store.assignment_of("s-out"),
        Some(None),
        "already moved out stays"
    );

    // Idempotent: a rerun changes nothing.
    assert_eq!(
        store
            .expel_unassigned_sessions(&["s-auto".to_string()])
            .unwrap(),
        0
    );
}

#[test]
fn expelled_assignment_survives_ensure_rematerialization() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "target", &[abs("web")]);
    // Root removal: auto members under it (s-auto) are written as explicit
    // move-outs.
    let removed = super::removed_roots(&[abs("web")], &[]);
    assert_eq!(removed, vec![abs("web")]);
    store
        .expel_unassigned_sessions(&["s-auto".to_string()])
        .expect("expel");
    store
        .update_project(&project.id, None, Some(vec![]))
        .expect("remove root");

    // A new session appears in that folder later → ensure rematerializes
    // (origin=folder project); the old session's move-out entry suppresses
    // tier-②, so the recreated project only picks up new sessions.
    let outcomes = ensure(&store, &[abs("web")]);
    assert!(
        matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.origin.as_deref() == Some("folder"))
    );
    assert_eq!(
        store.assignment_of("s-auto"),
        Some(None),
        "rematerialization must not revive old members of the removed root"
    );
}

// ── last_primary_root (§9.2 remembered primary folder) ─────────────────────

#[test]
fn last_primary_root_set_validate_and_persist() {
    let temp = tempfile::tempdir().expect("tempdir");
    {
        let store = store_in(&temp);
        let project = create(&store, "multi-root", &[abs("web"), abs("api")]);

        // Non-members are rejected.
        let error = store
            .set_last_primary_root(&project.id, &abs("other"))
            .expect_err("primary root must be a member");
        assert!(error.to_string().contains("project roots"));

        // Members accepted; same-value repeats are idempotent; survives
        // reopen.
        let updated = store
            .set_last_primary_root(&project.id, &abs("api"))
            .expect("set primary root");
        assert_eq!(updated.last_primary_root, Some(abs("api")));
        let again = store
            .set_last_primary_root(&project.id, &abs("api"))
            .expect("idempotent");
        assert_eq!(again.last_primary_root, Some(abs("api")));
    }
    let reopened = store_in(&temp);
    let project = reopened.list().into_iter().next().expect("project");
    assert_eq!(
        project.last_primary_root,
        Some(abs("api")),
        "old files missing the key read as None; once written it survives across processes"
    );
}

#[test]
fn legacy_file_without_last_primary_root_reads_as_none() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "legacy", &[abs("web")]);
    assert_eq!(
        project.last_primary_root, None,
        "new field defaults to absent"
    );
    // Persist → reopen; under skip_serializing_if old files lack the key and
    // read back as None.
    let reopened = store_in(&temp);
    assert_eq!(reopened.list()[0].last_primary_root, None);
}

// ── Anti-materialization exclusion list (§3) and anchored reuse (§9.9) ──────

#[test]
fn never_materialize_skips_ensure_and_is_revocable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    // Excluded: ensure skips it (no outcome, same as input dedup), no project
    // is created.
    let listed = store
        .set_never_materialize(&abs("scratch"), true)
        .expect("exclude");
    assert_eq!(listed.len(), 1);
    let outcomes = ensure(&store, &[abs("scratch"), abs("web")]);
    assert_eq!(
        outcomes.len(),
        1,
        "excluded roots produce no outcome: {outcomes:?}"
    );
    assert!(matches!(
        outcomes[0],
        super::EnsureFolderOutcome::Created { .. }
    ));
    assert_eq!(store.list().len(), 1);
    assert_eq!(store.list()[0].name, "web");

    // Idempotent: repeating the exclusion changes nothing; after revoking,
    // ensure materializes as usual.
    let again = store
        .set_never_materialize(&abs("scratch"), true)
        .expect("idempotent");
    assert_eq!(again, listed);
    let revoked = store
        .set_never_materialize(&abs("scratch"), false)
        .expect("revoke");
    assert!(revoked.is_empty());
    let outcomes = ensure(&store, &[abs("scratch")]);
    assert!(
        matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "scratch")
    );

    // Relative paths rejected.
    assert!(
        store
            .set_never_materialize(Path::new("relative/x"), true)
            .is_err()
    );
}

#[test]
fn never_materialize_list_persists_and_keeps_file_alive() {
    let temp = tempfile::tempdir().expect("tempdir");
    {
        let store = store_in(&temp);
        store
            .set_never_materialize(&abs("scratch"), true)
            .expect("exclude");
        // No projects, no assignments: the file must survive while the
        // exclusion list is non-empty (the empty-state-removes-file
        // convention must not swallow the user's explicit statement).
        assert!(temp.path().join("projects.json").exists());
    }
    let reopened = store_in(&temp);
    assert_eq!(
        reopened.never_materialize_roots().len(),
        1,
        "survives across processes"
    );
    // Revoke to empty + no projects/assignments → file removed.
    reopened
        .set_never_materialize(&abs("scratch"), false)
        .expect("revoke");
    assert!(!temp.path().join("projects.json").exists());
}

#[test]
fn ensure_anchor_reuse_only_for_folder_anchored_projects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);

    // A materialized project anchored at F → reuse (Covered), idempotent.
    ensure(&store, &[abs("web")]);
    let anchored = store.list()[0].clone();
    let replay = ensure(&store, &[abs("web")]);
    assert!(
        matches!(&replay[0], super::EnsureFolderOutcome::Covered { project_id }
        if *project_id == anchored.id)
    );
    assert_eq!(store.list().len(), 1);

    // A manual project referencing F (even as its only/primary root) → not
    // anchored coverage; the browse channel always creates a same-named
    // materialized project (§9.9 user decision), overlap coexists legally.
    create(&store, "manual", &[abs("docs")]);
    let outcomes = ensure(&store, &[abs("docs")]);
    assert!(
        matches!(&outcomes[0], super::EnsureFolderOutcome::Created { project }
        if project.name == "docs" && project.origin.as_deref() == Some("folder"))
    );
    assert_eq!(store.list().len(), 3);

    // After a materialized project gains another root, both paths its roots
    // contain exactly are anchors → both are reused ("materialized project
    // anchored at F" is judged by roots membership).
    store
        .update_project(&anchored.id, None, Some(vec![abs("web"), abs("api")]))
        .expect("add root");
    let replay = ensure(&store, &[abs("web")]);
    assert!(matches!(
        &replay[0],
        super::EnsureFolderOutcome::Covered { .. }
    ));
    let replay = ensure(&store, &[abs("api")]);
    assert!(matches!(
        &replay[0],
        super::EnsureFolderOutcome::Covered { .. }
    ));
}

// ── Align to project (§6/§9.7): assignment resolution and keychain shape ────

#[test]
fn resolve_session_project_explicit_then_position_tiebreak() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // Two projects reference the same directory (§9.9 overlap is legal): with
    // no assignment entry, the smallest position adopts.
    let first = create(&store, "earlier", &[abs("web")]);
    let second = create(&store, "later", &[abs("web")]);
    assert!(first.position < second.position);
    assert_eq!(
        store
            .resolve_session_project("s1", &abs("web"))
            .map(|p| p.id),
        Some(first.id.clone()),
        "tier-② multi-hit: smallest position"
    );
    // Explicit assignment wins (even to the later project).
    store
        .move_session_to_project("s1", Some(&second.id), None)
        .expect("explicit assign");
    assert_eq!(
        store
            .resolve_session_project("s1", &abs("web"))
            .map(|p| p.id),
        Some(second.id.clone())
    );
    // Explicit move-out blocks tier-②.
    store
        .move_session_to_project("s1", None, None)
        .expect("explicit move-out");
    assert!(store.resolve_session_project("s1", &abs("web")).is_none());
    // Stale assignment id (project deleted): delete rewrote the member as an
    // explicit move-out → None.
    store
        .move_session_to_project("s2", Some(&first.id), None)
        .expect("assign");
    store.delete_project(&first.id, &[]).expect("delete");
    assert!(store.resolve_session_project("s2", &abs("web")).is_none());
    // No assignment and path under no root → None; nested paths hit the root
    // prefix (folded key).
    assert!(store.resolve_session_project("s3", &abs("other")).is_none());
    assert_eq!(
        store
            .resolve_session_project("s3", &abs("web").join("sub/dir"))
            .map(|p| p.id),
        Some(second.id.clone()),
        "nested path hits the root prefix"
    );
}

/// Golden-vector cross-lock for the frontend's three-tier group resolution
/// (① explicit assignment → ② root hit decided by position → ③ ungrouped):
/// the same-semantic resolver in pinvou3-app/src reconciles against this
/// table case by case; a change on either side that diverges turns this test
/// red, preventing double-implementation drift (review #484 minor).
#[test]
fn resolve_session_project_golden_vectors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    // P0 < P1 < P2 (increasing positions); P0 and P1 cover the same directory
    // (§9.9 overlap is legal).
    let p0 = create(&store, "P0", &[abs("a")]);
    let p1 = create(&store, "P1", &[abs("a"), abs("b")]);
    let p2 = create(&store, "P2", &[abs("c")]);
    assert!(p0.position < p1.position && p1.position < p2.position);

    let resolve = |session_id: &str, workspace: &Path| {
        store
            .resolve_session_project(session_id, workspace)
            .map(|p| p.id)
    };

    // Tier-② vectors without assignment entries:
    assert_eq!(
        resolve("g1", &abs("a")),
        Some(p0.id.clone()),
        "multi-hit: smallest position"
    );
    assert_eq!(
        resolve("g2", &abs("a").join("deep/nested")),
        Some(p0.id.clone()),
        "nested path hits the root prefix"
    );
    assert_eq!(resolve("g3", &abs("b")), Some(p1.id.clone()), "single hit");
    assert_eq!(resolve("g4", &abs("c")), Some(p2.id.clone()));
    assert_eq!(resolve("g5", &abs("z")), None, "no hit → ungrouped");

    // Tier-① explicit assignment wins, regardless of whether the workspace
    // hits that project's roots.
    store
        .move_session_to_project("g6", Some(&p2.id), None)
        .expect("assign");
    assert_eq!(
        resolve("g6", &abs("a")),
        Some(p2.id.clone()),
        "explicit assignment beats tier-②'s position tiebreak"
    );
    store
        .move_session_to_project("g7", Some(&p1.id), None)
        .expect("assign");
    assert_eq!(
        resolve("g7", &abs("z")),
        Some(p1.id.clone()),
        "explicit assignment does not depend on a workspace hit"
    );

    // An explicit move-out (None entry) blocks all auto-grouping, even when
    // the workspace hits.
    store
        .move_session_to_project("g8", None, None)
        .expect("move out");
    assert_eq!(
        resolve("g8", &abs("a")),
        None,
        "explicit move-out → ungrouped"
    );
}

#[test]
fn update_roots_demotes_stale_last_primary_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "multi-root", &[abs("web"), abs("api")]);
    store
        .set_last_primary_root(&project.id, &abs("api"))
        .expect("set primary");

    // Removing the root the primary folder lives on: the stale memory demotes
    // to None and stops serving as the project channel's cwd.
    let updated = store
        .update_project(&project.id, None, Some(vec![abs("web")]))
        .expect("replace roots");
    assert_eq!(updated.last_primary_root, None, "stale primary demoted");

    // A replacement that keeps the root leaves the memory alone.
    store
        .set_last_primary_root(&project.id, &abs("web"))
        .expect("set primary again");
    let kept = store
        .update_project(&project.id, None, Some(vec![abs("web"), abs("api")]))
        .expect("replace roots keeping primary");
    assert_eq!(kept.last_primary_root, Some(abs("web")));

    // Same demotion on the update_project_and_expel path.
    let expelled = store
        .update_project_and_expel(&project.id, None, vec![abs("api")], &[])
        .expect("replace and expel");
    assert_eq!(expelled.last_primary_root, None);
}

#[test]
fn update_project_and_expel_is_one_transaction() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = store_in(&temp);
    let project = create(&store, "old-name", &[abs("web"), abs("api")]);
    let elsewhere = create(&store, "elsewhere", &[abs("other")]);
    // Entries that already exist (explicitly assigned to another project /
    // already moved out) are untouched per tier-① semantics.
    store
        .move_session_to_project("s-elsewhere", Some(&elsewhere.id), None)
        .expect("assign elsewhere");
    store
        .move_session_to_project("s-out", None, None)
        .expect("move out");

    let updated = store
        .update_project_and_expel(
            &project.id,
            Some("new-name".to_string()),
            vec![abs("api")],
            &[
                "s-auto".to_string(),
                "s-elsewhere".to_string(),
                "s-out".to_string(),
            ],
        )
        .expect("atomic replace + expel");
    assert_eq!(updated.name, "new-name");
    assert_eq!(updated.roots, vec![abs("api")]);
    assert_eq!(
        store.assignment_of("s-auto"),
        Some(None),
        "entry-less sessions get an explicit move-out"
    );
    assert_eq!(
        store.assignment_of("s-elsewhere"),
        Some(Some(elsewhere.id.clone())),
        "explicit assignment to another project untouched"
    );
    assert_eq!(
        store.assignment_of("s-out"),
        Some(None),
        "already moved out stays"
    );

    // Unknown project: neither roots nor assignments are written.
    let error = store
        .update_project_and_expel("prj-nope", None, vec![abs("x")], &["s-x".to_string()])
        .expect_err("unknown project rejected");
    assert!(error.to_string().contains("project not found"));
    assert_eq!(store.assignment_of("s-x"), None);

    // Idempotent retry: a replay whose removed set is already empty does not
    // rewrite existing entries.
    let replay = store
        .update_project_and_expel(&project.id, None, vec![abs("api")], &["s-auto".to_string()])
        .expect("idempotent replay");
    assert_eq!(replay.roots, vec![abs("api")]);
    assert_eq!(store.assignment_of("s-auto"), Some(None));
}

#[test]
fn keychain_for_workspace_keeps_cwd_first_and_strips_it_from_project_roots() {
    // Primary slot = the session's cwd (no door change); project roots minus
    // cwd, order preserved.
    let chain =
        super::ProjectStore::keychain_for_workspace(&abs("b"), &[abs("a"), abs("b"), abs("c")]);
    assert_eq!(chain, vec![abs("b"), abs("a"), abs("c")]);
    // cwd not among the project roots (e.g. aligning a cross-directory
    // session) → cwd first, roots follow in full.
    let chain =
        super::ProjectStore::keychain_for_workspace(&abs("elsewhere"), &[abs("a"), abs("b")]);
    assert_eq!(chain, vec![abs("elsewhere"), abs("a"), abs("b")]);
    // Empty project (pure label) → cwd only.
    let chain = super::ProjectStore::keychain_for_workspace(&abs("x"), &[]);
    assert_eq!(chain, vec![abs("x")]);
}
