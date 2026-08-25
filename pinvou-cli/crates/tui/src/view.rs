use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::model::{
    ConnectionState, Interaction, Model, Overlay, ToolState, TranscriptEntry, TurnState,
};

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 16;
const KEYMAP_IDLE: &str = "Enter send  ·  Ctrl+R runtime  ·  Ctrl+C detach/exit";
const KEYMAP_STARTING: &str = "Starting/Waiting  ·  Ctrl+C detach";
const KEYMAP_ACTIVE: &str = "Esc interrupt  ·  Ctrl+C detach";
const KEYMAP_INTERRUPT_PENDING: &str = "Cancelling/Waiting  ·  Ctrl+C detach";
const KEYMAP_APPROVAL: &str = "1 Allow once  ·  3 Deny  ·  Ctrl+C detach";
const KEYMAP_APPROVAL_RESOLVING: &str = "Approving/Waiting  ·  Ctrl+C detach";
const KEYMAP_INPUT: &str = "Enter submit  ·  Ctrl+C detach";
const KEYMAP_INPUT_RESOLVING: &str = "Submitting/Waiting  ·  Ctrl+C detach";
const KEYMAP_RUNTIME: &str = "↑/↓ select  ·  Enter switch  ·  Esc close";
const KEYMAP_RUNTIME_PENDING: &str = "Switching/Waiting  ·  Ctrl+C detach";

pub fn render(frame: &mut Frame<'_>, model: &Model) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(frame, area);
        return;
    }

    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_context(frame, regions[0], model);
    render_transcript(frame, regions[1], model);
    render_composer(frame, regions[2], model);
    render_status(frame, regions[3], model);

    if matches!(model.interaction, Interaction::None) && matches!(model.turn, TurnState::Idle) {
        match &model.overlay {
            Overlay::None => {}
            Overlay::Help { commands } => render_help_overlay(frame, area, model, commands),
            Overlay::RuntimeList => render_runtime_overlay(frame, area, model),
            Overlay::ResumeList => render_session_overlay(frame, area, model),
            Overlay::ModelList => render_model_overlay(frame, area, model),
            Overlay::PermissionList => render_permission_overlay(frame, area, model),
            Overlay::FullAccessConfirmation => render_full_access_confirmation(frame, area),
        }
    }
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect) {
    if area.height >= 2 {
        let message_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let exit_area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        frame.render_widget(
            Paragraph::new("Terminal too small (minimum 60x16)")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            message_area,
        );
        frame.render_widget(
            Paragraph::new("Ctrl+C detach/exit").alignment(Alignment::Center),
            exit_area,
        );
    } else {
        frame.render_widget(Paragraph::new("Ctrl+C").alignment(Alignment::Center), area);
    }
}

