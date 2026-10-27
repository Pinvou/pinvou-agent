//! Bounded RT_VERSION fixed words from held bytes, never executable or publisher trust.
//! Standard VS_VERSION_INFO framing matches the T09 Windows version observer.
//! Child framing is bounded and checked; neither CompanyName nor a certificate is used.
use super::{Reader, refused, u16_at, u32_at};
use crate::{download_stage::poll, error::Result, json};
use std::io::{Read, Seek};

// Use the existing PE directory bound rather than a smaller unrelated limit.
const MAX_LEAVES: u32 = super::MAX_DIRECTORY_ENTRIES as u32;
const VERSION_KEY: &[u8] = b"V\0S\0_\0V\0E\0R\0S\0I\0O\0N\0_\0I\0N\0F\0O\0\0\0";

/// Plain parsing DATA. Custody, expected byte identity and current installation
/// must be established and rechecked by the native caller around this parser.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ObservedVersion {
    pub(crate) canonical_version: String,
    pub(crate) version_resources: u32,
}

fn directory_offset(value: u32) -> Result<u32> {
    if value & 0x8000_0000 == 0 {
        return refused();
    }
    Ok(value & 0x7fff_ffff)
}

// PE named entries use offsets to length-prefixed UTF-16 words. Compare the
// actual words, not their offsets, to detect duplicate or unordered names.
fn validate_directory_keys<R: Read + Seek>(
    reader: &mut Reader<'_, R>,
    rows: &[(u32, u32)],
) -> Result<()> {
    let mut previous_name: Option<Vec<u16>> = None;
    let mut previous_id: Option<u32> = None;
    for &(key, _) in rows {
        if key & 0x8000_0000 != 0 {
            let offset = key & 0x7fff_ffff;
            if offset % 2 != 0 {
                return refused();
            }
            let length = reader.metadata(offset, 2)?;
            let count = usize::from(u16_at(&length, 0));
            let start = offset.checked_add(2).ok_or(crate::Error::ImageRefused)?;
            let raw = reader.metadata(start, count * 2)?;
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|word| u16::from_le_bytes([word[0], word[1]]))
                .collect();
            if char::decode_utf16(units.iter().copied()).any(|character| character.is_err())
                || previous_name
                    .as_ref()
                    .is_some_and(|previous| previous >= &units)
            {
                return refused();
            }
            previous_name = Some(units);
        } else {
            if previous_id.is_some_and(|previous| previous >= key) {
                return refused();
            }
            previous_id = Some(key);
        }
    }
    Ok(())
}

fn fixed_version(bytes: &[u8]) -> Result<[u16; 4]> {
    if bytes.len() < 92
        || usize::from(u16_at(bytes, 0)) != bytes.len()
        || u16_at(bytes, 2) != 52
        || u16_at(bytes, 4) != 0
        || &bytes[6..38] != VERSION_KEY
        || bytes[38..40] != [0, 0]
        || u32_at(bytes, 40) != 0xfeef_04bd
        || u32_at(bytes, 44) != 0x0001_0000
    {
        return refused();
    }
    let parts = |high: u32, low: u32| {
        [
            (high >> 16) as u16,
            high as u16,
            (low >> 16) as u16,
            low as u16,
        ]
    };
    let file = parts(u32_at(bytes, 48), u32_at(bytes, 52));
    let product = parts(u32_at(bytes, 56), u32_at(bytes, 60));
    if file != product || file[3] != 0 {
        return refused();
    }
    validate_children(bytes, 92, bytes.len(), 0, &mut 0)?;
    Ok(file)
}

