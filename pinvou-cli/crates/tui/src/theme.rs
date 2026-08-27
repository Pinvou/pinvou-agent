use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Rgb(82, 201, 164);
pub const ACCENT_SOFT: Color = Color::Rgb(109, 218, 184);
pub const TEXT: Color = Color::Rgb(224, 220, 212);
pub const MUTED: Color = Color::Rgb(137, 131, 122);
pub const BORDER: Color = Color::Rgb(75, 71, 65);
pub const SELECTED_BG: Color = Color::Rgb(24, 51, 43);
pub const USER_MESSAGE_BG: Color = Color::Rgb(52, 53, 65);
pub const THINKING_BG: Color = Color::Rgb(38, 43, 48);
pub const TOOL: Color = Color::Rgb(214, 169, 85);
pub const SUCCESS: Color = Color::Rgb(119, 187, 150);
pub const WARNING: Color = Color::Rgb(226, 177, 79);
pub const ERROR: Color = Color::Rgb(224, 117, 110);

pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

pub fn accent_bold() -> Style {
    accent().add_modifier(Modifier::BOLD)
}

pub fn text() -> Style {
    Style::default().fg(TEXT)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn border() -> Style {
    Style::default().fg(BORDER)
}

pub fn selected() -> Style {
    Style::default()
        .fg(ACCENT_SOFT)
        .bg(SELECTED_BG)
        .add_modifier(Modifier::BOLD)
}

pub fn warning() -> Style {
    Style::default().fg(WARNING)
}

pub fn error() -> Style {
    Style::default().fg(ERROR)
}
