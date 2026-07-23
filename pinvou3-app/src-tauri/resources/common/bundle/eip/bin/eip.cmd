@echo off
rem Pinvou EIP CLI wrapper (Windows).
rem Model shell env is whitelist-sanitized, so inherited AGENT_* get dropped;
rem inject them here in the child process before calling eip-cli.exe. device_id /
rem credentials are shared with pinvou3 eip.rs (%USERPROFILE%\.pinvou3\eip\)
rem to keep the login state consistent. Comments MUST stay ASCII: cmd.exe on CP936
rem mis-parses UTF-8 CJK bytes (some equal < & |) and corrupts the batch parse.
setlocal
set "d=%USERPROFILE%\.pinvou3\eip"
if exist "%d%\device_id" set /p AGENT_DEVICE_ID=<"%d%\device_id"
set "AGENT_CREDENTIALS_DIR=%d%\credentials"
set "AGENT_NON_INTERACTIVE=1"
"%~dp0eip-cli.exe" %*
