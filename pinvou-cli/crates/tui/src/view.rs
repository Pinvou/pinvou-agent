use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    commands::{CommandSpec, suggestions},
    model::{ConnectionState, Interaction, Model, Overlay, ToolState, TranscriptEntry, TurnState},
    theme,
};

mod overlays;

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

    let command_suggestions = visible_command_suggestions(model);
    let welcome_height = if model.transcript.entries().is_empty() && command_suggestions.is_empty()
    {
        6
    } else {
        0
    };
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(welcome_height),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);

    if welcome_height > 0 {
        render_welcome(frame, regions[0], model);
    }
    render_transcript(frame, regions[1], model);
    render_command_menu(frame, regions[1], model, &command_suggestions);
    render_composer(frame, regions[2], model);
    render_status(frame, regions[3], model);

    if matches!(model.interaction, Interaction::None) && matches!(model.turn, TurnState::Idle) {
        match &model.overlay {
            Overlay::None => {}
            Overlay::Help { commands } => overlays::render_help(frame, area, model, commands),
            Overlay::RuntimeList => overlays::render_runtime(frame, area, model),
            Overlay::ResumeList => overlays::render_session(frame, area, model),
            Overlay::ModelList => overlays::render_model(frame, area, model),
            Overlay::ModelLevelList => overlays::render_model_level(frame, area, model),
            Overlay::ApiKeyInput => overlays::render_api_key(frame, area, model),
            Overlay::PermissionList => overlays::render_permission(frame, area, model),
            Overlay::FullAccessConfirmation => {
                overlays::render_full_access_confirmation(frame, area)
            }
        }
    }
}

fn visible_command_suggestions(model: &Model) -> Vec<&'static CommandSpec> {
    if model.turn != TurnState::Idle
        || model.interaction != Interaction::None
        || model.overlay != Overlay::None
    {
        return Vec::new();
    }
    suggestions(&model.composer.input)
}

