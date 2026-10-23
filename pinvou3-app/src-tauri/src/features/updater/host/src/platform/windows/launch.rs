//! Continuous protected pathname bridge for ordinary-user app/launcher starts.
//! CreateProcessW is not a handle-launch API. Native image/ancestor leases stay
//! held across its loader re-open, suspended child check and exact ResumeThread.
use super::{
    client::Client,
    handle::Handle,
    layout::ordinal_equal,
    process::{RetainedProcess, creation_time},
};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    json,
};
use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION,
    ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

pub(super) enum Target<'a> {
    App,
    RestartLauncher(&'a str),
}
struct SuspendedChild {
    process: Handle,
    thread: Option<Handle>,
    process_id: u32,
    resume_attempted: bool,
}
impl Drop for SuspendedChild {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // Only THIS CreateProcess result, never a caller-selected PID. Once a
        // resume was attempted, uncertainty must not kill a possible live GUI.
        if !self.resume_attempted {
            if unsafe { TerminateProcess(self.process.raw(), 1) } != 0 {
                unsafe {
                    WaitForSingleObject(self.process.raw(), 30_000);
                }
            }
        }
    }
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
            out.push(c);
        }
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}
#[allow(unsafe_code)]
pub(super) fn spawn(client: &Client, target: Target<'_>) -> Result<RetainedProcess> {
    client.recheck()?;
    let (path, image, arguments) = match target {
        Target::App => (client.layout.app_path(), &client.app, Vec::new()),
        Target::RestartLauncher(id) => {
            if !json::digest(id) {
                return Err(Error::IpcInvalid);
            }
            (
                client.layout.launcher_path(),
                &client.launcher,
                vec!["--restart-handoff".to_owned(), id.to_owned()],
            )
        }
    };
    let application: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut command = quote(path.to_str().ok_or(Error::ImageRefused)?);
    for argument in arguments {
        command.push(' ');
        command.push_str(&quote(&argument));
    }
    let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let cwd = path.parent().ok_or(Error::ImageRefused)?;
    let cwd: Vec<u16> = cwd.as_os_str().encode_wide().chain(Some(0)).collect();
    if command.len() > 32767 || application.len() > 32767 || cwd.len() > 32767 {
        return Err(Error::ImageRefused);
    }
    let original =
        pinvou_upgrade_custody::identity(&image.file).map_err(|_| Error::ImageRefused)?;
    if original != image.identity {
        return Err(Error::ImageRefused);
    }
    let mut startup = STARTUPINFOW::default();
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut information = PROCESS_INFORMATION::default();
    // Fixed absolute application and own ordinary token, no shell/search path,
    // inherited handles, System token, arbitrary arguments, or privileged env.
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            ptr::null(),
            cwd.as_ptr(),
            &startup,
            &mut information,
        )
    } == 0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    let mut child = SuspendedChild {
        process: Handle::owned(information.hProcess)?,
        thread: None,
        process_id: information.dwProcessId,
        resume_attempted: false,
    };
    child.thread = Some(Handle::owned(information.hThread)?);
    complete_suspended(client, image.identity, &path, child)
}
fn complete_suspended(
    client: &Client,
    identity: pinvou_upgrade_custody::Identity,
    path: &std::path::Path,
    mut child: SuspendedChild,
) -> Result<RetainedProcess> {
    let created = creation_time(child.process.raw())?;
    let observed = RetainedProcess::open(child.process_id, None)?;
    if observed.selector
        != (ProcessSelector {
            process_id: child.process_id,
            creation_file_time: created,
        })
        || observed.token.user != client.current.token.user
        || observed.token.app_container
        || observed.token.network
        || observed.token.integrity != client.current.token.integrity
        || observed.token.elevated != client.current.token.elevated
        || created.ticks() < client.current.selector.creation_file_time.ticks()
        || !ordinal_equal(
            observed.image.to_str().ok_or(Error::ImageRefused)?,
            path.to_str().ok_or(Error::ImageRefused)?,
        )?
    {
        return Err(Error::ImageRefused);
    }
    client.recheck()?;
    let file = if path == client.layout.app_path() {
        &client.app.file
    } else {
        &client.launcher.file
    };
    if pinvou_upgrade_custody::identity(file).map_err(|_| Error::ImageRefused)? != identity {
        return Err(Error::ImageRefused);
    }
    let thread = child
        .thread
        .as_ref()
        .ok_or(Error::HandoffOutcomeUnknown)?
        .raw();
    child.resume_attempted = true;
    #[allow(unsafe_code)]
    let previous = unsafe { ResumeThread(thread) };
    // Unexpected suspend counts are not permission to issue another resume.
    if previous != 1 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    drop(child);
    Ok(observed)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_windows_argument_quoting_never_concatenates_an_escape() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote(r"a\"), "\"a\\\\\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
}
