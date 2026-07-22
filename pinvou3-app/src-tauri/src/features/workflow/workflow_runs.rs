//! 工作流 run 第一公民：run.json 元数据 + index.json 台账。
//!
//! 真相源层级（spec §3）：进度真相 = `project/_state/workflow_progress.json`（SDAN 07，
//! 本模块绝不复制角色级状态）；run.json 只缓存列表页摘要字段；index.json 是纯缓存，
//! 可丢弃，丢失/损坏即扫 `workflows/*/run.json` 全量重建。

// P0 Task6 接线（commands/forwarder 调用）后删：当前 pub API 暂无调用方。
#![allow(dead_code)]

use crate::platform::paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub scenario: String,
    pub title: String,
    pub created_at: String, // RFC3339
    pub updated_at: String,
    /// running | completed | blocked | abandoned —— 从 _state 派生缓存，不另造状态机
    pub status: String,
}

/// 元数据读-改-写互斥锁（I-2）：reconcile_run / update_run_status / create_run 的
/// 读-改-写段整体持锁，防 reconcile 派生值覆盖并发写入的 abandoned（唯一不可从
/// _state 重建的用户手动态）。单进程 Tauri，std Mutex 够用；调用方经 spawn_blocking
/// 进来是同步上下文，不会 hold-across-await。
static RUN_META_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// run_id = `wf-<YYYYMMDD-HHMM>-<4hex>`（hex 取自纳秒时戳，无需新依赖）。
pub fn gen_run_id() -> String {
    use chrono::Utc;
    let now = Utc::now();
    let nanos = now.timestamp_subsec_nanos();
    format!("wf-{}-{:04x}", now.format("%Y%m%d-%H%M"), (nanos & 0xffff))
}

/// 校验外部传入 run_id 形如 `wf-\d{8}-\d{4}-[0-9a-f]{4}`（I-3 路径穿越防线）。
/// run_id 会暴露给前端 invoke + cli_server HTTP，必须挡掉 `../sessions/xxx` 这类逃逸。
/// 手写按段检查，不引 regex 依赖。
pub fn is_valid_run_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    if parts.len() != 4 || parts[0] != "wf" {
        return false;
    }
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let all_lower_hex = |s: &str| s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    parts[1].len() == 8
        && all_digits(parts[1])
        && parts[2].len() == 4
        && all_digits(parts[2])
        && parts[3].len() == 4
        && all_lower_hex(parts[3])
}

fn run_json_path(run_id: &str) -> std::path::PathBuf {
    paths::workflow_run_dir(run_id).join("run.json")
}

