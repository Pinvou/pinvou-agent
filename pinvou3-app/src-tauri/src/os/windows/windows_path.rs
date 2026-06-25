use std::path::{Path, PathBuf, MAIN_SEPARATOR};

const PDF_TOOLS: &[&str] = &["pdftotext", "pdftoppm"];
const PANDOC_TOOL: &str = "pandoc";
const TESSERACT_TOOL: &str = "tesseract";

pub fn user_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        let home = format!("{drive}{path}");
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return platform_compat_path(&home);
        }
    }
    std::env::temp_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = trimmed.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }

    let normalized = trimmed.replace('\\', "/");
    if normalized == "/tmp" || normalized.starts_with("/tmp/") {
        let rest = normalized
            .trim_start_matches("/tmp")
            .trim_start_matches('/');
        return if rest.is_empty() {
            std::env::temp_dir()
        } else {
            std::env::temp_dir().join(rest.replace('/', "\\"))
        };
    }

    PathBuf::from(trimmed)
}

pub fn bundled_poppler_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_poppler_dir_for_exe(&exe))
        .filter(|path| path.is_dir())
}

pub fn bundled_pandoc_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_pandoc_dir_for_exe(&exe))
        .filter(|path| path.is_dir())
}

pub fn bundled_asr_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_asr_dir_for_exe(&exe))
        .filter(|path| path.is_dir())
}

pub fn bundled_tesseract_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_tesseract_dir_for_exe(&exe))
        .filter(|path| path.is_dir())
}

pub fn bundled_poppler_dir_for_exe(exe_path: &Path) -> PathBuf {
    exe_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("poppler")
}

pub fn bundled_pandoc_dir_for_exe(exe_path: &Path) -> PathBuf {
    exe_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("pandoc")
}

pub fn bundled_asr_dir_for_exe(exe_path: &Path) -> PathBuf {
    exe_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("asr")
}

pub fn bundled_tesseract_dir_for_exe(exe_path: &Path) -> PathBuf {
    exe_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tesseract")
}

pub fn bundled_pdf_tool_path(command: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pdf_tool_path_for_exe(&exe, command))
}

pub fn bundled_pandoc_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pandoc_tool_path_for_exe(&exe))
}

pub fn bundled_asr_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_tool_path_for_exe(&exe))
}

pub fn bundled_tesseract_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tesseract_tool_path_for_exe(&exe))
}

pub fn bundled_tessdata_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tessdata_dir_for_exe(&exe))
}

pub fn bundled_pdf_tool_path_for_exe(exe_path: &Path, command: &str) -> Option<PathBuf> {
    let filename = pdf_tool_filename(command)?;
    let path = bundled_poppler_dir_for_exe(exe_path).join(filename);
    path.is_file().then_some(path)
}

pub fn bundled_pandoc_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_pandoc_dir_for_exe(exe_path).join(pandoc_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_asr_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_asr_dir_for_exe(exe_path).join(asr_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_tesseract_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_tesseract_dir_for_exe(exe_path).join(tesseract_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_tessdata_dir_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_tesseract_dir_for_exe(exe_path).join("tessdata");
    path.is_dir().then_some(path)
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    bundled_pdf_tool_path(command).unwrap_or_else(|| PathBuf::from(command))
}

pub fn pandoc_tool_path() -> PathBuf {
    bundled_pandoc_tool_path().unwrap_or_else(|| PathBuf::from(PANDOC_TOOL))
}

pub fn tesseract_tool_path() -> PathBuf {
    bundled_tesseract_tool_path().unwrap_or_else(|| PathBuf::from(TESSERACT_TOOL))
}

pub fn bundled_tessdata_has_required_languages() -> bool {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_tessdata_has_required_languages_for_exe(&exe))
        .unwrap_or(false)
}

pub fn bundled_tessdata_has_required_languages_for_exe(exe_path: &Path) -> bool {
    let dir = bundled_tesseract_dir_for_exe(exe_path).join("tessdata");
    dir.join("chi_sim.traineddata").is_file() && dir.join("eng.traineddata").is_file()
}

fn pdf_tool_filename(command: &str) -> Option<String> {
    if command.contains(['/', '\\', MAIN_SEPARATOR]) {
        return None;
    }
    let stem = command.strip_suffix(".exe").unwrap_or(command);
    PDF_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(stem))
        .then(|| format!("{stem}.exe"))
}

fn pandoc_tool_filename() -> &'static str {
    "pandoc.exe"
}

fn asr_tool_filename() -> &'static str {
    "pinvou-asr.exe"
}

fn tesseract_tool_filename() -> &'static str {
    "tesseract.exe"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_tmp_path_maps_to_temp_dir() {
        assert_eq!(
            platform_compat_path("/tmp/pinvou3-test-override"),
            std::env::temp_dir().join("pinvou3-test-override")
        );
    }

    #[test]
    fn bundled_poppler_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_poppler_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou").join("poppler")
        );
    }

    #[test]
    fn bundled_pandoc_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_pandoc_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou").join("pandoc")
        );
    }

    #[test]
    fn extended_length_path_maps_to_normal_windows_path() {
        assert_eq!(
            platform_compat_path(r"\\?\C:\Users\z27014\Downloads\a.pdf"),
            PathBuf::from(r"C:\Users\z27014\Downloads\a.pdf")
        );
        assert_eq!(
            platform_compat_path(r"\\?\UNC\server\share\a.pdf"),
            PathBuf::from(r"\\server\share\a.pdf")
        );
    }

    #[test]
    fn bundled_pdf_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 poppler 路径测试 {}", std::process::id()));
        let poppler = root.join("poppler");
        std::fs::create_dir_all(&poppler).unwrap();
        let tool = poppler.join("pdftotext.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pdf_tool_path_for_exe(&exe, "pdftotext"), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_pandoc_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 pandoc 路径测试 {}", std::process::id()));
        let pandoc = root.join("pandoc");
        std::fs::create_dir_all(&pandoc).unwrap();
        let tool = pandoc.join("pandoc.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pandoc_tool_path_for_exe(&exe), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_tesseract_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\pinvou app\pinvou3.exe");
        assert_eq!(
            bundled_tesseract_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\pinvou app").join("tesseract")
        );
    }

    #[test]
    fn bundled_tesseract_tool_and_tessdata_paths_prefer_bundled_runtime() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 tesseract path test {}", std::process::id()));
        let tesseract = root.join("tesseract");
        let tessdata = tesseract.join("tessdata");
        std::fs::create_dir_all(&tessdata).unwrap();
        let tool = tesseract.join("tesseract.exe");
        let chi = tessdata.join("chi_sim.traineddata");
        let eng = tessdata.join("eng.traineddata");
        std::fs::write(&tool, b"fake exe").unwrap();
        std::fs::write(&chi, b"fake chi").unwrap();
        std::fs::write(&eng, b"fake eng").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_tesseract_tool_path_for_exe(&exe), Some(tool));
        assert_eq!(bundled_tessdata_dir_for_exe(&exe), Some(tessdata));
        assert!(bundled_tessdata_has_required_languages_for_exe(&exe));

        std::fs::remove_dir_all(&root).ok();
    }
}
