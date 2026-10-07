; Tile's NSIS installer hooks, wired in through
; tauri.conf.json > bundle > windows > nsis > installerHooks.

!include LogicLib.nsh

; Only a release package may touch the login item, mirroring
; BuildKind::manages_autostart in apps/tile/src/build_kind.rs. The release
; workflow sets TILE_BUILD_KIND=installed for the whole job, and makensis
; inherits it from the Tauri CLI; $%VAR% reads it at compile time. A locally
; built installer contains a development binary, which never manages (and so
; could never remove) the login item, so it must not create one.
!if "$%TILE_BUILD_KIND%" == "installed"
  !define TILE_MANAGES_LOGIN_ITEM
!endif

; Waits for an updating Tile to finish exiting.
;
; The updater launches this installer and then calls std::process::exit, so
; the old process is usually still tearing down when the installer starts.
; Newer Tauri templates check for a running app with Restart Manager, which
; sees a process whose windows are already gone as an "Unknown App" it cannot
; shut down, and aborts with "Failed to kill Tile". Opening the executable for
; writing fails while any process still has it mapped, so poll that until the
; old process is gone (bounded, so a Tile that really is still running falls
; through to Tauri's own check). Only updates race like this; a manual
; install with Tile running goes straight to Tauri's prompt.
;
; A macro rather than a Function so both the installer and the uninstaller
; (which the updater runs with /UPDATE before reinstalling) can use it.
!macro TILE_WAIT_FOR_UPDATING_APP_TO_EXIT
  ${If} $UpdateMode = 1
  ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    Push $0
    Push $1
    StrCpy $1 0
    ${Do}
      ClearErrors
      FileOpen $0 "$INSTDIR\${MAINBINARYNAME}.exe" a
      ${IfNot} ${Errors}
        FileClose $0
        ${Break}
      ${EndIf}
      ${If} $1 >= 80
        DetailPrint "${PRODUCTNAME} is still running after 20 seconds."
        ${Break}
      ${EndIf}
      ${If} $1 = 0
        DetailPrint "Waiting for ${PRODUCTNAME} to exit..."
      ${EndIf}
      IntOp $1 $1 + 1
      Sleep 250
    ${Loop}
    Pop $1
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro TILE_WAIT_FOR_UPDATING_APP_TO_EXIT
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro TILE_WAIT_FOR_UPDATING_APP_TO_EXIT
!macroend

; Restores the OS login item after every install.
;
; Tauri's uninstaller deletes HKCU\...\Run\Tile unless it runs with /UPDATE,
; and a manual upgrade with the GUI installer runs the previous version's
; uninstaller without it ("Uninstall before installing" is the default). Tile
; re-creates the login item itself, but only the next time it runs - and the
; installer has just closed it. Without this hook a manual upgrade that is not
; followed by launching Tile leaves nothing to start it at the next sign-in.
;
; The user's choice is respected: an explicit "launchOnLogin": false in the
; config (the only way that preference is ever off) skips the restore. A
; login item disabled in Task Manager stays disabled, because that state lives
; in StartupApproved\Run, which this does not touch.
;
; Tauri includes this file before it defines PRODUCTNAME and MAINBINARYNAME,
; so anything that uses them lives in the macro, which is only expanded inside
; the Install section, after those defines.
!macro NSIS_HOOK_POSTINSTALL
  !ifdef TILE_MANAGES_LOGIN_ITEM
    Push $0
    Push $1

    ReadEnvStr $0 APPDATA
    Push "$0\Tile\Tile\config\config.json"
    Call TileLaunchOnLoginDisabled
    Pop $1

    ${If} $1 == 1
      DetailPrint "Launch on login is off in Tile's settings; leaving the login item alone."
    ${Else}
      WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart'
      DetailPrint "Registered ${PRODUCTNAME} to start at sign-in."
    ${EndIf}

    Pop $1
    Pop $0
  !else
    DetailPrint "Development package: leaving the login item alone."
  !endif
!macroend

!ifdef TILE_MANAGES_LOGIN_ITEM
; Input on the stack: path to Tile's config.json.
; Output on the stack: 1 if it contains "launchOnLogin": false, else 0.
;
; Whitespace is skipped and the last 21 non-whitespace characters are kept in
; a rolling window that carries across lines and read-buffer boundaries, so
; the key and value may be split over lines in any way JSON allows while the
; window never grows past the target's length.
Function TileLaunchOnLoginDisabled
  Exch $0 ; path
  Push $1 ; file handle
  Push $2 ; chunk read
  Push $3 ; rolling window of non-whitespace characters
  Push $4 ; index into the chunk
  Push $5 ; current character
  Push $6 ; result
  Push $7 ; window length
  StrCpy $6 0
  StrCpy $3 ""
  ClearErrors
  FileOpen $1 $0 r
  ${IfNot} ${Errors}
    ${Do}
      ClearErrors
      FileRead $1 $2
      ${If} ${Errors}
        ${Break}
      ${EndIf}

      StrCpy $4 0
      ${Do}
        StrCpy $5 $2 1 $4
        ${If} $5 == ""
          ${Break}
        ${EndIf}
        IntOp $4 $4 + 1
        ${If} $5 == " "
        ${OrIf} $5 == "$\t"
        ${OrIf} $5 == "$\r"
        ${OrIf} $5 == "$\n"
          ${Continue}
        ${EndIf}
        StrCpy $3 "$3$5"
        StrLen $7 $3
        ${If} $7 > 21
          StrCpy $3 $3 "" 1
        ${EndIf}
        ${If} $3 S== '"launchOnLogin":false'
          StrCpy $6 1
          ${Break}
        ${EndIf}
      ${Loop}

      ${If} $6 == 1
        ${Break}
      ${EndIf}
    ${Loop}
    FileClose $1
  ${EndIf}

  StrCpy $0 $6
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Exch $0
FunctionEnd
!endif
