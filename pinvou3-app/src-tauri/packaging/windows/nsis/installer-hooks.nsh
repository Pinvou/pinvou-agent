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
  SetOutPath "$INSTDIR"
  ClearErrors
  ExecWait '"$PLUGINSDIR\VC_redist.x64.exe" /install /quiet /norestart' $5
  IfErrors pinvou_vc_redist_exec_failed

  IntCmp $5 0 pinvou_vc_redist_ready 0 0
  IntCmp $5 3010 pinvou_vc_redist_reboot 0 0
  IntCmp $5 1641 pinvou_vc_redist_reboot pinvou_vc_redist_exit_failed pinvou_vc_redist_exit_failed

pinvou_vc_redist_exec_failed:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable installer could not be started."
  MessageBox MB_ICONSTOP|MB_OK "Microsoft Visual C++ Redistributable installer could not be started." /SD IDOK
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
