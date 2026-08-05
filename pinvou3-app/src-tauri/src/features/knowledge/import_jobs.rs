//! 知识集批量导入任务的持久化状态机。
//!
//! 任务与文件状态写入知识库同一 SQLite；进程异常退出后将 `running` 文件恢复为
//! `pending`，由用户明确续跑。正式文档分块的原子提交由 `L1Store` 完成。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const FAILED_FILES_PREVIEW_LIMIT: usize = 20;
const FAILED_FILES_PAGE_MAX: usize = 50;

#[derive(Debug, Clone)]
pub(super) struct ImportItem {
    pub id: i64,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FailedImportFile {
    pub item_id: i64,
    pub name: String,
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FailedImportFilePage {
    pub files: Vec<FailedImportFile>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportJobState {
    pub job_id: Option<String>,
    pub running: bool,
    pub resumable: bool,
    pub collection_id: i64,
    /// idle / preparing / parsing / interrupted / done / done_with_errors / cancelled
    pub phase: String,
    /// 已处理文件数（成功、跳过和失败）。
    pub done: u64,
    pub total: u64,
    pub completed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub current_path: Option<String>,
    pub current_chunks_done: u64,
    pub current_chunks_total: u64,
    pub failed_files: Vec<FailedImportFile>,
    pub started_at: i64,
    pub finished_at: i64,
}

#[derive(Clone)]
pub(super) struct ImportJobStore {
    conn: Arc<Mutex<Connection>>,
}

impl ImportJobStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// 进程已经消失，数据库里的 running/preparing 不可能仍在执行。将当前文件退回
    /// pending，保留已暂存的 chunks，等待用户续跑。
    pub fn recover_interrupted(&self) -> rusqlite::Result<Option<ImportJobState>> {
        let c = self.conn.lock();
        let now = now();
        c.execute(
            "UPDATE knowledge_import_items SET state='pending',updated_at=?1 \
             WHERE state='running' AND job_id IN (SELECT id FROM knowledge_import_jobs \
             WHERE state IN ('preparing','running'))",
            params![now],
        )?;
        c.execute(
            "UPDATE knowledge_import_jobs SET state='interrupted',updated_at=?1 \
             WHERE state IN ('preparing','running')",
            params![now],
        )?;
        drop(c);
        self.latest_state()
    }

    pub fn create(&self, collection_id: i64, roots: &[PathBuf]) -> rusqlite::Result<String> {
        let id = format!(
            "kb-import-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let roots: Vec<String> = roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let roots_json = serde_json::to_string(&roots).unwrap_or_else(|_| "[]".into());
        let now = now();
        let changed = self.conn.lock().execute(
            "INSERT INTO knowledge_import_jobs(id,collection_id,roots_json,state,created_at,updated_at) \
             SELECT ?1,?2,?3,'preparing',?4,?4 WHERE EXISTS(SELECT 1 FROM collections WHERE id=?2)",
            params![id, collection_id, roots_json, now],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(id)
    }

    pub fn roots(&self, job_id: &str) -> rusqlite::Result<Vec<PathBuf>> {
        let raw: String = self.conn.lock().query_row(
            "SELECT roots_json FROM knowledge_import_jobs WHERE id=?1",
            params![job_id],
            |r| r.get(0),
        )?;
        let roots: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(roots.into_iter().map(PathBuf::from).collect())
    }

    pub fn prepare_items(&self, job_id: &str, files: &[PathBuf]) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let state: String = tx.query_row(
            "SELECT state FROM knowledge_import_jobs WHERE id=?1",
            params![job_id],
            |r| r.get(0),
        )?;
        if state == "cancelled" {
            tx.commit()?;
            return Ok(());
        }
        let now = now();
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO knowledge_import_items(job_id,path,name,state,updated_at) \
                 VALUES(?1,?2,?3,'pending',?4)",
            )?;
            for path in files {
                let path_str = path.to_string_lossy();
                let name = path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("(unnamed)");
                stmt.execute(params![job_id, path_str.as_ref(), name, now])?;
            }
        }
        tx.execute(
            "UPDATE knowledge_import_jobs SET state='running',updated_at=?2 WHERE id=?1",
            params![job_id, now],
        )?;
        tx.commit()
    }

    pub fn item_count(&self, job_id: &str) -> rusqlite::Result<i64> {
        self.conn.lock().query_row(
            "SELECT COUNT(*) FROM knowledge_import_items WHERE job_id=?1",
            params![job_id],
            |r| r.get(0),
        )
    }

    pub fn resume(&self, job_id: &str) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let now = now();
        let changed = tx.execute(
            "UPDATE knowledge_import_jobs SET state='running',updated_at=?2,finished_at=NULL \
             WHERE id=?1 AND state='interrupted'",
            params![job_id, now],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.execute(
            "UPDATE knowledge_import_items SET state='pending',updated_at=?2 \
             WHERE job_id=?1 AND state='running'",
            params![job_id, now],
        )?;
        tx.commit()
    }

    pub fn retry_item(&self, job_id: &str, item_id: i64) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let now = now();
        // 只允许从中断或部分失败的任务重试单个失败文件；已取消或已完成的任务不能被单文件
        // 重试悄悄复活。job 与 item 的状态迁移在同一事务内，任一不满足条件整体回滚。
        let job_changed = tx.execute(
            "UPDATE knowledge_import_jobs SET state='running',updated_at=?2,finished_at=NULL \
             WHERE id=?1 AND state IN ('interrupted','done_with_errors')",
            params![job_id, now],
        )?;
        if job_changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        let item_changed = tx.execute(
            "UPDATE knowledge_import_items SET state='pending',error=NULL,updated_at=?3 \
             WHERE id=?2 AND job_id=?1 AND state='failed'",
            params![job_id, item_id, now],
        )?;
        if item_changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.commit()
    }

