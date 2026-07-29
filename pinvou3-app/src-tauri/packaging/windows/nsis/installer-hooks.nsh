!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Checking Microsoft Visual C++ Redistributable 2015-2022 (x64)..."

  SetRegView 64
  ClearErrors
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  IfErrors pinvou_vc_redist_install
  IntCmp $0 1 pinvou_vc_redist_ready pinvou_vc_redist_install pinvou_vc_redist_install

pinvou_vc_redist_install:
  DetailPrint "Installing Microsoft Visual C++ Redistributable 2015-2022 (x64)..."
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=$PLUGINSDIR\VC_redist.x64.exe" "${__FILEDIR__}\..\..\..\windows-runtime\nsis\vc_redist\VC_redist.x64.exe"
  SetOutPath "$INSTDIR"
  ExecWait '"$PLUGINSDIR\VC_redist.x64.exe" /install /quiet /norestart' $1

  IntCmp $1 0 pinvou_vc_redist_ready 0 0
  IntCmp $1 3010 pinvou_vc_redist_reboot 0 0
  IntCmp $1 1641 pinvou_vc_redist_reboot 0 0
  MessageBox MB_ICONSTOP|MB_OK "Microsoft Visual C++ Redistributable installation failed. Exit code: $1"
  Abort

pinvou_vc_redist_reboot:
  DetailPrint "Microsoft Visual C++ Redistributable requested a reboot."
  SetRebootFlag true

pinvou_vc_redist_ready:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable 2015-2022 (x64) is ready."
!macroend
