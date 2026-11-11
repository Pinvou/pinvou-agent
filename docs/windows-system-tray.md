# Windows system tray

On Windows, closing the main window (including Alt+F4) hides it to the
notification area and keeps the application and active tasks running. Windows
may place the icon in the notification area's overflow panel.

- Left-click the Pinvou Agent tray icon to show, restore and focus the window.
- Right-click for **Open Pinvou Agent** and **Exit**. Exit terminates the
  application through its normal browser and child-process cleanup path.
- Menu labels follow the application's Chinese, English or Japanese language
  setting, including changes made while the application is running.

Auxiliary windows retain their normal close behavior. If tray initialization
fails or Windows reports that the icon is not registered, the main window closes
normally. Registration is checked on each close, so shell recovery can restore
tray behavior for the same owned icon. Other operating systems retain their
existing lifecycle behavior.

Restoration uses the native main window, independently of browser child
WebViews attached to it. Tray activation, repeated launches, the global
shortcut and pet navigation continue to target the same existing window.

Explicit exit cancels pending ACP startup/load/configuration requests before
collecting sessions, so an unresponsive startup peer cannot hold the pool lock
and prevent process termination. Ordinary long-running requests retain their
existing behavior until application shutdown.

## Verification

Automated checks cover close interception, missing-tray and hide-failure
fallbacks, native click predicates, registration success/failure codes, and the
shared menu translations. A controlled stdio peer covers cancellation and
owned-child termination during an unanswered ACP session load. Source-wiring
checks do not establish interactive Windows behavior.

The Windows regression job explicitly selects `features::system_tray::`, the
portable ACP shutdown tests, and the Windows controlled-peer tests. The owned-icon
shell-registration recovery probe is opt-in because it requires an interactive
Explorer notification area:

```powershell
cargo test --locked --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib features::system_tray::platform::windows::registration::tests::shell_registration_failure_and_recovery_keep_close_safe -- --ignored --exact
```

Before shipping, verify in a Windows desktop session:

1. Close the main window with its title-bar button and Alt+F4. Confirm its
   taskbar entry disappears, its process remains, and running tasks continue.
2. Left-click the tray icon. Confirm the same session returns and receives
   focus. Repeat after minimizing the window and after several close/open cycles.
3. Right-click the icon and use Open. Switch among Chinese, English and
   Japanese and confirm the menu updates without restarting.
4. With the main window hidden and auxiliary windows open, use Exit. Confirm
   all app windows and the tray icon disappear and owned child processes stop.
5. Relaunch a second instance while hidden. Confirm the existing main window
   appears rather than creating a second application process.
6. Open the built-in browser and load a tab, then repeat the hide/restore
   checks using the tray icon, Open menu, second launch, enabled global
   shortcut and pet navigation. Repeat with the main window minimized;
   confirm the browser tab and conversation remain intact.
