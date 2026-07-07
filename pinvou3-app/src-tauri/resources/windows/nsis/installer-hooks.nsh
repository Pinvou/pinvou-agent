!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Checking Microsoft Visual C++ Redistributable 2015-2022 (x64)..."

  SetRegView 64
  ClearErrors
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  IfErrors vc_redist_install
  IntCmp $0 1 0 vc_redist_install vc_redist_install

  ClearErrors
  ReadRegDWORD $1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Major"
  IfErrors vc_redist_install
  IntCmpU $1 14 vc_redist_check_minor vc_redist_install vc_redist_present

vc_redist_check_minor:
  ClearErrors
  ReadRegDWORD $2 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Minor"
  IfErrors vc_redist_install
  IntCmpU $2 51 vc_redist_check_build vc_redist_install vc_redist_present

vc_redist_check_build:
  ClearErrors
  ReadRegDWORD $3 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Bld"
  IfErrors vc_redist_install
  IntCmpU $3 36247 vc_redist_check_revision vc_redist_install vc_redist_present

vc_redist_check_revision:
  ClearErrors
  ReadRegDWORD $4 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Rbld"
  IfErrors vc_redist_install
  IntCmpU $4 0 vc_redist_present vc_redist_install vc_redist_present

vc_redist_install:
  DetailPrint "Installing Microsoft Visual C++ Redistributable 2015-2022 (x64)..."
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=$PLUGINSDIR\VC_redist.x64.exe" "${__FILEDIR__}\..\..\..\..\resources\windows\vc_redist\VC_redist.x64.exe"
  SetOutPath "$INSTDIR"
  ExecWait '"$PLUGINSDIR\VC_redist.x64.exe" /install /quiet /norestart' $5

  IntCmp $5 0 vc_redist_present 0 0
  IntCmp $5 3010 vc_redist_reboot_required 0 0
  IntCmp $5 1641 vc_redist_reboot_required 0 0

  MessageBox MB_ICONSTOP|MB_OK "Microsoft Visual C++ Redistributable installation failed. Exit code: $5"
  Abort

vc_redist_reboot_required:
  DetailPrint "Microsoft Visual C++ Redistributable requested a reboot."
  SetRebootFlag true

vc_redist_present:
  SetRegView lastused
  DetailPrint "Microsoft Visual C++ Redistributable 2015-2022 (x64) is ready."
!macroend

!macro PINVOU_RUN_RUNTIME_ENV MODE REQUIRED
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=$PLUGINSDIR\pinvou-runtime-env.ps1" "${__FILEDIR__}\..\..\..\..\resources\windows\nsis\runtime-env.ps1"
  nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\pinvou-runtime-env.ps1" -Mode "${MODE}" -InstallDir "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    DetailPrint "Pinvou runtime environment update failed: $1"
    ${If} "${REQUIRED}" == "true"
      MessageBox MB_ICONSTOP|MB_OK "Failed to configure bundled Python/Node environment. Please run the installer as administrator."
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Delete "$INSTDIR\dump_system_prompt.exe"
  Delete "$INSTDIR\pinvou-asr.exe"
  Delete "$INSTDIR\llama-funasr-sensevoice.exe"
  Delete "$INSTDIR\fsmn-vad.gguf"
  RmDir /r "$INSTDIR\7zip"
  RmDir /r "$INSTDIR\asr"
  RmDir /r "$INSTDIR\node"
  RmDir /r "$INSTDIR\pandoc"
  RmDir /r "$INSTDIR\poppler"
  RmDir /r "$INSTDIR\python"
  RmDir /r "$INSTDIR\tesseract"
  !insertmacro PINVOU_RUN_RUNTIME_ENV "Install" "true"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro PINVOU_RUN_RUNTIME_ENV "Uninstall" "false"

  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$PROFILE\.pinvou3"
  ${EndIf}
!macroend
