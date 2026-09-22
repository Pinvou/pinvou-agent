use super::prelude::*;

#[tauri::command]
pub async fn list_personas() -> Result<Vec<crate::features::personas::PersonaSummary>, String> {
    Ok(crate::features::personas::all_summaries())
}

/// 读单个专家的完整人设正文（详情 modal 预览用）。
#[tauri::command]
pub async fn read_persona_body(persona_id: String) -> Result<String, String> {
    crate::features::personas::get(&persona_id)
        .map(|c| c.body.clone())
        .ok_or_else(|| format!("未知专家面具: {persona_id}"))
}

/// 给当前 session 加持一张专家面具（点卡片"加持给 AI"）。
/// Side B: 存 persona_id + 把完整 body 挂为 pending（下一条 chat 一次性 prepend）；
/// 之后每 turn 只注入轻锚点。返回摘要供前端渲染挂件 + 系统消息。
#[tauri::command]
pub async fn equip_persona(
    session_id: String,
    persona_id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<crate::features::personas::PersonaSummary, String> {
    equip_persona_state_with(
        &store,
        &session_id,
        &persona_id,
        || {},
        || {
            super::sessions::emit_session_event(
                &app,
                "session:persona_changed",
                &session_id,
                "equipped",
            );
        },
    )
}

fn equip_persona_state_with(
    store: &SessionStore,
    session_id: &str,
    persona_id: &str,
    after_read: impl FnOnce(),
    after_publish: impl FnOnce(),
) -> Result<crate::features::personas::PersonaSummary, String> {
    crate::features::personas::with_card(persona_id, |card| {
        after_read();
        let summary = card.summary();
        store.set_persona(
            session_id,
            Some(persona_id.to_string()),
            Some(crate::features::personas::equip_body_injection(card)),
        );
        after_publish();
        summary
    })
    .ok_or_else(|| format!("未知专家面具: {persona_id}"))
}

// ── 用户自创卡 CRUD ────────────────────────────────────────────────

/// 前端建/改卡传入的字段(不含 id/source —— create 由后端生成 id;update 用 persona_id)。
#[derive(Debug, serde::Deserialize)]
pub struct PersonaInput {
    pub name: String,
    pub dept: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
}

impl PersonaInput {
    fn into_card(self, id: String) -> crate::features::personas::PersonaCard {
        crate::features::personas::PersonaCard {
            id,
            dept: self.dept,
            name: self.name,
            description: self.description,
            emoji: if self.emoji.is_empty() {
                "🃏".into()
            } else {
                self.emoji
            },
            color: if self.color.is_empty() {
                "#7C3AED".into()
            } else {
                self.color
            },
            body: self.body,
            source: "user".into(),
            // 用户自创卡都是干活的领域卡,照常带全量工具;元卡标记只属内置卡。
            conversational_only: false,
        }
    }
}

/// 新建自制卡 → 写 `~/.pinvou3/user/personas/<id>.json`,返回摘要(含生成的 id)。
/// 下一次普通多智能体 turn 会从更新后的卡池生成同轮 Fleet 配置与候选提醒。
#[tauri::command]
pub async fn create_persona(
    input: PersonaInput,
) -> Result<crate::features::personas::PersonaSummary, String> {
    crate::features::personas::create_user_persona(input.into_card(String::new()))
}

/// 编辑自制卡(persona_id 必须是 user- 前缀)。
#[tauri::command]
pub async fn update_persona(
    persona_id: String,
    input: PersonaInput,
) -> Result<crate::features::personas::PersonaSummary, String> {
    crate::features::personas::update_user_persona(input.into_card(persona_id))
}

/// 删除自制卡。
#[tauri::command]
pub async fn delete_persona(
    persona_id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    delete_persona_state_with(&store, &persona_id, |session_ids| {
        for session_id in session_ids {
            super::sessions::emit_session_event(
                &app,
                "session:persona_changed",
                session_id,
                "unequipped",
            );
        }
    })?;
    Ok(())
}

#[cfg(test)]
fn delete_persona_state(store: &SessionStore, persona_id: &str) -> Result<Vec<String>, String> {
    delete_persona_state_with(store, persona_id, |_| {})
}

fn delete_persona_state_with(
    store: &SessionStore,
    persona_id: &str,
    after_clear: impl FnOnce(&[String]),
) -> Result<Vec<String>, String> {
    crate::features::personas::delete_user_persona_with(persona_id, || {
        let session_ids = store.remove_persona_from_all(persona_id);
        after_clear(&session_ids);
        session_ids
    })
}

/// 保存某 session 的卡牌加持/卸下事件时间线(sidecar,不进 messages)。
/// events 是前端定义的 opaque JSON 数组,后端只透明落盘。
#[tauri::command]
pub async fn save_session_persona_events(
    session_id: String,
    events: serde_json::Value,
) -> Result<(), String> {
    let path = crate::platform::paths::session_persona_events(&session_id);
    super::sessions::write_session_sidecar(&path, &events)
}

/// 读某 session 的卡牌事件时间线(无则返回空数组)。
#[tauri::command]
pub async fn get_session_persona_events(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::platform::paths::session_persona_events(&session_id);
    match std::fs::read_to_string(&path) {
        Ok(txt) => Ok(serde_json::from_str(&txt).unwrap_or_else(|_| serde_json::json!([]))),
        Err(_) => Ok(serde_json::json!([])),
    }
}

/// Pinvou 召唤检阅时间线（opaque JSON，后端透明落盘，同 persona_events 范式）。
/// 前端每次召唤后存，load_session 时读回，rerender 按 pos 插回审查卡——独立于
/// messages，绝不进 LLM 上下文（设计 §6 / `docs/品悟v4-常驻检阅助手设计.md`）。
/// 落盘前保留盘上已有的 resolution：防止后续全量 save（典型=核账 record 用不含 resolution
/// 的快照）冲掉 Boss 已做的逐条裁决。按数组下标对齐——pinvouReviews 是 append-only、每条
/// review 内容不可变，下标稳定可靠。new 自带 resolution 就用 new（允许 Boss 改裁决）；new
/// 缺失才继承 old。根治「resolution 写进 sidecar 后被无 resolution 的全量 save 覆盖」的实测 bug。
fn preserve_resolutions(path: &std::path::Path, new: serde_json::Value) -> serde_json::Value {
    let old: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(txt) => match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(_) => return new,
        },
        Err(_) => return new,
    };
    merge_resolutions(old, new)
}

