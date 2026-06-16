use std::ffi::OsStr;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    super::super::platform::open_target(target, label)
}

pub fn command_exists(command: &str) -> bool {
    super::super::platform::command_exists(command)
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    super::super::platform::nvidia_smi_candidates()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_candidates_starts_with_generic_command() {
        assert_eq!(nvidia_smi_candidates().first().copied(), Some("nvidia-smi"));
    }
}
