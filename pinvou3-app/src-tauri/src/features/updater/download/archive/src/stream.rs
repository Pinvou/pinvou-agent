use crate::{ArchiveError as Error, MemberBytes, directory::Entry, poll};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

pub(crate) fn member(
    file: &mut File,
    entry: &Entry,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<MemberBytes, Error> {
    poll(cancelled)?;
    file.seek(SeekFrom::Start(entry.data_offset))
        .map_err(|_| Error::IoUnavailable)?;
    let input = file.take(entry.compressed);
    let digest = if entry.method == 8 {
        deflated(input, entry, cancelled)?
    } else {
        let mut input = input;
        let digest = hash(&mut input, entry, cancelled)?;
        if input.limit() != 0 {
            return Err(Error::InvalidEntryBytes);
        }
        digest
    };
    Ok(MemberBytes {
        name: entry.name.clone(),
        size: entry.expanded,
        sha256: digest,
    })
}

fn deflated(
    mut input: std::io::Take<impl Read>,
    entry: &Entry,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<[u8; 32], Error> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut decoder = Decompress::new(false);
    let mut compressed = vec![0; 65_536];
    let mut expanded = vec![0; 65_536];
    let mut available = 0;
    let mut offset = 0;
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    loop {
        poll(cancelled)?;
        if offset == available && input.limit() != 0 {
            available = match input.read(&mut compressed) {
                Ok(0) => return Err(Error::InvalidEntryBytes),
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::InvalidEntryBytes),
            };
            offset = 0;
        }
        poll(cancelled)?;
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let flush = if input.limit() == 0 && offset == available {
            FlushDecompress::Finish
        } else {
            FlushDecompress::None
        };
        // One codec step receives at most 64 KiB of input and output. There are
        // no hidden file reads inside a decoder call; cancellation is rechecked
        // between every raw read and every bounded codec step.
        let status = decoder
            .decompress(&compressed[offset..available], &mut expanded, flush)
            .map_err(|_| Error::InvalidEntryBytes)?;
        let consumed = (decoder.total_in() - before_in) as usize;
        let produced = (decoder.total_out() - before_out) as usize;
        offset += consumed;
        size = size
            .checked_add(produced as u64)
            .filter(|n| *n <= entry.expanded)
            .ok_or(Error::InvalidEntryBytes)?;
        hash.update(&expanded[..produced]);
        crc.update(&expanded[..produced]);
        if status == Status::StreamEnd {
            poll(cancelled)?;
            if decoder.total_in() != entry.compressed {
                return Err(Error::CompressedTail);
            }
            if size != entry.expanded || crc.finalize() != entry.crc {
                return Err(Error::InvalidEntryBytes);
            }
            return Ok(hash.finalize().into());
        }
        if consumed == 0 && produced == 0 && (offset != available || input.limit() == 0) {
            return Err(Error::InvalidEntryBytes);
        }
    }
}

fn hash(
    reader: &mut impl Read,
    entry: &Entry,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<[u8; 32], Error> {
    let mut bytes = vec![0; 65_536];
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    loop {
        poll(cancelled)?;
        let count = match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::InvalidEntryBytes),
        };
        size = size
            .checked_add(count as u64)
            .filter(|n| *n <= entry.expanded)
            .ok_or(Error::InvalidEntryBytes)?;
        hash.update(&bytes[..count]);
        crc.update(&bytes[..count]);
    }
    poll(cancelled)?;
    if size != entry.expanded || crc.finalize() != entry.crc {
        return Err(Error::InvalidEntryBytes);
    }
    Ok(hash.finalize().into())
}
