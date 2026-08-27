use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};

use crate::{
    backend::{PermissionControlStrength, PermissionMode},
    model::Model,
    theme,
};

use super::keymap;

pub(super) fn render_help(frame: &mut Frame<'_>, area: Rect, model: &Model, commands: &[&str]) {
    let width = area.width.saturating_sub(8).clamp(20, 60);
    let desired_height = (commands.len() as u16).saturating_add(7);
    let height = desired_height.min(area.height.saturating_sub(4)).max(5);
    let popup = centered_rect(area, width, height);
    let mut lines = vec![Line::from(Span::styled(
        "Commands",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        commands
            .iter()
            .map(|command| Line::from(format!("  {command}"))),
    );
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(keymap(model), theme::muted())));
    render_popup(frame, popup, "Help · commands", lines);
}

pub(super) fn render_runtime(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let pending = model
        .pending_runtime_switch
        .as_ref()
        .map(|switch| format!("Switching to {}…", switch.target));
    let mut lines = vec![
        Line::from(Span::styled(
            "Choose the runtime attached to this logical session",
            theme::muted(),
        )),
        Line::default(),
    ];
    if model.pending_runtime_list.is_some() {
        lines.push(Line::from("Loading runtimes…"));
    } else if model.runtime_candidates.is_empty() {
        lines.push(Line::from(format!(
            "● {} ({}) · {}",
            model.runtime.display_name,
            model.runtime.id,
            if model.runtime.available {
                "available"
            } else {
                "unavailable"
            }
        )));
    } else {
        let fixed_rows = if pending.is_some() { 5 } else { 4 };
        let capacity = overlay_content_capacity(area).saturating_sub(fixed_rows);
        for row in visible_rows(
            model.runtime_candidates.len(),
            model.selected_runtime,
            capacity,
        ) {
            let Some(index) = row else {
                lines.push(ellipsis_line());
                continue;
            };
            let runtime = &model.runtime_candidates[index];
            let active = if runtime.id == model.runtime.id {
                "current"
            } else {
                ""
            };
            let availability = if runtime.available {
                "available"
            } else {
                "unavailable"
            };
            let capabilities = runtime
                .capability_summary
                .as_deref()
                .map(|summary| format!(" · {summary}"))
                .unwrap_or_default();
            lines.push(choice_line(
                index == model.selected_runtime,
                format!(
                    "{}  ·  {}  ·  {active} {availability}{capabilities}",
                    runtime.display_name, runtime.id,
                ),
            ));
        }
    }
    lines.push(Line::default());
    if let Some(pending) = pending {
        lines.push(Line::from(pending));
    }
    lines.push(Line::from(keymap(model)));
    render_list_popup(frame, area, "Switch runtime", lines);
}

