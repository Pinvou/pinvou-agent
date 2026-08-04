//! 系统工具探测、依赖体检与外部命令构建。
//!
//! 把各 ingest 子模块共用的「系统工具是否存在」「体检项」与「拼装 poppler /
//! pandoc / tesseract / 7z / libreoffice 命令」集中在此，供 [`super`] facade
//! 与各格式子模块复用。命令路径与平台策略全部委托 [`crate::platform`]。
//!
//! [`super`]: super

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;

/// 各 ingest 子模块需要的系统工具探测结果（启动时缓存一次）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SystemTools {
    pub pandoc: bool,
    pub pdftotext: bool,
    /// LibreOffice headless —— 用来转 .doc / .ppt / .xls 等旧 office 格式。
    pub libreoffice: bool,
    /// tesseract OCR —— 图片文字识别 + 扫描件 PDF 兜底。
    pub tesseract: bool,
    /// pdftoppm（poppler-utils）—— 把扫描件 PDF 逐页转图再喂给 tesseract。
    pub pdftoppm: bool,
    /// 7z —— 解 zip/7z/rar 等压缩包；Windows 优先使用内置 7-Zip。
    pub sevenzip: bool,
    /// Python —— 解析 .eml 邮件（标准库 email 模块，无额外依赖）。
    pub python: bool,
    /// msgconvert（libemail-outlook-message-perl）—— Linux .msg → .eml；Windows 走 Rust 原生解析。
    pub msgconvert: bool,
}

static SYSTEM_TOOLS: OnceLock<SystemTools> = OnceLock::new();

/// 启动时（或第一次 ingest 时）检测一次系统工具。
pub fn system_tools() -> SystemTools {
    *SYSTEM_TOOLS.get_or_init(|| SystemTools {
        pandoc: crate::platform::os::pandoc_tool_exists(),
        pdftotext: crate::platform::os::pdf_tool_exists("pdftotext"),
        libreoffice: crate::platform::os::command_exists("soffice")
            || crate::platform::os::command_exists("libreoffice"),
        tesseract: crate::platform::os::ocr_tool_exists(),
        pdftoppm: crate::platform::os::pdf_tool_exists("pdftoppm"),
        sevenzip: crate::platform::os::archive_tool_exists(),
        python: crate::platform::os::command_exists(&crate::platform::paths::python_command()),
        msgconvert: !crate::platform::os::msg_converter_required()
            || crate::platform::os::command_exists("msgconvert"),
    })
}

/// 设置页「依赖体检」一项：一类文件解析能力 + 它所需系统工具是否齐全 + 缺失时的 apt 包。
/// `apt` 是空格分隔的包名串，可直接拼进 `sudo apt install <apt>`。能力名走前端 i18n（按 key 映射）。
/// `hint` 为缺省时给用户的手动安装指引（如 macOS 邮件依赖无 Homebrew formula），
/// 前端优先于 `apt` 显示；`skip_serializing_if` 保证老前端无感知。
#[derive(Debug, Clone, Serialize)]
pub struct DependencyCheckItem {
    pub key: String,
    pub installed: bool,
    pub apt: String,
    pub install_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// 一键安装进度的实时事件载荷。平台适配器经纯 Rust 回调上报,
/// 本域层包成此结构后 `app.emit("deps:install_progress", …)`。
#[derive(Clone, Serialize)]
pub struct DepInstallProgress {
    pub package: String,
    pub current: usize,
    pub total: usize,
    pub detail: Option<String>,
}

/// 体检各项可选依赖的安装状态。**实时检测（不走 `system_tools` 的 OnceLock 缓存）**——
/// 用户照提示装完依赖后重新体检要能立刻反映，不能被首次缓存钉死。命名/分组与
/// `ingest` 内各格式分支的 warning 文案同源，缺啥装啥一致。
pub fn check_dependencies() -> Vec<DependencyCheckItem> {
    let item = |key: &str, installed: bool, apt: &str| DependencyCheckItem {
        key: key.into(),
        installed,
        apt: apt.into(),
        install_action: None,
        hint: None,
    };
    let libreoffice = crate::platform::os::command_exists("soffice")
        || crate::platform::os::command_exists("libreoffice");
    let mut items = Vec::new();
    if crate::platform::os::show_pdf_dependency_check() {
        items.push(item(
            "pdf",
            crate::platform::os::pdf_tool_exists("pdftotext"),
            crate::platform::os::pdf_dependency_packages(),
        ));
    }
    if crate::platform::os::show_pandoc_dependency_check() {
        items.push(item(
            "office_modern",
            crate::platform::os::pandoc_tool_exists(),
            crate::platform::os::pandoc_dependency_packages(),
        ));
    }
    items.push(item(
        "voice_asr",
        crate::features::voice::asr_tool_exists(),
        crate::features::voice::asr_dependency_packages(),
    ));
    items.push(item("office_legacy", libreoffice, "libreoffice"));
    if crate::platform::os::show_ocr_dependency_check() {
        items.push(item(
            "ocr",
            crate::platform::os::ocr_tool_exists()
                && crate::platform::os::pdf_tool_exists("pdftoppm"),
            crate::platform::os::ocr_dependency_packages(),
        ));
    }
    if crate::platform::os::show_archive_dependency_check() {
        items.push(item(
            "archive",
            crate::platform::os::archive_tool_exists(),
            crate::platform::os::archive_dependency_packages(),
        ));
    }
    // 邮件依赖(msgconvert)来自 Perl 模块 Email::Outlook::Message,无 Homebrew formula,
    // 无法一键安装。给出手动安装指引(hint),前端优先于 apt 显示,让用户知道怎么装;
    // email_dependency_packages() 仍返回空串,确保不参与一键安装。
    let email_hint = crate::platform::os::email_manual_hint().map(str::to_string);
    items.push(DependencyCheckItem {
        key: "email".into(),
        installed: crate::platform::os::email_tool_exists(),
        apt: crate::platform::os::email_dependency_packages().into(),
        install_action: None,
        hint: email_hint,
    });
    items
}

/// 体检卡「一键安装」：委托 OS 调度层安装缺失依赖。
/// Linux 由 OS 层保留包名白名单和 pkexec/apt 行为；其他系统清晰降级。
pub async fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::features::dependencies::install_dependencies(packages)
    })
    .await
    .map_err(|e| format!("安装任务失败: {e}"))?
}

