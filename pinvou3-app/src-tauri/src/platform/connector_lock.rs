//! 连接器 CLI 的跨功能低层原语：编译期 lock 表访问、可执行文件命名、文件 SHA-256。
//!
//! lock 表（`resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`）
//! 同时被两类功能消费：`features/connectors`（native_installer 下载安装）与
//! `features/marketplace`（store 首启导入时校验存量 CLI 二进制）。按「跨功能复用的
//! 低层能力进全局 platform」的边界从这里供数；`connectors::platform` 的 `lock_json` /
//! `executable_name` 保留为委托，既有调用方零改动。

use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const LOCK_JSON: &str =
    include_str!("../../resources/platforms/linux/aarch64/bundle/connectors/connectors.lock.json");
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LOCK_JSON: &str =
    include_str!("../../resources/platforms/linux/x86_64/bundle/connectors/connectors.lock.json");
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const LOCK_JSON: &str =
    include_str!("../../resources/platforms/macos/aarch64/bundle/connectors/connectors.lock.json");
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const LOCK_JSON: &str =
    include_str!("../../resources/platforms/macos/x86_64/bundle/connectors/connectors.lock.json");
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const LOCK_JSON: &str =
    include_str!("../../resources/platforms/windows/x86_64/bundle/connectors/connectors.lock.json");
#[cfg(not(any(
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const LOCK_JSON: &str = "";

pub fn lock_json() -> &'static str {
    LOCK_JSON
}

pub fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// lock 表钉住的单个 CLI 制品信息（版本 + 二进制 SHA-256）。
/// 只暴露校验存量二进制所需的两个字段；下载地址等仍由 native_installer 私有schema持有。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorArtifactPin {
    pub version: String,
    pub binary_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorLock {
    schema_version: u32,
    platform: String,
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    name: String,
    version: String,
    binary_sha256: String,
}

/// 按名取 lock 表钉住的制品信息。lock 缺失 / schema 版本不符 / 平台不匹配 /
/// 无此条目时返回 None（调用方按「无法校验」处理，不当作校验通过）。
pub fn artifact_pin(name: &str) -> Option<ConnectorArtifactPin> {
    load_pins()?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, pin)| pin)
}

/// lock 表全部制品的 pin（PATH 注入等需要枚举全部 CLI 的场景）。
/// 与 [`artifact_pin`] 同一校验口径；lock 不可用时返回空 vec。
pub fn all_artifact_pins() -> Vec<(String, ConnectorArtifactPin)> {
    load_pins().unwrap_or_default()
}

fn load_pins() -> Option<Vec<(String, ConnectorArtifactPin)>> {
    let lock: ConnectorLock = serde_json::from_str(lock_json()).ok()?;
    if lock.schema_version != 1 {
        return None;
    }
    let expected =
        super::paths::connector_platform_dir(std::env::consts::OS, std::env::consts::ARCH)?;
    if lock.platform != expected {
        return None;
    }
    Some(
        lock.artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.name.clone(),
                    ConnectorArtifactPin {
                        version: artifact.version.clone(),
                        binary_sha256: artifact.binary_sha256.clone(),
                    },
                )
            })
            .collect(),
    )
}

/// 按 lock 表解析 CLI 二进制的「当前应然路径」：
/// `~/.pinvou3/assets/cli/<name>/<version>/<exe>`。版本化布局路径的**唯一解析
/// 入口**——spawn、存量校验、PATH 注入全部从这里取，不散落拼路径。
/// 无 lock 条目（如走 npm 的 tmeet）或平台不支持 → None（调用方回退旧布局/shim）。
pub fn locked_cli_path(name: &str) -> Option<std::path::PathBuf> {
    let pin = artifact_pin(name)?;
    Some(super::paths::assets_cli_dir(name, &pin.version).join(executable_name(name)))
}

/// 文件 SHA-256（小写 hex），供存量 CLI 二进制对照 lock 表。
pub fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(super::encoding::hex_lower(&digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路径解析单点：lock 表内的 CLI 解析到版本化资产目录；tmeet（npm 安装，
    /// 无 lock 条目）与未知名 → None（调用方回退旧布局/shim）。
    #[test]
    fn locked_cli_path_resolves_versioned_layout() {
        let Some(path) = locked_cli_path("lark-cli") else {
            return; // 当前平台无 lock（不支持的架构），无从断言
        };
        let pin = artifact_pin("lark-cli").unwrap();
        let expected = super::super::paths::assets_cli_dir("lark-cli", &pin.version)
            .join(executable_name("lark-cli"));
        assert_eq!(path, expected);
        assert!(
            path.to_string_lossy().contains("assets"),
            "应在版本化资产库下: {}",
            path.display()
        );
        assert!(locked_cli_path("tmeet").is_none(), "tmeet 无 lock 条目");
        assert!(locked_cli_path("no-such-cli").is_none());
        assert!(all_artifact_pins().len() >= 3, "lock 表应含全部制品");
    }
}