fn render_command_menu(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &Model,
    commands: &[&CommandSpec],
) {
    if commands.is_empty() || area.height < 3 {
        return;
    }
    let height = (commands.len() as u16 + 2).min(area.height);
    let popup = Rect::new(
        area.x,
        area.bottom().saturating_sub(height),
        area.width,
        height,
    );
    let capacity = height.saturating_sub(2) as usize;
    let selected = model.selected_command.min(commands.len().saturating_sub(1));
    let start = selected
        .saturating_sub(capacity.saturating_sub(1))
        .min(commands.len().saturating_sub(capacity));
    let lines = commands
        .iter()
        .skip(start)
        .take(capacity)
        .enumerate()
        .map(|(offset, command)| {
            let is_selected = start + offset == selected;
            Line::from(vec![
                Span::styled(
                    if is_selected { " › " } else { "   " },
                    if is_selected {
                        theme::accent_bold()
                    } else {
                        theme::muted()
                    },
                ),
                Span::styled(
                    format!("{:<14}", command.name),
                    if is_selected {
                        theme::accent_bold()
                    } else {
                        theme::text()
                    },
                ),
                Span::styled(command.description, theme::muted()),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title("Commands")
                .borders(Borders::ALL)
                .border_style(theme::border()),
        ),
        popup,
    );
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

fn render_welcome(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let connection = match &model.connection {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Failed(_) => "failed",
    };
    let title = Line::from(vec![
        Span::styled("◆ PINVOU", theme::accent_bold()),
        Span::raw("  ·  multi-runtime coding agent"),
    ]);
    let model_name = model_label(model);
    let model_level = model.model_level.as_deref().unwrap_or("discovering");
    let context = Line::from(format!("  {}", model.workspace.display()));
    let runtime = Line::from(format!(
        "  {}  ·  model: {}  ·  level: {}  ·  {}  ·  {connection}",
        model.runtime.display_name,
        model_name,
        model_level,
        model.permission_profile.as_str()
    ));
    frame.render_widget(
        Paragraph::new(vec![
            title,
            Line::default(),
            Line::from(Span::styled(
                "What would you like to build?",
                theme::text().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Start with a task, or use /resume to restore context.",
                theme::muted(),
            )),
            context,
            runtime,
        ]),
        area,
    );
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut lines = Vec::new();
    for entry in model.transcript.entries() {
        match entry {
            TranscriptEntry::User(text) => push_user(&mut lines, text, area.width),
            TranscriptEntry::Thinking(text) => push_thinking(&mut lines, text, area.width),
            TranscriptEntry::Assistant(text) => {
                push_flow(&mut lines, "●", text, theme::ACCENT_SOFT)
            }
            TranscriptEntry::Error(message) => {
                push_notice(&mut lines, "Error", message, theme::ERROR)
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
                push_tool(&mut lines, name, state, output);
            }
        }
    }

    render_activity(&mut lines, model);

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

    if let Some(error) = &model.last_backend_error
        && !matches!(
            model.transcript.entries().last(),
            Some(TranscriptEntry::Error(message)) if message == error.safe_message()
        )
    {
        push_notice(
            &mut lines,
            "Error",
            &format!(
                "{}\nThe current session is unchanged. Retry or reopen the relevant command.",
                error.safe_message()
            ),
            theme::ERROR,
        );
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

fn render_activity(lines: &mut Vec<Line<'static>>, model: &Model) {
    if !matches!(model.interaction, Interaction::None) {
        return;
    }
    let (label, detail) = match model.turn {
        TurnState::Idle => return,
        TurnState::Starting { .. } => (
            format!("Starting {} turn", model.runtime.display_name),
            "waiting for runtime",
        ),
        TurnState::Streaming { .. } if model.pending_interrupt.is_some() => {
            ("Cancelling turn".to_owned(), "waiting for runtime")
        }
        TurnState::Streaming { .. } => match model.transcript.entries().last() {
            Some(TranscriptEntry::Tool {
                name,
                state: ToolState::Running,
                ..
            }) => (format!("Running {name}"), "Esc to interrupt"),
            _ => ("Generating response".to_owned(), "Esc to interrupt"),
        },
    };
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    const FRAMES: [&str; 4] = ["◒", "◐", "◓", "◑"];
    let frame = FRAMES[usize::from(model.activity_frame()) % FRAMES.len()];
    let elapsed = model.activity_elapsed().as_secs();
    lines.push(Line::from(vec![
        Span::styled(format!("  {frame} "), theme::warning()),
        Span::styled(label, theme::muted()),
        Span::styled(format!("  ·  {elapsed}s  ·  {detail}"), theme::muted()),
    ]));
}

fn push_tool(lines: &mut Vec<Line<'static>>, name: &str, state: &str, output: &str) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let state_style = match state {
        "done" => Style::default().fg(theme::SUCCESS),
        "failed" => theme::error(),
        _ => theme::warning(),
    };
    lines.push(Line::from(vec![
        Span::styled("  ⌁ ", Style::default().fg(theme::TOOL)),
        Span::styled(format!("Tool · {name}"), Style::default().fg(theme::TOOL)),
        Span::styled(format!("  ·  {state}"), state_style),
    ]));
    for line in output.lines() {
        lines.push(Line::from(vec![
            Span::styled("    │ ", theme::border()),
            Span::styled(line.to_owned(), theme::muted()),
        ]));
    }
}

fn push_user(lines: &mut Vec<Line<'static>>, text: &str, width: u16) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let width = usize::from(width);
    let content_width = width.saturating_sub(4).max(1);
    let background = Style::default().bg(theme::USER_MESSAGE_BG);
    let marker = theme::accent_bold().bg(theme::USER_MESSAGE_BG);
    let message = theme::text().bg(theme::USER_MESSAGE_BG);

    lines.push(Line::from(Span::styled(" ".repeat(width), background)));
    for (index, content) in wrap_visual(text, content_width).into_iter().enumerate() {
        let content_width = UnicodeWidthStr::width(content.as_str());
        lines.push(Line::from(vec![
            Span::styled(if index == 0 { "  ❯ " } else { "    " }, marker),
            Span::styled(content, message),
            Span::styled(
                " ".repeat(width.saturating_sub(4 + content_width)),
                background,
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(" ".repeat(width), background)));
}

fn push_thinking(lines: &mut Vec<Line<'static>>, text: &str, width: u16) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let width = usize::from(width);
    let content_width = width.saturating_sub(4).max(1);
    let background = Style::default().bg(theme::THINKING_BG);
    let marker = theme::warning().bg(theme::THINKING_BG);
    let message = theme::muted().bg(theme::THINKING_BG);

    lines.push(Line::from(Span::styled(" ".repeat(width), background)));
    for (index, content) in wrap_visual(text, content_width).into_iter().enumerate() {
        let rendered_width = UnicodeWidthStr::width(content.as_str());
        lines.push(Line::from(vec![
            Span::styled(if index == 0 { "  ◇ " } else { "    " }, marker),
            Span::styled(content, message),
            Span::styled(
                " ".repeat(width.saturating_sub(4 + rendered_width)),
                background,
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(" ".repeat(width), background)));
}

fn wrap_visual(text: &str, width: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for source in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for character in source.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if !line.is_empty() && line_width + character_width > width {
                wrapped.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if character_width <= width {
                line.push(character);
                line_width += character_width;
            }
        }
        wrapped.push(line);
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

fn push_flow(lines: &mut Vec<Line<'static>>, marker: &'static str, text: &str, color: Color) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let mut text_lines = text.lines();
    let first = text_lines.next().unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {marker} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(first.to_owned()),
    ]));
    for line in text_lines {
        lines.push(Line::from(format!("    {line}")));
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

fn push_notice(lines: &mut Vec<Line<'static>>, title: &str, content: &str, color: Color) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::styled("  │ ", Style::default().fg(color)),
        Span::styled(
            title.to_owned(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]));
    for line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(color)),
            Span::raw(line.to_owned()),
        ]));
    }
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let composer = if matches!(model.interaction, Interaction::InputPending(_)) {
        &model.input_composer
    } else {
        &model.composer
    };
    let input_width = usize::from(area.width.saturating_sub(7));
    let visible_input = composer_tail(&composer.input, input_width);
    let input = if composer.input.is_empty() {
        Span::styled("Message Pinvou", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw(visible_input.as_str())
    };
    let border_style = if matches!(model.connection, ConnectionState::Connected) {
        theme::border()
    } else {
        theme::warning()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("❯ ", theme::accent_bold()),
            input,
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .padding(Padding::horizontal(1)),
        ),
        area,
    );

    let composer_is_focused = model.overlay == Overlay::None
        && (matches!(model.interaction, Interaction::InputPending(_))
            || (matches!(model.interaction, Interaction::None)
                && matches!(model.turn, TurnState::Idle)));
    if composer_is_focused {
        let cursor_x = area
            .x
            .saturating_add(4)
            .saturating_add(UnicodeWidthStr::width(visible_input.as_str()) as u16)
            .min(area.right().saturating_sub(3));
        frame.set_cursor_position((cursor_x, area.y.saturating_add(1)));
    }
}

fn composer_tail(input: &str, max_width: usize) -> String {
    let line = input.rsplit('\n').next().unwrap_or_default();
    let mut width = 0;
    let mut start = line.len();
    for (index, character) in line.char_indices().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        width += character_width;
        start = index;
    }
    line[start..].to_owned()
}

fn left_elide(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }

    let tail_width = max_width.saturating_sub(1);
    let mut width = 0;
    let mut start = value.len();
    for (index, character) in value.char_indices().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > tail_width {
            break;
        }
        width += character_width;
        start = index;
    }
    format!("…{}", &value[start..])
}

