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
const EXIT_HINT: &str = "Ctrl+C exit";

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

    if matches!(model.overlay, Overlay::RuntimeList) {
        render_runtime_overlay(frame, area, model);
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
            Paragraph::new("Ctrl+C").alignment(Alignment::Center),
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
    let context = Line::from(format!(
        "{}  ·  {}  ·  {connection}",
        model.workspace.display(),
        model.runtime.display_name
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
                "{}\nTool: {}\n[y] allow once  [n] deny",
                request.summary, request.tool
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
            &format!("{}\nType your answer and press Enter", request.prompt),
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

    let content_width = area.width.max(1) as usize;
    let visible_height = area.height as usize;
    let visual_height: usize = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(content_width))
        .sum();
    let scroll = visual_height
        .saturating_sub(visible_height)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
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
        TurnState::Streaming { .. } => "working…  Ctrl+C interrupt",
    };
    let input = if model.composer.input.is_empty() {
        Span::styled("Message Pinvou", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw(model.composer.input.as_str())
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
    let left = model.status_message.as_deref().unwrap_or("Ready");
    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default().fg(if model.last_backend_error.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("  ·  Ctrl+R runtime  ·  Ctrl+L clear  ·  "),
        Span::styled(EXIT_HINT, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_runtime_overlay(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let width = area.width.saturating_sub(8).min(56).max(20);
    let height = 8.min(area.height.saturating_sub(4)).max(5);
    let popup = centered_rect(area, width, height);
    frame.render_widget(Clear, popup);
    let pending = model.pending_runtime_switch.as_ref().map_or_else(
        || "↑/↓ select · Enter switch · Esc close".to_owned(),
        |switch| format!("Switching to {}…", switch.target),
    );
    let active = format!("● {} ({})", model.runtime.display_name, model.runtime.id);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(active),
            Line::default(),
            Line::from(pending),
        ])
        .block(
            Block::default()
                .title("Switch runtime")
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
            Overlay, PendingRuntimeSwitch, ToolState, TranscriptEntry, TurnState,
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
            "Ctrl+R runtime",
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
        assert!(approval_output.contains("[y] allow once"));

        approval.interaction = Interaction::ApprovalResolving {
            request: match &approval.interaction {
                Interaction::ApprovalPending(r) => r.clone(),
                _ => unreachable!(),
            },
            decision: ApprovalDecision::AllowOnce,
        };
        assert!(screen(&approval, 100, 30).contains("Resolving approval"));

        let mut input = model();
        input.interaction = Interaction::InputPending(InputRequest {
            turn_id: "turn-1".into(),
            input_id: "input-1".into(),
            operation_token: OperationToken::new(2),
            prompt: "Which package?".into(),
        });
        assert!(screen(&input, 100, 30).contains("Input requested"));
        assert!(screen(&input, 100, 30).contains("Which package?"));
        input.interaction = Interaction::InputResolving {
            request: match &input.interaction {
                Interaction::InputPending(request) => request.clone(),
                _ => unreachable!(),
            },
            value: "pinvou-tui".into(),
        };
        assert!(screen(&input, 100, 30).contains("Submitting input"));

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
}
