//! 旧布局一次性迁移（spec §7，boot 时跑，幂等可续跑）：
//!
//! 旧布局：工作流项目寄生在 `sessions/<sid>/workspace/ppt-<ts>-<scenario>/`，
//! 身份记在 `sessions/_skill_bindings.json` 的 `binding.project_dir`。
//! 新布局：`workflows/<run_id>/project/` + run.json + index.json（见 workflow_runs）。
//!
//! 钉死顺序（migrate_if_needed）：
//! 1. `workflows/.migration_done` 存在 → return Ok（幂等）
//! 2. 扫 `sessions/*/workspace/ppt-*/`（仅含 `_state/workflow_progress.json` 的才算项目），
//!    run_id 从目录名时间戳 + 目录名稳定散列推导（同目录永远推出同 run_id —— 半途崩
//!    续跑的前提，绝不能用随机数）；fs::rename 进新家 + 写 run.json。
//!    跳过判据 = **target（run_dir/project/）存在**而非 run_dir 存在（崩在建 run_dir
//!    与 rename 之间时空 run_dir 不能挡住续跑）；target 已存在且源还在 = 不同 session
//!    同名目录撞 run_id → 对 `"<dirname>#<n>"` 重新散列去重，绝不静默丢项目
//! 3. 修复扫描 + rebuild_index：先遍历 `workflows/*/`，`project/` 存在而 `run.json`
//!    缺失（上一轮崩在 rename 后、写 meta 前）→ 按目录名（即 run_id）+ project 内
//!    progress/brief 推导补写 run.json——rebuild_index 只认 run.json，不补 = run 永久隐身
//! 4. 直读 `_skill_bindings.json`（serde_json::Value，不经 SessionStore —— 它还没 boot）：
//!    删掉所有 `project_dir` 非 null 的条目（工作流宿主）。写回 bindings **之前**先把
//!    待处理 sid 列表（含上一轮残留）持久化到 `workflows/.pending_hosts.json` ——
//!    步骤 5 失败后重跑时 bindings 已清空，sid 只能从该文件续跑
//! 5. 对 pending 列表的每个 sid：宿主**一律归档不删除**——整体 move 进
//!    `sessions/_archived_workflow_hosts/`（同盘 rename 零拷贝）。历史教训：宿主
//!    workspace 里有 ppt-* 之外的真实价值物（deck 快照、agent_transcripts 等），
//!    "聊天为空"推不出"目录无价值"，删除分支已整体移除。
//!    任何归档失败 → 返回 Err、不写 marker、保留 .pending_hosts.json（下次续跑）；
//!    全部成功 → 删 .pending_hosts.json
//! 6. 写 `.migration_done`（内容 = 迁移摘要 JSON，事后审计用）
//!
//! ⚠️ ship gate：本迁移与 T6（start_workflow 切新布局）必须同版发布——marker 一次性，
//! 旧布局写入方还活着时先跑迁移会让新项目滞留旧布局。

use crate::bridge::paths;
use crate::workflow_runs;
use std::path::{Path, PathBuf};

fn marker_path() -> PathBuf {
    paths::workflows_root().join(".migration_done")
}

/// FNV-1a 64（手写 8 行，不引依赖）。取低 16 位做 run_id 的 4hex 段：
/// 同目录名永远推出同 run_id，幂等续跑的关键。
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `ppt-20260601-120000-solution_deck` → `wf-20260601-1200-<4hex>`。
/// `ppt-` 后解析不出合法时间戳 → None（调用方警告跳过）。
///
/// salt：撞名去重用。不同 session 下同名目录推出同 run_id，第二个项目对
/// `"<dirname>#<salt>"` 重新散列（salt 从 2 起）；salt<=1 用原目录名——
/// 与历史推导完全兼容，同目录续跑永远推出同 id（幂等前提）。
fn derive_run_id_salted(dir_name: &str, salt: usize) -> Option<String> {
    let rest = dir_name.strip_prefix("ppt-")?;
    let date = rest.get(0..8)?;
    let time6 = rest.get(9..15)?;
    if !date.bytes().all(|b| b.is_ascii_digit())
        || rest.as_bytes().get(8) != Some(&b'-')
        || !time6.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hhmm = &time6[..4];
    let key = if salt <= 1 {
        dir_name.to_string()
    } else {
        format!("{dir_name}#{salt}")
    };
    let id = format!("wf-{date}-{hhmm}-{:04x}", fnv1a64(&key) & 0xffff);
    // 产物必须过统一校验（路径穿越防线同源），过不了宁可跳过
    workflow_runs::is_valid_run_id(&id).then_some(id)
}

