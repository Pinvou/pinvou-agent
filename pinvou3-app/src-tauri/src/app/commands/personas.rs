#[tauri::command]
pub async fn list_personas() -> Result<Vec<crate::personas::PersonaSummary>, String> {
    Ok(crate::personas::all_summaries())
}

/// 读单个专家的完整人设正文（详情 modal 预览用）。
#[tauri::command]
pub async fn read_persona_body(persona_id: String) -> Result<String, String> {
    crate::personas::get(&persona_id)
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
    store: State<'_, SessionStore>,
) -> Result<crate::personas::PersonaSummary, String> {
    let card =
        crate::personas::get(&persona_id).ok_or_else(|| format!("未知专家面具: {persona_id}"))?;
    let summary = card.summary();
    store.set_pending_persona_body(
        &session_id,
        Some(crate::personas::equip_body_injection(&card)),
    );
    store.set_active_persona(&session_id, Some(persona_id));
    Ok(summary)
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
    fn into_card(self, id: String) -> crate::personas::PersonaCard {
        crate::personas::PersonaCard {
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
#[tauri::command]
pub async fn create_persona(
    input: PersonaInput,
) -> Result<crate::personas::PersonaSummary, String> {
    crate::personas::create_user_persona(input.into_card(String::new()))
}

/// 编辑自制卡(persona_id 必须是 user- 前缀)。
#[tauri::command]
pub async fn update_persona(
    persona_id: String,
    input: PersonaInput,
) -> Result<crate::personas::PersonaSummary, String> {
    crate::personas::update_user_persona(input.into_card(persona_id))
}

/// 删除自制卡。
#[tauri::command]
pub async fn delete_persona(persona_id: String) -> Result<(), String> {
    crate::personas::delete_user_persona(&persona_id)
}

/// 保存某 session 的卡牌加持/卸下事件时间线(sidecar,不进 messages)。
/// events 是前端定义的 opaque JSON 数组,后端只透明落盘。
#[tauri::command]
pub async fn save_session_persona_events(
    session_id: String,
    events: serde_json::Value,
) -> Result<(), String> {
    let path = crate::bridge::paths::session_persona_events(&session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 session 目录失败: {e}"))?;
    }
    let json = serde_json::to_string(&events).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写卡牌事件失败: {e}"))
}

/// 读某 session 的卡牌事件时间线(无则返回空数组)。
#[tauri::command]
pub async fn get_session_persona_events(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::bridge::paths::session_persona_events(&session_id);
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
fn merge_resolutions(old: serde_json::Value, mut new: serde_json::Value) -> serde_json::Value {
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
                if ni.get("resolution").map_or(false, |v| !v.is_null()) {
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
    let path = crate::bridge::paths::session_pinvou_reviews(&session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 session 目录失败: {e}"))?;
    }
    let merged = preserve_resolutions(&path, reviews);
    let json = serde_json::to_string(&merged).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写 Pinvou 审查失败: {e}"))
}

/// 读某 session 的 Pinvou 审查时间线（无则返回空数组）。
#[tauri::command]
pub async fn get_session_pinvou_reviews(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::bridge::paths::session_pinvou_reviews(&session_id);
    match std::fs::read_to_string(&path) {
        Ok(txt) => Ok(serde_json::from_str(&txt).unwrap_or_else(|_| serde_json::json!([]))),
        Err(_) => Ok(serde_json::json!([])),
    }
}

/// 摘下当前 session 的专家面具（点挂件取消 / 卡片"已加持"再点）。
#[tauri::command]
pub async fn unequip_persona(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store.set_active_persona(&session_id, None);
    store.set_pending_persona_body(&session_id, None);
    Ok(())
}

/// 查当前 session 加持的专家面具摘要（前端启动 / 切 session 时拉，用于还原挂件）。
/// 无加持返回 None。
#[tauri::command]
pub async fn get_active_persona(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<crate::personas::PersonaSummary>, String> {
    Ok(store
        .active_persona_id(&session_id)
        .and_then(|pid| crate::personas::get(&pid).map(|c| c.summary())))
}
