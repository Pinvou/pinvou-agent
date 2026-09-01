//! 运行时记忆渲染：注入块（`<pinvou_user_memory>`）拼装、设备快照文档生成，
//! 以及 runtime prompt 文件刷新/保证存在。
//!
//! 抽离自 `mod.rs`。`render_from_parts` 与快照文档是纯函数；`refresh_runtime_prompt`
//! 等会触发过期归档并落盘 runtime 文件。

use std::io as stdio;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::platform::paths;

use super::io;
use super::types::{
    CURRENT_FOCUS_MAX_INJECTED, InjectedMemoryItem, MemoryProfile, NeverMemoryItem,
    PendingMemoryItem, PreferenceFile, RECENT_ACTIVITY_MAX_INJECTED, RECENT_WORK_MAX_INJECTED,
    RecentWorkItem, RuntimeMemorySnapshot, TimedMemoryItem, WorkContextFile,
};
use super::util::{push_if_present, write_text_atomic};

pub fn render_memory_block() -> stdio::Result<(String, Vec<InjectedMemoryItem>)> {
    let profile = io::load_profile()?;
    let preferences = io::load_preferences()?;
    let work_context = io::load_work_context()?;
    let current_focus = io::load_current_focus()?;
    let recent_activity = io::load_recent_activity()?;
    let legacy_recent_work = io::load_recent_work()?;
    Ok(render_from_parts(
        &profile,
        &preferences,
        &work_context,
        &current_focus,
        &recent_activity,
        &legacy_recent_work,
        Utc::now(),
    ))
}

