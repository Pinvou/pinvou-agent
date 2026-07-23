use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn hide_child_console(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

pub(crate) fn sibling_sensevoice_candidates(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.join("llama-funasr-sensevoice.exe"),
        dir.join("runtime").join("llama-funasr-sensevoice.exe"),
        dir.join("runtime")
            .join("bin")
            .join("llama-funasr-sensevoice.exe"),
        dir.join("bin").join("llama-funasr-sensevoice.exe"),
    ]
}

pub(crate) fn sibling_paddlespeech_candidates(dir: &Path) -> Vec<PathBuf> {
    vec![
        dir.join("paddlespeech.exe"),
        dir.join("runtime").join("Scripts").join("paddlespeech.exe"),
        dir.join("runtime").join("paddlespeech.exe"),
    ]
}

pub(crate) fn executable_names(command: &OsStr) -> Vec<PathBuf> {
    let raw = command.to_string_lossy();
    if Path::new(raw.as_ref()).extension().is_some() {
        return vec![PathBuf::from(raw.as_ref())];
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut names = vec![PathBuf::from(raw.as_ref())];
    for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
        let ext = if ext.starts_with('.') {
            ext.to_string()
        } else {
            format!(".{ext}")
        };
        names.push(PathBuf::from(format!("{raw}{ext}")));
    }
    names
}