    pub fn claim_next(&self, job_id: &str) -> rusqlite::Result<Option<ImportItem>> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let row = tx
            .query_row(
                "SELECT id,path FROM knowledge_import_items \
                 WHERE job_id=?1 AND state='pending' ORDER BY id LIMIT 1",
                params![job_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((id, path)) = row else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute(
            "UPDATE knowledge_import_items SET state='running',attempts=attempts+1,\
             error=NULL,updated_at=?2 WHERE id=?1",
            params![id, now()],
        )?;
        tx.commit()?;
        Ok(Some(ImportItem {
            id,
            path: PathBuf::from(path),
        }))
    }

    /// 只允许当前任务仍在运行时把它已 claim 的文件标为失败。取消任务或删除知识集后，
    /// 晚到的后台结果必须成为 no-op，不能把 cancelled 状态复活为 failed。
    pub fn mark_failed(&self, job_id: &str, item_id: i64, error: &str) {
        let _ = self.conn.lock().execute(
            "UPDATE knowledge_import_items SET state='failed',error=?3,updated_at=?4 \
             WHERE id=?2 AND job_id=?1 AND state='running' \
             AND EXISTS(SELECT 1 FROM knowledge_import_jobs WHERE id=?1 AND state='running')",
            params![job_id, item_id, error, now()],
        );
    }

    pub fn interrupt(&self, job_id: &str) {
        let mut c = self.conn.lock();
        let Ok(tx) = c.transaction() else { return };
        let now = now();
        let _ = tx.execute(
            "UPDATE knowledge_import_items SET state='pending',updated_at=?2 \
             WHERE job_id=?1 AND state='running'",
            params![job_id, now],
        );
        let _ = tx.execute(
            "UPDATE knowledge_import_jobs SET state='interrupted',updated_at=?2 \
             WHERE id=?1 AND state IN ('preparing','running')",
            params![job_id, now],
        );
        let _ = tx.commit();
    }

    pub fn cancel(&self, job_id: &str) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let now = now();
        tx.execute(
            "UPDATE knowledge_import_jobs SET state='cancelled',updated_at=?2,finished_at=?2 \
             WHERE id=?1 AND state IN ('preparing','running','interrupted')",
            params![job_id, now],
        )?;
        tx.execute(
            "UPDATE knowledge_import_items SET state='cancelled',updated_at=?2 \
             WHERE job_id=?1 AND state IN ('pending','running')",
            params![job_id, now],
        )?;
        tx.execute(
            "DELETE FROM knowledge_import_staged_chunks WHERE job_id=?1",
            params![job_id],
        )?;
        tx.commit()
    }

    pub fn is_cancelled(&self, job_id: &str) -> bool {
        self.conn
            .lock()
            .query_row(
                "SELECT state='cancelled' FROM knowledge_import_jobs WHERE id=?1",
                params![job_id],
                |r| r.get(0),
            )
            .unwrap_or(true)
    }