fn render_context(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let connection = match &model.connection {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Failed(_) => "failed",
    };
    let title = Line::from(vec![
        Span::styled(
            "Pinvou Agent",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  multi-runtime coding agent"),
    ]);
    let model_name = model.model_id.as_deref().unwrap_or("model: auto");
    let context = Line::from(format!(
        "{}  ·  {}  ·  {}  ·  {}  ·  {connection}",
        model.workspace.display(),
        model.runtime.display_name,
        model_name,
        model.permission_profile.as_str()
    ));
    frame.render_widget(
        Paragraph::new(vec![title, context]).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut lines = Vec::new();
    for entry in model.transcript.entries() {
        match entry {
            TranscriptEntry::User(text) => push_plain(&mut lines, "You", text, Color::Green),
            TranscriptEntry::Assistant(text) => {
                push_plain(&mut lines, "Assistant", text, Color::Cyan)
            }
            TranscriptEntry::Tool {
                name,
                output,
                state,
                ..
            } => {
                let state = match state {
                    ToolState::Running => "running",
                    ToolState::Completed => "done",
                    ToolState::Failed => "failed",
                };
                push_card(
                    &mut lines,
                    &format!("Tool · {name} · {state}"),
                    output,
                    Color::Blue,
                );
            }
        }
    }

    match &model.interaction {
        Interaction::ApprovalPending(request) => push_card(
            &mut lines,
            "Approval required",
            &format!(
                "{}\nTool: {}\n{}",
                request.summary, request.tool, KEYMAP_APPROVAL
            ),
            Color::Yellow,
        ),
        Interaction::ApprovalResolving { request, .. } => push_card(
            &mut lines,
            "Resolving approval",
            &request.summary,
            Color::Yellow,
        ),
        Interaction::InputPending(request) => push_card(
            &mut lines,
            "Input requested",
            &format!("{}\n{KEYMAP_INPUT}", request.prompt),
            Color::Magenta,
        ),
        Interaction::InputResolving { request, .. } => push_card(
            &mut lines,
            "Submitting input",
            &request.prompt,
            Color::Magenta,
        ),
        Interaction::None => {}
    }

    if let Some(error) = &model.last_backend_error {
        push_card(&mut lines, "Error", error.safe_message(), Color::Red);
    }

    let visible_height = area.height as usize;
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let visual_height = paragraph.line_count(area.width);
    let bottom = visual_height
        .saturating_sub(visible_height)
        .min(u16::MAX as usize) as u16;
    let scroll = bottom.saturating_sub(model.transcript_scroll);
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
}

fn push_plain(lines: &mut Vec<Line<'static>>, role: &'static str, text: &str, color: Color) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        format!("  {role}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )));
    for line in text.lines() {
        lines.push(Line::from(format!("    {line}")));
    }
    if text.is_empty() {
        lines.push(Line::from("    "));
    }
}

fn push_card(lines: &mut Vec<Line<'static>>, title: &str, content: &str, color: Color) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        format!("  ╭─ {title}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )));
    for line in content.lines() {
        lines.push(Line::from(format!("  │  {line}")));
    }
    lines.push(Line::from(Span::styled("  ╰─", Style::default().fg(color))));
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let turn = match model.turn {
        TurnState::Idle => "ready",
        TurnState::Starting { .. } => "starting…",
        TurnState::Streaming { .. } => "working…",
    };
    let composer = if matches!(model.interaction, Interaction::InputPending(_)) {
        &model.input_composer
    } else {
        &model.composer
    };
    let input = if composer.input.is_empty() {
        Span::styled("Message Pinvou", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw(composer.input.as_str())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("❯ ", Style::default().fg(Color::Cyan)),
            input,
        ]))
        .block(
            Block::default()
                .title(turn)
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_status(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut spans = Vec::new();
    if let Some(status) = model.status_message.as_deref() {
        spans.push(Span::styled(
            status,
            Style::default().fg(if model.last_backend_error.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            }),
        ));
        spans.push(Span::raw("  ·  "));
    }
    spans.push(Span::styled(
        keymap(model),
        Style::default().fg(Color::DarkGray),
    ));
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

fn keymap(model: &Model) -> &'static str {
    match &model.interaction {
        Interaction::ApprovalPending(_) => return KEYMAP_APPROVAL,
        Interaction::ApprovalResolving { .. } => return KEYMAP_APPROVAL_RESOLVING,
        Interaction::InputPending(_) => return KEYMAP_INPUT,
        Interaction::InputResolving { .. } => return KEYMAP_INPUT_RESOLVING,
        Interaction::None => {}
    }
    match model.turn {
        TurnState::Starting { .. } => return KEYMAP_STARTING,
        TurnState::Streaming { .. } => {
            return if model.pending_interrupt.is_some() {
                KEYMAP_INTERRUPT_PENDING
            } else {
                KEYMAP_ACTIVE
            };
        }
        TurnState::Idle => {}
    }
    if model.pending_runtime_switch.is_some() {
        return KEYMAP_RUNTIME_PENDING;
    }
    if matches!(model.overlay, Overlay::RuntimeList) {
        return KEYMAP_RUNTIME;
    }
    KEYMAP_IDLE
}

fn render_help_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model, commands: &[&str]) {
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
    lines.push(Line::from(Span::styled(
        keymap(model),
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title("Help · commands")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

fn render_runtime_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let width = area.width.saturating_sub(8).clamp(20, 56);
    let height = (model.runtime_candidates.len() as u16 + 6)
        .min(area.height.saturating_sub(4))
        .max(5);
    let popup = centered_rect(area, width, height);
    frame.render_widget(Clear, popup);
    let pending = model
        .pending_runtime_switch
        .as_ref()
        .map(|switch| format!("Switching to {}…", switch.target));
    let mut lines = Vec::new();
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
        for (index, runtime) in model.runtime_candidates.iter().enumerate() {
            let selected = if index == model.selected_runtime {
                ">"
            } else {
                " "
            };
            let active = if runtime.id == model.runtime.id {
                "●"
            } else {
                " "
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
            lines.push(Line::from(format!(
                "{selected} {active} {} ({}) · {availability}{capabilities}",
                runtime.display_name, runtime.id,
            )));
        }
    }
    lines.push(Line::default());
    if let Some(pending) = pending {
        lines.push(Line::from(pending));
    }
    lines.push(Line::from(keymap(model)));
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title("Switch runtime")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

fn render_session_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model) {
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
    let mut lines = vec![Line::from(format!("Search: {}_", model.session_query))];
    if model.pending_session_list.is_some() {
        lines.push(Line::from("Loading sessions…"));
    } else if candidates.is_empty() {
        lines.push(Line::from("No matching sessions"));
    } else {
        lines.extend(candidates.iter().enumerate().map(|(index, session)| {
            let selected = if index == model.selected_session {
                ">"
            } else {
                " "
            };
            Line::from(format!(
                "{selected} {} · {} · {} · {}",
                session.title, session.runtime_id, session.status, session.last_active_at
            ))
        }));
    }
    lines.push(Line::default());
    lines.push(Line::from(
        "Type to filter · ↑/↓ select · Enter resume · Esc close",
    ));
    render_list_popup(frame, area, "Resume session", lines);
}

fn render_model_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut lines = Vec::new();
    if model.pending_model_list.is_some() {
        lines.push(Line::from("Loading models…"));
    } else {
        lines.extend(
            model
                .model_candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let selected = if index == model.selected_model {
                        ">"
                    } else {
                        " "
                    };
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
                    Line::from(format!(
                        "{selected} {} ({}) · {} {}",
                        candidate.display_name, candidate.id, current, availability
                    ))
                }),
        );
    }
    lines.push(Line::default());
    lines.push(Line::from("↑/↓ select · Enter switch · Esc close"));
    render_list_popup(frame, area, "Switch model", lines);
}

fn render_permission_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut lines = Vec::new();
    if model.pending_permissions.is_some() {
        lines.push(Line::from("Loading permission modes…"));
    } else if let Some(status) = &model.permission_status {
        lines.push(Line::from(format!(
            "Control: {:?} · evidence {}",
            status.control_strength, status.evidence_version
        )));
        for (index, profile) in status.supported_profiles.iter().enumerate() {
            let selected = if index == model.selected_permission {
                ">"
            } else {
                " "
            };
            let current = if *profile == model.permission_profile {
                "current"
            } else {
                ""
            };
            lines.push(Line::from(format!(
                "{selected} {} · {current}",
                profile.as_str()
            )));
        }
        for guard in &status.residual_guards {
            lines.push(Line::from(format!("  residual guard: {guard}")));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from("↑/↓ select · Enter switch · Esc close"));
    render_list_popup(frame, area, "Permissions", lines);
}

fn render_full_access_confirmation(frame: &mut Frame<'_>, area: Rect) {
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

fn render_list_popup(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'_>>) {
    let width = area.width.saturating_sub(8).clamp(32, 76);
    let height = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(4))
        .max(5);
    let popup = centered_rect(area, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend};

    use super::render;
    use crate::{
        action::ApprovalDecision,
        backend::RuntimeStatus,
        model::{
            ApprovalRequest, ConnectionState, InputRequest, Interaction, Model, OperationToken,
            Overlay, PendingInterrupt, PendingRuntimeSwitch, ToolState, TranscriptEntry, TurnState,
        },
    };

    fn model() -> Model {
        let mut model = Model::new(
            PathBuf::from("D:/work/pinvou"),
            RuntimeStatus::new("codex", "OpenAI Codex", true),
        );
        model.connection = ConnectionState::Connected;
        model.transcript.push_user("Explain the change".into());
        model
            .transcript
            .append_assistant("I updated the runtime stream.");
        model
    }

    fn screen(model: &Model, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn standard_view_has_four_regions_and_plain_transcript_rows() {
        let output = screen(&model(), 100, 30);
        for expected in [
            "Pinvou Agent",
            "D:/work/pinvou",
            "OpenAI Codex",
            "connected",
            "You",
            "Assistant",
            "Explain the change",
            "I updated the runtime stream.",
            "Message Pinvou",
            "Enter send",
            "Ctrl+R runtime",
            "Ctrl+C detach/exit",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        assert!(
            !output.contains("┌─ You"),
            "plain messages must not use heavy cards"
        );
        assert!(
            !output.contains("┌─ Assistant"),
            "plain messages must not use heavy cards"
        );
    }

    #[test]
    fn approval_input_error_and_tool_content_use_lightweight_cards() {
        let mut approval = model();
        approval.interaction = Interaction::ApprovalPending(ApprovalRequest {
            turn_id: "turn-1".into(),
            approval_id: "approval-1".into(),
            operation_token: OperationToken::new(1),
            tool: "shell".into(),
            summary: "Run cargo test".into(),
            options: vec!["allow".into(), "deny".into()],
        });
        let approval_output = screen(&approval, 100, 30);
        assert!(approval_output.contains("Approval required"));
        assert!(approval_output.contains("Run cargo test"));
        assert!(approval_output.contains("1 Allow once"));
        assert!(approval_output.contains("3 Deny"));
        assert!(!approval_output.contains("[y]"));

        approval.interaction = Interaction::ApprovalResolving {
            request: match &approval.interaction {
                Interaction::ApprovalPending(r) => r.clone(),
                _ => unreachable!(),
            },
            decision: ApprovalDecision::AllowOnce,
        };
        let resolving_approval = screen(&approval, 100, 30);
        assert!(resolving_approval.contains("Resolving approval"));
        assert!(resolving_approval.contains("Approving/Waiting"));
        assert!(!resolving_approval.contains("1 Allow once"));
        assert!(!resolving_approval.contains("3 Deny"));

        let mut input = model();
        input.interaction = Interaction::InputPending(InputRequest {
            turn_id: "turn-1".into(),
            input_id: "input-1".into(),
            operation_token: OperationToken::new(2),
            prompt: "Which package?".into(),
        });
        assert!(screen(&input, 100, 30).contains("Input requested"));
        assert!(screen(&input, 100, 30).contains("Which package?"));
        assert!(screen(&input, 100, 30).contains("Enter submit"));
        input.interaction = Interaction::InputResolving {
            request: match &input.interaction {
                Interaction::InputPending(request) => request.clone(),
                _ => unreachable!(),
            },
            value: "pinvou-tui".into(),
        };
        let resolving_input = screen(&input, 100, 30);
        assert!(resolving_input.contains("Submitting input"));
        assert!(resolving_input.contains("Submitting/Waiting"));
        assert!(!resolving_input.contains("Enter submit"));

        let mut error = model();
        error.status_message = Some("runtime disconnected unexpectedly".into());
        error.last_backend_error = Some(crate::backend::BackendError::new(
            crate::backend::BackendErrorKind::Operation,
            "runtime disconnected unexpectedly",
        ));
        assert!(screen(&error, 100, 30).contains("Error"));

        let mut tool = model();
        tool.transcript.start_tool("tool-1".into(), "shell".into());
        tool.transcript
            .append_tool_output("tool-1", "cargo test: ok");
        tool.transcript.complete_tool("tool-1", false);
        let tool_output = screen(&tool, 100, 30);
        assert!(tool_output.contains("Tool · shell"));
        assert!(tool_output.contains("cargo test: ok"));
        assert!(matches!(
            tool.transcript.entries().last(),
            Some(TranscriptEntry::Tool {
                state: ToolState::Completed,
                ..
            })
        ));
    }

    #[test]
    fn runtime_overlay_lists_candidates_and_pending_target() {
        let mut model = model();
        model.overlay = Overlay::RuntimeList;
        model.pending_runtime_switch = Some(PendingRuntimeSwitch {
            target: "claude".into(),
            operation_token: OperationToken::new(9),
        });
        let output = screen(&model, 100, 30);
        assert!(output.contains("Switch runtime"));
        assert!(output.contains("OpenAI Codex"));
        assert!(output.contains("Switching to claude"));
        assert!(output.contains("Switching/Waiting"));
        assert!(!output.contains("↑/↓ select"));
        assert!(!output.contains("Enter switch"));
        assert!(!output.contains("Esc close"));

        model.pending_runtime_switch = None;
        let selectable = screen(&model, 100, 30);
        assert!(selectable.contains("↑/↓ select"));
        assert!(selectable.contains("Enter switch"));
        assert!(selectable.contains("Esc close"));
    }

    #[test]
    fn every_overlay_variant_renders_current_commands_without_future_features() {
        let none = screen(&model(), 100, 30);
        assert!(!none.contains("Help · commands"));
        assert!(!none.contains("Switch runtime"));

        let mut help = model();
        help.overlay = Overlay::Help {
            commands: vec!["/help", "/runtime", "/exit", "/quit"],
        };
        let output = screen(&help, 100, 30);
        for command in ["/help", "/runtime", "/exit", "/quit"] {
            assert!(output.contains(command));
        }
        for future in ["/resume", "/model", "/permissions"] {
            assert!(!output.contains(future));
        }
        assert!(output.contains("Help · commands"));
        assert!(output.contains("Enter send"));
        assert!(output.contains("Ctrl+C detach/exit"));
        assert!(screen(&help, 20, 5).contains("Ctrl+C"));

        help.overlay = Overlay::RuntimeList;
        assert!(screen(&help, 100, 30).contains("Switch runtime"));
        assert!(screen(&help, 20, 5).contains("Ctrl+C"));
    }

    #[test]
    fn state_specific_keymap_never_advertises_conflicting_or_unimplemented_keys() {
        let mut streaming = model();
        streaming.turn = TurnState::Starting {
            operation_token: OperationToken::new(4),
        };
        let output = screen(&streaming, 100, 30);
        assert!(output.contains("Starting/Waiting"));
        assert!(!output.contains("Esc interrupt"));
        assert!(output.contains("Ctrl+C detach"));
        streaming.turn = TurnState::Streaming {
            operation_token: OperationToken::new(4),
            turn_id: "turn-4".into(),
        };
        let streaming_output = screen(&streaming, 60, 16);
        assert!(streaming_output.contains("Esc interrupt"));
        assert!(streaming_output.contains("Ctrl+C detach"));
        streaming.pending_interrupt = Some(PendingInterrupt {
            turn_id: "turn-4".into(),
            operation_token: OperationToken::new(5),
        });
        let cancelling = screen(&streaming, 60, 16);
        assert!(cancelling.contains("Cancelling/Waiting"));
        assert!(cancelling.contains("Ctrl+C detach"));
        assert!(!cancelling.contains("Esc interrupt"));
        for forbidden in ["Ctrl+C interrupt", "Ctrl+L clear", "[y]", "[n]"] {
            assert!(
                !output.contains(forbidden) && !streaming_output.contains(forbidden),
                "unexpected {forbidden:?}\n{output}\n{streaming_output}"
            );
        }
    }

    #[test]
    fn keymap_priority_is_interaction_then_active_turn_then_runtime() {
        let mut model = model();
        model.turn = TurnState::Streaming {
            operation_token: OperationToken::new(7),
            turn_id: "turn-7".into(),
        };
        model.overlay = Overlay::RuntimeList;
        model.pending_runtime_switch = Some(PendingRuntimeSwitch {
            target: "claude".into(),
            operation_token: OperationToken::new(8),
        });
        let runtime_pending = screen(&model, 100, 30);
        assert!(runtime_pending.contains("Esc interrupt"));
        assert!(!runtime_pending.contains("Switching/Waiting"));
        assert!(!runtime_pending.contains("Enter switch"));

        model.turn = TurnState::Idle;
        let idle_runtime_pending = screen(&model, 100, 30);
        assert!(idle_runtime_pending.contains("Switching/Waiting"));
        assert!(!idle_runtime_pending.contains("Esc interrupt"));

        model.interaction = Interaction::ApprovalPending(ApprovalRequest {
            turn_id: "turn-7".into(),
            approval_id: "approval-7".into(),
            operation_token: OperationToken::new(7),
            tool: "shell".into(),
            summary: "Run tests".into(),
            options: vec!["allow".into(), "deny".into()],
        });
        let approval = screen(&model, 100, 30);
        assert!(approval.contains("1 Allow once"));
        assert!(approval.contains("3 Deny"));
        assert!(!approval.contains("Esc interrupt"));
        assert!(!approval.contains("Enter switch"));
    }

    #[test]
    fn actionable_interactions_hide_stale_overlays_and_remain_visible() {
        let mut approval = model();
        approval.turn = TurnState::Streaming {
            operation_token: OperationToken::new(10),
            turn_id: "turn-10".into(),
        };
        approval.overlay = Overlay::RuntimeList;
        approval.interaction = Interaction::ApprovalPending(ApprovalRequest {
            turn_id: "turn-10".into(),
            approval_id: "approval-10".into(),
            operation_token: OperationToken::new(10),
            tool: "shell".into(),
            summary: "Delete generated cache?".into(),
            options: vec!["allow".into(), "deny".into()],
        });
        let approval_output = screen(&approval, 100, 30);
        assert!(approval_output.contains("Delete generated cache?"));
        assert!(approval_output.contains("1 Allow once"));
        assert!(approval_output.contains("3 Deny"));
        assert!(!approval_output.contains("Switch runtime"));

        let mut input = model();
        input.turn = TurnState::Streaming {
            operation_token: OperationToken::new(11),
            turn_id: "turn-11".into(),
        };
        input.overlay = Overlay::Help {
            commands: vec!["/help", "/runtime", "/exit", "/quit"],
        };
        input.interaction = Interaction::InputPending(InputRequest {
            turn_id: "turn-11".into(),
            input_id: "input-11".into(),
            operation_token: OperationToken::new(11),
            prompt: "Choose the target package".into(),
        });
        let input_output = screen(&input, 100, 30);
        assert!(input_output.contains("Choose the target package"));
        assert!(input_output.contains("Enter submit"));
        assert!(!input_output.contains("Help · commands"));
    }

    #[test]
    fn active_turn_hides_stale_runtime_overlay_and_keeps_interrupt_keymap() {
        let mut model = model();
        model.turn = TurnState::Streaming {
            operation_token: OperationToken::new(12),
            turn_id: "turn-12".into(),
        };
        model.overlay = Overlay::RuntimeList;
        let output = screen(&model, 100, 30);
        assert!(output.contains("Esc interrupt"));
        assert!(!output.contains("Enter switch"));
        assert!(!output.contains("Switch runtime"));
    }

    #[test]
    fn minimum_and_tiny_sizes_show_safe_exit_guidance_without_panicking() {
        let boundary = screen(&model(), 59, 15);
        assert!(boundary.contains("Terminal too small"));
        assert!(boundary.contains("Ctrl+C"));
        for (width, height) in [(1, 1), (2, 2), (8, 3), (20, 5), (60, 16)] {
            let output = screen(&model(), width, height);
            if width < 60 || height < 16 {
                assert!(output.contains("Ctrl+C") || width < 6 || height < 2);
            } else {
                assert!(output.contains("Pinvou Agent"));
            }
        }
    }

    #[test]
    fn long_unicode_content_keeps_latest_transcript_and_status_visible() {
        let mut model = model();
        for index in 0..40 {
            model.transcript.push_user(format!(
                "第{index}条消息 🐋：这是很长的内容 {}",
                "界".repeat(30)
            ));
        }
        model.transcript.append_assistant("LATEST 最新消息 ✅");
        model.turn = TurnState::Streaming {
            operation_token: OperationToken::new(3),
            turn_id: "turn-3".into(),
        };
        model.status_message = Some("Important status".into());
        model.diagnostic_message = Some("ordinary diagnostic".into());
        let output = screen(&model, 60, 16);
        assert!(output.contains("LATEST"));
        assert!(output.contains("Important status"));
        assert!(!output.contains("ordinary diagnostic"));
    }

    #[test]
    fn word_boundary_wrapping_uses_actual_paragraph_height_when_scrolling_to_latest() {
        let mut model = model();
        for index in 0..2 {
            model.transcript.push_user(format!("earlier-{index}"));
        }
        let word_a = "a".repeat(30);
        let word_b = "b".repeat(30);
        let word_c = "c".repeat(30);
        model
            .transcript
            .append_assistant(&format!("{word_a} {word_b} {word_c}\nLATEST-TAIL"));
        let output = screen(&model, 60, 16);
        assert!(
            output.contains("LATEST-TAIL"),
            "latest wrapped tail missing\n{output}"
        );
    }
}