// VS_VERSION_INFO children begin on DWORD boundaries. wLength includes the
// child header, key, value and nested children, but not alignment before the
// next sibling. Text value lengths are UTF-16 words; binary lengths are bytes.
fn validate_children(
    bytes: &[u8],
    mut at: usize,
    end: usize,
    depth: usize,
    count: &mut usize,
) -> Result<()> {
    if depth > 32 {
        return refused();
    }
    while at < end {
        let aligned = at.checked_add(3).ok_or(crate::Error::ImageRefused)? & !3;
        let padding_end = aligned.min(end);
        if bytes[at..padding_end].iter().any(|b| *b != 0) {
            return refused();
        }
        at = padding_end;
        if at == end {
            break;
        }
        if end - at < 6 {
            return refused();
        }
        *count += 1;
        if *count > 4096 {
            return refused();
        }
        let len = usize::from(u16_at(bytes, at));
        let value_len = usize::from(u16_at(bytes, at + 2));
        let kind = u16_at(bytes, at + 4);
        if len < 8 || kind > 1 {
            return refused();
        }
        let child_end = at
            .checked_add(len)
            .filter(|n| *n <= end)
            .ok_or(crate::Error::ImageRefused)?;
        let mut pos = at + 6;
        let mut units = Vec::new();
        loop {
            if child_end - pos < 2 {
                return refused();
            }
            let word = u16_at(bytes, pos);
            pos += 2;
            if word == 0 {
                break;
            }
            units.push(word);
        }
        if units.is_empty() || char::decode_utf16(units).any(|c| c.is_err()) {
            return refused();
        }
        if pos == child_end && value_len == 0 {
            at = child_end;
            continue;
        }
        let value_at = pos.checked_add(3).ok_or(crate::Error::ImageRefused)? & !3;
        if value_at > child_end || bytes[pos..value_at].iter().any(|b| *b != 0) {
            return refused();
        }
        let value_bytes = if kind == 1 {
            value_len.checked_mul(2).ok_or(crate::Error::ImageRefused)?
        } else {
            value_len
        };
        let value_end = value_at
            .checked_add(value_bytes)
            .filter(|n| *n <= child_end)
            .ok_or(crate::Error::ImageRefused)?;
        validate_children(bytes, value_end, child_end, depth + 1, count)?;
        at = child_end;
    }
    Ok(())
}

