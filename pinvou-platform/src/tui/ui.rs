//! 平台 TUI 事件循环和渲染。

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    prelude::{CrosstermBackend, Widget},
};

use crate::engine_factory::PinvouEngine;

use super::app::{EngineStatus, Focus, PlatformApp, PlatformScreen};
use super::chat::ChatView;
use super::input::InputWidget;
use super::launcher::{LauncherState, LauncherWidget};
use super::sidebar::{MilestoneItem, SidebarState, SidebarWidget};

/// 运行平台 TUI
pub fn run_platform_tui(apps_dir: PathBuf, engine: PinvouEngine) -> Result<()> {
    // 终端设置
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, apps_dir, engine);

    // 恢复终端
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    apps_dir: PathBuf,
    engine: PinvouEngine,
) -> Result<()> {
    let mut app = PlatformApp::new(apps_dir, engine);
    let mut launcher_state = LauncherState::default();
    let mut sidebar_state = SidebarState::default();
    let mut input_buf = String::new();
    let mut cursor_pos: usize = 0;

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    while app.running {
        // 渲染
        terminal.draw(|f| match app.screen {
            PlatformScreen::Launcher => render_launcher(f, &app, &mut launcher_state),
            PlatformScreen::Conversation => {
                render_conversation(f, &app, &mut sidebar_state, &input_buf, cursor_pos)
            }
            PlatformScreen::QuitConfirm => render_quit_confirm(f),
        })?;

        // 事件处理（超时允许流畅 UI 更新）
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                match app.screen {
                    PlatformScreen::Launcher => {
                        handle_launcher_key(&mut app, &mut launcher_state, key);
                    }
                    PlatformScreen::Conversation => {
                        handle_conversation_key(
                            &mut app,
                            &mut sidebar_state,
                            &mut input_buf,
                            &mut cursor_pos,
                            key,
                        );
                    }
                    PlatformScreen::QuitConfirm => match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.running = false;
                            app.should_quit = true;
                        }
                        _ => {
                            app.screen = if app.current_app.is_some() {
                                PlatformScreen::Conversation
                            } else {
                                PlatformScreen::Launcher
                            };
                        }
                    },
                }
            }
        }

        // 周期性 tick（更新引擎状态等）
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
            app_tick(&mut app);
        }

        if app.should_quit {
            app.running = false;
        }
    }

    Ok(())
}

// === 渲染函数 ===

fn render_launcher(f: &mut ratatui::Frame, app: &PlatformApp, launcher_state: &mut LauncherState) {
    let area = f.area();
    let widget = LauncherWidget {
        apps: app.registry.list(),
        search_query: "",
    };
    f.render_stateful_widget(widget, area, launcher_state);
}

fn render_conversation(
    f: &mut ratatui::Frame,
    app: &PlatformApp,
    sidebar_state: &mut SidebarState,
    input: &str,
    cursor_pos: usize,
) {
    let area = f.area();

    // 布局: 顶栏 + (对话 | 侧边栏) + 输入
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top bar
            Constraint::Min(4),    // content
            Constraint::Length(3), // input
        ])
        .split(area);

    // 内容区: 对话 | 侧边栏
    let content_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(main_layout[1]);

    // --- 顶栏 ---
    let header = if let Some(ref current_app) = app.current_app {
        format!(
            " {} {} — {} | 模型: {}",
            current_app.icon, current_app.name, current_app.description, app.current_model
        )
    } else {
        " pinvou3 平台模式".to_string()
    };
    let header_span = ratatui::text::Span::styled(
        header,
        ratatui::style::Style::default().fg(ratatui::style::Color::Cyan),
    );
    ratatui::widgets::Paragraph::new(ratatui::text::Line::from(header_span))
        .render(main_layout[0], f.buffer_mut());

    // --- 对话区 ---
    let chat = ChatView {
        messages: &app.messages,
        streaming_content: app.streaming_content.as_deref(),
        engine_status: engine_status_str(app.engine_status),
        scroll_offset: app.scroll_offset,
    };
    f.render_widget(chat, content_layout[0]);

    // --- 侧边栏 ---
    let ms_items: Vec<MilestoneItem> = app
        .milestone_suggestions()
        .into_iter()
        .map(|(id, label, status)| MilestoneItem { id, label, status })
        .collect();

    let sidebar = SidebarWidget::new("步骤引导")
        .items(ms_items)
        .focused(app.focus == Focus::Sidebar);
    f.render_stateful_widget(sidebar, content_layout[1], sidebar_state);

    // --- 输入区 ---
    let input_widget = InputWidget {
        input,
        cursor_pos,
        focused: app.focus == Focus::Input,
        placeholder: "输入你的问题，或按 / 使用命令...",
        model: &app.current_model,
        app_name: app.current_app.as_ref().map(|a| a.name.as_str()),
        engine_status: engine_status_str(app.engine_status),
    };
    f.render_widget(input_widget, main_layout[2]);
}

fn render_quit_confirm(f: &mut ratatui::Frame) {
    let area = f.area();
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "  确定退出吗？",
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::Yellow)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "  [y] 是，退出    [任意键] 否，继续",
            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
        )),
    ];
    ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines))
        .centered()
        .render(inner, f.buffer_mut());
}

// === 键盘处理 ===

