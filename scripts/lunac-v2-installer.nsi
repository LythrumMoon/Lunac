; Lunac v2 (Flutter) installer -- NSIS 3.x
; Produced by scripts\build-release-v2.ps1; staging dir = release\LunacV2\
;
; Differences from lunac-installer.nsi (v1 / Tauri):
;   - The Flutter Windows bundle is shipped recursively (lunac.exe + DLLs + data\).
;   - agent.exe is built from core-agent (release) and packed next to lunac.exe.
;   - No bundled ext-plugins sections: v2 installs plugins from the market into
;     Modules\ (never wiped on upgrade -- we only add/overwrite files here).
;
; Front-end (2026-11-22):
;   - Lunac dark palette (#16181B / #1B1E22 / accent #CFC8BB) on the MUI pages.
;   - One combined page for the install folder + "start on boot" + desktop shortcut.
;   - The finish page has an "Open Lunac now" checkbox (checked by default).
;
; ASCII only on purpose: NSIS + PS 5.1 both mis-decode BOM-less CJK.

; Lunac dark palette must be defined before MUI2 loads: MUI_INTERFACE applies
; its defaults there, and later defines do not always win for page titles.
!define MUI_BGCOLOR "16181B"
!define MUI_TEXTCOLOR "F2F1EE"

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"

; makensis resolves File / OutFile relative to this script's directory.
!cd ${__FILEDIR__}\..\release

Name "Lunac"
BrandingText "Lunac"
!define PRODUCT_VERSION "1.0.1"
OutFile "Lunac-${PRODUCT_VERSION}-Setup.exe"
InstallDir "$LOCALAPPDATA\Lunac"
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "InstallLocation"
RequestExecutionLevel user
SetCompressor lzma

; ── Lunac theme ────────────────────────────────────────────────────
!define MUI_ABORTWARNING
!define MUI_ICON "${__FILEDIR__}\..\app-flutter\windows\runner\resources\app_icon.ico"
!define MUI_UNICON "${__FILEDIR__}\..\app-flutter\windows\runner\resources\app_icon.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_RIGHT
!define MUI_HEADERIMAGE_BITMAP "${__FILEDIR__}\installer-header.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${__FILEDIR__}\installer-welcome.bmp"

!define MUI_WELCOMEPAGE_TITLE "Welcome to Lunac"
!define MUI_WELCOMEPAGE_TEXT "Lunac is a local-first desktop AI assistant: search, chat, plugins and a desktop pet, all running on your machine.$\r$\n$\r$\nThis wizard installs Lunac for the current user. No administrator rights are required."

!define MUI_FINISHPAGE_TITLE "Lunac is ready"
!define MUI_FINISHPAGE_TEXT "Lunac has been installed. You can open it now, or start it later from the Start menu or the tray."
!define MUI_FINISHPAGE_RUN "$INSTDIR\lunac.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Lunac now"
!define MUI_FINISHPAGE_RUN_CHECKED
!define MUI_CUSTOMFUNCTION_GUIINIT LunacGuiInit

Var AutoStart
Var CreateDesktopShortcut
Var InstallDirField

; ── Pages ─────────────────────────────────────────────────────────
!insertmacro MUI_PAGE_WELCOME
Page custom InstallOptionsPage InstallOptionsLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

; Paint the outer dialog (the strip behind the title and the button bar) so the
; native white chrome does not clash with the dark page body.
Function LunacGuiInit
  SetCtlColors $HWNDPARENT "F2F1EE" "16181B"
  ; Branding strip in the bottom-left (control ids used by MUI2).
  GetDlgItem $0 $HWNDPARENT 1028
  SetCtlColors $0 "" "16181B"
  GetDlgItem $0 $HWNDPARENT 1256
  SetCtlColors $0 "8B8E92" "16181B"
FunctionEnd

; ── Install folder + startup options (one page) ───────────────────
Function InstallOptionsPage
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ; The MUI colours cover standard pages; custom pages need the dialog
  ; handle painted explicitly.
  SetCtlColors $0 "F2F1EE" "16181B"
  SetCtlColors $HWNDPARENT "F2F1EE" "16181B"

  ${NSD_CreateLabel} 0 0 100% 14u "Install Lunac"
  Pop $0
  CreateFont $1 "$(^Font)" "12" "700"
  SendMessage $0 ${WM_SETFONT} $1 1
  SetCtlColors $0 "CFC8BB" "16181B"

  ${NSD_CreateLabel} 0 16u 100% 12u "Choose where Lunac lives and how it starts."
  Pop $0
  SetCtlColors $0 "8B8E92" "16181B"

  ${NSD_CreateLabel} 0 38u 100% 10u "Install folder"
  Pop $0
  SetCtlColors $0 "C2C3C4" "16181B"

  ${NSD_CreateDirRequest} 0 50u 78% 13u "$INSTDIR"
  Pop $InstallDirField
  SetCtlColors $InstallDirField "F2F1EE" "1B1E22"

  ${NSD_CreateBrowseButton} 80% 50u 20% 13u "Browse..."
  Pop $0
  ${NSD_OnClick} $0 OnBrowseInstallDir

  ${NSD_CreateLabel} 0 67u 100% 10u "Installs for this Windows user; no administrator rights needed."
  Pop $0
  SetCtlColors $0 "8B8E92" "16181B"

  ${NSD_CreateCheckbox} 0 88u 100% 12u "Start Lunac on boot (runs in the tray at login)"
  Pop $AutoStart
  SetCtlColors $AutoStart "F2F1EE" "16181B"

  ${NSD_CreateCheckbox} 0 106u 100% 12u "Create a desktop shortcut"
  Pop $CreateDesktopShortcut
  SetCtlColors $CreateDesktopShortcut "F2F1EE" "16181B"
  ${NSD_Check} $CreateDesktopShortcut

  nsDialogs::Show
FunctionEnd

Function OnBrowseInstallDir
  Pop $0 ; control handle
  nsDialogs::SelectFolderDialog "Select the Lunac install folder" "$INSTDIR"
  Pop $0
  ${If} $0 != error
    ${NSD_SetText} $InstallDirField "$0"
  ${EndIf}
FunctionEnd

Function InstallOptionsLeave
  ${NSD_GetText} $InstallDirField $0
  ${If} $0 == ""
    MessageBox MB_ICONEXCLAMATION|MB_OK "Choose an install folder first."
    Abort
  ${EndIf}
  StrCpy $INSTDIR $0
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