fn derive_run_id(dir_name: &str) -> Option<String> {
    derive_run_id_salted(dir_name, 1)
}

/// 目录名尾段取 scenario：`ppt-<8位日期>-<6位时间>-<scenario>`。
fn scenario_from_dir_name(dir_name: &str) -> Option<String> {
    let rest = dir_name.strip_prefix("ppt-")?;
    let tail = rest.get(16..)?; // 8 日期 + '-' + 6 时间 + '-'
    if rest.as_bytes().get(15) != Some(&b'-') || tail.is_empty() {
        return None;
    }
    Some(tail.to_string())
}

/// title 优先 `_state/brief.json` 的 user_request_raw 前 16 字（按字符不按字节，中文安全）。
fn derive_title(state_dir: &Path, dir_name: &str) -> String {
    std::fs::read_to_string(state_dir.join("brief.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("user_request_raw")
                .and_then(|r| r.as_str())
                .map(|r| r.chars().take(16).collect::<String>())
        })
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| dir_name.to_string())
}

/// 迁移单个项目目录。返回 Ok(true)=真搬了，Ok(false)=跳过（推不出 run_id）。
///
/// 跳过/重试判据是 **target**（run_dir/project/）而不是 run_dir：崩在
/// create_dir_all(run_dir) 与 rename 之间时 run_dir 是空壳、项目还在旧址，
/// 按 run_dir 判会永久跳过 → 项目滞留旧址，步骤 5 删空壳宿主时连项目一起删。
///
/// target 已存在时（src 必然还在——搬走的项目 scan 不会再扫到）＝不同 session
/// 下同名目录撞 run_id。绝不静默跳过：对 `"<dirname>#<n>"` 重新散列去重，
/// 循环至不撞（去重推导确定，续跑仍幂等），保证每个真实项目都被迁移。
fn migrate_one(src: &Path, dir_name: &str) -> Result<bool, String> {
    let Some(mut run_id) = derive_run_id(dir_name) else {
        eprintln!("[workflow_migrate] 跳过（目录名无合法时间戳）: {}", src.display());
        return Ok(false);
    };
    let mut salt = 1usize;
    loop {
        let occupied = paths::workflow_project_dir(&run_id);
        if !occupied.exists() {
            break;
        }
        // 占坑的同名 run 若缺 run.json（上一轮崩在 rename 后、写 meta 前）：
        // 同名目录派生出的 meta 等价，顺手补上再去重。
        if !paths::workflow_run_dir(&run_id).join("run.json").exists() {
            write_run_meta(&run_id, &occupied, dir_name)?;
        }
        salt += 1;
        if salt > 0xffff {
            return Err(format!("run_id 去重耗尽（撞名规模异常）: {dir_name}"));
        }
        eprintln!(
            "[workflow_migrate] run_id 撞名，去重重推: {} -> {dir_name}#{salt}",
            src.display()
        );
        run_id = derive_run_id_salted(dir_name, salt)
            .ok_or_else(|| format!("去重 run_id 推导失败: {dir_name}#{salt}"))?;
    }
    let run_dir = paths::workflow_run_dir(&run_id);
    let target = paths::workflow_project_dir(&run_id);
    std::fs::create_dir_all(&run_dir).map_err(|e| format!("create {}: {e}", run_dir.display()))?;
    std::fs::rename(src, &target)
        .map_err(|e| format!("rename {} -> {}: {e}", src.display(), target.display()))?;
    write_run_meta(&run_id, &target, dir_name)?;
    Ok(true)
}

