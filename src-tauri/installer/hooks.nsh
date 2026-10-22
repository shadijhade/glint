; Instant search is a Windows service (shadui-index.exe, copied to Program Files when turned on).
; It is off unless the user turns it on in the app's settings, or the installer runs with
; /INSTANTSEARCH (for silent installs). Either way Windows asks for administrator approval.

; Until 0.3.1 the app was called shadui-ripgrep, and Windows keeps an installed app under its name:
; its folder, shortcuts and Installed apps entry. Installing Glint removes those (settings, tabs and
; the instant search service carry over, as they don't go by the name) and makes the same shortcuts
; for Glint, also in update mode, which otherwise makes none. Running the old uninstaller instead
; would remove the instant search service.
!define OLDNAME "shadui-ripgrep"
Var OldInstallDir
Var OldStartMenuShortcut
Var OldDesktopShortcut

!macro NSIS_HOOK_PREINSTALL
  StrCpy $OldStartMenuShortcut 0
  StrCpy $OldDesktopShortcut 0
  ReadRegStr $OldInstallDir HKCU "Software\${MANUFACTURER}\${OLDNAME}" ""
  ${If} $OldInstallDir != ""
    ; Close it if it's open, asking first unless the install is passive (as an update is) or silent.
    !insertmacro RestartManager_StartSession $R0
    ${If} $R0 != ""
      !insertmacro RestartManager_RegisterFile $R0 "$OldInstallDir\${OLDNAME}.exe"
      ${If} $0 = 0
        System::Call 'RSTRTMGR::RmGetList(p R0, *i .r1, *i .r2, p 0, *i .r3) i .r0'
        ${If} $0 = ${ERROR_MORE_DATA}
          ${IfNot} ${Silent}
          ${AndIf} $PassiveMode <> 1
            MessageBox MB_OKCANCEL|MB_ICONINFORMATION "ShadUI Ripgrep is now Glint, which takes its place.$\n$\nClick OK to close ShadUI Ripgrep and go on." IDOK glint_close_old
            !insertmacro RestartManager_EndSession $R0
            Abort "ShadUI Ripgrep is still open."
          ${EndIf}
          glint_close_old:
          System::Call 'RSTRTMGR::RmShutdown(p R0, i ${RmForceShutdown}, p 0) i .r0'
        ${EndIf}
      ${EndIf}
      !insertmacro RestartManager_EndSession $R0
    ${EndIf}

    ; Only its own files, in case it was installed into a folder shared with other things.
    Delete "$OldInstallDir\${OLDNAME}.exe"
    ${IfNot} ${FileExists} "$OldInstallDir\${OLDNAME}.exe"
      Delete "$OldInstallDir\rg.exe"
      Delete "$OldInstallDir\fd.exe"
      Delete "$OldInstallDir\dust.exe"
      Delete "$OldInstallDir\shadui-index.exe"
      Delete "$OldInstallDir\LICENSE.txt"
      Delete "$OldInstallDir\THIRD_PARTY_LICENSES.md"
      Delete "$OldInstallDir\uninstall.exe"
      RMDir "$OldInstallDir"

      !insertmacro IsShortcutTarget "$SMPROGRAMS\${OLDNAME}.lnk" "$OldInstallDir\${OLDNAME}.exe"
      Pop $0
      ${If} $0 = 1
        !insertmacro UnpinShortcut "$SMPROGRAMS\${OLDNAME}.lnk"
        Delete "$SMPROGRAMS\${OLDNAME}.lnk"
        StrCpy $OldStartMenuShortcut 1
      ${EndIf}
      !insertmacro IsShortcutTarget "$DESKTOP\${OLDNAME}.lnk" "$OldInstallDir\${OLDNAME}.exe"
      Pop $0
      ${If} $0 = 1
        !insertmacro UnpinShortcut "$DESKTOP\${OLDNAME}.lnk"
        Delete "$DESKTOP\${OLDNAME}.lnk"
        StrCpy $OldDesktopShortcut 1
      ${EndIf}

      DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${OLDNAME}"
      DeleteRegKey HKCU "Software\${MANUFACTURER}\${OLDNAME}"
    ${EndIf}
    ; In case Glint goes into the old folder, which RMDir just removed.
    SetOutPath $INSTDIR
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $NoShortcutMode <> 1
    ${If} $OldStartMenuShortcut = 1
    ${AndIfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $OldDesktopShortcut = 1
    ${AndIfNot} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
      CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ClearErrors
  ${GetOptions} $CMDLINE "/INSTANTSEARCH" $0
  ${IfNot} ${Errors}
    ExecShellWait "runas" "$INSTDIR\shadui-index.exe" "install" SW_HIDE
  ${EndIf}
!macroend

; File Explorer's "Open in Glint" entries and opening folders in Glint by default, which the app's
; settings add under HKCU\Software\Classes (src-tauri/src/verbs.rs). Uninstalling removes them where
; they run this copy and puts back what folders opened with, and the keys above them go once empty.
; The Scoop manifest's uninstaller does the same for Scoop's copies (scripts/package-manifests.mjs).
!macro GLINT_REMOVE_VERB PLACE
  ReadRegStr $R0 HKCU "Software\Classes\${PLACE}\${PRODUCTNAME}\command" ""
  StrCpy $R0 $R0 $R2
  ${If} $R0 == $R1
    DeleteRegKey HKCU "Software\Classes\${PLACE}\${PRODUCTNAME}"
  ${EndIf}
!macroend

!macro GLINT_REMOVE_EXPLORER_VERBS
  ; Commands start with this copy's exe in quotes.
  StrCpy $R1 '"$INSTDIR\${MAINBINARYNAME}.exe"'
  StrLen $R2 $R1
  ReadRegStr $R0 HKCU "Software\Classes\Directory\shell\${PRODUCTNAME}\command" ""
  StrCpy $R0 $R0 $R2
  ${If} $R0 == $R1
    ReadRegStr $R0 HKCU "Software\Classes\Directory\shell" ""
    ${If} $R0 == "${PRODUCTNAME}"
      ClearErrors
      ReadRegStr $R0 HKCU "Software\Classes\Directory\shell\${PRODUCTNAME}" "PreviousDefault"
      ${If} ${Errors}
        DeleteRegValue HKCU "Software\Classes\Directory\shell" ""
      ${Else}
        WriteRegStr HKCU "Software\Classes\Directory\shell" "" $R0
      ${EndIf}
    ${EndIf}
  ${EndIf}
  !insertmacro GLINT_REMOVE_VERB "Directory\shell"
  !insertmacro GLINT_REMOVE_VERB "Directory\Background\shell"
  !insertmacro GLINT_REMOVE_VERB "Drive\shell"
  DeleteRegKey /ifempty HKCU "Software\Classes\Directory\Background\shell"
  DeleteRegKey /ifempty HKCU "Software\Classes\Directory\Background"
  DeleteRegKey /ifempty HKCU "Software\Classes\Directory\shell"
  DeleteRegKey /ifempty HKCU "Software\Classes\Directory"
  DeleteRegKey /ifempty HKCU "Software\Classes\Drive\shell"
  DeleteRegKey /ifempty HKCU "Software\Classes\Drive"
  ; SHCNE_ASSOCCHANGED: File Explorer reads them again.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

; Uninstalling the app removes File Explorer's entries for it, starting with Windows, and the service
; and its index files (an update keeps them all).
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro GLINT_REMOVE_EXPLORER_VERBS
    ; Starting with Windows (Settings > Quick search), where it starts this copy.
    StrCpy $R1 '"$INSTDIR\${MAINBINARYNAME}.exe"'
    StrLen $R2 $R1
    ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    StrCpy $R0 $R0 $R2
    ${If} $R0 == $R1
      DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    ${EndIf}
    nsExec::ExecToStack 'sc.exe query ShadUIRipgrepIndex'
    Pop $0
    Pop $1
    ${If} $0 == 0
      ExecShellWait "runas" "$INSTDIR\shadui-index.exe" "uninstall" SW_HIDE
    ${EndIf}
  ${EndIf}
!macroend