pub(super) fn render_from_parts(
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    legacy_recent_work: &[RecentWorkItem],
    now: DateTime<Utc>,
) -> (String, Vec<InjectedMemoryItem>) {
    let mut items = Vec::new();
    let mut profile_lines = Vec::new();

    if !profile.identity.call_name.is_empty() {
        profile_lines.push(format!("- 称呼：{}", profile.identity.call_name));
        items.push(InjectedMemoryItem {
            id: "profile.call_name".to_string(),
            kind: "profile".to_string(),
            text: format!("称呼：{}", profile.identity.call_name),
        });
    }
    if !profile.identity.assistant_alias.is_empty() {
        profile_lines.push(format!("- 助手昵称：{}", profile.identity.assistant_alias));
        items.push(InjectedMemoryItem {
            id: "profile.assistant_alias".to_string(),
            kind: "profile".to_string(),
            text: format!("助手昵称：{}", profile.identity.assistant_alias),
        });
    }

    let mut habits = Vec::new();
    push_if_present(&mut habits, &profile.conventions.language);
    if !profile.conventions.doc_standard.is_empty() {
        habits.push(format!("公文格式遵 {}", profile.conventions.doc_standard));
    }
    if !profile.conventions.number_usage.is_empty() {
        habits.push(format!("数字用法遵 {}", profile.conventions.number_usage));
    }
    habits.extend(profile.conventions.style_notes.iter().cloned());
    if !habits.is_empty() {
        let text = habits.join("；");
        profile_lines.push(format!("- 输出习惯：{text}"));
        items.push(InjectedMemoryItem {
            id: "profile.conventions".to_string(),
            kind: "profile".to_string(),
            text: format!("输出习惯：{text}"),
        });
    }

    let mut preference_lines = Vec::new();
    for pref in preferences
        .iter()
        .filter(|p| p.scope == "unconditional" && !p.text.is_empty())
        .take(20)
    {
        preference_lines.push(format!("- {}", pref.text));
        items.push(InjectedMemoryItem {
            id: if pref.id.is_empty() {
                format!("preference.{}", pref.topic)
            } else {
                pref.id.clone()
            },
            kind: "preference".to_string(),
            text: pref.text.clone(),
        });
    }

    let mut work_context_lines = Vec::new();
    for ctx in work_context
        .iter()
        .filter(|item| !item.text.is_empty())
        .take(5)
    {
        work_context_lines.push(format!("- {}", ctx.text));
        items.push(InjectedMemoryItem {
            id: if ctx.id.is_empty() {
                format!("work_context.{}", ctx.topic)
            } else {
                ctx.id.clone()
            },
            kind: "work_context".to_string(),
            text: ctx.text.clone(),
        });
    }

    let mut focus_lines = Vec::new();
    for item in io::active_timed_memory(current_focus, now)
        .into_iter()
        .take(CURRENT_FOCUS_MAX_INJECTED)
    {
        focus_lines.push(format!("- {}", item.text));
        items.push(InjectedMemoryItem {
            id: format!("current_focus.{}", item.id),
            kind: "current_focus".to_string(),
            text: item.text.clone(),
        });
    }

    let mut activity_lines = Vec::new();
    for item in io::active_timed_memory(recent_activity, now)
        .into_iter()
        .take(RECENT_ACTIVITY_MAX_INJECTED)
    {
        activity_lines.push(format!("- {}", item.text));
        items.push(InjectedMemoryItem {
            id: format!("recent_activity.{}", item.id),
            kind: "recent_activity".to_string(),
            text: item.text.clone(),
        });
    }

    let mut recent_lines = Vec::new();
    for item in io::active_recent_work(legacy_recent_work, now)
        .into_iter()
        .take(RECENT_WORK_MAX_INJECTED)
    {
        let text = if item.summary.is_empty() {
            format!("正在处理：{}", item.title)
        } else {
            format!("正在处理：{}（{}）", item.title, item.summary)
        };
        recent_lines.push(format!("- {text}"));
        items.push(InjectedMemoryItem {
            id: format!("recent_work.{}", item.id),
            kind: "current_focus".to_string(),
            text,
        });
    }

    if profile_lines.is_empty()
        && preference_lines.is_empty()
        && work_context_lines.is_empty()
        && focus_lines.is_empty()
        && activity_lines.is_empty()
        && recent_lines.is_empty()
    {
        return (String::new(), items);
    }

    let mut block =
        String::from("<pinvou_user_memory>\n权威层级：低于用户当前指令；与本轮冲突以本轮为准。\n");
    if !profile_lines.is_empty() {
        block.push_str("画像：\n");
        block.push_str(&profile_lines.join("\n"));
        block.push('\n');
    }
    if !preference_lines.is_empty() {
        block.push_str("长期偏好：\n");
        block.push_str(&preference_lines.join("\n"));
        block.push('\n');
    }
    if !work_context_lines.is_empty() {
        block.push_str("工作背景：\n");
        block.push_str(&work_context_lines.join("\n"));
        block.push('\n');
    }
    if !focus_lines.is_empty() {
        block.push_str("当前关注（会过期）：\n");
        block.push_str(&focus_lines.join("\n"));
        block.push('\n');
    }
    if !activity_lines.is_empty() {
        block.push_str("近期动态（会过期）：\n");
        block.push_str(&activity_lines.join("\n"));
        block.push('\n');
    }
    if !recent_lines.is_empty() {
        block.push_str("当前关注（兼容旧近期工作，会过期）：\n");
        block.push_str(&recent_lines.join("\n"));
        block.push('\n');
    }
    block.push_str("</pinvou_user_memory>\n");
    (block, items)
}

pub fn refresh_runtime_prompt(session_id: &str) -> stdio::Result<RuntimeMemorySnapshot> {
    let _guard = io::write_lock().lock();
    if !io::memory_enabled() {
        return io::disabled_runtime_snapshot(session_id);
    }
    let now = Utc::now();
    let _ = io::refresh_recent_work_expiry_unlocked(now)?;
    let _ = io::refresh_timed_memory_expiry_unlocked("current_focus", now)?;
    let _ = io::refresh_timed_memory_expiry_unlocked("recent_activity", now)?;
    let (block, items) = render_memory_block()?;
    let path = io::runtime_prompt_path(session_id);
    write_text_atomic(&path, &block)?;
    Ok(RuntimeMemorySnapshot {
        session_id: session_id.to_string(),
        runtime_path: path.display().to_string(),
        block,
        items,
    })
}

pub fn ensure_runtime_prompt(session_id: &str) -> stdio::Result<PathBuf> {
    let path = io::runtime_prompt_path(session_id);
    if !path.exists() {
        let _ = refresh_runtime_prompt(session_id)?;
    }
    Ok(path)
}

pub fn runtime_snapshot(session_id: &str) -> stdio::Result<RuntimeMemorySnapshot> {
    refresh_runtime_prompt(session_id)
}