/// 从（已搬到新家的）项目目录读 progress/brief 派生元数据，写 run.json。
/// 不复用 workflow_runs::create_run（它会生成随机 run_id），但 status 派生走
/// 同一个 derive_status，绝不另造状态机。
fn write_run_meta(run_id: &str, project_dir: &Path, dir_name: &str) -> Result<(), String> {
    let state_dir = project_dir.join("_state");
    let progress = std::fs::read_to_string(state_dir.join("workflow_progress.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let status = progress
        .as_ref()
        .map(workflow_runs::derive_status)
        .unwrap_or("running");
    let scenario = progress
        .as_ref()
        .and_then(|p| p.get("scenario").and_then(|s| s.as_str()).map(String::from))
        .or_else(|| scenario_from_dir_name(dir_name))
        .unwrap_or_else(|| "unknown".to_string());
    let now = chrono::Utc::now().to_rfc3339();
    let meta = workflow_runs::RunMeta {
        run_id: run_id.to_string(),
        scenario,
        title: derive_title(&state_dir, dir_name),
        created_at: now.clone(),
        updated_at: now,
        status: status.to_string(),
    };
    let s = serde_json::to_string_pretty(&meta).map_err(|e| format!("serialize run.json: {e}"))?;
    std::fs::write(paths::workflow_run_dir(run_id).join("run.json"), s)
        .map_err(|e| format!("write run.json {run_id}: {e}"))
}

/// 扫 `sessions/*/workspace/ppt-*/`，仅含 `_state/workflow_progress.json` 的才算项目。
fn scan_and_migrate_projects() -> Result<usize, String> {
    let sessions = paths::sessions_root();
    let Ok(entries) = std::fs::read_dir(&sessions) else {
        return Ok(0); // 全新安装没有 sessions/，没东西可迁
    };
    let mut migrated = 0;
    for entry in entries.flatten() {
        let sid_dir = entry.path();
        if !sid_dir.is_dir() || entry.file_name() == "_archived_workflow_hosts" {
            continue;
        }
        let workspace = sid_dir.join("workspace");
        let Ok(projects) = std::fs::read_dir(&workspace) else {
            continue;
        };
        for proj in projects.flatten() {
            let p = proj.path();
            let name = proj.file_name();
            let Some(name) = name.to_str() else { continue };
            if !p.is_dir()
                || !name.starts_with("ppt-")
                || !p.join("_state").join("workflow_progress.json").is_file()
            {
                continue;
            }
            if migrate_one(&p, name)? {
                migrated += 1;
            }
        }
    }
    Ok(migrated)
}

/// 步骤 5 的续跑账本：待处理宿主 sid 列表。步骤 4 写回 bindings **之前**先持久化
/// 到这里——否则步骤 5 失败后重跑时 bindings 已清空，sid 列表就丢了（宿主成孤儿）。
/// 步骤 5 全部处理完才删；marker 写出前该文件存在＝步骤 5 还有活没干完。
fn pending_hosts_path() -> PathBuf {
    paths::workflows_root().join(".pending_hosts.json")
}

fn read_pending_hosts() -> Vec<String> {
    std::fs::read_to_string(pending_hosts_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// 步骤 4：直读 bindings JSON，删掉 project_dir 非 null 的条目（工作流宿主），
/// 写回。返回待处理宿主 sid 列表（含上一轮 `.pending_hosts.json` 残留）。
/// 注意普通聊天 binding 序列化里 `project_dir: null` 恒存在（无
/// skip_serializing_if），所以判据是"非 null"而不是"含键"。
///
/// 顺序铁律：先持久化 pending 文件，**再**写回 bindings——崩在两步之间时
/// sid 列表已落盘，下次 boot 步骤 5 仍能从文件续跑。
fn clean_bindings() -> Result<Vec<String>, String> {
    let mut pending = read_pending_hosts(); // 上一轮步骤 5 没干完的残留
    let bf = paths::sessions_root().join("_skill_bindings.json");
    let mut cleaned_doc: Option<serde_json::Value> = None;
    if let Ok(s) = std::fs::read_to_string(&bf) {
        let mut v: serde_json::Value =
            serde_json::from_str(&s).map_err(|e| format!("parse _skill_bindings.json: {e}"))?;
        if let Some(map) = v.as_object_mut() {
            let removed: Vec<String> = map
                .iter()
                .filter(|(_, b)| b.get("project_dir").is_some_and(|p| !p.is_null()))
                .map(|(k, _)| k.clone())
                .collect();
            if !removed.is_empty() {
                for k in &removed {
                    map.remove(k);
                }
                for sid in removed {
                    if !pending.contains(&sid) {
                        pending.push(sid);
                    }
                }
                cleaned_doc = Some(v);
            }
        }
    }
    if pending.is_empty() {
        return Ok(pending);
    }
    let pj = serde_json::to_string_pretty(&pending)
        .map_err(|e| format!("serialize pending hosts: {e}"))?;
    std::fs::write(pending_hosts_path(), pj)
        .map_err(|e| format!("write .pending_hosts.json: {e}"))?;
    if let Some(v) = cleaned_doc {
        let out =
            serde_json::to_string_pretty(&v).map_err(|e| format!("serialize bindings: {e}"))?;
        std::fs::write(&bf, out).map_err(|e| format!("write _skill_bindings.json: {e}"))?;
    }
    Ok(pending)
}

/// sid 来自 bindings 文件（外部可写盘），拼路径前挡掉穿越形态。
fn sid_is_safe(sid: &str) -> bool {
    !sid.is_empty() && !sid.contains('/') && !sid.contains('\\') && !sid.contains("..")
}

/// 步骤 5：宿主**一律归档** = move 进 `_archived_workflow_hosts/`（同盘 rename
/// 零拷贝）。绝不删除——历史教训：宿主 workspace 里有 ppt-* 之外的真实价值物
/// （deck 快照、agent_transcripts 等），"聊天为空"推不出"目录无价值"。
/// 返回 (archived, skipped_unsafe, failed) 计数。failed > 0 时上层必须返回 Err 不写
/// marker（吞错 = binding 已删的宿主文件成无主孤儿），下次 boot 从
/// `.pending_hosts.json` 续跑重试本步骤。
fn archive_hosts(sids: &[String]) -> (usize, usize, usize) {
    let sessions = paths::sessions_root();
    let archive_root = sessions.join("_archived_workflow_hosts");
    let (mut archived, mut skipped_unsafe, mut failed) = (0usize, 0usize, 0usize);
    for sid in sids {
        if !sid_is_safe(sid) {
            // 不计 failed：垃圾 sid 重试一万次也不会好，别让它卡死整个迁移
            eprintln!("[workflow_migrate] 跳过可疑 sid（路径穿越防线）: {sid}");
            skipped_unsafe += 1;
            continue;
        }
        let sid_json = sessions.join(format!("{sid}.json"));
        let sid_dir = sessions.join(sid);
        let arch_json = archive_root.join(format!("{sid}.json"));
        let arch_dir = archive_root.join(sid);
        if std::fs::create_dir_all(&archive_root).is_err() {
            failed += 1; // 建不出归档目录就什么都别动，上报让上层不写 marker
            continue;
        }
        // json / 目录各自独立 rename：上一轮归档一半（json 先走、目录失败）续跑时
        // 已不在原地的部分自然跳过，剩下的继续搬，幂等
        let mut sid_failed = false;
        if sid_json.exists() {
            if let Err(e) = std::fs::rename(&sid_json, &arch_json) {
                eprintln!("[workflow_migrate] 归档失败 {sid}.json: {e}");
                sid_failed = true;
            }
        }
        if sid_dir.is_dir() {
            if let Err(e) = std::fs::rename(&sid_dir, &arch_dir) {
                eprintln!("[workflow_migrate] 归档失败 {sid}/: {e}");
                sid_failed = true;
            }
        }
        if sid_failed {
            failed += 1;
        } else {
            archived += 1;
        }
    }
    (archived, skipped_unsafe, failed)
}

/// 步骤 3 前置修复扫描（I1）：rename 成功后、写 run.json 前崩 → project/ 已在
/// 新家但没有 run.json。rebuild_index 只认 run.json，不补 = run 永久隐身且
/// scan_and_migrate 也不会再扫到（源已搬走）。按目录名（即 run_id）+ project 内
/// progress/brief 推导补写（复用 write_run_meta 的派生逻辑）。
fn repair_missing_run_meta() -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(paths::workflows_root()) else {
        return Ok(0);
    };
    let mut repaired = 0;
    for entry in entries.flatten() {
        let run_dir = entry.path();
        let Some(run_id) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        // 只认过统一校验的目录名（路径穿越防线同源），.pending_hosts.json 等天然滤掉
        if !run_dir.is_dir()
            || !workflow_runs::is_valid_run_id(&run_id)
            || !run_dir.join("project").is_dir()
            || run_dir.join("run.json").exists()
        {
            continue;
        }
        // dir_name 传 run_id：scenario 由 progress 派生（推不出时 unknown），
        // title 由 brief 派生（推不出时退回 run_id），与撞名补写分支同一逻辑
        write_run_meta(&run_id, &run_dir.join("project"), &run_id)?;
        eprintln!("[workflow_migrate] 补写隐身 run 的 run.json: {run_id}");
        repaired += 1;
    }
    Ok(repaired)
}

/// 一次性迁移入口。boot 时在 SessionStore::boot **之前**调用
/// （迁移要动 `_skill_bindings.json` + `sessions/` 目录，store boot 之后再动会跟内存态打架）。
/// 任何一步失败返回 Err 且**不写 marker**——下次 boot 续跑（每步自身幂等）。
pub fn migrate_if_needed() -> Result<(), String> {
    if marker_path().exists() {
        return Ok(()); // 1. 幂等短路
    }
    std::fs::create_dir_all(paths::workflows_root())
        .map_err(|e| format!("create workflows root: {e}"))?;

    let migrated = scan_and_migrate_projects()?; // 2.
    let repaired = repair_missing_run_meta()?; // 3a. 先补隐身 run 的 run.json
    workflow_runs::rebuild_index()?; // 3b.
    let pending_sids = clean_bindings()?; // 4.（先落 .pending_hosts.json 再写回 bindings）
    let (archived, skipped_unsafe, failed) = archive_hosts(&pending_sids); // 5. 一律归档
    if failed > 0 {
        // 步骤 5 失败：不写 marker、保留 .pending_hosts.json，下次 boot 续跑重试。
        // 步骤 2-4 已完成的工作天然幂等可重入（搬过的项目 scan 不会再扫到、
        // bindings 再清是空操作、pending 文件会被原样重写）。
        return Err(format!(
            "{failed} 个工作流宿主归档失败（已归档: {archived}），下次启动续跑"
        ));
    }
    let _ = std::fs::remove_file(pending_hosts_path()); // 步骤 5 干完，撤掉续跑账本

    // 6. marker 内容 = 迁移摘要 JSON，事后审计用（deleted_hosts 字段已随删除分支废弃）
    let summary = serde_json::json!({
        "migrated": migrated,
        "repaired_run_meta": repaired,
        "archived": archived,
        "skipped_unsafe_sids": skipped_unsafe,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(marker_path(), summary.to_string())
        .map_err(|e| format!("write .migration_done: {e}"))?;
    if migrated + repaired + archived + skipped_unsafe > 0 {
        eprintln!(
            "[workflow_migrate] 迁移完成: {migrated} 项目 / {repaired} run.json 补写 / {archived} 宿主归档 / {skipped_unsafe} 可疑 sid 跳过"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_projects_and_cleans_bindings() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        // 旧布局: sessions/s1/workspace/ppt-20260601-120000-solution_deck/_state/workflow_progress.json
        let proj = crate::bridge::paths::sessions_root()
            .join("s1/workspace/ppt-20260601-120000-solution_deck/_state");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("workflow_progress.json"),
            r#"{"scenario":"solution_deck","roles":{"pm":{"status":"completed"}}}"#,
        )
        .unwrap();
        // 宿主 session: 空消息 s1.json + binding
        std::fs::write(
            crate::bridge::paths::sessions_root().join("s1.json"),
            r#"{"metadata":{"id":"s1","title":"PPT · 完整方案型"},"messages":[]}"#,
        )
        .unwrap();
        std::fs::write(
            crate::bridge::paths::sessions_root().join("_skill_bindings.json"),
            r#"{"s1":{"name":"h3c-ppt","phases":[],"project_dir":"/old/path"}}"#,
        )
        .unwrap();

        migrate_if_needed().unwrap();

        // 项目搬进 workflows/<run_id>/project/，台账有一条，状态 completed
        let runs = crate::workflow_runs::list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "completed");
        // run_id 推导自目录名时间戳（HHMM）+ 稳定散列，且过统一校验
        assert!(
            runs[0].run_id.starts_with("wf-20260601-1200-"),
            "run_id 推导错: {}",
            runs[0].run_id
        );
        assert!(crate::workflow_runs::is_valid_run_id(&runs[0].run_id));
        assert_eq!(runs[0].scenario, "solution_deck");
        assert!(crate::bridge::paths::workflow_project_dir(&runs[0].run_id)
            .join("_state/workflow_progress.json")
            .exists());
        // binding 清掉、宿主**归档**（一律归档不删除——"聊天为空"推不出"目录无价值"）、标记文件存在
        let b = std::fs::read_to_string(
            crate::bridge::paths::sessions_root().join("_skill_bindings.json"),
        )
        .unwrap();
        assert!(!b.contains("project_dir"));
        assert!(!crate::bridge::paths::sessions_root().join("s1.json").exists());
        assert!(!crate::bridge::paths::sessions_root().join("s1").exists());
        let arch = crate::bridge::paths::sessions_root().join("_archived_workflow_hosts");
        assert!(arch.join("s1.json").exists(), "空聊天宿主也必须归档而非删除");
        assert!(arch.join("s1").is_dir(), "宿主目录（含 workspace 残留物）必须归档");
        assert!(crate::bridge::paths::workflows_root()
            .join(".migration_done")
            .exists());
        // marker 内容是审计摘要 JSON：deleted_hosts 字段已废弃（C1），统一计 archived
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                crate::bridge::paths::workflows_root().join(".migration_done"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["migrated"], 1);
        assert_eq!(marker["archived"], 1);
        assert!(marker.get("deleted_hosts").is_none(), "删除分支已整体移除");
        // 幂等：再跑一次不炸不重复
        migrate_if_needed().unwrap();
        assert_eq!(crate::workflow_runs::list_runs().unwrap().len(), 1);
        std::env::remove_var("PINVOU3_HOME");
    }

    // 补充：带 user 消息的宿主 → 归档进 _archived_workflow_hosts/ 而非删除；
    // title 优先 brief.json 的 user_request_raw 前 16 字（按字符，中文安全）。
    #[test]
    fn archives_host_with_user_messages_instead_of_deleting() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-arch-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let state = crate::bridge::paths::sessions_root()
            .join("s2/workspace/ppt-20260602-090000-internal_quick/_state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("workflow_progress.json"),
            r#"{"scenario":"internal_quick","roles":{"pm":{"status":"running"}}}"#,
        )
        .unwrap();
        std::fs::write(
            state.join("brief.json"),
            r#"{"user_request_raw":"做一份AI智慧教室解决方案汇报材料给客户看"}"#,
        )
        .unwrap();
        // 宿主带真实用户消息（system 注入不算，所以混入一条 system 验证判据）
        std::fs::write(
            crate::bridge::paths::sessions_root().join("s2.json"),
            r#"{"metadata":{"id":"s2"},"messages":[{"role":"system","content":"注入"},{"role":"user","content":"开始做PPT"}]}"#,
        )
        .unwrap();
        std::fs::write(
            crate::bridge::paths::sessions_root().join("_skill_bindings.json"),
            r#"{"s2":{"name":"h3c-ppt","phases":[],"project_dir":"/old/p2"},"chat1":{"name":"deep-research","phases":[],"project_dir":null}}"#,
        )
        .unwrap();

        migrate_if_needed().unwrap();

        let runs = crate::workflow_runs::list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert_eq!(runs[0].scenario, "internal_quick");
        assert_eq!(runs[0].title, "做一份AI智慧教室解决方案汇报材"); // 前 16 字符（含 A/I）
        // 宿主归档而非删除
        let arch = crate::bridge::paths::sessions_root().join("_archived_workflow_hosts");
        assert!(arch.join("s2.json").exists());
        assert!(arch.join("s2").is_dir());
        assert!(!crate::bridge::paths::sessions_root().join("s2.json").exists());
        // project_dir 为 null 的普通聊天 binding 必须保留
        let b = std::fs::read_to_string(
            crate::bridge::paths::sessions_root().join("_skill_bindings.json"),
        )
        .unwrap();
        assert!(b.contains("chat1"));
        assert!(!b.contains("s2"));
        std::env::remove_var("PINVOU3_HOME");
    }

    // 缺口1a：崩在 create_dir_all(run_dir) 与 rename 之间 → 空 run_dir 已存在、
    // 项目还在旧址。续跑判据必须看 target（run_dir/project/）而不是 run_dir，
    // 否则永久跳过 → 项目滞留旧址，宿主清理时被连带删除。
    #[test]
    fn resumes_rename_when_run_dir_exists_but_target_missing() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-resume-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let dir_name = "ppt-20260603-100000-internal_quick";
        let old_proj = crate::bridge::paths::sessions_root()
            .join("s3/workspace")
            .join(dir_name);
        std::fs::create_dir_all(old_proj.join("_state")).unwrap();
        std::fs::write(
            old_proj.join("_state/workflow_progress.json"),
            r#"{"scenario":"internal_quick","roles":{"pm":{"status":"completed"}}}"#,
        )
        .unwrap();
        // 模拟半途崩：run_dir 已建出来（空壳），project/ 没搬进去
        let run_id = derive_run_id(dir_name).unwrap();
        std::fs::create_dir_all(crate::bridge::paths::workflow_run_dir(&run_id)).unwrap();

        migrate_if_needed().unwrap();

        // 项目真的搬进去了，而不是被"目标 run 已存在"误跳过
        assert!(
            crate::bridge::paths::workflow_project_dir(&run_id)
                .join("_state/workflow_progress.json")
                .exists(),
            "续跑必须重试 rename 把项目搬进 target"
        );
        assert!(!old_proj.exists(), "旧址项目应已搬走");
        let runs = crate::workflow_runs::list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, run_id);
        std::env::remove_var("PINVOU3_HOME");
    }

    // 缺口1b：不同 session 下同名 ppt-* 目录推出同 run_id（撞名）。
    // 第二个项目绝不能静默跳过（否则宿主清理连项目一起删），
    // 必须对 "<dirname>#<n>" 重新散列去重，两个 run 都迁移成功。
    #[test]
    fn dedups_run_id_when_same_dir_name_in_two_sessions() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-dedup-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let dir_name = "ppt-20260604-110000-solution_deck";
        for (sid, tag) in [("sA", "A"), ("sB", "B")] {
            let proj = crate::bridge::paths::sessions_root()
                .join(sid)
                .join("workspace")
                .join(dir_name);
            std::fs::create_dir_all(proj.join("_state")).unwrap();
            std::fs::write(
                proj.join("_state/workflow_progress.json"),
                r#"{"scenario":"solution_deck","roles":{"pm":{"status":"completed"}}}"#,
            )
            .unwrap();
            std::fs::write(proj.join("who.txt"), tag).unwrap();
        }

        migrate_if_needed().unwrap();

        // 两个 run 都在、台账 2 条、run_id 互异且都过统一校验
        let runs = crate::workflow_runs::list_runs().unwrap();
        assert_eq!(runs.len(), 2, "两个同名项目都必须被迁移");
        assert_ne!(runs[0].run_id, runs[1].run_id);
        let mut tags: Vec<String> = runs
            .iter()
            .map(|r| {
                assert!(crate::workflow_runs::is_valid_run_id(&r.run_id));
                std::fs::read_to_string(
                    crate::bridge::paths::workflow_project_dir(&r.run_id).join("who.txt"),
                )
                .unwrap()
            })
            .collect();
        tags.sort();
        assert_eq!(tags, vec!["A", "B"], "两个真实项目内容都得在新家");
        std::env::remove_var("PINVOU3_HOME");
    }

    // 缺口2：步骤 5 归档失败不能吞错——binding 已删（步骤4），若静默成功写 marker，
    // 宿主文件成无主孤儿。失败必须返回 Err、不写 marker、保留 .pending_hosts.json；
    // 清掉障碍重跑 → 从 pending 文件续跑步骤 5，成功后写 marker + 删 pending。
    #[test]
    fn host_archive_failure_returns_err_and_resumes_from_pending_file() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-pending-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let sessions = crate::bridge::paths::sessions_root();
        // 宿主 s9：带用户消息（要归档）+ 有 s9/ 目录
        std::fs::create_dir_all(sessions.join("s9/workspace")).unwrap();
        std::fs::write(sessions.join("s9/workspace/note.txt"), "残留").unwrap();
        std::fs::write(
            sessions.join("s9.json"),
            r#"{"metadata":{"id":"s9"},"messages":[{"role":"user","content":"做PPT"}]}"#,
        )
        .unwrap();
        std::fs::write(
            sessions.join("_skill_bindings.json"),
            r#"{"s9":{"name":"h3c-ppt","phases":[],"project_dir":"/old/p9"}}"#,
        )
        .unwrap();
        // 障碍：归档目标位置被同名普通文件占住 → rename(s9/ → 归档/s9) 必失败
        let arch = sessions.join("_archived_workflow_hosts");
        std::fs::create_dir_all(&arch).unwrap();
        std::fs::write(arch.join("s9"), "挡路文件").unwrap();

        let err = migrate_if_needed();
        assert!(err.is_err(), "归档失败必须上报 Err，不能吞错");
        let marker = crate::bridge::paths::workflows_root().join(".migration_done");
        assert!(!marker.exists(), "失败不写 marker（与模块头注释自洽）");
        let pending = crate::bridge::paths::workflows_root().join(".pending_hosts.json");
        let pending_s = std::fs::read_to_string(&pending)
            .expect(".pending_hosts.json 必须残留供续跑");
        assert!(pending_s.contains("s9"));

        // 清掉障碍重跑 → 续跑步骤 5 成功（此时 bindings 已清空，sid 只能来自 pending 文件）
        std::fs::remove_file(arch.join("s9")).unwrap();
        migrate_if_needed().unwrap();
        assert!(marker.exists());
        assert!(!pending.exists(), "步骤 5 处理完必须删掉 pending 文件");
        assert!(arch.join("s9.json").exists(), "宿主 json 归档到位");
        assert!(arch.join("s9").is_dir(), "宿主目录归档到位");
        assert!(!sessions.join("s9").exists());
        assert!(!sessions.join("s9.json").exists());
        std::env::remove_var("PINVOU3_HOME");
    }

    // I1：rename 成功后、写 run.json 前崩 → project/ 在新家但没有 run.json，
    // rebuild_index 只认 run.json → run 永久隐身。migrate 必须在 rebuild_index 前
    // 跑修复扫描：按目录名（即 run_id）+ project 内 progress 推导补写 run.json。
    #[test]
    fn repair_scan_rewrites_missing_run_json_before_rebuild_index() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-repair-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        // 手工造"半截 run"：合法 run_id 目录 + project/_state/progress，无 run.json
        let run_id = "wf-20260605-1300-abcd";
        let state = crate::bridge::paths::workflow_project_dir(run_id).join("_state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("workflow_progress.json"),
            r#"{"scenario":"internal_quick","roles":{"pm":{"status":"completed"}}}"#,
        )
        .unwrap();
        assert!(!crate::bridge::paths::workflow_run_dir(run_id)
            .join("run.json")
            .exists());

        migrate_if_needed().unwrap();

        // run.json 补写到位，台账含该条且状态/场景派生自 progress
        assert!(crate::bridge::paths::workflow_run_dir(run_id)
            .join("run.json")
            .exists());
        let runs = crate::workflow_runs::list_runs().unwrap();
        assert_eq!(runs.len(), 1, "隐身 run 必须重新出现在台账");
        assert_eq!(runs[0].run_id, run_id);
        assert_eq!(runs[0].status, "completed");
        assert_eq!(runs[0].scenario, "internal_quick");
        std::env::remove_var("PINVOU3_HOME");
    }

    // Minor：sid_is_safe 拒掉的可疑 sid 不计 failed（重试一万次也不会好），
    // 但要在 marker 摘要里留下 skipped_unsafe_sids 计数（审计可见）。
    #[test]
    fn unsafe_sids_skipped_and_counted_in_summary() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = format!("/tmp/pinvou3-migrate-unsafe-{}", std::process::id());
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(crate::bridge::paths::workflows_root()).unwrap();
        std::fs::create_dir_all(crate::bridge::paths::sessions_root()).unwrap();
        // 上一轮残留的 pending 文件里混入路径穿越形态 sid
        std::fs::write(
            crate::bridge::paths::workflows_root().join(".pending_hosts.json"),
            r#"["../evil", "..\\evil2"]"#,
        )
        .unwrap();

        migrate_if_needed().unwrap();

        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                crate::bridge::paths::workflows_root().join(".migration_done"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["skipped_unsafe_sids"], 2);
        // 可疑 sid 不算 failed：迁移正常完成、pending 文件撤掉
        assert!(!crate::bridge::paths::workflows_root()
            .join(".pending_hosts.json")
            .exists());
        std::env::remove_var("PINVOU3_HOME");
    }
}
