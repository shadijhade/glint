; Instant search is a Windows service (shadui-index.exe, copied to Program Files when turned on).
; It is off unless the user turns it on in the app's settings, or the installer runs with
; /INSTANTSEARCH (for silent installs). Either way Windows asks for administrator approval.

!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ${GetOptions} $CMDLINE "/INSTANTSEARCH" $0
  ${IfNot} ${Errors}
    ExecShellWait "runas" "$INSTDIR\shadui-index.exe" "install" SW_HIDE
  ${EndIf}
!macroend

; Uninstalling the app removes the service and its index files too (an update keeps them).
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    nsExec::ExecToStack 'sc.exe query ShadUIRipgrepIndex'
    Pop $0
    Pop $1
    ${If} $0 == 0
      ExecShellWait "runas" "$INSTDIR\shadui-index.exe" "uninstall" SW_HIDE
    ${EndIf}
  ${EndIf}
!macroend
