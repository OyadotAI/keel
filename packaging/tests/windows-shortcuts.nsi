; Compile the shipping hook with a scratch-file matcher seam; no real shortcuts or registry writes.
Unicode True
RequestExecutionLevel user
SilentInstall silent
OutFile "${FIXTURE_ROOT}\shortcut-fixture.exe"
InstallDir "${FIXTURE_ROOT}\state"
!include "LogicLib.nsh"

; Match the production seam's Boolean/stack/register contract using synthetic target records.
!macro IsShortcutTarget LINK TARGET
  ClearErrors
  FileOpen $0 "${LINK}" r
  ${If} ${Errors}
    StrCpy $1 ""
  ${Else}
    FileRead $0 $1
    FileClose $0
  ${EndIf}
  StrCpy $2 "${TARGET}"
  StrCpy $3 "matcher clobbered this register"
  ${If} $1 == $2
    Push 1
  ${Else}
    Push 0
  ${EndIf}
!macroend

!include "${FACTORY_HOOK}"
!include "${TEST_CASES}"
