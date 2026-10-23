//! Bounded public build material only. Used by build.rs and its real reader tests.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};
const MAXIMUM: u64 = 1_048_576;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "HOST_BUILD_INPUT_LIMIT")
}
/// Only build-time PUBLIC Seed data selects a profile. Runtime callers cannot
/// supply a kind, trust root or acceptance hook. Full trust parsing stays in Seed.
pub(crate) fn validate_profile_mode(
    seed: &[u8],
    acceptance_hooks: bool,
) -> Result<(), &'static str> {
    if seed.is_empty() && !acceptance_hooks {
        return Ok(()); // The original disabled build has neither Seed nor Root.
    }
    let value: serde_json::Value = serde_json::from_slice(seed).map_err(|_| {
        if acceptance_hooks {
            "HOST_ACCEPTANCE_LAB_SEED_REQUIRED"
        } else {
            "HOST_BUILD_PROFILE_INVALID"
        }
    })?;
    let kind = value.get("profileKind").and_then(|v| v.as_str());
    let id = value.get("profileId").and_then(|v| v.as_str());
    if acceptance_hooks && (kind != Some("lab") || !id.is_some_and(|v| v.contains("-fault-"))) {
        return Err("HOST_ACCEPTANCE_SEPARATE_FAULT_PROFILE_REQUIRED");
    }
    let kind = kind.ok_or("HOST_BUILD_PROFILE_INVALID")?;
    let id = id.ok_or("HOST_BUILD_PROFILE_INVALID")?;
    if !matches!(kind, "lab" | "production") || (kind == "production" && id.contains("-fault-")) {
        return Err("HOST_BUILD_PROFILE_INVALID");
    }
    Ok(())
}
pub(crate) fn read_public_input(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAXIMUM {
        return Err(invalid());
    }
    read_bounded(file, metadata.len())
}
fn read_bounded(reader: impl Read, known_size: u64) -> io::Result<Vec<u8>> {
    if known_size == 0 || known_size > MAXIMUM {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(known_size as usize);
    reader.take(MAXIMUM + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAXIMUM {
        return Err(invalid());
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn seed(kind: &str, id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"profileKind": kind, "profileId": id})).unwrap()
    }
    #[test]
    fn build_modes_preserve_disabled_lab_and_separate_fault_profiles() {
        assert!(validate_profile_mode(&[], false).is_ok());
        assert_eq!(
            validate_profile_mode(&[], true),
            Err("HOST_ACCEPTANCE_LAB_SEED_REQUIRED")
        );
        let normal = seed("lab", "normal-profile");
        let fault = seed("lab", "normal-fault-profile");
        assert!(validate_profile_mode(&normal, false).is_ok());
        assert!(validate_profile_mode(&normal, true).is_err());
        assert!(validate_profile_mode(&fault, false).is_ok());
        assert!(validate_profile_mode(&fault, true).is_ok());
    }
    #[test]
    fn production_build_requires_explicit_kind_and_cannot_enable_fault_hooks() {
        let production = seed("production", "stable-installation");
        assert!(validate_profile_mode(&production, false).is_ok());
        assert_eq!(
            validate_profile_mode(&production, true),
            Err("HOST_ACCEPTANCE_SEPARATE_FAULT_PROFILE_REQUIRED")
        );
        for kind in ["", "Production", "shipping", "fault"] {
            assert!(validate_profile_mode(&seed(kind, "stable-installation"), false).is_err());
        }
        let fault = seed("production", "stable-fault-installation");
        assert!(validate_profile_mode(&fault, false).is_err());
        assert!(validate_profile_mode(&fault, true).is_err());
        for invalid in [
            b"{}".as_slice(),
            br#"{"profileKind":"production"}"#,
            b"null",
        ] {
            assert!(validate_profile_mode(invalid, false).is_err());
            assert_eq!(
                validate_profile_mode(invalid, true),
                Err("HOST_ACCEPTANCE_SEPARATE_FAULT_PROFILE_REQUIRED")
            );
        }
    }
    #[test]
    fn growth_after_metadata_is_read_only_to_the_real_limit_plus_one() {
        let mut source = io::repeat(b'x').take(MAXIMUM * 8);
        assert!(read_bounded(&mut source, 1).is_err());
        assert_eq!(source.limit(), MAXIMUM * 8 - (MAXIMUM + 1));
        assert_eq!(
            read_bounded(io::repeat(b'x').take(MAXIMUM), MAXIMUM)
                .unwrap()
                .len(),
            MAXIMUM as usize
        );
        assert!(read_bounded(io::empty(), 1).is_err());
    }
    #[test]
    fn actual_regular_file_metadata_rejects_empty_and_oversize_inputs() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "pinvou-host-public-build-input-{}",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        assert!(read_public_input(&path).is_err());
        file.write_all(b"{}").unwrap();
        file.flush().unwrap();
        assert_eq!(read_public_input(&path).unwrap(), b"{}");
        file.set_len(MAXIMUM + 1).unwrap();
        file.flush().unwrap();
        assert!(read_public_input(&path).is_err());
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
