; Independent enabled host template. No upstream hooks, native plugins or extracted helper.
Unicode true
ManifestDPIAware true
RequestExecutionLevel admin
CRCCheck force
SetCompressor /SOLID lzma
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!if "${NSIS_VERSION}" != "v3.11"
  !error "HOST_BUILD_NSIS_VERSION_UNSUPPORTED"
!endif
!include "${PINVOU_HOST_BUILD_INCLUDE}"
; Numeric file/product words are validated appVersion.0, never authority.
VIProductVersion "${HOST_FILE_VERSION}"
VIFileVersion "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=1033 "ProductName" "${HOST_PRODUCT_EN}"
VIAddVersionKey /LANG=1033 "FileDescription" "${HOST_PRODUCT_EN} installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" ""
VIAddVersionKey /LANG=2052 "ProductName" "${HOST_PRODUCT_ZH}"
VIAddVersionKey /LANG=2052 "FileDescription" "${HOST_PRODUCT_ZH}安装器"
VIAddVersionKey /LANG=2052 "FileVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" ""
VIAddVersionKey /LANG=1041 "ProductName" "${HOST_PRODUCT_JA}"
VIAddVersionKey /LANG=1041 "FileDescription" "${HOST_PRODUCT_JA}インストーラー"
VIAddVersionKey /LANG=1041 "FileVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=1041 "ProductVersion" "${HOST_FILE_VERSION}"
VIAddVersionKey /LANG=1041 "LegalCopyright" ""
Name "${HOST_DISPLAY_NAME}"
OutFile "${HOST_OUTFILE}"
ShowInstDetails show
AutoCloseWindow false
LoadLanguageFile "${NSISDIR}\Contrib\Language files\English.nlf"
LoadLanguageFile "${NSISDIR}\Contrib\Language files\SimpChinese.nlf"
LoadLanguageFile "${NSISDIR}\Contrib\Language files\Japanese.nlf"
LangString HostFailure 1033 "Installation stopped. The protected host stage did not complete."
LangString HostFailure 2052 "安装已停止。受保护的宿主步骤未完成。"
LangString HostFailure 1041 "インストールを停止しました。保護されたホスト処理が完了しませんでした。"
LangString HostOptional 1033 "Installation completed. Starting the application was not confirmed. Use its launcher shortcut."
LangString HostOptional 2052 "安装已完成，尚未确认应用已启动。请使用启动器快捷方式。"
LangString HostOptional 1041 "インストールは完了しました。アプリの起動は未確認です。ランチャーのショートカットを使用してください。"
LangString HostRun 1033 "Start the installed application as the desktop user?"
LangString HostRun 2052 "是否以桌面用户身份启动已安装的应用？"
LangString HostRun 1041 "デスクトップユーザーとしてアプリを起動しますか？"
Var HostSource
Var HostCode
Var HostApp
Var HostRemove
Var HostLaunch
Var HostStageExit
Var HostOptionMask
Page instfiles

!macro HostCall STAGE
  ClearErrors
  ExecWait '$\"$HostSource\pinvou-upgrade-helper.exe$\" --installer-stage ${STAGE}' $HostStageExit
  IfErrors host_stage_failed
  StrCmp $HostStageExit 0 +2
  Goto host_stage_failed
!macroend

Function .onInit
  ; These select built-in views only; they perform no target mutation.
  SetRegView 64
  SetShellVarContext all
  StrCpy $HostSource "$PROGRAMFILES64\Pinvou\${HOST_SOURCE_LEAF}\${HOST_PROFILE_ID}\${HOST_BUILD_ID}"
  StrCpy $HostCode "$PROGRAMFILES64\Pinvou\${HOST_CODE_LEAF}\${HOST_PROFILE_ID}"
  StrCpy $HostApp "$PROGRAMFILES64\Pinvou\${HOST_APP_LEAF}\${HOST_PROFILE_ID}"
  StrCpy $HostRemove 0
  StrCpy $HostLaunch 0
  StrCpy $HostOptionMask 0
  ; Reject a possibly truncated NSIS string before decoding any option.
  StrLen $0 $CMDLINE
  IntCmp $0 ${NSIS_MAX_STRLEN} host_stage_failed 0 host_stage_failed
  IntOp $1 ${NSIS_MAX_STRLEN} - 1
  IntCmp $0 $1 host_stage_failed 0 host_stage_failed
  ${GetParameters} $0
  StrCpy $1 ""
host_argument_next:
  StrCmp $0 "" host_argument_last
  StrCpy $2 $0 1
  StrCpy $0 $0 "" 1
  StrCmp $2 " " host_argument_end
  StrCpy $1 "$1$2"
  Goto host_argument_next
host_argument_end:
  StrCmp $1 "" host_argument_next
  Call HostArgument
  StrCpy $1 ""
  Goto host_argument_next
host_argument_last:
  StrCmp $1 "" +2
  Call HostArgument
  StrCmp $HostRemove 1 host_remove_option_check host_options_done
