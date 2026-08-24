use thiserror::Error;

pub const AVAILABLE_COMMANDS: [&str; 4] = ["/help", "/runtime", "/exit", "/quit"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlashCommand {
    Help,
    Runtime,
    Exit,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CommandError {
    #[error("unknown command: {0}")]
    Unknown(String),
}

pub fn parse(input: &str) -> Result<Option<SlashCommand>, CommandError> {
    match input.trim() {
        value if !value.starts_with('/') => Ok(None),
        "/help" => Ok(Some(SlashCommand::Help)),
        "/runtime" => Ok(Some(SlashCommand::Runtime)),
        "/exit" | "/quit" => Ok(Some(SlashCommand::Exit)),
        value => Err(CommandError::Unknown(value.to_owned())),
    }
}
