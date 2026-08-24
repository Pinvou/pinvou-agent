pub(crate) fn ensure_staged_attachments_supported(
    has_staged_workspace: bool,
) -> Result<(), &'static str> {
    if cfg!(windows) && has_staged_workspace {
        return Err("attachments_platform_security_unsupported");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_staged_attachments_supported;

    #[test]
    fn plain_runs_are_supported_on_every_platform() {
        assert_eq!(ensure_staged_attachments_supported(false), Ok(()));
    }

    #[test]
    fn staged_runs_follow_the_platform_security_gate() {
        let result = ensure_staged_attachments_supported(true);
        if cfg!(windows) {
            assert_eq!(result, Err("attachments_platform_security_unsupported"));
        } else {
            assert_eq!(result, Ok(()));
        }
    }
}