/// Inspect every RT_VERSION name/language leaf of an already held PE. Missing,
/// malformed, duplicate or unordered resource paths and disagreement refuse.
/// Aggregate records are bounded BEFORE each payload allocation/read. Polling
/// preserves caller cancellation; there is no wall-clock or download deadline.
pub(crate) fn observe_version<R: Read + Seek>(
    file: &mut R,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<ObservedVersion> {
    poll(cancelled)?;
    let mut reader = Reader::new(file)?;
    poll(cancelled)?;
    let root = reader.directory(0, &[])?;
    validate_directory_keys(&mut reader, &root)?;
    let Some((_, types)) = root.into_iter().find(|(kind, _)| *kind == 16) else {
        return refused();
    };
    let types = directory_offset(types)?;
    let names = reader.directory(types, &[0])?;
    if names.is_empty() {
        return refused();
    }
    validate_directory_keys(&mut reader, &names)?;
    let mut observed = None;
    let mut leaves = 0u32;
    let mut total_bytes = 0usize;
    for (_, languages) in names {
        poll(cancelled)?;
        let languages = directory_offset(languages)?;
        let rows = reader.directory(languages, &[0, types])?;
        if rows.is_empty() {
            return refused();
        }
        validate_directory_keys(&mut reader, &rows)?;
        for (_, entry) in rows {
            poll(cancelled)?;
            if leaves == MAX_LEAVES {
                return refused();
            }
            let remaining = json::RECORD_MAX
                .checked_sub(total_bytes)
                .ok_or(crate::Error::ImageRefused)?;
            let bytes = reader.payload(entry, remaining)?;
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .ok_or(crate::Error::ImageRefused)?;
            let version = fixed_version(&bytes)?;
            if observed.is_some_and(|previous| previous != version) {
                return refused();
            }
            observed = Some(version);
            leaves += 1;
        }
    }
    poll(cancelled)?;
    let version = observed.ok_or(crate::Error::ImageRefused)?;
    Ok(ObservedVersion {
        canonical_version: format!("{}.{}.{}", version[0], version[1], version[2]),
        version_resources: leaves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn word(b: &mut [u8], at: usize, value: u16) {
        b[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn dword(b: &mut [u8], at: usize, value: u32) {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    // Numeric name1 with distinct language leaves sharing one physical record.
    // This is synthetic byte DATA, never an installed native observation.
    fn fixture(leaves: usize, record_size: usize) -> (Vec<u8>, usize) {
        let entry = 0x50 + leaves * 8;
        let payload = entry + 16;
        let size = (payload + record_size).div_ceil(512) * 512;
        let mut b = super::super::tests::fixture();
        b.resize(512 + size, 0);
        b[512..].fill(0);
        dword(&mut b, 204, size as u32);
        dword(&mut b, 320, size as u32);
        dword(&mut b, 328, size as u32);
        word(&mut b, 526, 1);
        dword(&mut b, 528, 16);
        dword(&mut b, 532, 0x8000_0020);
        word(&mut b, 558, 1);
        dword(&mut b, 560, 1);
        dword(&mut b, 564, 0x8000_0040);
        word(&mut b, 590, leaves as u16);
        for index in 0..leaves {
            dword(&mut b, 592 + index * 8, index as u32);
            dword(&mut b, 596 + index * 8, entry as u32);
        }
        dword(&mut b, 512 + entry, 4096 + payload as u32);
        dword(&mut b, 516 + entry, record_size as u32);
        let at = 512 + payload;
        word(&mut b, at, record_size as u16);
        word(&mut b, at + 2, 52);
        b[at + 6..at + 38].copy_from_slice(VERSION_KEY);
        dword(&mut b, at + 40, 0xfeef_04bd);
        dword(&mut b, at + 44, 0x0001_0000);
        for (offset, value) in [
            (48, 0x0001_0002),
            (52, 0x0003_0000),
            (56, 0x0001_0002),
            (60, 0x0003_0000),
        ] {
            dword(&mut b, at + offset, value);
        }
        if record_size > 92 {
            // A single binary child fills the remainder, with a DWORD-aligned value.
            assert!(record_size >= 104);
            word(&mut b, at + 92, (record_size - 92) as u16);
            word(&mut b, at + 94, (record_size - 104) as u16);
            word(&mut b, at + 98, b'X' as u16);
        }
        (b, at)
    }
    fn read(b: Vec<u8>) -> Result<ObservedVersion> {
        observe_version(&mut Cursor::new(b), &mut || false)
    }

    #[test]
    fn derives_actual_fixed_words_and_requires_every_language_to_agree() {
        let (mut b, at) = fixture(2, 92);
        assert_eq!(
            read(b.clone()).unwrap(),
            ObservedVersion {
                canonical_version: "1.2.3".into(),
                version_resources: 2,
            }
        );
        // Point the second language at a different complete fixed record.
        let new_entry = at + 92;
        let new_payload = new_entry + 16;
        let copy = b[at..at + 92].to_vec();
        b[new_payload..new_payload + 92].copy_from_slice(&copy);
        dword(&mut b, 604, (new_entry - 512) as u32);
        dword(&mut b, new_entry, (new_payload - 512 + 4096) as u32);
        dword(&mut b, new_entry + 4, 92);
        dword(&mut b, new_payload + 48, 0x0001_0004);
        dword(&mut b, new_payload + 56, 0x0001_0004);
        assert!(read(b).is_err());
    }

    #[test]
    fn rejects_missing_empty_or_malformed_version_paths() {
        assert!(read(super::super::tests::fixture()).is_err());
        for mutate in [
            |b: &mut Vec<u8>| word(b, 558, 0),
            |b: &mut Vec<u8>| word(b, 590, 0),
            |b: &mut Vec<u8>| dword(b, 564, 0x8000_0020),
            |b: &mut Vec<u8>| dword(b, 596, 0x8000_0000),
            |b: &mut Vec<u8>| {
                word(b, 556, 1);
                word(b, 558, 0);
                dword(b, 560, 0x8000_0101); // Unaligned UTF-16 name.
            },
            |b: &mut Vec<u8>| dword(b, 612, u32::MAX),
        ] {
            let (mut b, _) = fixture(1, 92);
            mutate(&mut b);
            assert!(read(b).is_err());
        }
    }

    #[test]
    fn named_resource_keys_are_validated_by_utf16_value() {
        let (mut b, _) = fixture(2, 92);
        word(&mut b, 556, 1);
        word(&mut b, 558, 0);
        dword(&mut b, 560, 0x8000_0120);
        word(&mut b, 588, 2);
        word(&mut b, 590, 0);
        dword(&mut b, 592, 0x8000_0100);
        dword(&mut b, 600, 0x8000_0110);
        for (at, character) in [(768, b'A'), (784, b'B'), (800, b'C')] {
            word(&mut b, at, 1);
            word(&mut b, at + 2, character as u16);
        }
        assert_eq!(read(b.clone()).unwrap().version_resources, 2);
        let mut duplicate = b.clone();
        word(&mut duplicate, 786, b'A' as u16);
        assert!(read(duplicate).is_err());
        let mut invalid_utf16 = b.clone();
        word(&mut invalid_utf16, 770, 0xd800);
        assert!(read(invalid_utf16).is_err());
        let mut out_of_bounds = b;
        dword(&mut out_of_bounds, 592, 0x8000_01fe);
        word(&mut out_of_bounds, 1022, 1);
        assert!(read(out_of_bounds).is_err());
    }

    #[test]
    fn numeric_resource_keys_must_be_in_increasing_order() {
        let (mut b, _) = fixture(2, 92);
        dword(&mut b, 592, 1);
        dword(&mut b, 600, 0);
        assert!(read(b).is_err());
    }

    #[test]
    fn root_keys_and_child_framing_are_checked() {
        let (mut root, _) = fixture(1, 92);
        // Two numeric root entries, out of order, while retaining RT_VERSION.
        word(&mut root, 526, 2);
        dword(&mut root, 528, 16);
        dword(&mut root, 536, 1);
        dword(&mut root, 540, 0x8000_0020);
        assert!(read(root).is_err());
        let (good, at) = fixture(1, 112);
        assert!(read(good.clone()).is_ok());
        for (offset, value) in [(92, 0), (92, 21), (94, 9), (96, 2), (98, 0)] {
            let mut bad = good.clone();
            word(&mut bad, at + offset, value);
            assert!(read(bad).is_err(), "child offset {offset}");
        }
        let mut zeros = good;
        zeros[at + 92..at + 112].fill(0);
        assert!(read(zeros).is_err());
    }

    #[test]
    fn rejects_noncanonical_fixed_records_without_manifest_or_nsis_fallback() {
        let (base, at) = fixture(1, 92);
        for (offset, replacement) in [
            (0, 91u32),
            (2, 51),
            (4, 1),
            (6, 0),
            (38, 1),
            (40, 0),
            (44, 0),
            (44, 0x0002_0000),
            (52, 0x0003_0001),
            (56, 0x0001_0004),
        ] {
            let mut b = base.clone();
            if offset < 40 {
                word(&mut b, at + offset, replacement as u16);
            } else {
                dword(&mut b, at + offset, replacement);
            }
            assert!(read(b).is_err(), "fixed offset {offset}");
        }
        let mut both_fourth = base;
        dword(&mut both_fourth, at + 52, 0x0003_0001);
        dword(&mut both_fourth, at + 60, 0x0003_0001);
        assert!(read(both_fourth).is_err());
    }

    #[test]
    fn bounds_all_leaf_records_and_bytes_including_shared_payloads() {
        assert_eq!(read(fixture(4096, 92).0).unwrap().version_resources, 4096);
        assert!(read(fixture(4097, 92).0).is_err());
        assert_eq!(
            read(fixture(16, u16::MAX as usize).0)
                .unwrap()
                .version_resources,
            16
        );
        assert!(read(fixture(17, u16::MAX as usize).0).is_err());
    }

    #[test]
    fn cancellation_after_an_earlier_valid_leaf_returns_no_partial_observation() {
        let mut polls = 0;
        let result = observe_version(&mut Cursor::new(fixture(8, 92).0), &mut || {
            polls += 1;
            polls >= 5
        });
        assert_eq!(result, Err(crate::Error::StagingCanceled));
        assert_eq!(polls, 5);
    }

    #[test]
    #[ignore = "opt-in read-only check against an independently supplied PE"]
    fn supplied_actual_pe_version_matches_independent_observation() {
        let path = std::env::var_os("PINVOU_TEST_VERSION_PE").expect("explicit PE path");
        let expected = std::env::var("PINVOU_TEST_VERSION_EXPECTED").expect("independent version");
        let mut file = std::fs::File::open(path).expect("read-only PE");
        let observed = observe_version(&mut file, &mut || false).expect("valid version resources");
        assert_eq!(observed.canonical_version, expected);
    }
}