/// 纯合并逻辑（抽出便于单测）：new 缺 resolution 的条目继承 old 同下标的。
pub(super) fn merge_resolutions(
    old: serde_json::Value,
    mut new: serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;
    let old_arr = match old.as_array() {
        Some(a) => a,
        None => return new,
    };
    let new_arr = match new.as_array_mut() {
        Some(a) => a,
        None => return new,
    };
    for (i, entry) in new_arr.iter_mut().enumerate() {
        let old_entry = match old_arr.get(i) {
            Some(e) => e,
            None => continue,
        };
        for field in ["issues", "recommendations"] {
            let ptr = format!("/review/{field}");
            let old_items = match old_entry.pointer(&ptr).and_then(Value::as_array) {
                Some(a) => a,
                None => continue,
            };
            let new_items = match entry.pointer_mut(&ptr).and_then(Value::as_array_mut) {
                Some(a) => a,
                None => continue,
            };
            for (j, ni) in new_items.iter_mut().enumerate() {
                if ni.get("resolution").is_some_and(|v| !v.is_null()) {
                    continue; // new 已带裁决，尊重 new（含 Boss 改裁决/取消）
                }
                if let Some(old_res) = old_items.get(j).and_then(|x| x.get("resolution")) {
                    if !old_res.is_null() {
                        if let Some(obj) = ni.as_object_mut() {
                            obj.insert("resolution".to_string(), old_res.clone());
                        }
                    }
                }
            }
        }
    }
    new
}

#[tauri::command]
pub async fn save_session_pinvou_reviews(
    session_id: String,
    reviews: serde_json::Value,
) -> Result<(), String> {
    let path = crate::platform::paths::session_pinvou_reviews(&session_id);
    let merged = preserve_resolutions(&path, reviews);
    super::sessions::write_session_sidecar(&path, &merged)
}