pub(super) fn render_session(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let query = model.session_query.to_ascii_lowercase();
    let candidates = model
        .session_candidates
        .iter()
        .filter(|session| {
            query.is_empty()
                || session.title.to_ascii_lowercase().contains(&query)
                || session.id.to_ascii_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    let mut lines = vec![
        Line::from(Span::styled(
            "Recent logical sessions in this workspace",
            theme::muted(),
        )),
        Line::from(format!("Search  › {}_", model.session_query)),
        Line::default(),
    ];
    if model.pending_session_list.is_some() {
        lines.push(Line::from("Loading sessions…"));
    } else if candidates.is_empty() {
        lines.push(Line::from("No matching sessions"));
    } else {
        let capacity = overlay_content_capacity(area).saturating_sub(5);
        for row in visible_rows(candidates.len(), model.selected_session, capacity) {
            let Some(index) = row else {
                lines.push(ellipsis_line());
                continue;
            };
            let session = candidates[index];
            lines.push(choice_line(
                index == model.selected_session,
                format!(
                    "{}  ·  {}  ·  {}  ·  {}",
                    session.title, session.runtime_id, session.status, session.last_active_at
                ),
            ));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(
        "Type to filter · ↑/↓ select · Enter resume · Esc close",
    ));
    render_list_popup(frame, area, "Resume session", lines);
}

pub(super) fn render_model(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "Available from the active {} runtime",
                model.runtime.display_name
            ),
            theme::muted(),
        )),
        Line::default(),
    ];
    if model.pending_model_list.is_some() {
        lines.push(Line::from("Loading models…"));
    } else {
        let capacity = overlay_content_capacity(area).saturating_sub(4);
        for row in visible_rows(model.model_candidates.len(), model.selected_model, capacity) {
            let Some(index) = row else {
                lines.push(ellipsis_line());
                continue;
            };
            let candidate = &model.model_candidates[index];
            let current = if Some(candidate.id.as_str()) == model.model_id.as_deref() {
                "current"
            } else if candidate.is_default {
                "default"
            } else {
                ""
            };
            let availability = if candidate.available {
                ""
            } else {
                "unsupported"
            };
            let provider = candidate
                .provider_display_name
                .as_deref()
                .or(candidate.provider_id.as_deref())
                .unwrap_or("Runtime");
            let credential = if candidate.configured {
                ""
            } else if candidate.requires_api_key {
                "API key needed"
            } else {
                "not configured"
            };
            let level = (Some(candidate.id.as_str()) == model.model_id.as_deref())
                .then(|| model.model_level.as_deref())
                .flatten()
                .or(candidate.default_reasoning_level.as_deref())
                .map(|level| format!("level {level}"))
                .unwrap_or_default();
            let details = [current, availability, credential, level.as_str()]
                .into_iter()
                .filter(|detail| !detail.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
            let details = if details.is_empty() {
                String::new()
            } else {
                format!("  ·  {details}")
            };
            lines.push(choice_line(
                index == model.selected_model,
                format!(
                    "{} / {}  ·  {}{details}",
                    provider, candidate.display_name, candidate.id
                ),
            ));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from("↑/↓ select · Enter levels/switch · Esc close"));
    render_list_popup(frame, area, "Select model", lines);
}

pub(super) fn render_model_level(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(candidate) = model.model_candidates.get(model.selected_model) else {
        return;
    };
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Reasoning level for {}", candidate.display_name),
            theme::muted(),
        )),
        Line::default(),
    ];
    let capacity = overlay_content_capacity(area).saturating_sub(4);
    for row in visible_rows(
        candidate.supported_reasoning_levels.len(),
        model.selected_model_level,
        capacity,
    ) {
        let Some(index) = row else {
            lines.push(ellipsis_line());
            continue;
        };
        let level = &candidate.supported_reasoning_levels[index];
        let current = Some(candidate.id.as_str()) == model.model_id.as_deref()
            && Some(level.as_str()) == model.model_level.as_deref();
        let default = candidate.default_reasoning_level.as_deref() == Some(level.as_str());
        let detail = if current {
            "  ·  current"
        } else if default {
            "  ·  default"
        } else {
            ""
        };
        lines.push(choice_line(
            index == model.selected_model_level,
            format!("{level}{detail}"),
        ));
    }
    lines.push(Line::default());
    lines.push(Line::from("↑/↓ select · Enter switch · Esc models"));
    render_list_popup(frame, area, "Select reasoning level", lines);
}

pub(super) fn render_api_key(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(candidate) = model.model_candidates.get(model.selected_model) else {
        return;
    };
    let provider = candidate
        .provider_display_name
        .as_deref()
        .or(candidate.provider_id.as_deref())
        .unwrap_or("Provider");
    let masked = if model.credential_composer.len() == 0 {
        "Paste or type API key".to_owned()
    } else {
        "•".repeat(model.credential_composer.len().min(48))
    };
    let state = if model.pending_model_credential.is_some() {
        "Saving to the shared Desktop secure credential store…"
    } else {
        "The key is masked and is never written to settings.json or logs."
    };
    let lines = vec![
        Line::from(Span::styled(
            format!("{provider} · {}", candidate.display_name),
            theme::muted(),
        )),
        Line::default(),
        Line::from(format!("API key  › {masked}")),
        Line::default(),
        Line::from(Span::styled(state, theme::muted())),
        Line::default(),
        Line::from("Enter save · Esc models"),
    ];
    render_list_popup(frame, area, "Configure Provider", lines);
}

