use crate::{ArchiveError as Error, Budget, paths::Registry, poll, read_at, u16, u32};
use std::fs::File;

pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) directory: bool,
    pub(crate) method: u16,
    pub(crate) crc: u32,
    pub(crate) compressed: u64,
    pub(crate) expanded: u64,
    pub(crate) data_offset: u64,
    local_offset: u64,
    flags: u16,
    version: u16,
    modified_time: u16,
    modified_date: u16,
}

pub(crate) fn scan(
    file: &mut File,
    size: u64,
    budget: &mut Budget,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<Entry>, Error> {
    if size < 22 {
        return Err(Error::InvalidStructure);
    }
    let tail_size = size.min(22 + 65_535);
    let mut tail = vec![0; tail_size as usize];
    read_at(file, size - tail_size, &mut tail, cancelled)?;
    let mut end = None;
    for index in (0..=tail.len() - 22).rev() {
        if u32(&tail[index..]) == 0x06054b50
            && index + 22 + usize::from(u16(&tail[index + 20..])) == tail.len()
            && end.replace(index).is_some()
        {
            return Err(Error::InvalidStructure);
        }
    }
    let end = end.ok_or(Error::InvalidStructure)?;
    budget.metadata((tail.len() - end) as u64)?;
    let record = &tail[end..end + 22];
    if u16(&record[4..]) != 0 || u16(&record[6..]) != 0 || u16(&record[8..]) != u16(&record[10..]) {
        return Err(Error::UnsupportedFormat);
    }
    let count = u16(&record[10..]);
    let central_size = u64::from(u32(&record[12..]));
    let central_offset = u64::from(u32(&record[16..]));
    if count == u16::MAX
        || central_size == u64::from(u32::MAX)
        || central_offset == u64::from(u32::MAX)
    {
        return Err(Error::UnsupportedFormat);
    }
    let end_offset = size - tail_size + end as u64;
    if count > budget.limits.entries {
        return Err(Error::ResourceLimit);
    }
    if central_offset > end_offset
        || central_size != end_offset - central_offset
        || u64::from(count) * 46 > central_size
    {
        return Err(Error::InvalidStructure);
    }
    let mut paths = Registry::new(budget.limits.path_state_bytes);
    let mut entries = Vec::with_capacity(usize::from(count));
    let mut position = central_offset;
    for _ in 0..count {
        poll(cancelled)?;
        if end_offset - position < 46 {
            return Err(Error::InvalidStructure);
        }
        let mut fixed = [0; 46];
        read_at(file, position, &mut fixed, cancelled)?;
        if u32(&fixed) != 0x02014b50 {
            return Err(Error::InvalidStructure);
        }
        let flags = u16(&fixed[8..]);
        let method = u16(&fixed[10..]);
        let version = u16(&fixed[6..]);
        if !(10..=20).contains(&version)
            || (method == 8 && version < 20)
            || flags & !0x080e != 0
            || ![0, 8].contains(&method)
            || (method == 0 && flags & 6 != 0)
        {
            return Err(Error::UnsupportedFormat);
        }
        let compressed = u64::from(u32(&fixed[20..]));
        let expanded = u64::from(u32(&fixed[24..]));
        let local_offset = u64::from(u32(&fixed[42..]));
        if [compressed, expanded, local_offset].contains(&u64::from(u32::MAX))
            || u16(&fixed[34..]) != 0
        {
            return Err(Error::UnsupportedFormat);
        }
        let name_size = u64::from(u16(&fixed[28..]));
        let extra_size = u64::from(u16(&fixed[30..]));
        let comment_size = u64::from(u16(&fixed[32..]));
        let length = 46 + name_size + extra_size + comment_size;
        if !(1..=1025).contains(&name_size) || length > end_offset - position {
            return Err(Error::InvalidStructure);
        }
        budget.metadata(length)?;
        let creator = fixed[5];
        let attributes = u32(&fixed[38..]);
        validate_type(creator, attributes)?;
        let mut name_bytes = vec![0; name_size as usize];
        let mut extra = vec![0; extra_size as usize];
        read_at(file, position + 46, &mut name_bytes, cancelled)?;
        read_at(file, position + 46 + name_size, &mut extra, cancelled)?;
        validate_extra(&extra)?;
        let name = String::from_utf8(name_bytes).map_err(|_| Error::UnsafePath)?;
        let directory = name.ends_with('/');
        let kind = attributes >> 16 & 0xf000;
        if (attributes & 0x10 != 0 && !directory)
            || ([3, 19].contains(&creator)
                && ((kind == 0x8000 && directory) || (kind == 0x4000 && !directory)))
        {
            return Err(Error::UnsafeFileType);
        }
        paths.insert(&name, directory)?;
        let crc = u32(&fixed[16..]);
        if directory
            && (expanded != 0 || compressed != 0 || method != 0 || crc != 0 || flags & 8 != 0)
        {
            return Err(Error::InvalidStructure);
        }
        if method == 0 && compressed != expanded {
            return Err(Error::InvalidStructure);
        }
        budget.expanded(expanded, compressed)?;
        entries.push(Entry {
            name,
            directory,
            method,
            crc,
            compressed,
            expanded,
            data_offset: 0,
            local_offset,
            flags,
            version,
            modified_time: u16(&fixed[12..]),
            modified_date: u16(&fixed[14..]),
        });
        position += length;
    }
    if position != end_offset {
        return Err(Error::InvalidStructure);
    }
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_unstable_by_key(|index| entries[*index].local_offset);
    position = 0;
    for index in order {
        poll(cancelled)?;
        if entries[index].local_offset != position {
            return Err(Error::InvalidStructure);
        }
        position = local(file, central_offset, &mut entries[index], budget, cancelled)?;
    }
    if position != central_offset {
        return Err(Error::InvalidStructure);
    }
    Ok(entries)
}

fn validate_type(creator: u8, attributes: u32) -> Result<(), Error> {
    if ![0, 3, 10, 19].contains(&creator) {
        return Err(Error::UnsupportedFormat);
    }
    if attributes & 0xffff & !0x37 != 0 {
        return Err(Error::UnsafeFileType);
    }
    let mode = attributes >> 16;
    if [3, 19].contains(&creator) {
        if ![0, 0x8000, 0x4000].contains(&(mode & 0xf000)) || mode & 0o7000 != 0 {
            return Err(Error::UnsafeFileType);
        }
    } else if mode != 0 {
        return Err(Error::UnsafeFileType);
    }
    Ok(())
}

fn validate_extra(mut bytes: &[u8]) -> Result<(), Error> {
    let mut seen_timestamp = false;
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(Error::InvalidStructure);
        }
        let id = u16(bytes);
        let length = usize::from(u16(&bytes[2..]));
        if length > bytes.len() - 4 {
            return Err(Error::InvalidStructure);
        }
        if id != 0x5455 {
            return Err(Error::UnsupportedFormat);
        }
        if seen_timestamp {
            return Err(Error::InvalidStructure);
        }
        seen_timestamp = true;
        let data = &bytes[4..4 + length];
        if data.is_empty()
            || data[0] & !7 != 0
            || !(data.len() - 1).is_multiple_of(4)
            || data.len() > 13
        {
            return Err(Error::InvalidStructure);
        }
        bytes = &bytes[4 + length..];
    }
    Ok(())
}

