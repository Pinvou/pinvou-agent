!define PINVOU_VC_REDIST_MIN_MAJOR 14
!define PINVOU_VC_REDIST_MIN_MINOR 51
!define PINVOU_VC_REDIST_MIN_BUILD 36247
!define PINVOU_VC_REDIST_MIN_REVISION 0

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Checking Microsoft Visual C++ Redistributable 2015-2022 (x64)..."

  SetRegView 64
  ClearErrors
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  IfErrors pinvou_vc_redist_install
  IntCmp $0 1 pinvou_vc_redist_check_major pinvou_vc_redist_install pinvou_vc_redist_install

pinvou_vc_redist_check_major:
  ClearErrors
  ReadRegDWORD $1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Major"
  IfErrors pinvou_vc_redist_install
  IntCmpU $1 ${PINVOU_VC_REDIST_MIN_MAJOR} pinvou_vc_redist_check_minor pinvou_vc_redist_install pinvou_vc_redist_ready

pinvou_vc_redist_check_minor:
  ClearErrors
  ReadRegDWORD $2 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Minor"
  IfErrors pinvou_vc_redist_install
  IntCmpU $2 ${PINVOU_VC_REDIST_MIN_MINOR} pinvou_vc_redist_check_build pinvou_vc_redist_install pinvou_vc_redist_ready

pinvou_vc_redist_check_build:
  ClearErrors
  ReadRegDWORD $3 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Bld"
  IfErrors pinvou_vc_redist_install
  IntCmpU $3 ${PINVOU_VC_REDIST_MIN_BUILD} pinvou_vc_redist_check_revision pinvou_vc_redist_install pinvou_vc_redist_ready

pinvou_vc_redist_check_revision:
  ClearErrors
  ReadRegDWORD $4 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Rbld"
  IfErrors pinvou_vc_redist_install
  IntCmpU $4 ${PINVOU_VC_REDIST_MIN_REVISION} pinvou_vc_redist_ready pinvou_vc_redist_install pinvou_vc_redist_ready

pinvou_vc_redist_install:
  DetailPrint "Installing Microsoft Visual C++ Redistributable 2015-2022 (x64)..."
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=$PLUGINSDIR\VC_redist.x64.exe" "${__FILEDIR__}\..\..\..\windows-runtime\nsis\vc_redist\VC_redist.x64.exe"
  File "/oname=$PLUGINSDIR\pinvou-vcredist-temp-preflight.ps1" "${__FILEDIR__}\..\..\..\..\packaging\windows\nsis\vcredist-temp-preflight.ps1"
  nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\pinvou-vcredist-temp-preflight.ps1"'
  Pop $6
  Pop $7
  ${If} $6 != 0
    SetRegView lastused
    DetailPrint "VC++ prerequisite temp preflight failed: $7"
    MessageBox MB_ICONSTOP|MB_OK "无法修复 Microsoft Visual C++ 运行库所需的系统临时目录。请确认以管理员身份安装，并检查安全策略或杀毒软件是否阻止了安装程序脚本。$\r$\nFailed to repair the Windows Installer temporary directories: $7. Verify the install is elevated and that security policy or antivirus is not blocking installer scripts." /SD IDOK
    Abort
  ${EndIf}
  DetailPrint "$7"
  System::Call 'Kernel32::SetEnvironmentVariableW(w "TEMP", w "$WINDIR\Temp") i.r8'
  System::Call 'Kernel32::SetEnvironmentVariableW(w "TMP", w "$WINDIR\Temp") i.r8'
  SetOutPath "$INSTDIR"
  ClearErrors
  ExecWait '"$PLUGINSDIR\VC_redist.x64.exe" /install /quiet /norestart /log "$WINDIR\Temp\Pinvou3-vcredist.log"' $5
  IfErrors pinvou_vc_redist_exec_failed

  IntCmp $5 0 pinvou_vc_redist_ready 0 0
  IntCmp $5 3010 pinvou_vc_redist_reboot 0 0
  IntCmp $5 1632 pinvou_vc_redist_temp_failed 0 0
  IntCmp $5 -2147023264 pinvou_vc_redist_temp_failed 0 0
  IntCmp $5 1641 pinvou_vc_redist_reboot pinvou_vc_redist_exit_failed pinvou_vc_redist_exit_failed

pinvou_vc_redist_exec_failed:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable installer could not be started."
  MessageBox MB_ICONSTOP|MB_OK "Microsoft Visual C++ Redistributable installer could not be started." /SD IDOK
  Abort

pinvou_vc_redist_temp_failed:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable still cannot access Windows Installer temporary directories (exit code: $5; 1632 / 0x80070660)."
  MessageBox MB_ICONSTOP|MB_OK "Windows Installer 仍无法访问系统临时目录（错误 1632 / 0x80070660）。请释放系统盘空间，检查系统临时目录权限，重启 Windows 后重试。日志：$WINDIR\Temp\Pinvou3-vcredist.log$\r$\nWindows Installer cannot access its temporary directories. Free space on the system drive, check temporary-directory permissions, restart Windows, and retry. Log: $WINDIR\Temp\Pinvou3-vcredist.log" /SD IDOK
  Abort

pinvou_vc_redist_exit_failed:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable installation failed. Exit code: $5"
  MessageBox MB_ICONSTOP|MB_OK "Microsoft Visual C++ Redistributable installation failed. Exit code: $5" /SD IDOK
  Abort

pinvou_vc_redist_reboot:
  DetailPrint "Microsoft Visual C++ Redistributable requested a reboot."
  SetRebootFlag true

pinvou_vc_redist_ready:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable 2015-2022 (x64) is ready."
!macroend
