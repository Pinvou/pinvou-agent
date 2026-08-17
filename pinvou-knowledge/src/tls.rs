use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};

const TLS_DIR: &str = "tls";
const CA_CERT: &str = "ca.pem";
const CA_KEY: &str = "ca-key.pem";
const SERVER_CERT: &str = "server.pem";
const SERVER_KEY: &str = "server-key.pem";

#[derive(Debug, Clone)]
pub struct TlsIdentity {
    pub ca_pem: String,
    pub ca_encoded: String,
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

pub fn ensure_tls_identity(data_dir: &Path) -> Result<TlsIdentity, String> {
    let tls_dir = data_dir.join(TLS_DIR);
    fs::create_dir_all(&tls_dir).map_err(|error| format!("无法创建加密身份目录：{error}"))?;
    secure_directory(&tls_dir)?;
    let ca_cert_path = tls_dir.join(CA_CERT);
    let ca_key_path = tls_dir.join(CA_KEY);
    match (ca_cert_path.exists(), ca_key_path.exists()) {
        (false, false) => generate_ca(&ca_cert_path, &ca_key_path)?,
        (true, true) => {}
        _ => return Err("共享知识库加密身份不完整；为避免身份变化，已拒绝自动重建".to_string()),
    }
    secure_file(&ca_cert_path)?;
    secure_file(&ca_key_path)?;

    let ca_pem = fs::read_to_string(&ca_cert_path)
        .map_err(|error| format!("无法读取共享知识库加密身份：{error}"))?;
    let ca_key = fs::read_to_string(&ca_key_path)
        .map_err(|error| format!("无法读取共享知识库加密密钥：{error}"))?;
    let certificate_path = tls_dir.join(SERVER_CERT);
    let private_key_path = tls_dir.join(SERVER_KEY);
    generate_server_certificate(&ca_pem, &ca_key, &certificate_path, &private_key_path)?;

    Ok(TlsIdentity {
        ca_encoded: URL_SAFE_NO_PAD.encode(ca_pem.as_bytes()),
        ca_pem,
        certificate_path,
        private_key_path,
    })
}

fn generate_ca(certificate_path: &Path, key_path: &Path) -> Result<(), String> {
    let mut params = CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "PINVOU Shared Knowledge CA");
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let key = KeyPair::generate().map_err(|error| error.to_string())?;
    let certificate = params
        .self_signed(&key)
        .map_err(|error| error.to_string())?;
    write_private_file(key_path, key.serialize_pem().as_bytes())?;
    write_private_file(certificate_path, certificate.pem().as_bytes())
}

fn generate_server_certificate(
    ca_pem: &str,
    ca_key_pem: &str,
    certificate_path: &Path,
    key_path: &Path,
) -> Result<(), String> {
    let ca_key = KeyPair::from_pem(ca_key_pem)
        .map_err(|error| format!("共享知识库加密密钥无效：{error}"))?;
    let issuer = Issuer::from_ca_cert_pem(ca_pem, ca_key)
        .map_err(|error| format!("共享知识库加密证书无效：{error}"))?;
    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    for interface in
        if_addrs::get_if_addrs().map_err(|error| format!("无法读取本机网络地址：{error}"))?
    {
        let address = interface.addr.ip();
        if acceptable_certificate_address(address) {
            let value = address.to_string();
            if !names.contains(&value) {
                names.push(value);
            }
        }
    }
    let mut params = CertificateParams::new(names).map_err(|error| error.to_string())?;
    params
        .distinguished_name
        .push(DnType::CommonName, "PINVOU Shared Knowledge");
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate().map_err(|error| error.to_string())?;
    let certificate = params
        .signed_by(&key, &issuer)
        .map_err(|error| format!("无法签发共享知识库服务证书：{error}"))?;
    write_private_file(key_path, key.serialize_pem().as_bytes())?;
    write_private_file(certificate_path, certificate.pem().as_bytes())
}

fn acceptable_certificate_address(address: IpAddr) -> bool {
    !address.is_unspecified() && !address.is_multicast()
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let _ = fs::remove_file(&temporary);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("无法写入共享知识库加密身份：{error}"))?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), String> {
    // Product-hosted shared knowledge is Linux-only. On Windows the standalone
    // development binary inherits the data directory's user ACL; std::fs cannot
    // safely replace that ACL without platform security APIs and an explicit
    // account/SID policy, so we do not pretend POSIX modes provide protection.
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> Result<(), String> {
    // See secure_directory: non-Unix hosting is outside the supported product
    // boundary and relies on the caller-provided parent directory ACL.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_ca_stays_stable_while_leaf_is_refreshed() {
        let root = tempfile::tempdir().unwrap();
        let first = ensure_tls_identity(root.path()).unwrap();
        let first_leaf = fs::read(&first.certificate_path).unwrap();
        let first_key = fs::read(&first.private_key_path).unwrap();
        let second = ensure_tls_identity(root.path()).unwrap();
        assert_eq!(first.ca_pem, second.ca_pem);
        assert_eq!(first.ca_encoded, second.ca_encoded);
        assert_ne!(first_leaf, fs::read(&second.certificate_path).unwrap());
        assert_ne!(first_key, fs::read(&second.private_key_path).unwrap());
    }
}