fn render_status(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let connection = match &model.connection {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Failed(_) => "failed",
    };
    let model_name = model_label(model);
    let model_level = model.model_level.as_deref().unwrap_or("discovering");
    let context = Line::from(Span::styled(
        format!(
            "{}  ·  model: {}  ·  level: {}  ·  {}  ·  {connection}",
            model.runtime.display_name,
            model_name,
            model_level,
            model.permission_profile.as_str()
        ),
        theme::muted(),
    ));
    let keymap_text = keymap(model);
    let workspace_budget = usize::from(area.width)
        .saturating_sub(UnicodeWidthStr::width(keymap_text))
        .saturating_sub(10)
        .clamp(8, 72);
    let workspace = left_elide(&model.workspace.display().to_string(), workspace_budget);
    let mut spans = vec![
        Span::styled(format!("cwd: {workspace}"), theme::muted()),
        Span::raw("  ·  "),
    ];
    if let Some(status) =
        model
            .status_message
            .as_deref()
            .filter(|_| model.last_backend_error.is_none())
            .filter(|status| {
                !model.transcript.entries().iter().rev().any(
                    |entry| matches!(entry, TranscriptEntry::Error(message) if message == *status),
                )
            })
    {
        spans.push(Span::styled(
            status,
            Style::default().fg(if model.last_backend_error.is_some() {
                theme::ERROR
            } else {
                theme::MUTED
            }),
        ));
        spans.push(Span::raw("  ·  "));
    }
    spans.push(Span::styled(keymap_text, theme::muted()));
    let keymap = Line::from(spans);
    frame.render_widget(Paragraph::new(vec![context, keymap]), area);
}