fn handle_launcher_key(app: &mut PlatformApp, state: &mut LauncherState, key: KeyEvent) {
    let item_count = app.registry.list().len();

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            if key.modifiers.contains(event::KeyModifiers::CONTROL) {
                app.should_quit = true;
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            // coding 模式: 退出并以特殊退出码通知调用方
            app.should_quit = true;
            // 这个信号由 platform_main 处理
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.select_prev();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.select_next(item_count);
        }
        KeyCode::Enter => {
            let idx = state.selected_index();
            if idx < item_count {
                let app_id = app.registry.list()[idx].id.clone();
                app.select_app(&app_id);
            }
        }
        KeyCode::Esc => {
            app.should_quit = true;
        }
        _ => {}
    }
}

fn handle_conversation_key(
    app: &mut PlatformApp,
    sidebar_state: &mut SidebarState,
    input: &mut String,
    cursor_pos: &mut usize,
    key: KeyEvent,
) {
    match app.focus {
        Focus::Input => handle_input_key(app, input, cursor_pos, key),
        Focus::Sidebar => handle_sidebar_key(app, sidebar_state, key),
        Focus::Chat => handle_chat_key(app, key),
    }

    // 全局快捷键
    match key.code {
        KeyCode::Tab => app.cycle_focus(),
        KeyCode::Esc => {
            if app.screen == PlatformScreen::Conversation {
                app.back_to_launcher();
            }
        }
        KeyCode::Char('q') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        _ => {}
    }
}

fn handle_input_key(
    app: &mut PlatformApp,
    input: &mut String,
    cursor_pos: &mut usize,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Enter => {
            let msg = input.trim().to_string();
            if msg.is_empty() {
                return;
            }

            // 特殊命令
            if msg == "/q" || msg == "/quit" {
                app.screen = PlatformScreen::QuitConfirm;
                input.clear();
                *cursor_pos = 0;
                return;
            }
            if msg == "/c" || msg == "/coding" {
                app.should_quit = true;
                return;
            }

            // 添加用户消息
            let user_msg = msg.clone();
            app.add_user_message(msg);
            input.clear();
            *cursor_pos = 0;

            // Phase 3: 调用真实 LLM
            app.engine_status = EngineStatus::Thinking;
            let result = app
                .runtime
                .block_on(async { app.engine.process_message(&user_msg).await });
            match result {
                Ok(response) => {
                    app.add_assistant_message(response);
                    app.engine_status = EngineStatus::Idle;
                    app.sync_engine_state();
                }
                Err(e) => {
                    app.add_assistant_message(format!("引擎错误: {e}"));
                    app.engine_status = EngineStatus::Error;
                }
            }
        }
        KeyCode::Char(c) => {
            input.insert(*cursor_pos, c);
            *cursor_pos += c.len_utf8();
        }
        KeyCode::Backspace => {
            if *cursor_pos > 0 {
                // 找到前一个字符边界
                let prev = prev_char_boundary(input, *cursor_pos);
                input.remove(prev);
                *cursor_pos = prev;
            }
        }
        KeyCode::Delete => {
            if *cursor_pos < input.len() {
                input.remove(*cursor_pos);
            }
        }
        KeyCode::Left => {
            if *cursor_pos > 0 {
                *cursor_pos = prev_char_boundary(input, *cursor_pos);
            }
        }
        KeyCode::Right => {
            if *cursor_pos < input.len() {
                *cursor_pos = next_char_boundary(input, *cursor_pos);
            }
        }
        KeyCode::Home => {
            *cursor_pos = 0;
        }
        KeyCode::End => {
            *cursor_pos = input.len();
        }
        _ => {}
    }
}

fn handle_sidebar_key(app: &mut PlatformApp, sidebar_state: &mut SidebarState, key: KeyEvent) {
    let item_count = app.milestone_suggestions().len();

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            sidebar_state.select_prev(item_count);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            sidebar_state.select_next(item_count);
        }
        KeyCode::Enter => {
            // 选中一个里程碑，标记为完成并激活下一个
            if let Some(idx) = sidebar_state.list_state.selected() {
                let items = app.milestone_suggestions();
                if idx < items.len() {
                    let milestone_id = &items[idx].0;
                    app.mark_milestone_done(milestone_id);
                }
            }
        }
        KeyCode::Char('s') => {
            // 跳过当前里程碑
            if let Some(idx) = sidebar_state.list_state.selected() {
                let items = app.milestone_suggestions();
                if idx < items.len() {
                    let milestone_id = &items[idx].0;
                    app.skip_milestone(milestone_id);
                }
            }
        }
        _ => {}
    }
}

fn handle_chat_key(app: &mut PlatformApp, key: KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            // 向上滚动
            app.scroll_offset = app.scroll_offset.saturating_add(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            // 向下滚动
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        KeyCode::PageUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(10);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(10);
        }
        _ => {}
    }
}

// === Tick ===

fn app_tick(_app: &mut PlatformApp) {
    // 周期性任务：检查引擎状态、更新 UI 等
}

fn engine_status_str(status: EngineStatus) -> &'static str {
    match status {
        EngineStatus::Idle => "idle",
        EngineStatus::Thinking => "thinking",
        EngineStatus::Streaming => "streaming",
        EngineStatus::Error => "error",
    }
}

/// 找到前一个 UTF-8 字符边界
fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.saturating_sub(1);
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// 找到下一个 UTF-8 字符边界
fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos + 1;
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}
