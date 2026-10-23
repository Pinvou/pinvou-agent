//! Bounded PE resource reads from an already held package file, never its path.
//! Parsing bytes grants no protected-source or installation authority.
use crate::{
    error::{Error, Result},
    inventory::{FIRST_RESOURCE_ID, InventoryBytes, MAX_SHARDS},
    json,
};
use std::io::{Read, Seek, SeekFrom};

const MAX_SECTIONS: usize = 96;
const MAX_DIRECTORY_ENTRIES: usize = 4096;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

struct Section {
    virtual_address: u64,
    virtual_size: u64,
    raw: u64,
    size: u64,
}
struct Reader<'a, R> {
    file: &'a mut R,
    length: u64,
    sections: Vec<Section>,
    resource_rva: u64,
    resource_size: u64,
    metadata_read: usize,
}
fn refused<T>() -> Result<T> {
    Err(Error::ImageRefused)
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(Error::ImageRefused)
}
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
impl<'a, R: Read + Seek> Reader<'a, R> {
    fn new(file: &'a mut R) -> Result<Self> {
        let length = file
            .seek(SeekFrom::End(0))
            .map_err(|_| Error::ImageRefused)?;
        let mut value = Self {
            file,
            length,
            sections: Vec::new(),
            resource_rva: 0,
            resource_size: 0,
            metadata_read: 0,
        };
        let dos = value.bytes(0, 64)?;
        if &dos[..2] != b"MZ" {
            return refused();
        }
        let pe = u32_at(&dos, 60) as u64;
        let header = value.bytes(pe, 24)?;
        if &header[..4] != b"PE\0\0" {
            return refused();
        }
        let count = u16_at(&header, 6) as usize;
        let optional_size = u16_at(&header, 20) as usize;
        if count == 0 || count > MAX_SECTIONS || optional_size < 2 {
            return refused();
        }
        let optional = value.bytes(add(pe, 24)?, optional_size)?;
        let (directory, count_offset) = match u16_at(&optional, 0) {
            0x10b => (96, 92),
            0x20b => (112, 108),
            _ => return refused(),
        };
        if optional.len() < directory + 24 || u32_at(&optional, count_offset) < 3 {
            return refused();
        }
        value.resource_rva = u32_at(&optional, directory + 16) as u64;
        value.resource_size = u32_at(&optional, directory + 20) as u64;
        if value.resource_rva == 0 || value.resource_size < 16 {
            return refused();
        }
        add(value.resource_rva, value.resource_size)?;
        let section_table = value.bytes(add(add(pe, 24)?, optional_size as u64)?, count * 40)?;
        for row in section_table.chunks_exact(40) {
            let section = Section {
                virtual_size: u32_at(row, 8) as u64,
                virtual_address: u32_at(row, 12) as u64,
                size: u32_at(row, 16) as u64,
                raw: u32_at(row, 20) as u64,
            };
            if add(section.raw, section.size)? > length {
                return refused();
            }
            add(
                section.virtual_address,
                section.virtual_size.max(section.size),
            )?;
            value.sections.push(section);
        }
        // Reject overlapping RVA/raw mappings rather than choose the first row.
        for (i, left) in value.sections.iter().enumerate() {
            for right in &value.sections[i + 1..] {
                for (a, al, b, bl) in [
                    (
                        left.virtual_address,
                        left.virtual_size.max(left.size),
                        right.virtual_address,
                        right.virtual_size.max(right.size),
                    ),
                    (left.raw, left.size, right.raw, right.size),
                ] {
                    if al != 0 && bl != 0 && a < add(b, bl)? && b < add(a, al)? {
                        return refused();
                    }
                }
            }
        }
        value.map(value.resource_rva, value.resource_size)?;
        Ok(value)
    }
    fn bytes(&mut self, offset: u64, size: usize) -> Result<Vec<u8>> {
        if add(offset, size as u64)? > self.length {
            return refused();
        }
        let mut bytes = vec![0; size];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| Error::ImageRefused)?;
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::ImageRefused)?;
        Ok(bytes)
    }
    fn map(&self, rva: u64, size: u64) -> Result<u64> {
        let end = add(rva, size)?;
        let mut found = None;
        for section in &self.sections {
            if rva >= section.virtual_address && end <= add(section.virtual_address, section.size)?
            {
                let offset = add(section.raw, rva - section.virtual_address)?;
                if found.replace(offset).is_some() || add(offset, size)? > self.length {
                    return refused();
                }
            }
        }
        found.ok_or(Error::ImageRefused)
    }
    fn metadata(&mut self, relative: u32, size: usize) -> Result<Vec<u8>> {
        if add(relative as u64, size as u64)? > self.resource_size {
            return refused();
        }
        self.metadata_read = self
            .metadata_read
            .checked_add(size)
            .ok_or(Error::ImageRefused)?;
        if self.metadata_read > MAX_METADATA_BYTES {
            return refused();
        }
        let offset = self.map(add(self.resource_rva, relative as u64)?, size as u64)?;
        self.bytes(offset, size)
    }
    fn directory(&mut self, at: u32, ancestors: &[u32]) -> Result<Vec<(u32, u32)>> {
        if at & 0x8000_0000 != 0 || ancestors.contains(&at) {
            return refused();
        }
        let header = self.metadata(at, 16)?;
        let named = u16_at(&header, 12) as usize;
        let numeric = u16_at(&header, 14) as usize;
        let count = named.checked_add(numeric).ok_or(Error::ImageRefused)?;
        if count > MAX_DIRECTORY_ENTRIES {
            return refused();
        }
        let start = at.checked_add(16).ok_or(Error::ImageRefused)?;
        let rows = self.metadata(start, count * 8)?;
        let mut result = Vec::with_capacity(count);
        let mut keys = std::collections::BTreeSet::new();
        for (index, row) in rows.chunks_exact(8).enumerate() {
            let key = u32_at(row, 0);
            if (key & 0x8000_0000 != 0) != (index < named)
                || !keys.insert(key)
                || (index >= named && key > u16::MAX as u32)
            {
                return refused();
            }
            result.push((key, u32_at(row, 4)));
        }
        Ok(result)
    }
    fn resource(&mut self, id: u16) -> Result<Option<Vec<u8>>> {
        let root = self.directory(0, &[])?;
        let Some((_, resource_types)) = root.iter().find(|(key, _)| *key == 10) else {
            return Ok(None);
        };
        if resource_types & 0x8000_0000 == 0 {
            return refused();
        }
        let types = resource_types & 0x7fff_ffff;
        let names = self.directory(types, &[0])?;
        let Some((_, languages)) = names.iter().find(|(key, _)| *key == id as u32) else {
            return Ok(None);
        };
        if languages & 0x8000_0000 == 0 {
            return refused();
        }
        let languages = languages & 0x7fff_ffff;
        let rows = self.directory(languages, &[0, types])?;
        // Original contract accepts only the unique neutral language.
        if rows.len() != 1 || rows[0].0 != 0 || rows[0].1 & 0x8000_0000 != 0 {
            return refused();
        }
        let data = self.metadata(rows[0].1, 16)?;
        let rva = u32_at(&data, 0) as u64;
        let size = u32_at(&data, 4) as usize;
        if size == 0 || size > json::RECORD_MAX || u32_at(&data, 12) != 0 {
            return refused();
        }
        let relative = rva
            .checked_sub(self.resource_rva)
            .ok_or(Error::ImageRefused)?;
        if add(relative, size as u64)? > self.resource_size {
            return refused();
        }
        let offset = self.map(rva, size as u64)?;
        Ok(Some(self.bytes(offset, size)?))
    }
}