fn model_label(model: &Model) -> String {
    model
        .model_id
        .as_deref()
        .and_then(|current| {
            model
                .model_candidates
                .iter()
                .find(|candidate| candidate.id == current)
        })
        .map(|candidate| {
            candidate
                .provider_display_name
                .as_deref()
                .or(candidate.provider_id.as_deref())
                .map(|provider| format!("{provider} / {}", candidate.display_name))
                .unwrap_or_else(|| candidate.display_name.clone())
        })
        .or_else(|| model.model_id.clone())
        .unwrap_or_else(|| "discovering".into())
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
    if !visible_command_suggestions(model).is_empty() {
        return "↑/↓ select  ·  Enter run  ·  Esc close";
    }
    KEYMAP_IDLE
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend};

    use super::render;
    use crate::{
        action::ApprovalDecision,
        backend::{
            ModelCandidate, PermissionControlStrength, PermissionMode, PermissionStatus,
            RuntimeStatus,
        },
        model::{
            ApprovalRequest, ConnectionState, InputRequest, Interaction, Model, OperationToken,
            Overlay, PendingInterrupt, PendingRuntimeSwitch, ToolState, TranscriptEntry, TurnState,
        },
        theme,
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
    fn active_chat_visually_separates_user_and_assistant_messages() {
        let output = screen(&model(), 100, 30);
        for expected in [
            "❯ Explain the change",
            "● I updated the runtime stream.",
            "Explain the change",
            "I updated the runtime stream.",
            "Message Pinvou",
            "OpenAI Codex",
            "connected",
            "Enter send",
            "Ctrl+R runtime",
            "Ctrl+C detach/exit",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        assert!(
            !output.contains("Pinvou Agent"),
            "welcome must not remain pinned once chatting"
        );
        assert!(
            !output.contains("  You"),
            "user messages use the prompt glyph, not a role heading"
        );
        assert!(
            !output.contains("Assistant"),
            "assistant messages use a flow glyph, not a role heading"
        );
        assert!(
            !output.contains("multi-runtime coding agent"),
            "chat view must not keep a dashboard banner"
        );

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &model())).unwrap();
        let buffer = terminal.backend().buffer();
        let user_row = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("❯ Explain the change")
            })
            .expect("rendered user message row");
        assert!((0..buffer.area.width).all(|x| buffer[(x, user_row)].bg == theme::USER_MESSAGE_BG));
        let assistant_row = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("● I updated the runtime stream.")
            })
            .expect("rendered assistant message row");
        assert!(
            (0..buffer.area.width).all(|x| buffer[(x, assistant_row)].bg != theme::USER_MESSAGE_BG)
        );
    }

    #[test]
    fn thinking_and_errors_are_rendered_inside_the_session_flow() {
        let mut thinking = model();
        thinking
            .transcript
            .append_thinking("Checking the runtime route…");
        thinking
            .transcript
            .push_error("Provider rejected the request");
        let output = screen(&thinking, 100, 30);
        assert!(output.contains("◇ Checking the runtime route…"), "{output}");
        assert!(output.contains("Error"), "{output}");
        assert!(output.contains("Provider rejected the request"), "{output}");

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &thinking)).unwrap();
        let buffer = terminal.backend().buffer();
        let thinking_row = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("◇ Checking the runtime route")
            })
            .expect("rendered thinking row");
        assert!((0..buffer.area.width).all(|x| buffer[(x, thinking_row)].bg == theme::THINKING_BG));
    }

    #[test]
    fn focused_composer_places_the_terminal_cursor_after_unicode_input() {
        let mut focused = model();
        focused.composer.input = "你好".into();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &focused)).unwrap();

        terminal.backend_mut().assert_cursor_position((8, 26));
    }

    #[test]
    fn long_composer_input_keeps_the_cursor_inside_the_editor() {
        let mut focused = model();
        focused.composer.input = "界".repeat(40);
        let backend = TestBackend::new(60, 16);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &focused)).unwrap();

        terminal.backend_mut().assert_cursor_position((56, 12));
    }

    #[test]
    fn empty_chat_shows_the_welcome_context_once() {
        let mut empty = Model::new(
            PathBuf::from("D:/work/pinvou"),
            RuntimeStatus::new("codex", "OpenAI Codex", true),
        );
        empty.connection = ConnectionState::Connected;
        let output = screen(&empty, 100, 30);
        for expected in [
            "◆ PINVOU",
            "D:/work/pinvou",
            "OpenAI Codex",
            "connected",
            "Message Pinvou",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
    }

    #[test]
    fn connected_context_always_labels_current_runtime_model_and_level() {
        let mut connected = model();
        connected.model_id = Some("gpt-5.6-codex".into());
        connected.model_level = Some("high".into());

        let output = screen(&connected, 100, 30);

        assert!(output.contains("OpenAI Codex"));
        assert!(output.contains("model: gpt-5.6-codex"));
        assert!(output.contains("level: high"));
        assert!(output.contains("cwd: D:/work/pinvou"));
    }

    #[test]
    fn active_chat_keeps_the_tail_of_a_long_working_directory_visible() {
        let mut active = model();
        active.workspace = PathBuf::from(
            "/home/developer/Workspace/SourceCode/pinvou-agent-distributed-node-runtime-stage1",
        );

        let output = screen(&active, 60, 16);

        assert!(
            output.contains("cwd: …"),
            "missing elided cwd label\n{output}"
        );
        assert!(output.contains("-stage1"), "missing cwd tail\n{output}");
    }

    #[test]
    fn slash_input_shows_filterable_command_descriptions_and_navigation_help() {
        let mut commands = model();
        commands.transcript = crate::model::Transcript::default();
        commands.composer.input = "/".into();

        let output = screen(&commands, 60, 16);
        for expected in [
            "Commands",
            "/help",
            "/runtime",
            "/resume",
            "/model",
            "/permissions",
            "/exit",
            "/quit",
            "Enter run",
            "Esc close",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        assert!(output.contains("Show commands"));

        commands.composer.input = "/mo".into();
        let filtered = screen(&commands, 80, 24);
        assert!(filtered.contains("/model"));
        assert!(!filtered.contains("/runtime"));
        assert!(!filtered.contains("/permissions"));
    }

    #[test]
    fn active_turn_renders_a_visible_working_state_in_the_chat_flow() {
        let mut starting = model();
        starting.turn = TurnState::Starting {
            operation_token: OperationToken::new(41),
        };
        let started = std::time::Instant::now();
        starting.advance_activity(started);
        starting.advance_activity(started + std::time::Duration::from_millis(2_250));
        let starting_output = screen(&starting, 100, 30);
        assert!(starting_output.contains("Starting OpenAI Codex turn"));
        assert!(starting_output.contains("◓"));
        assert!(starting_output.contains("2s"));
        assert!(starting_output.contains("waiting for runtime"));

        starting.turn = TurnState::Streaming {
            operation_token: OperationToken::new(41),
            turn_id: "turn-41".into(),
        };
        let streaming_output = screen(&starting, 100, 30);
        assert!(streaming_output.contains("Generating response"));
        assert!(streaming_output.contains("Esc to interrupt"));

        starting.pending_interrupt = Some(PendingInterrupt {
            turn_id: "turn-41".into(),
            operation_token: OperationToken::new(42),
        });
        let cancelling_output = screen(&starting, 100, 30);
        assert!(cancelling_output.contains("Cancelling turn"));
        assert!(cancelling_output.contains("waiting for runtime"));
    }

    #[test]
    fn product_overlays_explain_source_current_state_and_risk() {
        let mut model = model();
        model.model_id = Some("gpt-5.6-codex".into());
        model.model_level = Some("medium".into());
        model.model_candidates = vec![
            ModelCandidate {
                id: "gpt-5.6-codex".into(),
                display_name: "GPT-5.6 Codex".into(),
                is_default: false,
                available: true,
                provider_id: Some("openai".into()),
                provider_display_name: Some("OpenAI".into()),
                configured: true,
                requires_api_key: true,
                supported_reasoning_levels: vec!["medium".into(), "high".into()],
                default_reasoning_level: Some("high".into()),
            },
            ModelCandidate {
                id: "gpt-5.6-mini".into(),
                display_name: "GPT-5.6 Mini".into(),
                is_default: true,
                available: false,
                provider_id: Some("openai".into()),
                provider_display_name: Some("OpenAI".into()),
                configured: false,
                requires_api_key: true,
                supported_reasoning_levels: vec!["medium".into()],
                default_reasoning_level: Some("medium".into()),
            },
        ];
        model.overlay = Overlay::ModelList;
        let model_output = screen(&model, 100, 30);
        for expected in [
            "Select model",
            "Available from the active OpenAI Codex runtime",
            "current",
            "default",
            "unsupported",
            "level medium",
        ] {
            assert!(
                model_output.contains(expected),
                "missing {expected:?}\n{model_output}"
            );
        }
        model.model_candidates = (0..12)
            .map(|index| ModelCandidate {
                id: format!("model-{index}"),
                display_name: format!("Model {index}"),
                is_default: false,
                available: true,
                provider_id: None,
                provider_display_name: None,
                configured: true,
                requires_api_key: false,
                supported_reasoning_levels: Vec::new(),
                default_reasoning_level: None,
            })
            .collect();
        model.selected_model = 11;
        model.model_id = Some("model-11".into());
        let narrow_model_output = screen(&model, 60, 16);
        assert!(narrow_model_output.contains("Model 11"));
        assert!(narrow_model_output.contains("current"));
        assert!(narrow_model_output.contains("Esc close"));

        model.overlay = Overlay::PermissionList;
        model.permission_status = Some(PermissionStatus {
            current_profile: PermissionMode::Request,
            supported_profiles: vec![
                PermissionMode::Request,
                PermissionMode::Assisted,
                PermissionMode::FullAccess,
            ],
            control_strength: PermissionControlStrength::Partial,
            native_mode: Some("on-request".into()),
            sandbox: Some("workspace-write".into()),
            residual_guards: vec!["OS policy remains active".into()],
            evidence_version: "codex-1".into(),
        });
        let permission_output = screen(&model, 100, 30);
        for expected in [
            "Permission mode",
            "partial",
            "Ask before side effects",
            "Known low-risk work",
            "Confirmation required",
            "OS policy remains active",
        ] {
            assert!(
                permission_output.contains(expected),
                "missing {expected:?}\n{permission_output}"
            );
        }
        let narrow_permission_output = screen(&model, 60, 16);
        for expected in ["Permission mode", "request", "current", "Esc close"] {
            assert!(
                narrow_permission_output.contains(expected),
                "missing {expected:?}\n{narrow_permission_output}"
            );
        }
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
        let error_output = screen(&error, 100, 30);
        assert!(error_output.contains("Error"));
        assert!(error_output.contains("current session is unchanged"));
        assert!(error_output.contains("Retry or reopen"));

        let mut tool = model();
        tool.transcript.start_tool("tool-1".into(), "shell".into());
        tool.transcript
            .append_tool_output("tool-1", "cargo test: ok");
        tool.turn = TurnState::Streaming {
            operation_token: OperationToken::new(13),
            turn_id: "turn-13".into(),
        };
        let running_tool_output = screen(&tool, 100, 30);
        assert!(running_tool_output.contains("Running shell"));
        tool.transcript.complete_tool("tool-1", false);
        let tool_output = screen(&tool, 100, 30);
        assert!(tool_output.contains("Tool · shell"));
        assert!(tool_output.contains("cargo test: ok"));
        assert!(
            !tool_output.contains("╭─ Tool"),
            "routine tool activity belongs in the continuous flow"
        );
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
                assert!(output.contains("Message Pinvou"));
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