fn write_run(meta: &RunMeta) -> Result<(), String> {
    let dir = paths::workflow_run_dir(&meta.run_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create run dir: {e}"))?;
    let s = serde_json::to_string_pretty(meta).map_err(|e| format!("serialize run.json: {e}"))?;
    std::fs::write(run_json_path(&meta.run_id), s).map_err(|e| format!("write run.json: {e}"))
}

/// 建 run 骨架：`workflows/<run_id>/`（project/ 由 harness::init_project 建）+ run.json + 进台账。
/// 持 RUN_META_LOCK：碰撞检查与写盘之间不许穿插另一个 create（M-1）。
pub fn create_run(scenario: &str, title: &str) -> Result<RunMeta, String> {
    let _g = RUN_META_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    create_run_locked(scenario, title, &mut gen_run_id)
}

/// 锁内实现；gen 可注入便于测碰撞重试。16bit 熵同分钟约 1/65536 碰撞，
/// 撞上（目录已存在）就重新 gen，最多 8 次仍撞则 Err，绝不静默覆盖。
fn create_run_locked(
    scenario: &str,
    title: &str,
    gen: &mut dyn FnMut() -> String,
) -> Result<RunMeta, String> {
    let mut run_id = gen();
    let mut retries = 0;
    while paths::workflow_run_dir(&run_id).exists() {
        retries += 1;
        if retries > 8 {
            return Err(format!("run_id 碰撞重试 8 次仍冲突: {run_id}"));
        }
        run_id = gen();
    }
    let now = chrono::Utc::now().to_rfc3339();
    let meta = RunMeta {
        run_id,
        scenario: scenario.to_string(),
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
        status: "running".to_string(),
    };
    write_run(&meta)?;
    rebuild_index()?; // run 数量几十级，直接重建比增量改简单且不会漂
    Ok(meta)
}

/// 读单 run 元数据。入口先校验 run_id（update/reconcile 都过这里，统一防穿越）。
pub fn read_run(run_id: &str) -> Result<RunMeta, String> {
    if !is_valid_run_id(run_id) {
        return Err(format!("非法 run_id（拒绝路径穿越）: {run_id}"));
    }
    let s = std::fs::read_to_string(run_json_path(run_id))
        .map_err(|e| format!("read run.json {run_id}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("parse run.json {run_id}: {e}"))
}

/// 改 run 状态 + updated_at，写回 run.json 并同步台账。持 RUN_META_LOCK（I-2）。
pub fn update_run_status(run_id: &str, status: &str) -> Result<(), String> {
    let _g = RUN_META_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    update_run_status_locked(run_id, status)
}

/// 锁内实现：reconcile_run 已持锁时复用，避免 std Mutex 不可重入死锁。
fn update_run_status_locked(run_id: &str, status: &str) -> Result<(), String> {
    let mut meta = read_run(run_id)?;
    meta.status = status.to_string();
    meta.updated_at = chrono::Utc::now().to_rfc3339();
    write_run(&meta)?;
    rebuild_index().map(|_| ())
}

/// 列全部 run（updated_at 新→旧）。index.json 缺失/损坏 → rebuild_index()。
pub fn list_runs() -> Result<Vec<RunMeta>, String> {
    match std::fs::read_to_string(paths::workflows_index_path()) {
        Ok(s) => match serde_json::from_str::<Vec<RunMeta>>(&s) {
            Ok(v) => Ok(v),
            Err(_) => rebuild_index(), // 损坏 → 重建（台账是纯缓存）
        },
        Err(_) => rebuild_index(), // 缺失 → 重建
    }
}

/// 扫 workflows/*/run.json 重建台账。单个坏 run.json 跳过并警告，别炸整张列表。
pub fn rebuild_index() -> Result<Vec<RunMeta>, String> {
    let root = paths::workflows_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("create workflows root: {e}"))?;
    let mut runs: Vec<RunMeta> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let p = entry.path().join("run.json");
            if !p.is_file() {
                continue;
            }
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str::<RunMeta>(&s).map_err(|e| e.to_string()))
            {
                Ok(m) => runs.push(m),
                Err(e) => eprintln!("[workflow_runs] skip broken {}: {e}", p.display()),
            }
        }
    }
    runs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let s = serde_json::to_string_pretty(&runs).map_err(|e| format!("serialize index: {e}"))?;
    std::fs::write(paths::workflows_index_path(), s).map_err(|e| format!("write index: {e}"))?;
    Ok(runs)
}

/// 从 workflow_progress.json 的 roles 派生 run 状态。映射表（角色级 → run 级）：
///
/// | 角色级状态（workflow_state.py RoleStatus） | run 级 |
/// |---|---|
/// | 任一 blocked / failed（重试耗尽终态，列表必须看得出坏死） | "blocked" |
/// | 非空且全 completed / skipped（skip_role 是真实转移路径，对齐 all_completed()） | "completed" |
/// | 其余（pending/ready/running/reviewing/stale…） | "running" |
pub fn derive_status(progress: &serde_json::Value) -> &'static str {
    let Some(roles) = progress.get("roles").and_then(|r| r.as_object()) else {
        return "running";
    };
    // 计划稿是闭包，但闭包返回借参引用推不出生命周期（E0700 类），等价改局部 fn。
    fn status_of(r: &serde_json::Value) -> &str {
        r.get("status").and_then(|s| s.as_str()).unwrap_or("")
    }
    if roles
        .values()
        .any(|r| matches!(status_of(r), "blocked" | "failed"))
    {
        return "blocked";
    }
    if !roles.is_empty()
        && roles
            .values()
            .all(|r| matches!(status_of(r), "completed" | "skipped"))
    {
        return "completed";
    }
    "running"
}

