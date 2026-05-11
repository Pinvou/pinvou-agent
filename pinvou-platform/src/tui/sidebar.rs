//! 里程碑侧边栏组件。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::workflow::MilestoneStatus;

/// 侧边栏状态
#[derive(Debug, Clone)]
pub struct SidebarState {
    /// 列表选中索引
    pub list_state: ListState,
    /// 是否聚焦
    pub focused: bool,
    /// 侧边栏宽度占比
    pub width_pct: u16,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            list_state: ListState::default(),
            focused: false,
            width_pct: 25,
        }
    }
}

impl SidebarState {
    pub fn select_next(&mut self, item_count: usize) {
        let i = self
            .list_state
            .selected()
            .map(|i| (i + 1) % item_count)
            .or(Some(0));
        self.list_state.select(i);
    }

    pub fn select_prev(&mut self, item_count: usize) {
        let i = self
            .list_state
            .selected()
            .map(|i| {
                if i == 0 {
                    item_count.saturating_sub(1)
                } else {
                    i - 1
                }
            })
            .or(Some(0));
        self.list_state.select(i);
    }
}

/// 里程碑列表项
#[derive(Debug, Clone)]
pub struct MilestoneItem {
    pub id: String,
    pub label: String,
    pub status: MilestoneStatus,
}

/// 侧边栏 Widget
pub struct SidebarWidget<'a> {
    pub title: &'a str,
    pub items: Vec<MilestoneItem>,
    pub focused: bool,
}

impl<'a> SidebarWidget<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            items: Vec::new(),
            focused: false,
        }
    }

    pub fn items(mut self, items: Vec<MilestoneItem>) -> Self {
        self.items = items;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl StatefulWidget for SidebarWidget<'_> {
    type State = SidebarState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        let block = Block::default()
            .title(Span::styled(
                self.title,
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::TOP | Borders::LEFT)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        if self.items.is_empty() {
            let hint = Paragraph::new("(无步骤)")
                .style(Style::default().fg(Color::DarkGray))
                .centered();
            hint.render(inner, buf);
            return;
        }

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(_i, item)| {
                let (icon, style) = milestone_icon_style(&item.status);

                let line = Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(icon, style),
                    Span::styled(" ", Style::default()),
                    Span::styled(&item.label, style),
                ]);

                ListItem::new(line)
            })
            .collect();

        let highlight_style = if self.focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        // 确保有默认选中
        if state.list_state.selected().is_none() && !self.items.is_empty() {
            // 找到第一个活跃的
            let active_idx = self
                .items
                .iter()
                .position(|item| matches!(item.status, MilestoneStatus::Active))
                .or(Some(0));
            state.list_state.select(active_idx);
        }

        StatefulWidget::render(list, inner, buf, &mut state.list_state);
    }
}

/// 里程碑状态 → (图标, 样式)
fn milestone_icon_style(status: &MilestoneStatus) -> (&'static str, Style) {
    match status {
        MilestoneStatus::Active => (
            "●",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        MilestoneStatus::Done => ("✓", Style::default().fg(Color::Green)),
        MilestoneStatus::Skipped => ("⏭", Style::default().fg(Color::DarkGray)),
        MilestoneStatus::Pending => ("○", Style::default().fg(Color::Gray)),
    }
}
