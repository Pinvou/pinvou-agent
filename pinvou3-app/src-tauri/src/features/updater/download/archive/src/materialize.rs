use crate::{
    ArchiveError as Error, ArchiveLimits, Artifact, Budget, MemberBytes, SAFE_SIZE, check_package,
    directory, inspect_file, paths::Registry, poll, stream,
};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

/// Independent owner-supplied member locator and byte identity. This constructor
/// validates shape; it does not authenticate native claims or choose FullPack.
pub struct ExpectedMember<'a> {
    name: &'a str,
    size: u64,
    sha256: [u8; 32],
}
impl<'a> ExpectedMember<'a> {
    pub fn new(name: &'a str, size: u64, sha256: [u8; 32]) -> Result<Self, Error> {
        if size > SAFE_SIZE {
            return Err(Error::InvalidExpectedMember);
        }
        Registry::new(crate::MAX_METADATA)
            .insert(name, false)
            .map_err(|_| Error::InvalidExpectedMember)?;
        Ok(Self { name, size, sha256 })
    }
}

/// Observed package/member facts only. No destination lock, durable identity,
/// native authenticity, protected staging token or installation right is supplied.
pub struct MaterializedMember<'a> {
    package: Artifact<'a>,
    member: MemberBytes,
}
impl<'a> MaterializedMember<'a> {
    pub fn package(&self) -> Artifact<'a> {
        self.package
    }
    pub fn member(&self) -> &MemberBytes {
        &self.member
    }
}

/// Stream one current regular member into a protected fresh destination supplied
/// by its owner, sync it and rescan the same destination handle to real EOF.
/// Old inventories are not inputs. Failure leaves untrusted bytes for owner cleanup.
pub fn materialize_member<'a>(
    source: &mut File,
    destination: &mut File,
    package: Artifact<'a>,
    expected: ExpectedMember<'_>,
    limits: ArchiveLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<MaterializedMember<'a>, Error> {
    materialize_with_sync(
        source,
        destination,
        package,
        expected,
        limits,
        &mut cancelled,
        &mut |file| file.sync_all().map_err(|_| Error::IoUnavailable),
    )
}

fn materialize_with_sync<'a>(
    source: &mut File,
    destination: &mut File,
    package: Artifact<'a>,
    expected: ExpectedMember<'_>,
    limits: ArchiveLimits,
    cancelled: &mut impl FnMut() -> bool,
    sync: &mut impl FnMut(&File) -> Result<(), Error>,
) -> Result<MaterializedMember<'a>, Error> {
    poll(cancelled)?;
    let metadata = destination.metadata().map_err(|_| Error::IoUnavailable)?;
    if !metadata.is_file() {
        return Err(Error::UnsafeFileType);
    }
    if metadata.len() != 0 {
        return Err(Error::DestinationNotEmpty);
    }
    let inventory = inspect_file(source, package, limits, &mut *cancelled)?;
    let selected = inventory
        .members()
        .iter()
        .find(|m| m.name == expected.name)
        .ok_or(Error::MemberMismatch)?;
    if selected.size != expected.size || selected.sha256 != expected.sha256 {
        return Err(Error::MemberMismatch);
    }
    // Reparse current physical records. Neither a previous inventory nor the
    // initial selected digest can replace the second actual member-byte read.
    let mut budget = Budget {
        limits,
        metadata: 0,
        expanded: 0,
    };
    let entries = directory::scan(source, package.size(), &mut budget, cancelled)?;
    let entry = entries
        .iter()
        .find(|e| !e.directory && e.name == expected.name)
        .ok_or(Error::MemberMismatch)?;
    if entry.expanded != expected.size {
        return Err(Error::MemberMismatch);
    }
    poll(cancelled)?;
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    let member = stream::member_with_visit(source, entry, cancelled, &mut |bytes, cancelled| {
        write_chunk(destination, bytes, cancelled)
    })?;
    if member.size != expected.size || member.sha256 != expected.sha256 {
        return Err(Error::MemberMismatch);
    }
    check_package(source, package, cancelled)?;
    poll(cancelled)?;
    sync(destination)?;
    poll(cancelled)?;
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    // Destination is an ordinary decoded file. Never reuse the source ZIP's
    // data offset or compressed span to limit this real EOF scan (including empty).
    let digest = stream::hash(destination, entry, cancelled, &mut |_, _| Ok(()))?;
    if digest != expected.sha256 {
        return Err(Error::MemberMismatch);
    }
    poll(cancelled)?;
    Ok(MaterializedMember { package, member })
}

fn write_chunk(
    writer: &mut impl Write,
    mut bytes: &[u8],
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
    while !bytes.is_empty() {
        poll(cancelled)?;
        match writer.write(bytes) {
            Ok(0) => return Err(Error::IoUnavailable),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::IoUnavailable),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
