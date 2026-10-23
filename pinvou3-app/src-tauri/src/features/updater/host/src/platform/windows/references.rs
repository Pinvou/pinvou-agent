//! Read-only all-process image references for this exact bootstrap inventory.
//! This is not an ApprovedRegistry, arbitrary handle scanner, or T45 owner.
use super::{
    handle::Handle, layout::ordinal_equal, process::RetainedProcess,
    source::SourceInstallerContext as Source,
};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
};
use std::{mem::size_of, path::Path};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, GetLastError},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    },
};

fn under(path: &Path, root: &Path) -> Result<bool> {
    let mut actual = path.components();
    for component in root.components() {
        let Some(found) = actual.next() else {
            return Ok(false);
        };
        if !ordinal_equal(
            found.as_os_str().to_str().ok_or(Error::Referenced)?,
            component.as_os_str().to_str().ok_or(Error::Referenced)?,
        )? {
            return Ok(false);
        }
    }
    Ok(actual.next().is_some())
}
pub(super) fn no_image_references(
    source: &Source,
    allowed_service: Option<&ProcessSelector>,
) -> Result<()> {
    let mut names = vec!["pinvou-upgrade-helper.exe", "pinvou-upgrade-launcher.exe"];
    for file in &source.manifest.app_files {
        names.push(
            file.relative_name
                .rsplit('/')
                .next()
                .ok_or(Error::StateCorrupt)?,
        );
    }
    scan(
        &names,
        &source.layout.code,
        &source.layout.app,
        allowed_service,
    )
}
#[allow(unsafe_code)]
fn scan(names: &[&str], code: &Path, app: &Path, allowed: Option<&ProcessSelector>) -> Result<()> {
    let snapshot = Handle::owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut present = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    let mut observed = 0u32;
    loop {
        if present == 0 {
            if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                return Ok(());
            }
            return Err(Error::Referenced);
        }
        observed = observed
            .checked_add(1)
            .filter(|count| *count <= 65_536)
            .ok_or(Error::Busy)?;
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(Error::Referenced)?;
        let name = String::from_utf16(&entry.szExeFile[..length]).map_err(|_| Error::Referenced)?;
        if names.iter().try_fold(false, |found, expected| {
            Ok::<_, Error>(found || ordinal_equal(&name, expected)?)
        })? {
            // A snapshot PID alone is not a reference proof. Query the actual
            // live image/token/creation incarnation. A disappeared or denied
            // candidate is unknown, never silently interpreted as outside.
            let process =
                RetainedProcess::open(entry.th32ProcessID, None).map_err(|_| Error::Referenced)?;
            if (under(&process.image, code)? || under(&process.image, app)?)
                && allowed != Some(&process.selector)
            {
                return Err(Error::Referenced);
            }
        }
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        present = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_snapshot_finds_current_image_and_never_confuses_sibling_prefix() {
        let process = RetainedProcess::current().unwrap();
        let parent = process.image.parent().unwrap();
        let name = process.image.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            scan(&[name], parent, Path::new(r"Q:\not-an-owner"), None),
            Err(Error::Referenced)
        );
        // Ordinary read-only snapshot control. No installed host, writer or
        // protected registry authority is constructed by this observation.
        assert!(under(Path::new(r"C:\lab-other\a.exe"), Path::new(r"C:\lab")).unwrap() == false);
        assert!(under(Path::new(r"C:\LAB\a.exe"), Path::new(r"C:\lab")).unwrap());
    }
}
