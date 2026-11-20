; Lunac v2 (Flutter) installer -- NSIS 3.x
; Produced by scripts\build-release-v2.ps1; staging dir = release\LunacV2\
;
; Differences from lunac-installer.nsi (v1 / Tauri):
;   - The Flutter Windows bundle is shipped recursively (lunac.exe + DLLs + data\).
;   - agent.exe is built from core-agent (release) and packed next to lunac.exe.
;   - No bundled ext-plugins sections: v2 installs plugins from the market into
;     Modules\ (never wiped on upgrade -- we only add/overwrite files here).
;
; ASCII only on purpose: NSIS + PS 5.1 both mis-decode BOM-less CJK.

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"

; makensis resolves File / OutFile relative to this script's directory.
!cd ${__FILEDIR__}\..\release

Name "Lunac"
!define PRODUCT_VERSION "1.0.0"
OutFile "Lunac-${PRODUCT_VERSION}-Setup.exe"
InstallDir "$LOCALAPPDATA\Lunac"
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "InstallLocation"
RequestExecutionLevel user
SetCompressor lzma

!define MUI_ABORTWARNING

Var AutoStart
Var CreateDesktopShortcut

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
Page custom FinishOptions FinishOptionsLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

Function FinishOptions
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 14u "Startup & shortcuts:"
  Pop $0
  CreateFont $1 "$(^Font)" "$(^FontSize)" "700"
  SendMessage $0 ${WM_SETFONT} $1 1

  ${NSD_CreateCheckbox} 10u 24u 100% 12u "Start on boot (auto-start Lunac at login)"
  Pop $AutoStart

  ${NSD_CreateCheckbox} 10u 42u 100% 12u "Create desktop shortcut"
  Pop $CreateDesktopShortcut

  ${NSD_Check} $CreateDesktopShortcut

  nsDialogs::Show
FunctionEnd

Function FinishOptionsLeave
  ${NSD_GetState} $AutoStart $0
  ${NSD_GetState} $CreateDesktopShortcut $1
FunctionEnd

Section "Install" SecCore
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; lunac.exe stays resident in the tray; a running instance locks the files.
  ; agent.exe is a child process -- /im only matches by name, so kill it too.
  DetailPrint "Stopping running Lunac..."
  nsExec::ExecToLog 'taskkill /f /im lunac.exe'
  nsExec::ExecToLog 'taskkill /f /im agent.exe'
  Sleep 1200

  ; Full Flutter bundle: lunac.exe, flutter_windows.dll, lunac_core.dll,
  ; WebView2Loader.dll, native assets, data\ and Modules\ (pet engine).
  ; File /r only adds/overwrites -- user-installed plugins under Modules\ survive.
  File /r "LunacV2\*"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Lunac"
  CreateShortcut "$SMPROGRAMS\Lunac\Lunac.lnk" "$INSTDIR\lunac.exe"
  CreateShortcut "$SMPROGRAMS\Lunac\Uninstall.lnk" "$INSTDIR\uninstall.exe"

  ${NSD_GetState} $CreateDesktopShortcut $0
  ${If} $0 == ${BST_CHECKED}
    CreateShortcut "$DESKTOP\Lunac.lnk" "$INSTDIR\lunac.exe"
  ${EndIf}

  ; --background is read by lib/main.dart: start in the tray without flashing a window.
  ${NSD_GetState} $AutoStart $0
  ${If} $0 == ${BST_CHECKED}
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lunac" '"$INSTDIR\lunac.exe" --background'
  ${EndIf}

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayName" "Lunac"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayIcon" "$INSTDIR\lunac.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "Publisher" "Lunac"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoRepair" 1
SectionEnd

; Silent install = the updater path: bring the user back into the app.
Function .onInstSuccess
  ${If} ${Silent}
    Exec "$INSTDIR\lunac.exe"
  ${EndIf}
FunctionEnd

Section "Uninstall"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lunac"

  DetailPrint "Stopping Lunac..."
  nsExec::ExecToLog 'taskkill /f /im lunac.exe'
  nsExec::ExecToLog 'taskkill /f /im agent.exe'
  Sleep 1500

  RMDir /r /REBOOTOK "$INSTDIR"

  Delete "$SMPROGRAMS\Lunac\Lunac.lnk"
  Delete "$SMPROGRAMS\Lunac\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Lunac"
  Delete "$DESKTOP\Lunac.lnk"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac"
SectionEnd