// ============== 外部命令构建（各 ingest 子模块共用）==============

pub(super) fn pdf_tool_command(command: &str) -> Command {
    crate::platform::process::HiddenCommand::new(crate::platform::os::pdf_tool_path(command))
}

pub(super) fn pandoc_tool_command() -> Command {
    crate::platform::process::HiddenCommand::new(crate::platform::os::pandoc_tool_path())
}

pub(super) fn ocr_tool_command() -> Command {
    crate::platform::process::HiddenCommand::new(crate::platform::os::ocr_tool_path())
}

pub(super) fn archive_tool_command() -> Command {
    crate::platform::process::HiddenCommand::new(crate::platform::os::archive_tool_path())
}

pub(super) fn libreoffice_tool_command() -> Command {
    crate::platform::process::HiddenCommand::new(crate::platform::os::libreoffice_tool_path())
}

pub(super) fn libreoffice_user_installation_arg(profile_dir: &Path) -> Result<String, String> {
    Ok(format!(
        "-env:UserInstallation={}",
        crate::platform::os::file_url_from_path(profile_dir)?
    ))
}

pub(super) fn add_ocr_tessdata_arg(command: &mut Command) {
    if let Some(tessdata_dir) = crate::platform::os::ocr_tessdata_dir() {
        command.arg("--tessdata-dir").arg(tessdata_dir);
    }
}

/// 体检卡「一键安装」：委托 OS 调度层安装缺失依赖。
/// Linux 由 OS 层保留包名白名单和 pkexec/apt 行为；其他系统清晰降级。
/// `app` 用于把平台适配器上报的纯 Rust 进度回调转成 Tauri 事件
/// `deps:install_progress`,前端据此实时刷新「正在安装 X (n/总数)…」,
/// 不再全程只有静态「安装中…」(libreoffice cask 长尾尤其像卡死)。
pub async fn install_dependencies(
    app: tauri::AppHandle,
    packages: Vec<String>,
) -> Result<(), String> {
    use tauri::Emitter;
    tokio::task::spawn_blocking(move || {
        // AppHandle 是 Clone+Send,克隆一份进阻塞线程(既有先例 dingtalk.rs
        // spawn_blocking(move || run_connect_flow(&app2)))。
        let app = app.clone();
        let report = move |package: &str, current: usize, total: usize, detail: Option<&str>| {
            let _ = app.emit(
                "deps:install_progress",
                DepInstallProgress {
                    package: package.to_string(),
                    current,
                    total,
                    detail: detail.map(str::to_string),
                },
            );
        };
        crate::features::dependencies::install_dependencies(packages, Some(&report))
    })
    .await
    .map_err(|e| format!("安装任务失败: {e}"))?
}

