; The Extra tools page: the tools Settings → Tools installs (src-tauri/src/tools.rs), the meaning model
; and instant search, each with what Glint uses it for and, under the list, what it is. The ones picked
; install once Glint is in place, by Glint itself (`Glint.exe tools install <id>`, as Settings → Tools
; installs them: with winget, or from the tool's own release, checked against its pinned SHA-256), so
; the installer keeps no downloads of its own. tools.rs's tests check this list against TOOLS.
; installer.nsi inserts the page after the folder page, and NSIS_HOOK_POSTINSTALL (hooks.nsh) installs
; what was picked: on the page, or with /TOOLS=jaq,uv on the command line (silent installs too).

; Every row, in order: its index, its id (tools.rs's), name, group, kind (service, model, winget or
; release), program and winget package folder (to tell whether it's on this PC already), what Glint uses
; it for (the list's second column) and what it is.
!macro GLINT_TOOLS ROW
  !insertmacro ${ROW} 0 instant-search "Instant search" 1 service "" "" \
    "Finds any file on every drive as you type" \
    "Glint's own index of your NTFS drives, like Everything's: any file on any drive found in milliseconds, best matches first. It runs as a Windows service, so Windows asks for administrator approval."
  !insertmacro ${ROW} 1 meaning "Meaning model" 1 model "" "" \
    "Searching text by what it means" \
    "potion-base-8M, a small model (30 MB, MIT) that runs on this PC: Text's Meaning search then ranks passages by what they're about, not only by the words in them."
  !insertmacro ${ROW} 2 czkawka "Czkawka" 2 winget windows_czkawka_cli qarmin.czkawka.cli \
    "Look-alike pictures, videos and music" \
    "Czkawka finds similar and broken files. Clean up uses it for look-alike pictures, videos and music, broken files and files with the wrong extension."
  !insertmacro ${ROW} 3 ffmpeg "FFmpeg" 2 winget ffmpeg gyan.ffmpeg \
    "Reading videos, for look-alike videos" \
    "FFmpeg reads almost any video and sound. Clean up uses it to compare videos. It's a large download, and an FFmpeg already on this PC is used instead."
  !insertmacro ${ROW} 4 magika "Magika" 2 winget magika google.magika \
    "A file's real type, in the File X-Ray" \
    "Google's Magika tells a file's type from its content. The File X-Ray uses it to catch a file that isn't what its name says: a program named photo.jpg, say."
  !insertmacro ${ROW} 5 yara-x "YARA-X" 2 winget yr virustotal.yara-x \
    "Checking files with YARA rules" \
    "VirusTotal's YARA-X matches files against YARA rules. Check uses it with the rules in a folder you choose in Settings."
  !insertmacro ${ROW} 6 kingfisher "Kingfisher" 2 release kingfisher kingfisher \
    "Finding passwords and keys left in files" \
    "MongoDB's Kingfisher finds passwords, keys and tokens left in files, so you can take them out before sharing. Nothing is sent anywhere."
  !insertmacro ${ROW} 7 rimage "rimage" 2 release rimage rimage \
    "Converting pictures to JPEG, PNG, WebP or AVIF" \
    "rimage converts and shrinks pictures. Convert pictures… uses it to make JPEG, PNG, WebP or AVIF files, smaller if you ask."
  !insertmacro ${ROW} 8 difftastic "Difftastic" 3 winget difft wilfred.difftastic \
    "Comparing code by its structure" \
    "Difftastic compares code by its syntax. Compare two files then shows what changed in the code, not which lines moved."
  !insertmacro ${ROW} 9 ast-grep "ast-grep" 3 winget ast-grep ast-grep.ast-grep \
    "Code patterns in Text search" \
    "ast-grep searches code by its syntax. Text then finds (and rewrites) code by its structure, such as console.log($$MSG) or $$X.unwrap()."
  !insertmacro ${ROW} 10 jaq "jaq" 3 release jaq 01mf02.jaq \
    "jq filters in JSON, YAML and TOML previews" \
    "jaq is jq, rewritten in Rust. The preview of a JSON, YAML or TOML file can then filter it with jq."
  !insertmacro ${ROW} 11 onefetch "onefetch" 3 release onefetch o2sh.onefetch \
    "A Git repository's card in its preview" \
    "onefetch sums up a Git repository. Its preview gets a card with its languages, who works on it, its last change and its license."
  !insertmacro ${ROW} 12 tokei "tokei" 3 winget tokei xampprocky.tokei \
    "Lines of code on a repository's card" \
    "tokei counts lines of code. A Git repository's card then shows its lines of code by language."
  !insertmacro ${ROW} 13 just "just" 3 winget just casey.just \
    "A justfile's recipes in the Run menu" \
    "just runs a project's commands. The Run menu then lists the recipes in a project's justfile."
  !insertmacro ${ROW} 14 uv "uv" 4 winget uv astral-sh.uv \
    "Running Python scripts and projects" \
    "uv is Astral's fast Python manager. The Run menu runs Python projects' scripts with it, and Scripts runs Python scripts."
  !insertmacro ${ROW} 15 bun "Bun" 4 winget bun oven-sh.bun \
    "Running TypeScript and JavaScript scripts" \
    "Bun is a fast JavaScript runtime. Scripts runs TypeScript and JavaScript scripts with it."
  !insertmacro ${ROW} 16 nushell "Nushell" 4 winget nu nushell.nushell \
    "A shell for the terminal, and Nu scripts" \
    "Nushell is a shell for structured data. The terminal under the files can start it, and Scripts runs Nushell scripts."
  !insertmacro ${ROW} 17 zoxide "zoxide" 4 winget zoxide ajeetdsouza.zoxide \
    "Folder history shared with your terminals" \
    "zoxide remembers the folders you go to. The folders you cd to in your terminals rank in Ctrl+K, and z finds the ones you open in Glint."
  !insertmacro ${ROW} 18 sendme "sendme" 5 winget sendme n0-computer.sendme \
    "Sending files to another PC" \
    "sendme sends files and folders from PC to PC, encrypted end to end, with a ticket instead of an account."
  !insertmacro ${ROW} 19 dufs "dufs" 5 winget dufs sigoden.dufs \
    "Sharing a folder with a phone" \
    "dufs serves a folder on this network. Share with a phone then gives a link that the phone's browser opens."
!macroend

!macro GLINT_TOOL_GROUPS GROUP
  !insertmacro ${GROUP} 1 "Glint's own"
  !insertmacro ${GROUP} 2 "Clean up, pictures and safety"
  !insertmacro ${GROUP} 3 "Code and projects"
  !insertmacro ${GROUP} 4 "Scripts and terminal"
  !insertmacro ${GROUP} 5 "Sharing"
!macroend

; The list view's messages and flags NSIS doesn't define, as numbers (System::Call takes no expressions).
!define /ifndef LVM_INSERTGROUP 0x1091
!define /ifndef LVM_ENABLEGROUPVIEW 0x109D
!define /ifndef LVN_ITEMCHANGED -101
!define /ifndef LVIS_SELECTED 2
!define /ifndef NM_CUSTOMDRAW -12
!define /ifndef CDDS_PREPAINT 1
!define /ifndef CDDS_ITEMPREPAINT 65537
!define /ifndef CDRF_NEWFONT 2
!define /ifndef CDRF_NOTIFYITEMDRAW 32
!define GLINT_GREY 7171437 ; RGB(109, 109, 109), as a COLORREF
!define GLINT_LIST_STYLE 0x5001400D ; WS_CHILD | WS_VISIBLE | WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOCOLUMNHEADER
!define GLINT_LIST_EXSTYLE 0x14024 ; LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER
!define GLINT_ITEM_MASK 257 ; LVIF_TEXT | LVIF_GROUPID
!define GLINT_GROUP_MASK 17 ; LVGF_HEADER | LVGF_GROUPID
!define GLINT_STATE_MASK 61440 ; LVIS_STATEIMAGEMASK: the check box's state
!define GLINT_UNCHECKED 4096
!define GLINT_CHECKED 8192

Var GlintList
Var GlintAbout
Var GlintCount
; An LVITEM for setting check boxes, allocated once, so handlers need no registers.
Var GlintItem
; The ids picked, each between spaces (" jaq uv "), and the rows that have no check box: on this PC
; already, or needing winget where it's missing (" 3 7 ").
Var GlintPicks
Var GlintInstalled
Var GlintNoWinget
Var GlintWinget
; The list's notifications and loops use these rather than registers: a check box set from NSIS sends
; one while the code that set it is still running.
Var GlintQuiet
Var GlintCode
Var GlintData
Var GlintRow
Var GlintNew
Var GlintOld
Var GlintChanged
Var GlintI
Var GlintN
Var GlintRows
Var GlintState
Var GlintSet
Var GlintText
Var GlintFound
; While installing: how many were picked, which one is installing, and what didn't install.
Var GlintTotal
Var GlintStep
Var GlintFailed

!macro GLINT_SET_CHECK ROW STATE
  System::Call '*$GlintItem(i 0, i 0, i 0, i ${STATE}, i ${GLINT_STATE_MASK})'
  SendMessage $GlintList ${LVM_SETITEMSTATE} ${ROW} $GlintItem
!macroend

!macro GLINT_ADD_GROUP ID NAME
  StrCpy $1 "${NAME}"
  System::Call '*(i 40, i ${GLINT_GROUP_MASK}, w r1, i 0, p 0, i 0, i ${ID}, i 0, i 0, i 0) p .r2'
  SendMessage $GlintList ${LVM_INSERTGROUP} -1 $2
  System::Free $2
!macroend

!macro GLINT_ADD_ROW INDEX ID NAME GROUP KIND EXE PACKAGE USE ABOUT
  StrCpy $1 "${NAME}"
  System::Call '*(i ${GLINT_ITEM_MASK}, i ${INDEX}, i 0, i 0, i 0, w r1, i 0, i 0, p 0, i 0, i ${GROUP}, i 0, p 0, p 0, i 0) p .r2'
  SendMessage $GlintList ${LVM_INSERTITEMW} 0 $2
  System::Free $2
  StrCpy $5 "${KIND}"
  StrCpy $6 "${ID}"
  StrCpy $7 "${EXE}"
  StrCpy $8 "${PACKAGE}"
  Call GlintOnThisPC
  StrCpy $1 "${USE}"
  StrCpy $3 0
  ${If} $9 = 1
    StrCpy $1 "Installed"
    StrCpy $GlintInstalled "$GlintInstalled${INDEX} "
    StrCpy $3 1
  !if "${KIND}" == "winget"
  ${ElseIf} $GlintWinget == ""
    StrCpy $1 "Needs winget"
    StrCpy $GlintNoWinget "$GlintNoWinget${INDEX} "
    StrCpy $3 1
  !endif
  ${EndIf}
  System::Call '*(i 0, i 0, i 1, i 0, i 0, w r1, i 0, i 0, p 0, i 0, i 0, i 0, p 0, p 0, i 0) p .r2'
  SendMessage $GlintList ${LVM_SETITEMTEXTW} ${INDEX} $2
  System::Free $2
  ; No check box on one that's installed or can't be; ticked again if it was picked (Back, then Next).
  ${If} $3 = 1
    !insertmacro GLINT_SET_CHECK ${INDEX} 0
  ${Else}
    ${StrLoc} $0 $GlintPicks " ${ID} " ">"
    ${If} $0 != ""
      !insertmacro GLINT_SET_CHECK ${INDEX} ${GLINT_CHECKED}
    ${EndIf}
  ${EndIf}
!macroend

!macro GLINT_ABOUT_ROW INDEX ID NAME GROUP KIND EXE PACKAGE USE ABOUT
  ${If} $GlintRow = ${INDEX}
    StrCpy $GlintText "${ABOUT}"
  ${EndIf}
!macroend

!macro GLINT_PICKED_ROW INDEX ID NAME GROUP KIND EXE PACKAGE USE ABOUT
  SendMessage $GlintList ${LVM_GETITEMSTATE} ${INDEX} ${GLINT_STATE_MASK} $GlintState
  ${IfThen} $GlintState = ${GLINT_CHECKED} ${|} StrCpy $GlintPicks "$GlintPicks${ID} " ${|}
!macroend

!macro GLINT_COUNT_PICK INDEX ID NAME GROUP KIND EXE PACKAGE USE ABOUT
  ${StrLoc} $0 $GlintPicks " ${ID} " ">"
  ${IfThen} $0 != "" ${|} IntOp $GlintTotal $GlintTotal + 1 ${|}
!macroend

; Installs one that was picked: instant search with the service's own installer (Windows asks for
; approval), the rest with Glint, which shows how far it's got on the status line as it goes.
!macro GLINT_INSTALL_PICK INDEX ID NAME GROUP KIND EXE PACKAGE USE ABOUT
  ${StrLoc} $0 $GlintPicks " ${ID} " ">"
  ${If} $0 != ""
    IntOp $GlintStep $GlintStep + 1
    !if "${KIND}" == "service"
      DetailPrint "Turning on instant search ($GlintStep of $GlintTotal): Windows asks for approval…"
      ExecShellWait "runas" "$INSTDIR\shadui-index.exe" "install" SW_HIDE
      ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\ShadUIRipgrepIndex" "ImagePath"
      ${If} $0 == ""
        DetailPrint "Instant search isn't on: Settings → Instant search in Glint turns it on."
        StrCpy $GlintFailed "$GlintFailed, ${NAME}"
      ${EndIf}
    !else
      DetailPrint "Installing ${NAME} ($GlintStep of $GlintTotal)…"
      nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" tools install ${ID} --installer $HWNDPARENT --data "$LOCALAPPDATA\${BUNDLEID}"'
      Pop $0
      ${IfThen} $0 != 0 ${|} StrCpy $GlintFailed "$GlintFailed, ${NAME}" ${|}
    !endif
  ${EndIf}
!macroend

; The page itself, after the folder page.
!macro GLINT_PAGE_TOOLS
  Page custom GlintToolsPage GlintToolsLeave

  ; Whether a row's tool is on this PC already ($9 = 1): kind, id, program and package in $5 to $8. As
  ; Glint looks (proc.rs): its own downloads, then PATH, winget's, Scoop's and Cargo's folders, and
  ; winget's package folders.
  Function GlintOnThisPC
    StrCpy $9 0
    ${If} $5 == service
      ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\ShadUIRipgrepIndex" "ImagePath"
      ${IfThen} $0 != "" ${|} StrCpy $9 1 ${|}
    ${ElseIf} $5 == model
      ${IfThen} ${FileExists} "$LOCALAPPDATA\${BUNDLEID}\models\potion-base-8M\model.safetensors" ${|} StrCpy $9 1 ${|}
    ${Else}
      ClearErrors
      SearchPath $0 "$7.exe"
      ${IfNot} ${Errors}
      ${OrIf} ${FileExists} "$LOCALAPPDATA\${BUNDLEID}\tools\$6\$7.exe"
      ${OrIf} ${FileExists} "$LOCALAPPDATA\Microsoft\WinGet\Links\$7.exe"
      ${OrIf} ${FileExists} "$PROFILE\scoop\shims\$7.exe"
      ${OrIf} ${FileExists} "$PROFILE\.cargo\bin\$7.exe"
        StrCpy $9 1
      ${Else}
        FindFirst $0 $1 "$LOCALAPPDATA\Microsoft\WinGet\Packages\*$8*"
        FindClose $0
        ${IfThen} $1 != "" ${|} StrCpy $9 1 ${|}
      ${EndIf}
    ${EndIf}
  FunctionEnd

  Function GlintToolsPage
    ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
    !insertmacro MUI_HEADER_TEXT "Extra tools" "Glint comes with ripgrep, fd and pdu. These free tools add more features; Settings → Tools adds or removes them later."
    nsDialogs::Create 1018
    Pop $0
    ${IfThen} $0 == error ${|} Abort ${|}
    nsDialogs::SetRTL $(^RTL)
    ; Installing starts from here (the Start menu page after it is never shown), so the button says so.
    GetDlgItem $0 $HWNDPARENT 1
    SendMessage $0 ${WM_SETTEXT} 0 "STR:$(^InstallBtn)"

    nsDialogs::CreateControl SysListView32 ${GLINT_LIST_STYLE} ${WS_EX_CLIENTEDGE} 0 0 100% 96u ""
    Pop $GlintList
    SendMessage $GlintList ${LVM_SETEXTENDEDLISTVIEWSTYLE} ${GLINT_LIST_EXSTYLE} ${GLINT_LIST_EXSTYLE}
    System::Call 'uxtheme::SetWindowTheme(p $GlintList, w "Explorer", p 0)'
    System::Call '*(i, i, i, i, i, p, i, i, p, i, i, i, p, p, i) p .r0'
    StrCpy $GlintItem $0

    ; Two columns: the tool, a third of the width, and what Glint uses it for, the rest.
    System::Call '*(i, i, i, i) p .r0'
    System::Call 'user32::GetClientRect(p $GlintList, p r0)'
    System::Call '*$0(i, i, i .r1, i)'
    System::Free $0
    IntOp $1 $1 * 30
    IntOp $1 $1 / 100
    System::Call '*(i 6, i 0, i r1, w "Tool", i 0, i 0) p .r0'
    SendMessage $GlintList ${LVM_INSERTCOLUMNW} 0 $0
    System::Free $0
    System::Call '*(i 14, i 0, i 100, w "Used in Glint for", i 0, i 1) p .r0'
    SendMessage $GlintList ${LVM_INSERTCOLUMNW} 1 $0
    System::Free $0

    !insertmacro GLINT_TOOL_GROUPS GLINT_ADD_GROUP
    SendMessage $GlintList ${LVM_ENABLEGROUPVIEW} 1 0
    ClearErrors
    SearchPath $GlintWinget winget.exe
    StrCpy $GlintInstalled " "
    StrCpy $GlintNoWinget " "
    ${IfThen} $GlintPicks == "" ${|} StrCpy $GlintPicks " " ${|}
    !insertmacro GLINT_TOOLS GLINT_ADD_ROW
    SendMessage $GlintList ${LVM_GETITEMCOUNT} 0 0 $GlintRows
    SendMessage $GlintList ${LVM_SETCOLUMNWIDTH} 1 ${LVSCW_AUTOSIZE_USEHEADER}

    ${NSD_CreateLabel} 0 99u 100% 28u "Tools install for your account, with winget or from their own releases (checked against their published checksums). Select one to see what it is and what Glint uses it for."
    Pop $GlintAbout
    ${NSD_CreateLink} 0 129u 44u 10u "Select all"
    Pop $0
    ${NSD_OnClick} $0 GlintToolsAll
    ${NSD_CreateLink} 48u 129u 40u 10u "Clear all"
    Pop $0
    ${NSD_OnClick} $0 GlintToolsNone
    ${NSD_CreateLabel} 50% 129u 50% 10u ""
    Pop $GlintCount
    ${NSD_AddStyle} $GlintCount ${SS_RIGHT}
    Call GlintToolsCount

    StrCpy $GlintQuiet 0
    ${NSD_OnNotify} $GlintList GlintToolsNotify
    nsDialogs::Show
    System::Free $GlintItem
  FunctionEnd

  Function GlintToolsLeave
    StrCpy $GlintPicks " "
    !insertmacro GLINT_TOOLS GLINT_PICKED_ROW
  FunctionEnd

  ; A check box ticked or cleared (the count), a row selected (what it is, under the list), or a row
  ; about to be drawn (grey when it has no check box).
  Function GlintToolsNotify
    Pop $GlintData ; the list
    Pop $GlintCode
    Pop $GlintData ; its NMLISTVIEW: the row, its new and old state, and what changed
    ${If} $GlintCode = ${LVN_ITEMCHANGED}
    ${AndIf} $GlintQuiet = 0
      System::Call '*$GlintData(p, p, i, i .s)'
      Pop $GlintRow
      System::Call '*$GlintData(p, p, i, i, i, i .s)'
      Pop $GlintNew
      System::Call '*$GlintData(p, p, i, i, i, i, i .s)'
      Pop $GlintOld
      System::Call '*$GlintData(p, p, i, i, i, i, i, i .s)'
      Pop $GlintChanged
      IntOp $GlintChanged $GlintChanged & ${LVIF_STATE}
      ${If} $GlintChanged <> 0
        IntOp $GlintI $GlintNew ^ $GlintOld
        IntOp $GlintI $GlintI & ${GLINT_STATE_MASK}
        ${If} $GlintI <> 0
          IntOp $GlintI $GlintOld & ${GLINT_STATE_MASK}
          ${If} $GlintI = 0
            ; A row without a check box keeps none (Space would give it one).
            StrCpy $GlintQuiet 1
            !insertmacro GLINT_SET_CHECK $GlintRow 0
            StrCpy $GlintQuiet 0
          ${Else}
            Call GlintToolsCount
          ${EndIf}
        ${EndIf}
        IntOp $GlintI $GlintNew & ${LVIS_SELECTED}
        IntOp $GlintN $GlintOld & ${LVIS_SELECTED}
        ${If} $GlintI <> 0
        ${AndIf} $GlintN = 0
          Call GlintToolsAbout
        ${EndIf}
      ${EndIf}
    ${ElseIf} $GlintCode = ${NM_CUSTOMDRAW}
      ; Rows without a check box in grey: asked for each row (CDRF_NOTIFYITEMDRAW), then its text's colour.
      System::Call '*$GlintData(p, p, i, i .s)'
      Pop $GlintI
      ${If} $GlintI = ${CDDS_PREPAINT}
        ${NSD_Return} ${CDRF_NOTIFYITEMDRAW}
      ${ElseIf} $GlintI = ${CDDS_ITEMPREPAINT}
        System::Call '*$GlintData(p, p, i, i, p, i, i, i, i, p .s)'
        Pop $GlintI
        SendMessage $GlintList ${LVM_GETITEMSTATE} $GlintI ${GLINT_STATE_MASK} $GlintState
        ${If} $GlintState = 0
          System::Call '*$GlintData(p, p, i, i, p, i, i, i, i, p, i, p, i ${GLINT_GREY})'
          ${NSD_Return} ${CDRF_NEWFONT}
        ${EndIf}
      ${EndIf}
    ${EndIf}
  FunctionEnd

  Function GlintToolsAbout
    StrCpy $GlintText ""
    !insertmacro GLINT_TOOLS GLINT_ABOUT_ROW
    ${StrLoc} $GlintFound $GlintInstalled " $GlintRow " ">"
    ${If} $GlintFound != ""
      StrCpy $GlintText "Already on this PC. $GlintText"
    ${Else}
      ${StrLoc} $GlintFound $GlintNoWinget " $GlintRow " ">"
      ${IfThen} $GlintFound != "" ${|} StrCpy $GlintText "Needs winget, which App Installer from the Microsoft Store brings. $GlintText" ${|}
    ${EndIf}
    ${NSD_SetText} $GlintAbout $GlintText
  FunctionEnd

  Function GlintToolsCount
    StrCpy $GlintN 0
    StrCpy $GlintI 0
    ${DoWhile} $GlintI < $GlintRows
      SendMessage $GlintList ${LVM_GETITEMSTATE} $GlintI ${GLINT_STATE_MASK} $GlintState
      ${IfThen} $GlintState = ${GLINT_CHECKED} ${|} IntOp $GlintN $GlintN + 1 ${|}
      IntOp $GlintI $GlintI + 1
    ${Loop}
    ${If} $GlintN = 0
      ${NSD_SetText} $GlintCount "None selected"
    ${Else}
      ${NSD_SetText} $GlintCount "$GlintN selected"
    ${EndIf}
  FunctionEnd

  Function GlintToolsAll
    Pop $GlintData
    StrCpy $GlintSet ${GLINT_CHECKED}
    Call GlintToolsSetAll
  FunctionEnd

  Function GlintToolsNone
    Pop $GlintData
    StrCpy $GlintSet ${GLINT_UNCHECKED}
    Call GlintToolsSetAll
  FunctionEnd

  ; Every check box ticked or cleared ($GlintSet), then counted once.
  Function GlintToolsSetAll
    StrCpy $GlintQuiet 1
    StrCpy $GlintI 0
    ${DoWhile} $GlintI < $GlintRows
      SendMessage $GlintList ${LVM_GETITEMSTATE} $GlintI ${GLINT_STATE_MASK} $GlintState
      ${If} $GlintState <> 0
        !insertmacro GLINT_SET_CHECK $GlintI $GlintSet
      ${EndIf}
      IntOp $GlintI $GlintI + 1
    ${Loop}
    StrCpy $GlintQuiet 0
    Call GlintToolsCount
  FunctionEnd
!macroend

; Installs what was picked, after Glint itself (NSIS_HOOK_POSTINSTALL): on the page, or on the command
; line with /TOOLS=jaq,uv and, as before, /INSTANTSEARCH. The finish page then says what didn't install.
!macro GLINT_INSTALL_TOOLS
  ClearErrors
  ${GetOptions} $CMDLINE "/TOOLS=" $0
  ${IfNot} ${Errors}
    ${WordReplace} $0 "," " " "+" $0
    StrCpy $GlintPicks "$GlintPicks $0 "
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/INSTANTSEARCH" $0
  ${IfNot} ${Errors}
    StrCpy $GlintPicks "$GlintPicks instant-search "
  ${EndIf}
  StrCpy $GlintTotal 0
  !insertmacro GLINT_TOOLS GLINT_COUNT_PICK
  ${If} $GlintTotal > 0
    StrCpy $GlintStep 0
    StrCpy $GlintFailed ""
    !insertmacro GLINT_TOOLS GLINT_INSTALL_PICK
    ${If} $GlintFailed == ""
      DetailPrint "Glint and the tools you picked are installed."
      StrCpy $GlintFinishText "${PRODUCTNAME} ${VERSION} is installed, with the tools you picked.$\r$\n$\r$\nClick Finish to close Setup."
    ${Else}
      StrCpy $GlintFailed $GlintFailed "" 2
      DetailPrint "Not installed: $GlintFailed. Settings → Tools in Glint can try again."
      StrCpy $GlintFinishText "${PRODUCTNAME} ${VERSION} is installed, but these aren't: $GlintFailed. Settings → Tools in Glint can try them again.$\r$\n$\r$\nClick Finish to close Setup."
    ${EndIf}
  ${EndIf}
!macroend
