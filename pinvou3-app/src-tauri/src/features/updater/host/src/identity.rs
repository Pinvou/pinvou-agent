//! Lossless native process selectors, not elapsed time or caller-created peer authority.
use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileTime(u64);
impl FileTime {
    pub(crate) fn from_native(high: u32, low: u32) -> Self {
        Self(((high as u64) << 32) | low as u64)
    }
    pub(crate) fn ticks(self) -> u64 {
        self.0
    }
    pub(crate) fn parse(text: &str) -> Result<Self> {
        if text.is_empty()
            || text.len() > 20
            || text.len() > 1 && text.starts_with('0')
            || !text.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::IpcInvalid);
        }
        text.parse::<u64>().map(Self).map_err(|_| Error::IpcInvalid)
    }
}
impl Serialize for FileTime {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for FileTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessSelector {
    pub(crate) process_id: u32,
    pub(crate) creation_file_time: FileTime,
}
impl ProcessSelector {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.process_id == 0 || self.creation_file_time.ticks() == 0 {
            Err(Error::PeerRefused)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LogonId {
    pub(crate) high: u32,
    pub(crate) low: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filetime_never_passes_through_a_safe_number() {
        for n in [
            0,
            9_007_199_254_740_991,
            9_007_199_254_740_992,
            133_000_000_000_000_001,
            u64::MAX,
        ] {
            let v = FileTime::from_native((n >> 32) as u32, n as u32);
            let bytes = serde_json::to_vec(&v).unwrap();
            assert_eq!(
                serde_json::from_slice::<FileTime>(&bytes).unwrap().ticks(),
                n
            );
        }
        assert!(
            FileTime::parse("133000000000000001").unwrap()
                != FileTime::parse("133000000000000002").unwrap()
        );
        for bad in [
            "",
            "00",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1.0",
            "1e2",
            "18446744073709551616",
        ] {
            assert!(FileTime::parse(bad).is_err());
        }
        assert!(serde_json::from_str::<FileTime>("133000000000000001").is_err());
    }
}