fn local(
    file: &mut File,
    boundary: u64,
    e: &mut Entry,
    budget: &mut Budget,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<u64, Error> {
    if e.local_offset > boundary || boundary - e.local_offset < 30 {
        return Err(Error::InvalidStructure);
    }
    let mut header = [0; 30];
    read_at(file, e.local_offset, &mut header, cancelled)?;
    if u32(&header) != 0x04034b50
        || u16(&header[4..]) != e.version
        || u16(&header[6..]) != e.flags
        || u16(&header[8..]) != e.method
        || u16(&header[10..]) != e.modified_time
        || u16(&header[12..]) != e.modified_date
    {
        return Err(Error::InvalidStructure);
    }
    let name_size = u64::from(u16(&header[26..]));
    let extra_size = u64::from(u16(&header[28..]));
    let start = e.local_offset + 30;
    if name_size != e.name.len() as u64 || name_size + extra_size > boundary - start {
        return Err(Error::InvalidStructure);
    }
    budget.metadata(30 + name_size + extra_size)?;
    let mut name = vec![0; name_size as usize];
    let mut extra = vec![0; extra_size as usize];
    read_at(file, start, &mut name, cancelled)?;
    read_at(file, start + name_size, &mut extra, cancelled)?;
    if name != e.name.as_bytes() {
        return Err(Error::InvalidStructure);
    }
    validate_extra(&extra)?;
    e.data_offset = start + name_size + extra_size;
    if e.compressed > boundary - e.data_offset {
        return Err(Error::InvalidStructure);
    }
    let mut next = e.data_offset + e.compressed;
    let crc = u32(&header[14..]);
    let compressed = u64::from(u32(&header[18..]));
    let expanded = u64::from(u32(&header[22..]));
    if e.flags & 8 == 0 {
        if crc != e.crc || compressed != e.compressed || expanded != e.expanded {
            return Err(Error::InvalidStructure);
        }
        return Ok(next);
    }
    if crc != 0 || compressed != 0 || expanded != 0 || boundary - next < 12 {
        return Err(Error::InvalidStructure);
    }
    let mut descriptor = [0; 16];
    let descriptor_start = next;
    read_at(file, next, &mut descriptor[..12], cancelled)?;
    let data = if u32(&descriptor) == 0x08074b50 {
        if boundary - next < 16 {
            return Err(Error::InvalidStructure);
        }
        read_at(file, next, &mut descriptor, cancelled)?;
        next += 16;
        &descriptor[4..]
    } else {
        next += 12;
        &descriptor[..12]
    };
    if u32(data) != e.crc
        || u64::from(u32(&data[4..])) != e.compressed
        || u64::from(u32(&data[8..])) != e.expanded
    {
        return Err(Error::InvalidStructure);
    }
    budget.metadata(next - descriptor_start)?;
    Ok(next)
}
