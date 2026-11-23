; Brand Windows display surfaces while retaining Keel's registry/install identity for upgrades.
; Tauri includes this before its product definitions: defer references to those into hook macros.
!include "LogicLib.nsh"

Caption "Oya Factory Setup"
UninstallCaption "Uninstall Oya Factory"
Var FactoryInstalled
Var FactoryExecutable
Var FactoryStartMenuFolder

; IsShortcutTarget clobbers $0-$3. Preserve the caller's registers and its stack on every path.
; Reinstall can recreate a legacy link beside our branded link. Remove only that owned duplicate.
!macro FactoryRenameOwnedShortcut SOURCE DESTINATION
  Push $0
  Push $1
  Push $2
  Push $3
  !insertmacro IsShortcutTarget "${SOURCE}" "$FactoryExecutable"
  Pop $0
  ${If} $0 == 1
    ${If} ${FileExists} "${DESTINATION}"
      !insertmacro IsShortcutTarget "${DESTINATION}" "$FactoryExecutable"
      Pop $0
      ${If} $0 == 1
        Delete "${SOURCE}"
      ${EndIf}
    ${Else}
      Rename "${SOURCE}" "${DESTINATION}"
    ${EndIf}
  ${EndIf}
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend

; Rename only links Tauri actually created; never add shortcuts after /NS or desktop opt-out.
Function FactoryRenameShortcuts
  !insertmacro FactoryRenameOwnedShortcut "$DESKTOP\Keel.lnk" "$DESKTOP\Oya Factory.lnk"
  !insertmacro FactoryRenameOwnedShortcut "$SMPROGRAMS\Keel.lnk" "$SMPROGRAMS\Oya Factory.lnk"
  ${If} $FactoryStartMenuFolder != ""
    !insertmacro FactoryRenameOwnedShortcut "$SMPROGRAMS\$FactoryStartMenuFolder\Keel.lnk" "$SMPROGRAMS\$FactoryStartMenuFolder\Oya Factory.lnk"
  ${EndIf}
FunctionEnd

; Only display metadata changes; Tauri owns upgrade detection, stored location and uninstall keys.
!macro NSIS_HOOK_POSTINSTALL
  StrCpy $FactoryExecutable "$INSTDIR\${MAINBINARYNAME}.exe"
  StrCpy $FactoryStartMenuFolder "$AppStartMenuFolder"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "Oya Factory"
  StrCpy $FactoryInstalled 1
  Call FactoryRenameShortcuts
!macroend

; Interactive finish pages create desktop shortcuts after POSTINSTALL; aborted wizards do nothing.
Function .onGUIEnd
  ${If} $FactoryInstalled == 1
    Call FactoryRenameShortcuts
  ${EndIf}
FunctionEnd

; A branded shortcut is removable only when its stored target is this installation's executable.
!macro FactoryRemoveOwnedShortcut LINK
  Push $0
  Push $1
  Push $2
  Push $3
  !insertmacro IsShortcutTarget "${LINK}" "$FactoryExecutable"
  Pop $0
  ${If} $0 == 1
    Delete "${LINK}"
  ${EndIf}
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend

; The executable may already be gone; IsShortcutTarget reads the shortcut's stored target.
Function un.FactoryRemoveShortcuts
  !insertmacro FactoryRemoveOwnedShortcut "$DESKTOP\Oya Factory.lnk"
  !insertmacro FactoryRemoveOwnedShortcut "$SMPROGRAMS\Oya Factory.lnk"
  ${If} $FactoryStartMenuFolder != ""
    !insertmacro FactoryRemoveOwnedShortcut "$SMPROGRAMS\$FactoryStartMenuFolder\Oya Factory.lnk"
  ${EndIf}
FunctionEnd

; An update's old uninstaller must leave working shortcuts for the replacement installation.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    StrCpy $FactoryExecutable "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $FactoryStartMenuFolder "$AppStartMenuFolder"
    Call un.FactoryRemoveShortcuts
  ${EndIf}
!macroend
