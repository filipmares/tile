; Tile's NSIS installer hooks, wired in through
; tauri.conf.json > bundle > windows > nsis > installerHooks.

!include LogicLib.nsh

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
!macroend

; Input on the stack: path to Tile's config.json.
; Output on the stack: 1 if it contains "launchOnLogin": false, else 0.
; Whitespace is ignored, so any formatting serde_json produces matches.
Function TileLaunchOnLoginDisabled
  Exch $0 ; path
  Push $1 ; file handle
  Push $2 ; line read
  Push $3 ; line with whitespace removed
  Push $4 ; index
  Push $5 ; char / candidate
  Push $6 ; result

  StrCpy $6 0
  ClearErrors
  FileOpen $1 $0 r
  ${IfNot} ${Errors}
    ${Do}
      ClearErrors
      FileRead $1 $2
      ${If} ${Errors}
        ${Break}
      ${EndIf}

      StrCpy $3 ""
      StrCpy $4 0
      ${Do}
        StrCpy $5 $2 1 $4
        ${If} $5 == ""
          ${Break}
        ${EndIf}
        ${If} $5 != " "
        ${AndIf} $5 != "$\t"
        ${AndIf} $5 != "$\r"
        ${AndIf} $5 != "$\n"
          StrCpy $3 "$3$5"
        ${EndIf}
        IntOp $4 $4 + 1
      ${Loop}

      StrCpy $4 0
      ${Do}
        StrCpy $5 $3 21 $4
        ${If} $5 == ""
          ${Break}
        ${EndIf}
        ${If} $5 S== '"launchOnLogin":false'
          StrCpy $6 1
          ${Break}
        ${EndIf}
        IntOp $4 $4 + 1
      ${Loop}

      ${If} $6 == 1
        ${Break}
      ${EndIf}
    ${Loop}
    FileClose $1
  ${EndIf}

  StrCpy $0 $6
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Exch $0
FunctionEnd
