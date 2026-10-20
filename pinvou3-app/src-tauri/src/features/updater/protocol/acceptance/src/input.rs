use crate::{Error, document};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const OBJECT_BYTES: usize = 1_048_576;
/// Individual protocol object reader; never allocate from an unchecked file size.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, Error> {
    let mut file = File::open(path).map_err(|_| Error::Unavailable)?;
    if !file.metadata().map_err(|_| Error::Unavailable)?.is_file() {
        return Err(Error::InvalidInput);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((OBJECT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() > OBJECT_BYTES {
        return Err(Error::InvalidInput);
    }
    document(&bytes)?;
    Ok(bytes)
}

/// Private QA file descriptor, not a frozen wire contract. Paths are single
/// input basenames and never become database/output destinations.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bundle {
    pub product: String,
    pub component: String,
    pub supplied_now_ms: i64,
    pub roots: Vec<String>,
    pub timestamp: String,
    pub snapshot: String,
    pub members: Vec<String>,
    pub denied_key_ids: Vec<String>,
    pub denied_by_role: BTreeMap<String, Vec<String>>,
}
impl Bundle {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let checked = document(bytes)?;
        let canonical = checked.canonical_bytes().map_err(|_| Error::InvalidInput)?;
        let value: Self = serde_json::from_slice(&canonical).map_err(|_| Error::InvalidInput)?;
        value.validate_fields()?;
        Ok(value)
    }
    fn validate_fields(&self) -> Result<(), Error> {
        if !identifier(&self.product)
            || !identifier(&self.component)
            || self.supplied_now_ms < 0
            || self.supplied_now_ms > 9_007_199_254_740_991
            || self.roots.len() > 4096
            || self.members.len() > 4096
            || self.denied_key_ids.len() > 4096
            || self.denied_by_role.len() > 6
            || self.denied_key_ids.iter().any(|v| !identifier(v))
            || self.denied_by_role.iter().any(|(role, ids)| {
                ![
                    "root",
                    "timestamp",
                    "snapshot",
                    "target",
                    "release",
                    "package",
                ]
                .contains(&role.as_str())
                    || ids.len() > 4096
                    || ids.iter().any(|v| !identifier(v))
            })
        {
            return Err(Error::InvalidInput);
        }
        for name in self
            .roots
            .iter()
            .chain(self.members.iter())
            .chain([&self.timestamp, &self.snapshot])
        {
            basename(name)?;
        }
        Ok(())
    }
    pub(crate) fn evidence(&self) -> Result<Vec<u8>, Error> {
        // The public Rust DTO can be constructed without parse(). Bound each
        // field/collection first, then stream encoding into a capped writer;
        // never clone an unchecked public DTO through serde_json::to_value.
        self.validate_fields()?;
        struct Bounded(Vec<u8>);
        impl std::io::Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > OBJECT_BYTES - self.0.len() {
                    return Err(std::io::Error::other(
                        "QA descriptor exceeds original object limit",
                    ));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Bounded(Vec::new());
        serde_json::to_writer(&mut writer, self).map_err(|_| Error::InvalidInput)?;
        document(&writer.0)?
            .canonical_bytes()
            .map_err(|_| Error::InvalidInput)
    }
    pub(crate) fn context(&self) -> Result<pinvou_upgrade_codec::Document, Error> {
        document(&crate::canonical(
            &serde_json::json!({"product":self.product,"component":self.component}),
        )?)
    }
    pub(crate) fn time(&self) -> Result<pinvou_upgrade_codec::SuppliedProtocolTime, Error> {
        document(self.supplied_now_ms.to_string().as_bytes())?
            .protocol_time_shape()
            .map_err(|_| Error::InvalidInput)
    }
    pub(crate) fn path(&self, directory: &Path, name: &str) -> Result<PathBuf, Error> {
        basename(name)?;
        Ok(directory.join(name))
    }
}
pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .enumerate()
            .all(|(index, b)| b.is_ascii_alphanumeric() || index > 0 && b"._:-".contains(&b))
}
fn basename(value: &str) -> Result<(), Error> {
    if !identifier(value) || value.contains(':') || value == "." || value == ".." {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

#[cfg(test)]
mod public_resource_tests {
    use super::*;
    #[test]
    fn unchecked_public_dto_is_validated_before_bounded_encoding() {
        let mut value=Bundle::parse(br#"{"product":"pinvou","component":"app","suppliedNowMs":0,"roots":[],"timestamp":"timestamp.json","snapshot":"snapshot.json","members":[],"deniedKeyIds":[],"deniedByRole":{}}"#).unwrap();
        assert!(value.evidence().is_ok());
        value.members = vec!["member.json".to_owned(); 4097];
        assert_eq!(value.evidence(), Err(Error::InvalidInput));
        value.members.clear();
        value.product = "p".repeat(OBJECT_BYTES + 1);
        assert_eq!(value.evidence(), Err(Error::InvalidInput));
        value.product = "pinvou".to_owned();
        for role in [
            "root",
            "timestamp",
            "snapshot",
            "target",
            "release",
            "package",
        ] {
            value
                .denied_by_role
                .insert(role.to_owned(), vec!["k".repeat(128); 4096]);
        }
        assert_eq!(value.evidence(), Err(Error::InvalidInput));
    }
    #[test]
    fn oversized_public_anchor_is_refused_before_hash_or_directory_creation() {
        let path =
            std::env::temp_dir().join(format!("pinvou-t27-refused-anchor-{}", std::process::id()));
        assert!(!path.exists());
        assert!(matches!(
            crate::QaStore::create_new(&path, &vec![0; OBJECT_BYTES + 1], &"0".repeat(64), 0),
            Err(Error::InvalidInput)
        ));
        assert!(!path.exists());
    }
}