pub fn write_memory_snapshot_document(
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    recent_work: &[RecentWorkItem],
    pending: &[PendingMemoryItem],
    never: &[NeverMemoryItem],
    runtime: Option<&RuntimeMemorySnapshot>,
) -> stdio::Result<PathBuf> {
    let path = io::snapshot_path();
    let generated_at = Utc::now().to_rfc3339();
    let doc = render_memory_snapshot_document(
        &generated_at,
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        runtime,
    )?;
    write_text_atomic(&path, &doc)?;
    Ok(path)
}

#[allow(clippy::too_many_arguments)]
fn render_memory_snapshot_document(
    generated_at: &str,
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    recent_work: &[RecentWorkItem],
    pending: &[PendingMemoryItem],
    never: &[NeverMemoryItem],
    runtime: Option<&RuntimeMemorySnapshot>,
) -> stdio::Result<String> {
    use std::fmt::Write as _;
    let mut doc = String::new();
    let _ = writeln!(&mut doc, "# PINVOU 设备记忆快照");
    let _ = writeln!(&mut doc);
    let _ = writeln!(&mut doc, "- 生成时间：{generated_at}");
    let _ = writeln!(
        &mut doc,
        "- 来源目录：{}",
        paths::user_memory_dir().display()
    );
    let _ = writeln!(
        &mut doc,
        "- 说明：本文件由“同步记忆”生成，仅用于查看、迁移排查和调试；结构化记忆文件仍是事实源。"
    );
    let _ = writeln!(
        &mut doc,
        "- 注意：`_pending`、`_never` 和 `runtime` 不会作为长期记忆直接注入模型。"
    );

    let _ = writeln!(&mut doc, "\n## 运行时注入摘要");
    if let Some(snapshot) = runtime {
        if snapshot.block.trim().is_empty() {
            let _ = writeln!(&mut doc, "当前没有可注入的有效记忆。");
        } else {
            let _ = writeln!(&mut doc, "```text\n{}```", snapshot.block);
        }
        let _ = writeln!(&mut doc, "- runtime 文件：{}", snapshot.runtime_path);
    } else {
        let _ = writeln!(&mut doc, "当前没有绑定 session，未生成运行时注入摘要。");
    }

    let _ = writeln!(&mut doc, "\n## 长期记忆");
    let _ = writeln!(&mut doc, "\n### 用户画像");
    push_snapshot_line(&mut doc, "用户称呼", &profile.identity.call_name);
    push_snapshot_line(&mut doc, "助手昵称", &profile.identity.assistant_alias);
    push_snapshot_line(&mut doc, "默认语言", &profile.conventions.language);
    push_snapshot_line(&mut doc, "文档标准", &profile.conventions.doc_standard);
    push_snapshot_line(&mut doc, "数字用法", &profile.conventions.number_usage);
    if !profile.conventions.style_notes.is_empty() {
        for note in &profile.conventions.style_notes {
            push_snapshot_line(&mut doc, "输出习惯", note);
        }
    }
    if profile.identity.call_name.is_empty()
        && profile.identity.assistant_alias.is_empty()
        && profile.conventions.language.is_empty()
        && profile.conventions.doc_standard.is_empty()
        && profile.conventions.number_usage.is_empty()
        && profile.conventions.style_notes.is_empty()
    {
        let _ = writeln!(&mut doc, "暂无用户画像。");
    }

    let _ = writeln!(&mut doc, "\n### 长期偏好");
    if preferences.is_empty() {
        let _ = writeln!(&mut doc, "暂无长期偏好。");
    } else {
        for item in preferences {
            let _ = writeln!(
                &mut doc,
                "- [{}] {}",
                snapshot_one_line(&item.topic),
                snapshot_one_line(&item.text)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n### 工作背景");
    if work_context.is_empty() {
        let _ = writeln!(&mut doc, "暂无工作背景。");
    } else {
        for item in work_context {
            let _ = writeln!(
                &mut doc,
                "- [{}] {}（置信度：{:.2}，来源：{}）",
                snapshot_one_line(&item.topic),
                snapshot_one_line(&item.text),
                item.confidence,
                snapshot_optional(&item.source)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n## 近期记忆");
    let _ = writeln!(&mut doc, "\n### 当前关注");
    if current_focus.is_empty() {
        let _ = writeln!(&mut doc, "暂无当前关注。");
    } else {
        for item in current_focus {
            push_timed_snapshot_line(&mut doc, item);
        }
    }

    let _ = writeln!(&mut doc, "\n### 近期动态");
    if recent_activity.is_empty() {
        let _ = writeln!(&mut doc, "暂无近期动态。");
    } else {
        for item in recent_activity {
            push_timed_snapshot_line(&mut doc, item);
        }
    }

    let _ = writeln!(&mut doc, "\n### 兼容旧近期工作");
    if recent_work.is_empty() {
        let _ = writeln!(&mut doc, "暂无旧近期工作。");
    } else {
        for item in recent_work {
            let summary = if item.summary.is_empty() {
                String::new()
            } else {
                format!("：{}", snapshot_one_line(&item.summary))
            };
            let _ = writeln!(
                &mut doc,
                "- [{}] {}{}（更新：{}，过期：{}）",
                snapshot_one_line(&item.status),
                snapshot_one_line(&item.title),
                summary,
                snapshot_optional(&item.updated_at),
                snapshot_optional(&item.expires_at)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n## 管理数据");
    let _ = writeln!(&mut doc, "\n### 待确认候选（不注入模型）");
    if pending.is_empty() {
        let _ = writeln!(&mut doc, "暂无待确认候选。");
    } else {
        for item in pending {
            let _ = writeln!(
                &mut doc,
                "- [{} / {}] {}",
                snapshot_one_line(&item.status),
                snapshot_one_line(&item.kind),
                snapshot_one_line(&item.content)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n### 不再提示（不注入模型）");
    if never.is_empty() {
        let _ = writeln!(&mut doc, "暂无不再提示记录。");
    } else {
        for item in never {
            let _ = writeln!(
                &mut doc,
                "- {}（原因：{}）",
                snapshot_one_line(&item.pattern),
                snapshot_optional(&item.reason)
            );
        }
    }

    let raw = serde_json::json!({
        "schema": "pinvou-memory-snapshot/v1",
        "generated_at": generated_at,
        "source_dir": paths::user_memory_dir().display().to_string(),
        "files": {
            "profile": io::profile_path().display().to_string(),
            "preferences": paths::user_memory_preferences_dir().display().to_string(),
            "work_context": io::work_context_dir().display().to_string(),
            "current_focus": io::current_focus_path().display().to_string(),
            "recent_activity": io::recent_activity_path().display().to_string(),
            "recent_work": io::recent_work_path().display().to_string(),
            "pending": io::pending_memory_path().display().to_string(),
            "never": io::never_memory_path().display().to_string(),
            "runtime_dir": paths::user_memory_runtime_dir().display().to_string()
        },
        "profile": profile,
        "preferences": preferences,
        "work_context": work_context,
        "current_focus": current_focus,
        "recent_activity": recent_activity,
        "recent_work": recent_work,
        "pending": pending,
        "never": never,
        "runtime": runtime
    });
    let raw = serde_json::to_string_pretty(&raw).map_err(super::util::invalid_data)?;
    let _ = writeln!(&mut doc, "\n## 结构化快照");
    let _ = writeln!(&mut doc, "~~~json\n{raw}\n~~~");
    Ok(doc)
}

fn push_snapshot_line(doc: &mut String, label: &str, value: &str) {
    use std::fmt::Write as _;
    if value.trim().is_empty() {
        return;
    }
    let _ = writeln!(
        doc,
        "- **{}**：{}",
        snapshot_one_line(label),
        snapshot_one_line(value)
    );
}

fn push_timed_snapshot_line(doc: &mut String, item: &TimedMemoryItem) {
    use std::fmt::Write as _;
    let _ = writeln!(
        doc,
        "- [{} / {} / {}天] {}（更新：{}，来源：{}）",
        snapshot_one_line(&item.status),
        snapshot_one_line(&item.topic),
        item.ttl_days,
        snapshot_one_line(&item.text),
        snapshot_optional(&item.updated_at),
        snapshot_optional(&item.source)
    );
}

fn snapshot_one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn snapshot_optional(value: &str) -> String {
    let value = snapshot_one_line(value);
    if value.is_empty() {
        "无".to_string()
    } else {
        value
    }
}
