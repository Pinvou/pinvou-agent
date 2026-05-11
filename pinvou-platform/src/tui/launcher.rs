//! 应用启动器屏幕 — 列表展示所有可用应用。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::app::AppConfig;

/// 启动器状态
#[derive(Debug, Clone)]
pub struct LauncherState {
    pub list_state: ListState,
    pub search_query: String,
    pub selected_idx: usize,
}

impl Default for LauncherState {
    fn default() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            list_state,
            search_query: String::new(),
            selected_idx: 0,
        }
    }
}

impl LauncherState {
    pub fn select_next(&mut self, item_count: usize) {
        let i = self
            .list_state
            .selected()
            .map_or(0, |i| (i + 1).min(item_count.saturating_sub(1)));
        self.list_state.select(Some(i));
    }

    pub fn select_prev(&mut self) {
        let i = self
            .list_state
            .selected()
            .map_or(0, |i| i.saturating_sub(1));
        self.list_state.select(Some(i));
    }

    pub fn selected_index(&self) -> usize {
        self.list_state.selected().unwrap_or(0)
    }
}

/// 应用启动器 Widget
pub struct LauncherWidget<'a> {
    pub apps: &'a [AppConfig],
    pub search_query: &'a str,
}

impl StatefulWidget for LauncherWidget<'_> {
    type State = LauncherState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // 整体分三块：标题 / 应用列表 / 底部提示
        let layout = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(5),
                ratatui::layout::Constraint::Min(6),
                ratatui::layout::Constraint::Length(3),
            ])
            .split(area);

        // --- 标题 ---
        let title_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let title_inner = title_block.inner(layout[0]);
        title_block.render(layout[0], buf);

        let title_lines = vec![
            Line::from(Span::styled(
                "  pinvou3 — 本地 AI 平台",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  选择一个应用开始，按 c 进入 coding 模式，按 q 退出",
                Style::default().fg(Color::Gray),
            )),
        ];
        Paragraph::new(Text::from(title_lines)).render(title_inner, buf);

        // --- 应用列表 ---
        let list_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(Color::Gray));
        let list_inner = list_block.inner(layout[1]);
        list_block.render(layout[1], buf);

        let filtered: Vec<&AppConfig> = if self.search_query.is_empty() {
            self.apps.iter().collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.apps
                .iter()
                .filter(|a| {
                    a.name.to_lowercase().contains(&q) || a.description.to_lowercase().contains(&q)
                })
                .collect()
        };

        if filtered.is_empty() {
            let hint = Paragraph::new(if self.search_query.is_empty() {
                "暂无应用。在 apps/ 目录下创建 .toml 文件来添加应用。"
            } else {
                "无匹配结果"
            })
            .style(Style::default().fg(Color::DarkGray))
            .centered();
            hint.render(list_inner, buf);
            return;
        }

        let items: Vec<ListItem> = filtered
            .iter()
            .map(|app| {
                let key_hint = if app.model_preference == "large" {
                    " [大模型]"
                } else if app.model_preference == "small" {
                    " [小模型]"
                } else {
                    ""
                };

                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(&app.icon, Style::default()),
                        Span::styled(
                            format!("  {:<20}", app.name),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(key_hint, Style::default().fg(Color::DarkGray)),
                    ]),
                    Line::from(vec![Span::styled(
                        format!("       {}", app.description),
                        Style::default().fg(Color::Gray),
                    )]),
                    Line::from(vec![Span::styled(
                        format!("       工具: {}", app.tools.join(", ")),
                        Style::default().fg(Color::DarkGray),
                    )]),
                ])
            })
            .collect();

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        StatefulWidget::render(list, list_inner, buf, &mut state.list_state);

        // --- 底部提示 ---
        let footer_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray));
        let footer_inner = footer_block.inner(layout[2]);
        footer_block.render(layout[2], buf);

        let footer_text = if !self.search_query.is_empty() {
            format!("搜索: {} │ 共 {} 个应用", self.search_query, filtered.len())
        } else {
            format!(
                "共 {} 个应用 │ Enter 选择 │ c coding 模式 │ q 退出",
                filtered.len()
            )
        };

        Paragraph::new(Text::from(Line::from(Span::styled(
            footer_text,
            Style::default().fg(Color::DarkGray),
        ))))
        .render(footer_inner, buf);
    }
}
