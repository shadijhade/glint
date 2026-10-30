; How Glint's installer looks and reads: Windows 11's font, Glint's own words on the welcome and finish
; pages, a question before Cancel quits, and artwork drawn for the screen's scaling (installer/art,
; made by scripts/installer-art.mjs), so it's sharp at every common scaling rather than stretched.
; installer.nsi inserts GLINT_INTERFACE before its pages.

!define GLINT_ART "${__FILEDIR__}\art"

; The screen's DPI, in $0.
!macro GLINT_DPI
  System::Call 'user32::GetDpiForWindow(p $HWNDPARENT) i .r0'
  ${If} $0 = 0
    System::Call 'user32::GetDC(p 0) p .r1'
    System::Call 'gdi32::GetDeviceCaps(p r1, i 90) i .r0' ; LOGPIXELSY
    System::Call 'user32::ReleaseDC(p 0, p r1)'
  ${EndIf}
!macroend

; A picture drawn for that DPI's scaling (125% to 300%, the nearest), in place of the 100% one MUI unpacked.
!macro GLINT_ART_FOR NAME FILE
  ${If} $0 >= 264
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-300.bmp"
  ${ElseIf} $0 >= 228
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-250.bmp"
  ${ElseIf} $0 >= 204
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-225.bmp"
  ${ElseIf} $0 >= 180
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-200.bmp"
  ${ElseIf} $0 >= 156
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-175.bmp"
  ${ElseIf} $0 >= 132
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-150.bmp"
  ${ElseIf} $0 >= 108
    File "/oname=$PLUGINSDIR\${FILE}" "${GLINT_ART}\${NAME}-125.bmp"
  ${EndIf}
!macroend

!macro GLINT_INTERFACE
  SetFont "Segoe UI" 9
  !define MUI_ABORTWARNING
  !define MUI_CUSTOMFUNCTION_GUIINIT GlintGUIInit
  !define MUI_CUSTOMFUNCTION_UNGUIINIT un.GlintGUIInit
  !define MUI_WELCOMEPAGE_TITLE "Welcome to ${PRODUCTNAME}"
  !define MUI_WELCOMEPAGE_TEXT "${PRODUCTNAME} is a file explorer for Windows that opens folders in milliseconds, shows the size of every folder, and finds any file on any drive in an instant.$\r$\n$\r$\nSetup installs ${PRODUCTNAME} ${VERSION} for your account, with no administrator rights needed. Next, you can pick extra tools that add features.$\r$\n$\r$\nClick Next to continue."
  !define MUI_FINISHPAGE_TITLE "${PRODUCTNAME} is ready"
  !define MUI_FINISHPAGE_TEXT "$GlintFinishText"
  !define MUI_FINISHPAGE_LINK "What's new in ${PRODUCTNAME} ${VERSION}"
  !define MUI_FINISHPAGE_LINK_LOCATION "https://github.com/shadijhade/glint/releases/tag/v${VERSION}"

  Var GlintFinishText

  Function GlintGUIInit
    StrCpy $GlintFinishText "${PRODUCTNAME} ${VERSION} is installed.$\r$\n$\r$\nClick Finish to close Setup."
    !insertmacro GLINT_DPI
    ${If} $0 >= 108
      !insertmacro GLINT_ART_FOR sidebar modern-wizard.bmp
      !insertmacro GLINT_ART_FOR header modern-header.bmp
      SetBrandingImage /IMGID=1046 /RESIZETOFIT "$PLUGINSDIR\modern-header.bmp"
    ${EndIf}
  FunctionEnd

  ; The uninstaller has only the header's.
  Function un.GlintGUIInit
    !insertmacro GLINT_DPI
    ${If} $0 >= 108
      !insertmacro GLINT_ART_FOR header modern-header.bmp
      SetBrandingImage /IMGID=1046 /RESIZETOFIT "$PLUGINSDIR\modern-header.bmp"
    ${EndIf}
  FunctionEnd
!macroend
