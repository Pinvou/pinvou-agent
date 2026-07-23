use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn hide_child_console(_command: &mut Command) {}

pub(crate) fn sibling_sensevoice_candidates(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.join("llama-funasr-sensevoice"),
        dir.join("runtime").join("llama-funasr-sensevoice"),
        dir.join("runtime")
            .join("bin")
            .join("llama-funasr-sensevoice"),
        dir.join("bin").join("llama-funasr-sensevoice"),
    ]
}

pub(crate) fn sibling_paddlespeech_candidates(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.join("paddlespeech"),
        dir.join("runtime").join("bin").join("paddlespeech"),
    ]
}

pub(crate) fn executable_names(command: &OsStr) -> Vec<PathBuf> {
    vec![PathBuf::from(command)]
}