pub(crate) fn inventory(file: &mut std::fs::File) -> Result<InventoryBytes> {
    let mut reader = Reader::new(file)?;
    let root = reader
        .resource(crate::profile::MANIFEST_RESOURCE_ID)?
        .ok_or(Error::ImageRefused)?;
    let mut shards = Vec::new();
    // Absence is checked against the parsed root by the caller. Preserve holes
    // as refusal instead of collapsing page numbers during collection.
    let mut absent = false;
    for index in 0..MAX_SHARDS {
        match reader.resource(FIRST_RESOURCE_ID + index as u16)? {
            Some(bytes) if !absent => shards.push(bytes),
            Some(_) => return refused(),
            None => absent = true,
        }
    }
    Ok(InventoryBytes { root, shards })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn word(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn dword(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture() -> Vec<u8> {
        let mut b = vec![0; 1024];
        b[..2].copy_from_slice(b"MZ");
        dword(&mut b, 60, 64);
        b[64..68].copy_from_slice(b"PE\0\0");
        word(&mut b, 70, 1);
        word(&mut b, 84, 224);
        word(&mut b, 88, 0x10b);
        dword(&mut b, 180, 16);
        dword(&mut b, 200, 4096);
        dword(&mut b, 204, 512);
        dword(&mut b, 320, 512);
        dword(&mut b, 324, 4096);
        dword(&mut b, 328, 512);
        dword(&mut b, 332, 512);
        word(&mut b, 526, 1);
        dword(&mut b, 528, 10);
        dword(&mut b, 532, 0x8000_0020);
        word(&mut b, 558, 1);
        dword(&mut b, 560, 34101);
        dword(&mut b, 564, 0x8000_0040);
        word(&mut b, 590, 1);
        dword(&mut b, 592, 0);
        dword(&mut b, 596, 96);
        dword(&mut b, 608, 4096 + 128);
        dword(&mut b, 612, 5);
        b[640..645].copy_from_slice(b"exact");
        b
    }
    fn read(bytes: Vec<u8>) -> Result<Option<Vec<u8>>> {
        Reader::new(&mut Cursor::new(bytes))?.resource(34101)
    }
    #[test]
    fn reads_unique_neutral_resource_from_file_offsets() {
        assert_eq!(read(fixture()).unwrap(), Some(b"exact".to_vec()));
        let mut cursor = Cursor::new(fixture());
        let mut reader = Reader::new(&mut cursor).unwrap();
        assert_eq!(reader.resource(34102).unwrap(), None);
    }
    #[test]
    fn rejects_cycles_duplicate_ids_and_language_ambiguity() {
        for mutate in [
            |b: &mut Vec<u8>| dword(b, 532, 0x8000_0000),
            |b: &mut Vec<u8>| {
                word(b, 558, 2);
                dword(b, 568, 34101);
                dword(b, 572, 0x8000_0040);
            },
            |b: &mut Vec<u8>| {
                word(b, 590, 2);
                dword(b, 600, 1033);
                dword(b, 604, 96);
            },
            |b: &mut Vec<u8>| dword(b, 592, 1033),
        ] {
            let mut b = fixture();
            mutate(&mut b);
            assert!(read(b).is_err());
        }
    }
    #[test]
    fn rejects_section_rva_file_and_payload_bounds() {
        for mutate in [
            |b: &mut Vec<u8>| dword(b, 332, u32::MAX),
            |b: &mut Vec<u8>| dword(b, 608, 4096 + 510),
            |b: &mut Vec<u8>| dword(b, 612, u32::MAX),
            |b: &mut Vec<u8>| dword(b, 564, 0xffff_fff0),
            |b: &mut Vec<u8>| {
                word(b, 70, 2);
                b.copy_within(312..352, 352);
            },
        ] {
            let mut b = fixture();
            mutate(&mut b);
            assert!(read(b).is_err());
        }
        for size in [0, 63, 200, 644] {
            assert!(read(fixture()[..size].to_vec()).is_err());
        }
    }
    #[test]
    fn rejects_partial_section_overlap_even_without_full_range_containment() {
        for (virtual_address, raw_offset) in [(4096 + 130, 1024), (8192, 642)] {
            let mut bytes = fixture();
            bytes.resize(1026, 0);
            word(&mut bytes, 70, 2);
            // The second section covers only two of the five payload bytes,
            // either virtually or in the original file. It cannot contain the
            // complete resource interval, yet the mapping remains ambiguous.
            dword(&mut bytes, 360, 2);
            dword(&mut bytes, 364, virtual_address);
            dword(&mut bytes, 368, 2);
            dword(&mut bytes, 372, raw_offset);
            assert!(read(bytes).is_err());
        }
    }
}
