; Lunac Installer Script
; Built with NSIS 3.x
;
; Features:
;   - Installs to %LOCALAPPDATA%\Lunac
;   - Start Menu shortcuts
;   - Optional auto-start on boot (HKCU Run registry key)
;   - Optional desktop shortcut
;   - Uninstaller with full cleanup (including auto-start reg key)

!include "MUI2.nsh"
!include "nsDialogs.nsh"

; ── 源文件解析基准 ────────────────────────────────────────────────
; makensis 把 File / OutFile 的相对路径按**本脚本所在目录**解析（不是调用方的 CWD），
; 而本脚本在 scripts\、待打包的暂存目录在 ..\release\Lunac —— 显式切过去，
; 这样无论从哪个目录调用 makensis 都能解析到同一份文件，Setup.exe 也固定落在 release\。
!cd ${__FILEDIR__}\..\release

Name "Lunac"
!define PRODUCT_VERSION "0.9.1"
OutFile "Lunac-${PRODUCT_VERSION}-Setup.exe"
InstallDir "$LOCALAPPDATA\Lunac"
RequestExecutionLevel user
SetCompressor lzma

!define MUI_ABORTWARNING

; ── Variables for checkbox state ──────────────────────────────────
Var AutoStart
Var CreateDesktopShortcut

; ── Pages ─────────────────────────────────────────────────────────
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY

; Custom options page (before INSTFILES so Section can read checkbox state)
Page custom FinishOptions FinishOptionsLeave

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

; ── Custom options page: auto-start + desktop shortcut ────────────
Function FinishOptions
  nsDialogs::Create 1018
  Pop $0

  ${If} $0 == error
    Abort
  ${EndIf}

  ; Heading
  ${NSD_CreateLabel} 0 0 100% 14u "Select additional install options:"
  Pop $0
  CreateFont $1 "$(^Font)" "$(^FontSize)" "700"
  SendMessage $0 ${WM_SETFONT} $1 1

  ${NSD_CreateCheckbox} 10u 24u 100% 12u "Start on boot (auto-start Lunac at login)"
  Pop $AutoStart

  ${NSD_CreateCheckbox} 10u 42u 100% 12u "Create desktop shortcut"
  Pop $CreateDesktopShortcut

  ; Pre-check "create desktop shortcut" by default
  ${NSD_Check} $CreateDesktopShortcut

  nsDialogs::Show
FunctionEnd

Function FinishOptionsLeave
  ; Read checkbox states into variables for Section to use
  ${NSD_GetState} $AutoStart $0
  ${NSD_GetState} $CreateDesktopShortcut $1
FunctionEnd

; ── Install Section ───────────────────────────────────────────────
Section "Install"
  SetOutPath "$INSTDIR"

  ; Main application
  ; 只有 lunac.exe + 自研 agent.exe —— **不要**把上游 cli.exe 放进来：
  ; 自研 agent 已完全替代它，且它是 Anthropic 版权的编译产物（见 .gitignore），
  ; 本机留存可以，随安装包分发不可以。build-release.ps1 会在打包前后各校验一次。
  File "Lunac\lunac.exe"
  File "Lunac\agent.exe"
  File "Lunac\WebView2Loader.dll"

  ; 技能 / 工具目录：README + .example 模板，供用户照抄（见 agent-templates\）。
  ; 只装模板不装可加载文件 —— *.json 与 SKILL.md 会被 agent 当成真实工具/技能。
  SetOutPath "$INSTDIR\skills"
  File /r "Lunac\skills\*"
  SetOutPath "$INSTDIR\tools"
  File /r "Lunac\tools\*"
  SetOutPath "$INSTDIR"

  ; VSCode extension -- optional, auto-installed by the "Attach to VSCode" button
  ; if present alongside lunac.exe
  !if /FileExists "Lunac\lunac.vsix"
    File "Lunac\lunac.vsix"
  !endif

  ; PaddleOCR-json (offline OCR engine) -- auto-downloaded by build-release.ps1 Step 8
  ; Installed to $INSTDIR\paddle-ocr\ subdirectory (matched by paddle_ocr.rs Priority 1)
  !if /FileExists "Lunac\paddle-ocr\PaddleOCR-json.exe"
    SetOutPath "$INSTDIR\paddle-ocr"
    File /r "Lunac\paddle-ocr\*"
    SetOutPath "$INSTDIR"
  !endif

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Start Menu shortcuts (always created)
  CreateDirectory "$SMPROGRAMS\Lunac"
  CreateShortcut "$SMPROGRAMS\Lunac\Lunac.lnk" "$INSTDIR\lunac.exe"
  CreateShortcut "$SMPROGRAMS\Lunac\Uninstall.lnk" "$INSTDIR\uninstall.exe"

  ; Desktop shortcut (optional, read from checkbox state)
  ${NSD_GetState} $CreateDesktopShortcut $0
  ${If} $0 == ${BST_CHECKED}
    CreateShortcut "$DESKTOP\Lunac.lnk" "$INSTDIR\lunac.exe"
  ${EndIf}

  ; Auto-start on boot (optional, read from checkbox state)
  ${NSD_GetState} $AutoStart $0
  ${If} $0 == ${BST_CHECKED}
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lunac" '"$INSTDIR\lunac.exe" --background'
  ${EndIf}

  ; Registry for Add/Remove Programs
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayName" "Lunac"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayIcon" "$INSTDIR\lunac.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "Publisher" "Lunac"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoRepair" 1
SectionEnd

; ── Uninstall Section ─────────────────────────────────────────────
Section "Uninstall"
  ; Remove auto-start registry key
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Lunac"

  ; Kill running Lunac processes before removing files
  DetailPrint "Stopping Lunac..."
  nsExec::ExecToLog 'taskkill /f /im lunac.exe'
  nsExec::ExecToLog 'taskkill /f /im agent.exe'
  Sleep 1500

  ; Remove installation directory recursively
  ; /REBOOTOK: if any files are still locked, schedule deletion on next reboot
  RMDir /r /REBOOTOK "$INSTDIR"

  ; Remove Start Menu shortcuts
  Delete "$SMPROGRAMS\Lunac\Lunac.lnk"
  Delete "$SMPROGRAMS\Lunac\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Lunac"

  ; Remove Desktop shortcut
  Delete "$DESKTOP\Lunac.lnk"

  ; Remove Add/Remove Programs registry entry
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac"
SectionEnd
