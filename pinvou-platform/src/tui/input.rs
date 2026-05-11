//! 输入组件。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget},
};

pub struct InputWidget<'a> {
    pub input: &'a str,
    pub cursor_pos: usize,
    pub focused: bool,
    pub placeholder: &'a str,
    pub model: &'a str,
    pub app_name: Option<&'a str>,
    pub engine_status: &'a str,
}

impl Widget for InputWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // 拼接 title: 状态 + 模型 + 应用名
        let status_icon = match self.engine_status {
            "idle" => "✓",
            "thinking" => "…",
            "streaming" => "▷",
            "error" => "✗",
            _ => "-",
        };
        let title = if let Some(app) = self.app_name {
            format!(
                " {status_icon} {app} | {} | Enter:发送 Tab:焦点 ^C:退出 ",
                self.model
            )
        } else {
            format!(
                " {status_icon} {} | Enter:发送 Tab:焦点 ^C:退出 ",
                self.model
            )
        };

        let block = if self.focused {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title)
        } else {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray))
                .title(title)
        };
        let inner = block.inner(area);
        block.render(area, buf);

        // 输入内容或占位符
        let display_text = if self.input.is_empty() {
            Span::styled(self.placeholder, Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(self.input, Style::default())
        };

        let text = Text::from(Line::from(display_text));
        Paragraph::new(text).render(inner, buf);

        // 光标位置提示
        if self.focused && inner.height > 0 {
            let cursor_hint = Span::styled(
                format!("{}|", self.cursor_pos),
                Style::default().fg(Color::Cyan),
            );
            let cursor_area = Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            };
            if cursor_area.y < area.y + area.height {
                Paragraph::new(Text::from(Line::from(cursor_hint))).render(cursor_area, buf);
            }
        }
    }
}
