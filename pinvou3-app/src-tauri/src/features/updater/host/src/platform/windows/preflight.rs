//! Read-only first-install admission. No prerequisite download/installation.
use super::{
    layout::ordinal_equal, security::inspect_registry, source::SourceInstallerContext as Source,
};
use crate::error::{Error, Result};
use std::{mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND},
    System::Registry::*,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
#[allow(unsafe_code)]
fn open(root: HKEY, path: &str, view: u32, machine: bool) -> Result<Option<Key>> {
    let mut current = None;
    for part in path.split('\\') {
        if part.is_empty() || part.contains(['/', ':', '\0']) {
            return Err(Error::SecurityRefused);
        }
        let name = wide(part);
        let mut raw = ptr::null_mut();
        let parent = current.as_ref().map_or(root, |key: &Key| key.0);
        let status = unsafe {
            RegOpenKeyExW(
                parent,
                name.as_ptr(),
                REG_OPTION_OPEN_LINK,
                KEY_READ | view,
                &mut raw,
            )
        };
        if matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
            return Ok(None);
        }
        if status != 0 || raw.is_null() {
            return Err(Error::PrerequisiteUnavailable);
        }
        let key = Key(raw);
        let mut kind = 0;
        let mut length = 0;
        let link = wide("SymbolicLinkValue");
        if unsafe {
            RegQueryValueExW(
                key.0,
                link.as_ptr(),
                ptr::null(),
                &mut kind,
                ptr::null_mut(),
                &mut length,
            )
        } != ERROR_FILE_NOT_FOUND
        {
            return Err(Error::SecurityRefused);
        }
        if machine {
            inspect_registry(key.0.cast(), false)?;
        }
        current = Some(key);
    }
    Ok(current)
}
#[allow(unsafe_code)]
fn value(key: &Key, name: &str, expected: u32) -> Result<Option<Vec<u8>>> {
    let name = wide(name);
    let mut kind = 0;
    let mut size = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != 0 || kind != expected || size > 4096 {
        return Err(Error::PrerequisiteUnavailable);
    }
    let mut bytes = vec![0u8; size as usize];
    let capacity = size;
    if unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    } != 0
        || kind != expected
        || size > capacity
    {
        return Err(Error::PrerequisiteUnavailable);
    }
    bytes.truncate(size as usize);
    Ok(Some(bytes))
}
fn text(key: &Key, name: &str) -> Result<Option<String>> {
    let Some(bytes) = value(key, name, REG_SZ)? else {
        return Ok(None);
    };
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
        return Err(Error::PrerequisiteUnavailable);
    }
    let mut words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if words.pop() != Some(0) || words.contains(&0) {
        return Err(Error::PrerequisiteUnavailable);
    }
    String::from_utf16(&words)
        .map(Some)
        .map_err(|_| Error::PrerequisiteUnavailable)
}
fn version(value: &str) -> Result<[u32; 4]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    if value.len() > 64 {
        return Err(Error::PrerequisiteUnavailable);
    }
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 4 {
        return Err(Error::PrerequisiteUnavailable);
    }
    let mut result = [0; 4];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::PrerequisiteUnavailable);
        }
        result[index] = part.parse().map_err(|_| Error::PrerequisiteUnavailable)?;
    }
    Ok(result)
}
pub(super) fn runtime(source: &Source) -> Result<()> {
    source.recheck()?;
    let runtime = open(
        HKEY_LOCAL_MACHINE,
        r"Software\Microsoft\VisualStudio\14.0\VC\Runtimes\x64",
        KEY_WOW64_64KEY,
        true,
    )?
    .ok_or(Error::PrerequisiteUnavailable)?;
    let installed =
        value(&runtime, "Installed", REG_DWORD)?.ok_or(Error::PrerequisiteUnavailable)?;
    if installed.len() != size_of::<u32>()
        || u32::from_le_bytes(
            installed
                .try_into()
                .map_err(|_| Error::PrerequisiteUnavailable)?,
        ) != 1
    {
        return Err(Error::PrerequisiteUnavailable);
    }
    let minimum = version(env!("PINVOU_HOST_MINIMUM_VC_RUNTIME"))?;
    if minimum[0] == 0
        || version(&text(&runtime, "Version")?.ok_or(Error::PrerequisiteUnavailable)?)? < minimum
    {
        return Err(Error::PrerequisiteUnavailable);
    }
    // Baseline Tauri config has no minimumWebview2Version override. Require a
    // real machine-wide installed Runtime for every desktop user, without
    // inventing an unapproved product-version floor or trusting another user's
    // HKCU. Actual app/Engine E2E remains a separate acceptance gate.
    // Microsoft distribution documentation and pinned Tauri 2.11.4 template
    // agree on this exact Runtime identifier and the 32-bit machine view.
    let webview = open(
        HKEY_LOCAL_MACHINE,
        r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        KEY_WOW64_32KEY,
        true,
    )?
    .ok_or(Error::PrerequisiteUnavailable)?;
    if version(&text(&webview, "pv")?.ok_or(Error::PrerequisiteUnavailable)?)?[0] == 0 {
        return Err(Error::PrerequisiteUnavailable);
    }
    source.recheck()
}
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
#[allow(unsafe_code)]
fn no_legacy(root: HKEY, view: u32, machine: bool) -> Result<()> {
    let Some(uninstall) = open(root, UNINSTALL, view, machine)? else {
        return Ok(());
    };
    for index in 0..=4096 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                uninstall.0,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            return Ok(());
        }
        if status != 0 || index == 4096 || length as usize >= name.len() {
            return Err(Error::ScopeConflict);
        }
        let name =
            String::from_utf16(&name[..length as usize]).map_err(|_| Error::ScopeConflict)?;
        if ordinal_equal(&name, "pinvou3")? || ordinal_equal(&name, "com.pinvou.pinvou3")? {
            return Err(Error::ScopeConflict);
        }
        let Some(product) = open(uninstall.0, &name, view, machine)? else {
            return Err(Error::ScopeConflict);
        };
        if text(&product, "DisplayName")?
            .as_deref()
            .map(|name| ordinal_equal(name, "pinvou3"))
            .transpose()?
            .unwrap_or(false)
        {
            return Err(Error::ScopeConflict);
        }
    }
    Err(Error::ScopeConflict)
}
/// Enumerate only real registry children, bounded independently of any URL,
/// JSON or caller identity. Unknown/unreadable/offline scope is not absence.
#[allow(unsafe_code)]
fn children(root: HKEY, mut visit: impl FnMut(&str) -> Result<()>) -> Result<()> {
    for index in 0..=4096 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                root,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            return Ok(());
        }
        if status != 0 || index == 4096 || length == 0 || length as usize >= name.len() {
            return Err(Error::PrerequisiteUnavailable);
        }
        let name = String::from_utf16(&name[..length as usize])
            .map_err(|_| Error::PrerequisiteUnavailable)?;
        visit(&name)?;
    }
    Err(Error::PrerequisiteUnavailable)
}
fn ordinary_profile_sid(name: &str) -> Result<bool> {
    if matches!(name, "S-1-5-18" | "S-1-5-19" | "S-1-5-20") {
        return Ok(false);
    }
    if (name.starts_with("S-1-5-21-") || name.starts_with("S-1-12-1-"))
        && name.split('-').count() == 8
        && name.split('-').skip(1).all(|part| {
            !part.is_empty()
                && (part == "0" || !part.starts_with('0'))
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u32>().is_ok()
        })
    {
        return Ok(true);
    }
    // Including .bak/unknown account profiles: this bootstrap does not load
    // offline hives or infer that their Pinvou data/old installation is absent.
    Err(Error::Unsupported)
}
fn no_user_legacy() -> Result<()> {
    let profiles = open(
        HKEY_LOCAL_MACHINE,
        r"Software\Microsoft\Windows NT\CurrentVersion\ProfileList",
        KEY_WOW64_64KEY,
        true,
    )?
    .ok_or(Error::PrerequisiteUnavailable)?;
    children(profiles.0, |sid| {
        if ordinary_profile_sid(sid)? {
            let hive = open(HKEY_USERS, sid, KEY_WOW64_64KEY, false)?.ok_or(Error::Unsupported)?;
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                no_legacy(hive.0, view, false)?;
            }
        }
        Ok(())
    })?;
    // Also cover every actually loaded ordinary hive, even if ProfileList was
    // changed. Different-admin UAC cannot replace the desktop user's hive with
    // the elevated administrator's HKCU. Per-user keys are untrusted read-only
    // conflict hints, not protected registration or data-preparation evidence.
    children(HKEY_USERS, |name| {
        if name == ".DEFAULT" {
            return Ok(());
        }
        let sid = name.strip_suffix("_Classes").unwrap_or(name);
        if ordinary_profile_sid(sid)? {
            let hive = open(HKEY_USERS, sid, KEY_WOW64_64KEY, false)?
                .ok_or(Error::PrerequisiteUnavailable)?;
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                no_legacy(hive.0, view, false)?;
            }
        }
        Ok(())
    })
}
pub(super) fn first_install(source: &Source) -> Result<()> {
    runtime(source)?;
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        no_legacy(HKEY_LOCAL_MACHINE, view, true)?;
    }
    no_user_legacy()?;
    let own = format!(r"{UNINSTALL}\{}", source.seed.service_name());
    if open(HKEY_LOCAL_MACHINE, &own, KEY_WOW64_64KEY, true)?.is_some() {
        return Err(Error::ScopeConflict);
    }
    source.recheck()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_or_unknown_user_scope_is_never_treated_as_a_clean_profile() {
        assert_eq!(ordinary_profile_sid("S-1-5-21-1-2-3-1001"), Ok(true));
        assert_eq!(ordinary_profile_sid("S-1-12-1-1-2-3-4"), Ok(true));
        assert_eq!(ordinary_profile_sid("S-1-5-18"), Ok(false));
        for value in [
            "S-1-5-21-1.bak",
            "S-1-5-21-1\\Software",
            "S-1-5-21-",
            ".DEFAULT",
            "S-1-5-32-544",
        ] {
            assert_eq!(ordinary_profile_sid(value), Err(Error::Unsupported));
        }
    }
    #[test]
    fn actual_build_floor_comparison_cannot_accept_the_old_14_44_runtime() {
        assert!(version("v14.44.35211.0").unwrap() < version("14.50.35717.0").unwrap());
        assert!(version("14.51.36247.0").unwrap() >= version("14.50.35717.0").unwrap());
        for bad in ["", "14.50", "14.50.35717.0.1", "14.+50.1.0", "v14.1.1.0 "] {
            assert!(version(bad).is_err());
        }
    }
}
