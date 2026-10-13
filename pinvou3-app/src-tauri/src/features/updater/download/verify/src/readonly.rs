//! Actual read-only handle byte scan; no content seal, ownership or rights proof.
use crate::{ByteCheckError, ByteMatch, check_file};
use pinvou_upgrade_cache_object::{ExclusiveCacheFile, ReadOnlyCacheHandle};
use pinvou_upgrade_transfer::Artifact;

/// Internally clone this current read-only handle, never reopen a path or borrow
/// an arbitrary caller-mutable File. The clone inherits the source permissions
/// and sharing rules, shares its cursor, and closes on every return path.
/// Run on an owner blocking worker, keeping every alias/writer coordinated and
/// preserving the original T26 validation budget through cancellation. No new
/// clock/deadline is created. Other writer retirement/namespace/VMO are external.
pub fn check_read_only_cache<'a>(
    view: &ReadOnlyCacheHandle,
    target: Artifact<'a>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    let mut file = view
        .file()
        .try_clone()
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    let matched = check_file(&mut file, target, &mut cancelled)?;
    // Hash finalization may take time after B3's EOF cancellation check.
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    Ok(matched)
}

/// Copy current read-only cache bytes into the supplied fresh exclusive target,
/// sync and fully rescan that same destination. No previous match is accepted.
/// Failure may leave untrusted partial output; the owner handles cleanup and all
/// protection/alias duties. This returns observed bytes, never sealed staging.
pub fn copy_read_only_cache<'a>(
    view: &ReadOnlyCacheHandle,
    destination: &mut ExclusiveCacheFile,
    target: Artifact<'a>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    let mut source = view
        .file()
        .try_clone()
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    let matched =
        crate::copy_and_check(&mut source, destination.file_mut(), target, &mut cancelled)?;
    // Destination hash finalization follows the last B3 EOF cancellation check.
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    Ok(matched)
}

#[cfg(test)]
mod tests;
