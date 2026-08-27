use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: [CommandSpec; 7] = [
    CommandSpec {
        name: "/help",
        description: "Show commands and keyboard help",
    },
    CommandSpec {
        name: "/runtime",
        description: "Select the active runtime",
    },
    CommandSpec {
        name: "/resume",
        description: "Resume a saved session",
    },
    CommandSpec {
        name: "/model",
        description: "Select the runtime model",
    },
    CommandSpec {
        name: "/permissions",
        description: "Change the permission mode",
    },
    CommandSpec {
        name: "/exit",
        description: "Exit Pinvou",
    },
    CommandSpec {
        name: "/quit",
        description: "Exit Pinvou",
    },
];

pub fn suggestions(input: &str) -> Vec<&'static CommandSpec> {
    let query = input.trim();
    if !query.starts_with('/') || query.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(query))
        .collect()
}

pub fn available_commands() -> Vec<&'static str> {
    COMMANDS.iter().map(|command| command.name).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlashCommand {
    Help,
    Runtime,
    Resume,
    Model,
    Permissions,
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
        "/resume" => Ok(Some(SlashCommand::Resume)),
        "/model" => Ok(Some(SlashCommand::Model)),
        "/permissions" => Ok(Some(SlashCommand::Permissions)),
        "/exit" | "/quit" => Ok(Some(SlashCommand::Exit)),
        value => Err(CommandError::Unknown(value.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::suggestions;

    #[test]
    fn slash_suggestions_share_the_parser_command_vocabulary_and_filter_by_prefix() {
        assert_eq!(
            suggestions("/")
                .into_iter()
                .map(|command| command.name)
                .collect::<Vec<_>>(),
            [
                "/help",
                "/runtime",
                "/resume",
                "/model",
                "/permissions",
                "/exit",
                "/quit"
            ]
        );
        assert_eq!(
            suggestions("/r")
                .into_iter()
                .map(|command| command.name)
                .collect::<Vec<_>>(),
            ["/runtime", "/resume"]
        );
        assert!(suggestions("hello").is_empty());
        assert!(suggestions("/model extra").is_empty());
    }
}
