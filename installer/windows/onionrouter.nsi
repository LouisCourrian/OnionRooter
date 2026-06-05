; OnionRouter — Windows installer (NSIS)
;
; Per-user install (HKCU, no admin required) into %LOCALAPPDATA%\OnionRouter.
; Drops the companion binary, generates the Native Messaging host manifest
; pointing at it, writes the HKCU registry key Firefox reads to discover
; the host, and bundles the (eventually signed) XPI alongside.
;
; Build:
;   makensis /DAPP_VERSION=0.4.0 /DREPO_ROOT=Z:\path\to\repo onionrouter.nsi
;
; (Driven by installer\build.ps1 — don't invoke makensis by hand normally.)

!ifndef APP_VERSION
  !define APP_VERSION "0.0.0"
!endif

!ifndef REPO_ROOT
  !error "REPO_ROOT must be defined via /DREPO_ROOT=..."
!endif

!ifndef OUTPUT_DIR
  ; Fallback for direct `makensis` invocation outside build.ps1 — older
  ; behaviour, kept for hand-debugging only. Prefer running build.ps1.
  !define OUTPUT_DIR "${REPO_ROOT}\installer\build"
!endif

!define APP_NAME       "OnionRouter"
!define COMPANY_NAME   "Louis COURRIAN"
!define APP_URL        "https://github.com/LouisCourrian/OnionRooter"
!define EXT_ID         "onionrouter@louis-courrian.dev"
!define NATIVE_HOST    "com.onionrouter.companion"
!define UNINST_KEY     "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define RUN_KEY        "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_VALUE      "OnionRouter"

; -------- Metadata --------
Name        "${APP_NAME} ${APP_VERSION}"
OutFile     "${OUTPUT_DIR}\OnionRouter-Setup-${APP_VERSION}.exe"
Unicode     True
SetCompressor /SOLID lzma

; -------- Per-user install --------
RequestExecutionLevel user
InstallDir              "$LOCALAPPDATA\${APP_NAME}"
InstallDirRegKey HKCU   "Software\${APP_NAME}" "InstallDir"

ShowInstDetails   show
ShowUnInstDetails show

; -------- Modern UI --------
!include "MUI2.nsh"
!define MUI_ABORTWARNING
!define MUI_ICON   "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

!define MUI_WELCOMEPAGE_TITLE "Welcome to the ${APP_NAME} installer"
!define MUI_WELCOMEPAGE_TEXT  "This will install the OnionRouter companion (~3 MB) and register it with Firefox so the OnionRouter extension can route .onion URLs through Tor automatically.$\r$\n$\r$\nNo admin rights required — installed only for the current user.$\r$\n$\r$\nClick Next to continue."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "Companion installed. The system-tray app is now running -- look for the purple onion icon in the notification area (bottom-right of your screen).$\r$\n$\r$\nNext step: install the Firefox extension. Open Firefox and drag$\r$\n  $INSTDIR\extension.xpi$\r$\ninto a browser window."
!define MUI_FINISHPAGE_LINK "Open install folder"
!define MUI_FINISHPAGE_LINK_LOCATION "$INSTDIR"
; MUI wraps this path in its own quotes when composing Exec internally,
; so we MUST pass the bare path -- adding our own quotes here produces
; an unparseable Exec call ("Exec expects 1 parameters, got 3").
!define MUI_FINISHPAGE_RUN "$INSTDIR\bin\onionrouter-companion.exe"
!define MUI_FINISHPAGE_RUN_PARAMETERS "--tray"
!define MUI_FINISHPAGE_RUN_TEXT "Launch OnionRouter tray now"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; -------- Install --------
Section "Install"
    SetOverwrite on

    ; Kill any previous tray instance so File overwrite doesn't fail
    ; with sharing-violation when upgrading. taskkill returns non-zero
    ; if no such process exists -- that's fine, swallow the result.
    nsExec::Exec 'taskkill /F /IM onionrouter-companion.exe'
    Pop $0

    ; Companion binary
    SetOutPath "$INSTDIR\bin"
    File "${REPO_ROOT}\companion\target\release\onionrouter-companion.exe"

    ; Bundled XPI (for the user to drag into Firefox)
    SetOutPath "$INSTDIR"
    File /oname=extension.xpi "${OUTPUT_DIR}\onionrouter-${APP_VERSION}.xpi"

    ; Write Native Messaging host manifest.
    ; JSON requires forward slashes OR escaped backslashes — we go with
    ; escaped backslashes to stay readable in the registry/file.
    Push "$INSTDIR\bin\onionrouter-companion.exe"
    Call EscapeBackslashes
    Pop $0

    FileOpen $1 "$INSTDIR\${NATIVE_HOST}.json" w
    FileWrite $1 "{$\r$\n"
    FileWrite $1 '  "name": "${NATIVE_HOST}",$\r$\n'
    FileWrite $1 '  "description": "OnionRouter Tor management companion",$\r$\n'
    FileWrite $1 '  "path": "$0",$\r$\n'
    FileWrite $1 '  "type": "stdio",$\r$\n'
    FileWrite $1 '  "allowed_extensions": ["${EXT_ID}"]$\r$\n'
    FileWrite $1 "}$\r$\n"
    FileClose $1

    ; Register host with Firefox.
    WriteRegStr HKCU "Software\Mozilla\NativeMessagingHosts\${NATIVE_HOST}" "" "$INSTDIR\${NATIVE_HOST}.json"

    ; Save install info.
    WriteRegStr HKCU "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "Software\${APP_NAME}" "Version"    "${APP_VERSION}"

    ; "Apps & Features" entry (user-scope).
    WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
    WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${APP_VERSION}"
    WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${COMPANY_NAME}"
    WriteRegStr   HKCU "${UNINST_KEY}" "URLInfoAbout"    "${APP_URL}"
    WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; Auto-start the tray at every user login. Per-user (HKCU), no
    ; admin needed. The quoted argument list survives spaces in INSTDIR.
    WriteRegStr HKCU "${RUN_KEY}" "${RUN_VALUE}" '"$INSTDIR\bin\onionrouter-companion.exe" --tray'

    DetailPrint "Installed companion at $INSTDIR\bin\onionrouter-companion.exe"
    DetailPrint "Registered Native Messaging host ${NATIVE_HOST}"
    DetailPrint "Registered auto-start: ${RUN_KEY}\${RUN_VALUE}"
SectionEnd

; -------- Uninstall --------
Section "Uninstall"
    ; Kill any running tray instance so its file handle releases the
    ; .exe we are about to delete. taskkill returns non-zero if the
    ; process wasn't running, which is fine -- swallow with no /B and
    ; /F to force termination.
    nsExec::Exec 'taskkill /F /IM onionrouter-companion.exe'
    Pop $0  ; discard the result

    ; Unregister from Firefox FIRST so a half-removed state doesn't leave
    ; a dangling registry pointer to a missing manifest.
    DeleteRegKey   HKCU "Software\Mozilla\NativeMessagingHosts\${NATIVE_HOST}"
    DeleteRegKey   HKCU "Software\${APP_NAME}"
    DeleteRegKey   HKCU "${UNINST_KEY}"
    DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"

    Delete "$INSTDIR\bin\onionrouter-companion.exe"
    RMDir  "$INSTDIR\bin"
    Delete "$INSTDIR\${NATIVE_HOST}.json"
    Delete "$INSTDIR\extension.xpi"

    ; Tor download cache + tray runtime state file.
    RMDir /r "$INSTDIR\tor"
    RMDir /r "$INSTDIR\runtime"

    Delete "$INSTDIR\uninstall.exe"
    RMDir  "$INSTDIR"

    DetailPrint "OnionRouter removed."
SectionEnd

; -------- Helpers --------
; Replace \ with \\ in a string on the stack (in-place).
Function EscapeBackslashes
    Exch $R0  ; original
    Push $R1  ; output
    Push $R2  ; current char
    Push $R3  ; index
    StrCpy $R1 ""
    StrCpy $R3 0
loop:
    StrCpy $R2 $R0 1 $R3
    StrCmp $R2 "" done
    StrCmp $R2 "\" 0 +3
        StrCpy $R1 "$R1\\"
        Goto next
    StrCpy $R1 "$R1$R2"
next:
    IntOp $R3 $R3 + 1
    Goto loop
done:
    Pop $R3
    Pop $R2
    Exch $R1
    Exch
    Pop $R0
FunctionEnd
