use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

pub(crate) fn eip_bin_path() -> Result<PathBuf, String> {
    let bin_dir = crate::platform::paths::bundle_skills_dir()
        .join("eip")
        .join("bin");
    let name = if std::env::consts::ARCH == "aarch64" {
        "eip-cli-aarch64"
    } else {
        "eip-cli"
    };
    let path = bin_dir.join(name);
    if !path.is_file() {
        if std::env::consts::ARCH == "aarch64" && bin_dir.join("eip-cli").is_file() {
            return Err(format!(
                "eip-cli Linux ARM64 binary missing: expected {}. Bundle contains eip-cli, but this Linux device is aarch64; please package a matching aarch64 binary as eip-cli-aarch64.",
                path.display()
            ));
        }
        if bin_dir.join("eip-cli.exe").is_file() {
            return Err(format!(
                "eip-cli Linux binary missing: expected {} for {}. Bundle only contains the Windows .exe; H3C EIP is unavailable on this Linux device until a matching Linux binary is packaged.",
                path.display(),
                std::env::consts::ARCH
            ));
        }
        return Err(format!(
            "eip-cli 未找到: {}（需先把 EIP 技能二进制打包进 bundle）",
            path.display()
        ));
    }
    prepare_linux_cli(&path, "eip-cli")?;
    Ok(path)
}

pub(crate) fn zhidao_bin_path() -> Result<PathBuf, String> {
    let name = if std::env::consts::ARCH == "aarch64" {
        "zhidao-cli-aarch64"
    } else {
        "zhidao-cli"
    };
    let path = crate::platform::paths::bundle_skills_dir()
        .join("zhidao")
        .join("bin")
        .join(name);
    if !path.is_file() {
        return Err(format!(
            "zhidao CLI 未找到: {}（需先把知道技能二进制打包进 bundle）",
            path.display()
        ));
    }
    prepare_linux_cli(&path, "zhidao-cli")?;
    Ok(path)
}

fn prepare_linux_cli(path: &Path, label: &str) -> Result<(), String> {
    validate_linux_cli_arch(path, label)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mode = metadata.permissions().mode();
        if mode & 0o111 == 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

fn validate_linux_cli_arch(path: &Path, label: &str) -> Result<(), String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(());
    };
    if bytes.len() < 20 || &bytes[0..4] != b"\x7FELF" || bytes[5] != 1 {
        return Ok(());
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    let actual = match machine {
        62 => "x86_64",
        183 => "aarch64",
        3 => "x86",
        40 => "arm",
        _ => "unknown",
    };
    let expected = std::env::consts::ARCH;
    let compatible = matches!(
        (expected, actual),
        ("x86_64", "x86_64") | ("aarch64", "aarch64") | ("arm", "arm") | ("x86", "x86")
    );
    if compatible {
        Ok(())
    } else {
        Err(format!(
            "{label} architecture mismatch: packaged binary is {actual}, but this Linux device is {expected}. Please package a matching {expected} Linux binary at {}.",
            path.display()
        ))
    }
}