/// tesseract 的 `-l` 语言参数。pinvou3 面向国内政企，中文是刚需，所以优先
/// `chi_sim+eng`；若没装中文包(`tesseract-ocr-chi-sim`)则降级 `eng`，不报错。
/// 探测一次缓存：跑 `tesseract --list-langs` 看输出里有没有 `chi_sim`。
pub(super) fn ocr_lang_arg() -> String {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| {
        let mut command = ocr_tool_command();
        command.arg("--list-langs");
        add_ocr_tessdata_arg(&mut command);
        let listed = command
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if listed.lines().any(|l| l.trim() == "chi_sim") {
            "chi_sim+eng".to_string()
        } else {
            "eng".to_string()
        }
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libreoffice_user_installation_uses_encoded_file_url() {
        let arg = libreoffice_user_installation_arg(&std::env::temp_dir().join("profile dir"))
            .expect("file URL");
        assert!(arg.starts_with("-env:UserInstallation=file://"));
        assert!(arg.ends_with("profile%20dir"));
    }

    #[test]
    fn pdf_tool_command_uses_os_layer_program() {
        let command = pdf_tool_command("pdftotext");
        assert_eq!(
            command.get_program(),
            crate::platform::os::pdf_tool_path("pdftotext").as_os_str()
        );
    }

    #[test]
    fn archive_tool_command_uses_os_layer_program() {
        let command = archive_tool_command();
        assert_eq!(
            command.get_program(),
            crate::platform::os::archive_tool_path().as_os_str()
        );
    }

    #[test]
    fn dependency_check_respects_pdf_visibility_policy() {
        let deps = check_dependencies();
        let has_pdf = deps.iter().any(|item| item.key == "pdf");
        let has_pandoc = deps.iter().any(|item| item.key == "office_modern");
        let has_ocr = deps.iter().any(|item| item.key == "ocr");
        let has_archive = deps.iter().any(|item| item.key == "archive");
        assert_eq!(has_pdf, crate::platform::os::show_pdf_dependency_check());
        assert_eq!(
            has_pandoc,
            crate::platform::os::show_pandoc_dependency_check()
        );
        assert_eq!(has_ocr, crate::platform::os::show_ocr_dependency_check());
        assert_eq!(
            has_archive,
            crate::platform::os::show_archive_dependency_check()
        );

        if !crate::platform::os::show_pdf_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("poppler") && !item.apt.contains("pdfto")),
                "hidden Windows Poppler dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::platform::os::show_pandoc_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("pandoc") && item.key != "office_modern"),
                "hidden Windows Pandoc dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::platform::os::show_ocr_dependency_check() {
            assert!(
                deps.iter().all(|item| {
                    !item.apt.contains("tesseract") && !item.apt.contains("tesseract-ocr")
                }),
                "hidden Windows OCR dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::platform::os::show_archive_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("p7zip") && item.key != "archive"),
                "hidden Windows archive dependency should not leave install hints: {deps:?}"
            );
        }
    }

    #[test]
    fn windows_email_dependency_check_uses_native_msg_parser() {
        if !crate::platform::capabilities::is_windows() {
            return;
        }
        let deps = check_dependencies();
        let email = deps
            .iter()
            .find(|item| item.key == "email")
            .expect("email dependency item should exist");

        assert!(email.installed);
        assert!(email.apt.is_empty());
        assert!(!email.apt.contains("libemail-outlook-message-perl"));
        assert!(!email.apt.contains("msgconvert"));
    }

    #[test]
    fn pandoc_tool_command_uses_os_layer_program() {
        let command = pandoc_tool_command();
        assert_eq!(
            command.get_program(),
            crate::platform::os::pandoc_tool_path().as_os_str()
        );
    }

    #[test]
    fn ocr_tool_command_uses_os_layer_program() {
        let command = ocr_tool_command();
        assert_eq!(
            command.get_program(),
            crate::platform::os::ocr_tool_path().as_os_str()
        );
    }

    #[test]
    fn ocr_tessdata_arg_is_added_when_os_layer_provides_dir() {
        let mut command = ocr_tool_command();
        add_ocr_tessdata_arg(&mut command);
        let args: Vec<_> = command.get_args().map(|arg| arg.to_os_string()).collect();
        if let Some(dir) = crate::platform::os::ocr_tessdata_dir() {
            assert!(args.iter().any(|arg| arg == "--tessdata-dir"));
            assert!(args.iter().any(|arg| arg == dir.as_os_str()));
        } else {
            assert!(args.iter().all(|arg| arg != "--tessdata-dir"));
        }
    }

    #[test]
    fn windows_pandoc_missing_message_points_to_repair_install() {
        if !crate::platform::capabilities::is_windows() {
            return;
        }
        let message = crate::platform::os::pandoc_missing_message();
        assert!(!message.contains("sudo apt install pandoc"));
        assert!(message.contains("修复") || message.contains("重新安装"));
    }
}