/// 读某 session 的 Pinvou 审查时间线（无则返回空数组）。
#[tauri::command]
pub async fn get_session_pinvou_reviews(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::platform::paths::session_pinvou_reviews(&session_id);
    match std::fs::read_to_string(&path) {
        Ok(txt) => Ok(serde_json::from_str(&txt).unwrap_or_else(|_| serde_json::json!([]))),
        Err(_) => Ok(serde_json::json!([])),
    }
}

/// 摘下当前 session 的专家面具（点挂件取消 / 卡片"已加持"再点）。
#[tauri::command]
pub async fn unequip_persona(
    session_id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store.set_active_persona(&session_id, None);
    store.set_pending_persona_body(&session_id, None);
    super::sessions::emit_session_event(&app, "session:persona_changed", &session_id, "unequipped");
    Ok(())
}

/// 查当前 session 加持的专家面具摘要（前端启动 / 切 session 时拉，用于还原挂件）。
/// 无加持返回 None。
#[tauri::command]
pub async fn get_active_persona(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<crate::features::personas::PersonaSummary>, String> {
    Ok(store
        .active_persona_id(&session_id)
        .and_then(|pid| crate::features::personas::get(&pid).map(|c| c.summary())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::personas::PersonaCard;
    use crate::platform::paths::tests::ENV_LOCK;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn delete_waits_for_in_flight_equip_then_clears_its_state() {
        let _environment = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let root = std::env::temp_dir().join(format!(
            "pinvou3-persona-equip-delete-race-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        // SAFETY: ENV_LOCK serializes process-wide test environment changes.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        crate::features::personas::reload_user();

        let summary = crate::features::personas::create_user_persona(PersonaCard {
            id: String::new(),
            dept: "specialized".into(),
            name: "Race card".into(),
            description: String::new(),
            emoji: "R".into(),
            color: "#000000".into(),
            body: "SECRET PERSONA BODY".into(),
            source: "user".into(),
            conversational_only: false,
        })
        .expect("create test persona");
        let store = SessionStore::boot_at_test_dir(&root).expect("boot session store");
        let session_id = "chat-persona-race";

        let (card_read_tx, card_read_rx) = mpsc::channel();
        let (resume_equip_tx, resume_equip_rx) = mpsc::channel();
        let equip_store = store.clone();
        let equip_persona_id = summary.id.clone();
        let equip = std::thread::spawn(move || {
            equip_persona_state_with(
                &equip_store,
                session_id,
                &equip_persona_id,
                || {
                    card_read_tx.send(()).expect("signal card read");
                    resume_equip_rx.recv().expect("resume equip");
                },
                || {},
            )
        });
        card_read_rx.recv().expect("equip read card");

        let (delete_started_tx, delete_started_rx) = mpsc::channel();
        let (delete_done_tx, delete_done_rx) = mpsc::channel();
        let delete_store = store.clone();
        let delete_persona_id = summary.id.clone();
        let deletion = std::thread::spawn(move || {
            delete_started_tx.send(()).expect("signal delete start");
            let result = delete_persona_state(&delete_store, &delete_persona_id);
            delete_done_tx.send(()).expect("signal delete done");
            result
        });
        delete_started_rx.recv().expect("delete started");
        let deleted_before_equip_committed = delete_done_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        resume_equip_tx.send(()).expect("release equip");

        equip.join().expect("equip thread").expect("equip succeeds");
        let affected = deletion
            .join()
            .expect("delete thread")
            .expect("delete succeeds");
        assert!(
            !deleted_before_equip_committed,
            "delete must not finish between card read and persona-state publication"
        );
        assert_eq!(affected, vec![session_id.to_string()]);
        let state = store.mode_state(session_id);
        assert!(state.active_persona.is_none());
        assert!(state.pending_persona_body.is_none());
        assert!(crate::features::personas::get(&summary.id).is_none());

        match previous_home {
            // SAFETY: ENV_LOCK remains held through restoration and cache reload.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: ENV_LOCK remains held through restoration and cache reload.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        crate::features::personas::reload_user();
        let _ = std::fs::remove_dir_all(root);
    }
}