host_remove_option_check:
  StrCmp $HostLaunch 0 host_options_done host_stage_failed
host_options_done:
  ; First executable action: independently pre-staged source helper, before any
  ; old uninstall, prerequisite, NSIS target write, kill or presentation write.
  ; It commits intent before copying the complete source assets natively, with
  ; protected security descriptors at creation. NSIS never copies target files.
  StrCmp $HostRemove 1 host_prepare_remove
!if ${HOST_SEED_VERSION} == 2
  ; Presentation switches /S and /P do not select the business upgrade protocol.
  !insertmacro HostCall prepare-local-install
!else
  !insertmacro HostCall prepare-install
!endif
  Return
host_prepare_remove:
  !insertmacro HostCall prepare-remove
  Return
host_stage_failed:
  SetErrorLevel 1
  MessageBox MB_OK|MB_ICONSTOP "$(HostFailure)" /SD IDOK
  Abort
FunctionEnd

Function HostArgument
  StrCmp $1 "/S" host_argument_silent
  StrCmp $1 "/P" host_argument_passive
  StrCmp $1 "/R" host_argument_run
  StrCmp $1 "/HOST_REPAIR" host_argument_repair
  StrCmp $1 "/HOST_UNINSTALL" host_argument_remove
  StrCmp $1 "/L=1033" host_language_en
  StrCmp $1 "/L=2052" host_language_zh
  StrCmp $1 "/L=1041" host_language_ja
  ; /D, /ARGS, /UPDATE, /NCRC, deep links and arbitrary destinations are unsupported.
  Goto host_bad_argument
host_language_en:
  IntOp $3 $HostOptionMask & 16
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 16
  StrCpy $LANGUAGE 1033
  Return
host_language_zh:
  IntOp $3 $HostOptionMask & 16
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 16
  StrCpy $LANGUAGE 2052
  Return
host_language_ja:
  IntOp $3 $HostOptionMask & 16
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 16
  StrCpy $LANGUAGE 1041
  Return
host_argument_passive:
  IntOp $3 $HostOptionMask & 1
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 1
  SetSilent silent
  Return
host_argument_silent:
  IntOp $3 $HostOptionMask & 1
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 1
  Return
host_argument_run:
  IntOp $3 $HostOptionMask & 2
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 2
  StrCpy $HostLaunch 1
  Return
host_argument_remove:
  IntOp $3 $HostOptionMask & 4
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 4
  StrCpy $HostRemove 1
  Return
host_argument_repair:
  IntOp $3 $HostOptionMask & 4
  StrCmp $3 0 +2
  Goto host_bad_argument
  IntOp $HostOptionMask $HostOptionMask | 4
host_argument_ok:
  Return
host_bad_argument:
  SetErrorLevel 1
  Abort
FunctionEnd

Section "Protected host"
  StrCmp $HostRemove 1 host_remove_target
  !insertmacro HostCall verify-files
  !insertmacro HostCall start-service
  ClearErrors
  CreateShortcut "$SMPROGRAMS\${HOST_DISPLAY_NAME}.lnk" "$HostCode\pinvou-upgrade-launcher.exe"
  CreateShortcut "$DESKTOP\${HOST_DISPLAY_NAME}.lnk" "$HostCode\pinvou-upgrade-launcher.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}" "DisplayName" "${HOST_DISPLAY_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}" "DisplayVersion" "${HOST_APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}" "UninstallString" '$\"$HostSource\installer.exe$\" /HOST_UNINSTALL'
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}" "NoRepair" 1
  IfErrors host_stage_failed
  Goto host_target_complete
host_remove_target:
  !insertmacro HostCall remove-target
  ClearErrors
  ; The native source owner has sealed the tombstone before presentation removal.
  Delete "$SMPROGRAMS\${HOST_DISPLAY_NAME}.lnk"
  Delete "$DESKTOP\${HOST_DISPLAY_NAME}.lnk"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${HOST_SERVICE_NAME}"
  IfErrors host_stage_failed
host_target_complete:
  Return
host_stage_failed:
  SetErrorLevel 1
  MessageBox MB_OK|MB_ICONSTOP "$(HostFailure)" /SD IDOK
  Abort
SectionEnd

Function .onInstSuccess
  StrCmp $HostRemove 1 host_optional_done
  StrCmp $HostLaunch 1 host_optional_launch
  IfSilent host_optional_done
  MessageBox MB_YESNO|MB_ICONQUESTION "$(HostRun)" /SD IDNO IDYES host_optional_launch
  Goto host_optional_done
host_optional_launch:
  ClearErrors
  ExecWait '$\"$HostSource\pinvou-upgrade-helper.exe$\" --installer-stage launch-desktop' $HostStageExit
  IfErrors host_optional_unknown
  StrCmp $HostStageExit 0 host_optional_done
host_optional_unknown:
  DetailPrint "$(HostOptional)"
  MessageBox MB_OK|MB_ICONINFORMATION "$(HostOptional)" /SD IDOK
host_optional_done:
FunctionEnd
