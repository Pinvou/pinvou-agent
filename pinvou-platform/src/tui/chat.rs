//! 对话视图组件 — 渲染聊天消息列表。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use super::app::{ChatMessage, MessageRole};

/// 对话视图 Widget
pub struct ChatView<'a> {
    pub messages: &'a [ChatMessage],
    pub streaming_content: Option<&'a str>,
    pub engine_status: &'a str, // "idle" / "thinking" / "streaming"
    pub scroll_offset: usize,
}

impl<'a> ChatView<'a> {
    pub fn new(messages: &'a [ChatMessage]) -> Self {
        Self {
            messages,
            streaming_content: None,
            engine_status: "idle",
            scroll_offset: 0,
        }
    }

    pub fn streaming(mut self, content: Option<&'a str>) -> Self {
        self.streaming_content = content;
        self
    }

    pub fn engine_status(mut self, status: &'a str) -> Self {
        self.engine_status = status;
        self
    }
}

impl Widget for ChatView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title("对话").borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);

        if self.messages.is_empty() && self.streaming_content.is_none() {
            let hint = Paragraph::new(
                "输入你的问题开始对话...\n\n你可以自由对话，侧边栏的步骤只是参考引导。",
            )
            .style(Style::default().fg(Color::DarkGray))
            .centered()
            .wrap(Wrap { trim: true });
            hint.render(inner, buf);
            return;
        }

        // 构建消息文本
        let mut lines: Vec<Line> = Vec::new();

        // 跳过已滚出的消息
        let visible_start = self.scroll_offset.min(self.messages.len());
        for msg in &self.messages[visible_start..] {
            let role_span = role_span(msg.role);
            lines.push(Line::from(vec![role_span]));

            // 截取消息预览（每行最多 120 字符）
            let truncated = truncate_lines(&msg.content, inner.width as usize);
            for content_line in truncated.lines() {
                lines.push(Line::from(vec![Span::raw(content_line.to_string())]));
            }
            lines.push(Line::from("")); // 空行分隔
        }

        // 流式内容
        if let Some(streaming) = self.streaming_content {
            lines.push(Line::from(vec![Span::styled(
                "AI (回复中...)",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            )]));
            for content_line in streaming.lines() {
                lines.push(Line::from(vec![Span::styled(
                    content_line,
                    Style::default().fg(Color::White),
                )]));
            }
        }

        // 引擎状态提示
        match self.engine_status {
            "thinking" => {
                lines.push(Line::from(vec![Span::styled(
                    "⏳ 思考中...",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
            "error" => {
                lines.push(Line::from(vec![Span::styled(
                    "⚠ 引擎错误，请重试",
                    Style::default().fg(Color::Red),
                )]));
            }
            _ => {}
        }

        let text = Text::from(lines);
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
        paragraph.render(inner, buf);
    }
}

fn role_span(role: MessageRole) -> Span<'static> {
    match role {
        MessageRole::User => Span::styled(
            "👤 你",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::Assistant => Span::styled(
            "🤖 AI",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::System => Span::styled(
            "ℹ 系统",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        MessageRole::Tool => Span::styled("🔧 工具", Style::default().fg(Color::Magenta)),
    }
}

fn truncate_lines(input: &str, max_width: usize) -> String {
    if max_width == 0 || max_width >= 120 {
        return input.to_string();
    }
    input
        .lines()
        .map(|line| {
            if line.chars().count() > max_width {
                let truncated: String = line.chars().take(max_width - 3).collect();
                format!("{truncated}...")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