pub(super) fn render_permission(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let compact = area.width < 80 || area.height < 22;
    let mut lines = Vec::new();
    if !compact {
        lines.extend([
            Line::from(Span::styled(
                format!(
                    "Pinvou policy mapped to {} runtime controls",
                    model.runtime.display_name
                ),
                theme::muted(),
            )),
            Line::default(),
        ]);
    }
    if model.pending_permissions.is_some() {
        lines.push(Line::from("Loading permission modes…"));
    } else if let Some(status) = &model.permission_status {
        let strength = permission_strength_label(status.control_strength);
        lines.push(Line::from(format!(
            "Control: {strength}  ·  native: {}  ·  sandbox: {}",
            status.native_mode.as_deref().unwrap_or("unknown"),
            status.sandbox.as_deref().unwrap_or("unknown")
        )));
        if !compact {
            lines.push(Line::from(Span::styled(
                format!("Capability evidence: {}", status.evidence_version),
                theme::muted(),
            )));
        }
        lines.push(Line::default());
        for (index, profile) in status.supported_profiles.iter().enumerate() {
            let current = if *profile == model.permission_profile {
                "current"
            } else {
                ""
            };
            let description = match profile {
                PermissionMode::Request => "Ask before side effects or escalation",
                PermissionMode::Assisted => "Known low-risk work may proceed automatically",
                PermissionMode::FullAccess => "Confirmation required; runtime hard guards remain",
            };
            lines.push(choice_line(
                index == model.selected_permission,
                format!("{}  ·  {current}  ·  {description}", profile.as_str()),
            ));
        }
        if !compact && !status.residual_guards.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "Remaining protections",
                theme::muted(),
            )));
        }
        if !compact {
            for guard in &status.residual_guards {
                lines.push(Line::from(format!("  · {guard}")));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from("↑/↓ select · Enter switch · Esc close"));
    render_list_popup(frame, area, "Permission mode", lines);
}

pub(super) fn render_full_access_confirmation(frame: &mut Frame<'_>, area: Rect) {
    render_list_popup(
        frame,
        area,
        "Confirm full access",
        vec![
            Line::from("Full access disables routine approval and sandbox restrictions."),
            Line::from("Only continue in a workspace you trust."),
            Line::default(),
            Line::from("Enter confirm · Esc cancel"),
        ],
    );
}

const fn permission_strength_label(strength: PermissionControlStrength) -> &'static str {
    match strength {
        PermissionControlStrength::Enforced => "enforced",
        PermissionControlStrength::Partial => "partial",
        PermissionControlStrength::Unsupported => "unsupported",
    }
}

fn choice_line(selected: bool, content: String) -> Line<'static> {
    let marker = if selected { "› " } else { "  " };
    let style = if selected {
        theme::selected()
    } else {
        theme::text()
    };
    Line::from(Span::styled(format!("{marker}{content}"), style))
}

fn overlay_content_capacity(area: Rect) -> usize {
    area.height.saturating_sub(6).max(3) as usize
}

fn visible_rows(total: usize, selected: usize, capacity: usize) -> Vec<Option<usize>> {
    if total == 0 || capacity == 0 {
        return Vec::new();
    }
    if total <= capacity {
        return (0..total).map(Some).collect();
    }
    let selected = selected.min(total - 1);
    let item_capacity = capacity.saturating_sub(2).max(1);
    let mut start = selected.saturating_sub(item_capacity / 2);
    if start + item_capacity > total {
        start = total - item_capacity;
    }
    let end = (start + item_capacity).min(total);
    let mut rows = Vec::with_capacity(capacity);
    if start > 0 {
        rows.push(None);
    }
    rows.extend((start..end).map(Some));
    if end < total {
        rows.push(None);
    }
    rows
}

fn ellipsis_line() -> Line<'static> {
    Line::from(Span::styled("  …", theme::muted()))
}

fn render_list_popup(frame: &mut Frame<'_>, area: Rect, title: &str, mut lines: Vec<Line<'_>>) {
    let max_content_lines = overlay_content_capacity(area);
    if lines.len() > max_content_lines {
        let footer = lines.pop();
        lines.truncate(max_content_lines.saturating_sub(1));
        if let Some(footer) = footer {
            lines.push(footer);
        }
    }
    let width = area.width.saturating_sub(8).clamp(32, 76);
    let height = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(4))
        .max(5);
    let popup = centered_rect(area, width, height);
    render_popup(frame, popup, title, lines);
}

fn render_popup(frame: &mut Frame<'_>, popup: Rect, title: &str, lines: Vec<Line<'_>>) {
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(theme::border())
                .title_style(theme::accent_bold())
                .padding(Padding::horizontal(1)),
        ),
        popup,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let horizontal = area.width.saturating_sub(width) / 2;
    let vertical = area.height.saturating_sub(height) / 2;
    area.inner(Margin {
        horizontal,
        vertical,
    })
}
