const KNOWN_DEP_PACKAGES: &[&str] = &[
    // 多智能体并行隔离（git worktree）依赖。
    "git",
    "ffmpeg",
    "poppler-utils",
    "pandoc",
    "libreoffice",
    "tesseract-ocr",
    "tesseract-ocr-chi-sim",
    "p7zip-full",
    "python3",
    "libemail-outlook-message-perl",
];

pub(super) fn validate_packages(packages: &[String]) -> Result<(), String> {
    if packages.is_empty() {
        return Err("没有需要安装的依赖".into());
    }
    for package in packages {
        if !KNOWN_DEP_PACKAGES.contains(&package.as_str()) {
            return Err(format!("非法包名（不在依赖白名单内）: {package}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_asr_ffmpeg_is_an_allowed_dependency() {
        assert_eq!(validate_packages(&["ffmpeg".to_string()]), Ok(()));
    }

    /// git 必须过白名单：缺失时它会与其它缺失依赖合并成一个批次提交，
    /// 不在白名单会让整批安装被拒（复核 P1）。
    #[test]
    fn git_passes_the_allowlist_alone_and_in_batches() {
        assert_eq!(validate_packages(&["git".to_string()]), Ok(()));
        assert_eq!(
            validate_packages(&["git".to_string(), "ffmpeg".to_string()]),
            Ok(())
        );
    }

    #[test]
    fn dependency_validation_still_rejects_unknown_packages() {
        let error = validate_packages(&["not-a-pinvou-package".to_string()]).unwrap_err();
        assert!(error.contains("不在依赖白名单内"));
    }
}