/// 决策性读取的互证（spec §3.2）：以 _state 为准刷新 run.json/index 后返回最新 meta。
/// abandoned 是用户手动态，不被 _state 派生覆盖。
/// 整个读-改-写段持 RUN_META_LOCK（I-2）：锁内读到的 meta 即最新，
/// 不会出现"读到 running → 并发标 abandoned → 派生值覆盖回去"。
pub fn reconcile_run(run_id: &str) -> Result<RunMeta, String> {
    let _g = RUN_META_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let meta = read_run(run_id)?; // 持锁后读 = 重读最新，abandoned 判定不会过期
    if meta.status == "abandoned" {
        return Ok(meta);
    }
    let progress_path = paths::workflow_project_dir(run_id)
        .join("_state")
        .join("workflow_progress.json");
    let derived = std::fs::read_to_string(&progress_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| derive_status(&v).to_string());
    match derived {
        Some(d) if d != meta.status => {
            update_run_status_locked(run_id, &d)?;
            read_run(run_id)
        }
        _ => Ok(meta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_list_roundtrip() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        let meta = create_run("solution_deck", "测试标题").unwrap();
        assert!(meta.run_id.starts_with("wf-"));
        assert!(crate::platform::paths::workflow_run_dir(&meta.run_id)
            .join("run.json")
            .exists());

        let listed = list_runs().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].run_id, meta.run_id);
        assert_eq!(listed[0].status, "running");

        // 台账可重建：删 index.json 后 list_runs 仍能列出（扫 run.json 重建）
        std::fs::remove_file(crate::platform::paths::workflows_index_path()).unwrap();
        assert_eq!(list_runs().unwrap().len(), 1);
        std::env::remove_var("PINVOU3_HOME");
    }

    #[test]
    fn derive_status_prefers_state_truth() {
        // _state 全 completed → derive 出 completed（index 互证用）
        let roles = serde_json::json!({"roles": {"pm": {"status": "completed"}}});
        assert_eq!(derive_status(&roles), "completed");
        let running = serde_json::json!({"roles": {"pm": {"status": "running"}}});
        assert_eq!(derive_status(&running), "running");
        let blocked = serde_json::json!({"roles": {"pm": {"status": "blocked"}}});
        assert_eq!(derive_status(&blocked), "blocked");
        let empty = serde_json::json!({"roles": {}});
        assert_eq!(derive_status(&empty), "running");
    }

    // I-1: 真相源 workflow_state.py all_completed() 把 SKIPPED 算完成（skip_role 真实转移路径）
    #[test]
    fn derive_status_counts_skipped_as_completed() {
        let mixed = serde_json::json!({"roles": {
            "pm": {"status": "completed"},
            "qa": {"status": "skipped"}
        }});
        assert_eq!(derive_status(&mixed), "completed");
        let all_skipped = serde_json::json!({"roles": {"qa": {"status": "skipped"}}});
        assert_eq!(derive_status(&all_skipped), "completed");
    }

    // M-2: failed（重试耗尽终态）不能映成 running，列表必须看得出坏死 run
    #[test]
    fn derive_status_maps_failed_to_blocked() {
        let failed = serde_json::json!({"roles": {
            "pm": {"status": "completed"},
            "qa": {"status": "failed"}
        }});
        assert_eq!(derive_status(&failed), "blocked");
    }

    // I-3: 外部 run_id 路径穿越必须被挡（后续暴露给前端 invoke + cli_server HTTP）
    #[test]
    fn read_run_rejects_path_traversal() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-trav-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        // workflows/ 必须真实存在，".." 才能在内核层被逐段解析穿出去（否则 ENOENT 假绿）
        std::fs::create_dir_all(crate::platform::paths::workflows_root()).unwrap();
        // 在 workflows/ 外造一个能被穿越读到的 run.json
        let evil_dir = std::path::Path::new(&tmp).join("sessions").join("evil");
        std::fs::create_dir_all(&evil_dir).unwrap();
        let evil_meta = serde_json::json!({
            "run_id": "../sessions/evil", "scenario": "x", "title": "x",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "status": "running"
        });
        std::fs::write(evil_dir.join("run.json"), evil_meta.to_string()).unwrap();

        assert!(read_run("../sessions/evil").is_err());
        assert!(read_run("../escape").is_err());
        assert!(update_run_status("../sessions/evil", "abandoned").is_err());
        assert!(reconcile_run("../sessions/evil").is_err());
        std::env::remove_var("PINVOU3_HOME");
    }

    // M-3①: abandoned 是用户手动态，reconcile 不被 _state 派生覆盖
    #[test]
    fn reconcile_keeps_abandoned_over_state() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-abd-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        let meta = create_run("solution_deck", "弃用测试").unwrap();
        update_run_status(&meta.run_id, "abandoned").unwrap();
        // _state 全 completed，但 abandoned 短路
        let state_dir = crate::platform::paths::workflow_project_dir(&meta.run_id).join("_state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("workflow_progress.json"),
            serde_json::json!({"roles": {"pm": {"status": "completed"}}}).to_string(),
        )
        .unwrap();

        let after = reconcile_run(&meta.run_id).unwrap();
        assert_eq!(after.status, "abandoned");
        std::env::remove_var("PINVOU3_HOME");
    }

    // M-3②: _state 全完成（含 skipped）而 run.json 落后 → reconcile 后 completed 且 index 同步
    #[test]
    fn reconcile_syncs_completed_from_state_and_index() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-rec-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        let meta = create_run("solution_deck", "互证测试").unwrap();
        assert_eq!(meta.status, "running");
        let state_dir = crate::platform::paths::workflow_project_dir(&meta.run_id).join("_state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("workflow_progress.json"),
            serde_json::json!({"roles": {
                "pm": {"status": "completed"},
                "qa": {"status": "skipped"}
            }})
            .to_string(),
        )
        .unwrap();

        let after = reconcile_run(&meta.run_id).unwrap();
        assert_eq!(after.status, "completed");
        // 台账同步
        let listed = list_runs().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, "completed");
        std::env::remove_var("PINVOU3_HOME");
    }

    // I-3: run_id 格式校验（手写按段检查，不引 regex）
    #[test]
    fn run_id_validation_rules() {
        // gen 产物必过校验
        for _ in 0..16 {
            let id = gen_run_id();
            assert!(is_valid_run_id(&id), "gen_run_id 产物必须合法: {id}");
        }
        assert!(is_valid_run_id("wf-20260610-1432-a3f9"));
        assert!(is_valid_run_id("wf-20260610-1432-0000"));
        // 非法形态
        assert!(!is_valid_run_id("../escape"));
        assert!(!is_valid_run_id("../sessions/evil"));
        assert!(!is_valid_run_id("wf-20260610-1432-A3F9")); // 大写 hex 拒
        assert!(!is_valid_run_id("wf-20260610-1432-a3f")); // hex 段短
        assert!(!is_valid_run_id("wf-2026061-1432-a3f9")); // 日期段短
        assert!(!is_valid_run_id("wf-20260610-143x-a3f9")); // 时间段非数字
        assert!(!is_valid_run_id("xx-20260610-1432-a3f9")); // 前缀错
        assert!(!is_valid_run_id("wf-20260610-1432-a3f9-extra")); // 多段
        assert!(!is_valid_run_id("wf-20260610-1432-../."));
        assert!(!is_valid_run_id(""));
    }

    // M-1: run_id 碰撞（目录已存在）→ 重 gen，最多 8 次仍撞 → Err 不静默覆盖
    #[test]
    fn create_run_retries_on_collision() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-col-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        // 预占一个 id 的目录，注入"先撞后让"的 gen → 第二次拿到新 id
        let taken = "wf-20260610-1200-aaaa";
        std::fs::create_dir_all(crate::platform::paths::workflow_run_dir(taken)).unwrap();
        let mut calls = 0;
        let meta = create_run_locked("solution_deck", "撞后重试", &mut || {
            calls += 1;
            if calls == 1 {
                taken.to_string()
            } else {
                "wf-20260610-1200-bbbb".to_string()
            }
        })
        .unwrap();
        assert_eq!(meta.run_id, "wf-20260610-1200-bbbb");

        // 永远撞 → 8 次后 Err，且不覆盖已存在目录
        let err = create_run_locked("solution_deck", "永撞", &mut || taken.to_string());
        assert!(err.is_err());
        std::env::remove_var("PINVOU3_HOME");
    }

    // M-3③: 单个坏 run.json 不炸 list_runs（台账纯缓存，跳过坏项）
    #[test]
    fn list_runs_skips_broken_run_json() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-wfruns-bad-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        let meta = create_run("solution_deck", "好的").unwrap();
        let bad_dir = crate::platform::paths::workflows_root().join("wf-20990101-0000-dead");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("run.json"), "{not json!!").unwrap();

        // 删 index 强制走 rebuild 扫描路径
        let _ = std::fs::remove_file(crate::platform::paths::workflows_index_path());
        let listed = list_runs().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].run_id, meta.run_id);
        std::env::remove_var("PINVOU3_HOME");
    }
}