    pub fn finish(&self, job_id: &str) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        let cancelled: bool = c.query_row(
            "SELECT state='cancelled' FROM knowledge_import_jobs WHERE id=?1",
            params![job_id],
            |r| r.get(0),
        )?;
        let pending: i64 = c.query_row(
            "SELECT COUNT(*) FROM knowledge_import_items WHERE job_id=?1 AND state IN ('pending','running')",
            params![job_id],
            |r| r.get(0),
        )?;
        if pending > 0 || cancelled {
            return Ok(());
        }
        let failed: i64 = c.query_row(
            "SELECT COUNT(*) FROM knowledge_import_items WHERE job_id=?1 AND state='failed'",
            params![job_id],
            |r| r.get(0),
        )?;
        let state = if failed > 0 {
            "done_with_errors"
        } else {
            "done"
        };
        let now = now();
        c.execute(
            "UPDATE knowledge_import_jobs SET state=?2,updated_at=?3,finished_at=?3 WHERE id=?1",
            params![job_id, state, now],
        )?;
        Ok(())
    }

    pub fn state(&self, job_id: &str) -> rusqlite::Result<ImportJobState> {
        self.read_state(Some(job_id))?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn latest_state(&self) -> rusqlite::Result<Option<ImportJobState>> {
        self.read_state(None)
    }

    pub fn failed_files_page(
        &self,
        job_id: &str,
        offset: usize,
        limit: usize,
    ) -> rusqlite::Result<FailedImportFilePage> {
        let limit = limit.clamp(1, FAILED_FILES_PAGE_MAX);
        let c = self.conn.lock();
        let exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM knowledge_import_jobs WHERE id=?1)",
            params![job_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        let mut stmt = c.prepare(
            "SELECT id,name,path,COALESCE(error,'') FROM knowledge_import_items \
             WHERE job_id=?1 AND state='failed' ORDER BY id LIMIT ?2 OFFSET ?3",
        )?;
        let files = stmt
            .query_map(params![job_id, (limit + 1) as i64, offset as i64], |r| {
                Ok(FailedImportFile {
                    item_id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    error: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = files.len() > limit;
        Ok(FailedImportFilePage {
            files: files.into_iter().take(limit).collect(),
            next_offset: has_more.then_some((offset + limit) as u64),
        })
    }

    fn read_state(&self, job_id: Option<&str>) -> rusqlite::Result<Option<ImportJobState>> {
        let c = self.conn.lock();
        let row = if let Some(id) = job_id {
            c.query_row(
                "SELECT id,collection_id,state,created_at,COALESCE(finished_at,0) \
                 FROM knowledge_import_jobs WHERE id=?1",
                params![id],
                read_job_row,
            )
            .optional()?
        } else {
            // 当前运行/可恢复/有失败项的任务始终优先于普通历史任务，避免新的成功任务
            // 遮蔽仍需用户处理的旧失败任务。多个失败任务按更新时间依次处理即可。
            c.query_row(
                "SELECT id,collection_id,state,created_at,COALESCE(finished_at,0) \
                 FROM knowledge_import_jobs ORDER BY \
                 CASE state \
                   WHEN 'preparing' THEN 0 WHEN 'running' THEN 0 \
                   WHEN 'interrupted' THEN 1 WHEN 'done_with_errors' THEN 2 \
                   ELSE 3 END, updated_at DESC LIMIT 1",
                [],
                read_job_row,
            )
            .optional()?
        };
        let Some((id, collection_id, phase, started_at, finished_at)) = row else {
            return Ok(None);
        };
        let (total, completed, skipped, failed): (i64, i64, i64, i64) = c.query_row(
            "SELECT COUNT(*),\
             COALESCE(SUM(CASE WHEN state='completed' THEN 1 ELSE 0 END),0),\
             COALESCE(SUM(CASE WHEN state='skipped' THEN 1 ELSE 0 END),0),\
             COALESCE(SUM(CASE WHEN state='failed' THEN 1 ELSE 0 END),0)\
             FROM knowledge_import_items WHERE job_id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let current = c
            .query_row(
                "SELECT path,completed_chunks,total_chunks FROM knowledge_import_items \
                 WHERE job_id=?1 AND (state='running' OR (state='pending' AND completed_chunks>0)) \
                 ORDER BY CASE state WHEN 'running' THEN 0 ELSE 1 END,completed_chunks DESC,id LIMIT 1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let mut stmt = c.prepare(
            "SELECT id,name,path,COALESCE(error,'') FROM knowledge_import_items \
             WHERE job_id=?1 AND state='failed' ORDER BY id LIMIT ?2",
        )?;
        let failed_files = stmt
            .query_map(params![id, FAILED_FILES_PREVIEW_LIMIT as i64], |r| {
                Ok(FailedImportFile {
                    item_id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    error: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (current_path, current_chunks_done, current_chunks_total) = current
            .map(|(path, done, total)| (Some(path), done as u64, total as u64))
            .unwrap_or((None, 0, 0));
        Ok(Some(ImportJobState {
            job_id: Some(id),
            running: matches!(phase.as_str(), "preparing" | "running"),
            resumable: phase == "interrupted",
            collection_id,
            phase: match phase.as_str() {
                "running" => "parsing".into(),
                other => other.into(),
            },
            done: (completed + skipped + failed) as u64,
            total: total as u64,
            completed: completed as u64,
            skipped: skipped as u64,
            failed: failed as u64,
            current_path,
            current_chunks_done,
            current_chunks_total,
            failed_files,
            started_at,
            finished_at,
        }))
    }
}

fn read_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, i64, String, i64, i64)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

pub(super) fn unique_existing_files(files: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut files: Vec<_> = files.into_iter().filter(|p| p.is_file()).collect();
    files.sort_by(|a, b| path_key(a).cmp(&path_key(b)));
    files.dedup_by(|a, b| path_key(a) == path_key(b));
    files
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::knowledge::l1::L1Store;
    use crate::features::knowledge::store::Store;

    fn setup() -> (ImportJobStore, L1Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn_arc();
        let l1 = L1Store::new(conn.clone(), None);
        let collection_id = l1.create_collection("测试", None, None).unwrap();
        (ImportJobStore::new(conn), l1, collection_id)
    }

    #[test]
    fn restart_preserves_completed_items_and_chunk_checkpoint() {
        let (jobs, _l1, collection_id) = setup();
        let roots = vec![PathBuf::from("/tmp")];
        let job_id = jobs.create(collection_id, &roots).unwrap();
        jobs.prepare_items(
            &job_id,
            &[PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md")],
        )
        .unwrap();
        let first = jobs.claim_next(&job_id).unwrap().unwrap();
        let second = {
            let c = jobs.conn.lock();
            c.execute(
                "UPDATE knowledge_import_items SET state='completed' WHERE id=?1",
                params![first.id],
            )
            .unwrap();
            drop(c);
            jobs.claim_next(&job_id).unwrap().unwrap()
        };
        jobs.conn
            .lock()
            .execute(
                "INSERT INTO knowledge_import_staged_chunks(job_id,item_id,ord,text,n_tokens) VALUES(?1,?2,0,'部分内容',4)",
                params![job_id, second.id],
            )
            .unwrap();
        jobs.conn
            .lock()
            .execute(
                "UPDATE knowledge_import_items SET total_chunks=3,completed_chunks=1 WHERE id=?1",
                params![second.id],
            )
            .unwrap();

        let recovered = jobs.recover_interrupted().unwrap().unwrap();
        assert!(recovered.resumable);
        assert_eq!(recovered.completed, 1);
        let pending: String = jobs
            .conn
            .lock()
            .query_row(
                "SELECT state FROM knowledge_import_items WHERE id=?1",
                params![second.id],
                |r| r.get(0),
            )
            .unwrap();
        let staged: i64 = jobs
            .conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM knowledge_import_staged_chunks WHERE item_id=?1",
                params![second.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, "pending");
        assert_eq!(staged, 1, "重启恢复不能丢失文件内分块检查点");
    }

    #[test]
    fn retry_only_requeues_selected_failed_item() {
        let (jobs, _l1, collection_id) = setup();
        let job_id = jobs.create(collection_id, &[]).unwrap();
        jobs.prepare_items(
            &job_id,
            &[PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md")],
        )
        .unwrap();
        let first = jobs.claim_next(&job_id).unwrap().unwrap();
        jobs.mark_failed(&job_id, first.id, "失败 A");
        let second = jobs.claim_next(&job_id).unwrap().unwrap();
        jobs.mark_failed(&job_id, second.id, "失败 B");
        jobs.finish(&job_id).unwrap();

        jobs.retry_item(&job_id, first.id).unwrap();
        let states: Vec<(i64, String)> = {
            let c = jobs.conn.lock();
            let mut stmt = c
                .prepare("SELECT id,state FROM knowledge_import_items ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(states[0], (first.id, "pending".into()));
        assert_eq!(states[1], (second.id, "failed".into()));
    }

    #[test]
    fn actionable_job_is_not_hidden_and_all_failed_items_are_returned() {
        let (jobs, _l1, collection_id) = setup();
        let failed_job = jobs.create(collection_id, &[]).unwrap();
        let files: Vec<PathBuf> = (0..105)
            .map(|n| PathBuf::from(format!("/tmp/failed-{n}.md")))
            .collect();
        jobs.prepare_items(&failed_job, &files).unwrap();
        while let Some(item) = jobs.claim_next(&failed_job).unwrap() {
            jobs.mark_failed(&failed_job, item.id, "解析失败");
        }
        jobs.finish(&failed_job).unwrap();

        let successful_job = jobs.create(collection_id, &[]).unwrap();
        jobs.prepare_items(&successful_job, &[]).unwrap();
        jobs.finish(&successful_job).unwrap();

        let visible = jobs.latest_state().unwrap().unwrap();
        assert_eq!(visible.job_id.as_deref(), Some(failed_job.as_str()));
        assert_eq!(visible.failed, 105);
        assert_eq!(visible.failed_files.len(), FAILED_FILES_PREVIEW_LIMIT);
        let first = jobs.failed_files_page(&failed_job, 0, 50).unwrap();
        let second = jobs
            .failed_files_page(&failed_job, first.next_offset.unwrap() as usize, 50)
            .unwrap();
        let third = jobs
            .failed_files_page(&failed_job, second.next_offset.unwrap() as usize, 50)
            .unwrap();
        assert_eq!(first.files.len(), 50);
        assert_eq!(second.files.len(), 50);
        assert_eq!(third.files.len(), 5);
        assert_eq!(third.next_offset, None);
    }

    #[test]
    fn late_failure_does_not_revive_cancelled_item() {
        let (jobs, _l1, collection_id) = setup();
        let job_id = jobs.create(collection_id, &[]).unwrap();
        jobs.prepare_items(&job_id, &[PathBuf::from("/tmp/a.md")])
            .unwrap();
        let item = jobs.claim_next(&job_id).unwrap().unwrap();
        jobs.cancel(&job_id).unwrap();
        jobs.mark_failed(&job_id, item.id, "晚到的错误");

        let state: String = jobs
            .conn
            .lock()
            .query_row(
                "SELECT state FROM knowledge_import_items WHERE id=?1",
                params![item.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "cancelled");
    }

    #[test]
    fn cancel_propagates_sqlite_failure_and_rolls_back_state() {
        let (jobs, _l1, collection_id) = setup();
        let job_id = jobs.create(collection_id, &[]).unwrap();
        jobs.prepare_items(&job_id, &[PathBuf::from("/tmp/a.md")])
            .unwrap();
        jobs.conn
            .lock()
            .execute("DROP TABLE knowledge_import_staged_chunks", [])
            .unwrap();

        assert!(jobs.cancel(&job_id).is_err());
        let state = jobs.state(&job_id).unwrap();
        assert!(state.running, "取消事务失败时不得伪装为已取消");
    }

    #[test]
    fn retry_file_cannot_revive_cancelled_job() {
        let (jobs, _l1, collection_id) = setup();
        let job_id = jobs.create(collection_id, &[]).unwrap();
        jobs.prepare_items(
            &job_id,
            &[PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md")],
        )
        .unwrap();
        let first = jobs.claim_next(&job_id).unwrap().unwrap();
        jobs.mark_failed(&job_id, first.id, "失败 A");
        // 取消后 failed 项保持 failed；重试它绝不能把 cancelled 任务悄悄翻回 running，
        // 否则其余被取消的文件会被永久遗弃。
        jobs.cancel(&job_id).unwrap();

        assert!(jobs.retry_item(&job_id, first.id).is_err());
        let job_state = jobs.state(&job_id).unwrap();
        assert_eq!(
            job_state.phase, "cancelled",
            "已取消任务不得被单文件重试复活"
        );
        let item_state: String = jobs
            .conn
            .lock()
            .query_row(
                "SELECT state FROM knowledge_import_items WHERE id=?1",
                params![first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(item_state, "failed", "重试被拒时失败项状态不得变动");
    }
}
