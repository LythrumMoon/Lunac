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
!define PRODUCT_VERSION "0.9.6"
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
; 拓展插件勾选页（2026-09-29，用户要求）：拓展插件**默认不装**，勾了才装 ——
; 对应的 Section 见下面「拓展插件（勾选才装）」那一段。
!insertmacro MUI_PAGE_COMPONENTS

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
  ${NSD_CreateLabel} 0 0 100% 14u "Startup & shortcuts:"
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

; ── Install Section（核心：永远装，用户在组件页上取消不掉）─────────────
Section "Install" SecCore
  SectionIn RO
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

  ; 插件目录（2026-09-28）：只装一份 README（插件开发规范）—— 既给用户看，也是
  ; 「让 Lunac 自己写插件」的依据。插件本体由插件市场按需下载（含各自的依赖）。
  ; **升级安装时不要清空这个目录**：用户已装的插件要留着（这里没有 RMDir，因此天然保留）。
  !if /FileExists "Lunac\Modules\README.md"
    SetOutPath "$INSTDIR\Modules"
    File "Lunac\Modules\README.md"
    SetOutPath "$INSTDIR"
  !endif

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

; ── 拓展插件（勾选才装，2026-09-29 用户要求）───────────────────────
; 规则：拓展插件**不随安装包默认安装**，但安装包里要有勾选项。下面四段都带 `/o`
; （unselected）⇒ 默认全不装；勾了的会在 `$INSTDIR\Modules\<id>\` 落一份**完整插件**
; （index.js + lunac-plugin.json），形状与「从市场装过一遍」完全一致 —— 启动后
; `refreshMarketPlugins()` 扫盘即认，用户不必再点一次下载。
;
; 插件包由 `scripts\build-plugins.ps1` 暂存到 `release\ext-plugins\<id>\`；
; 没暂存过（例如只打了主程序）时 `!if /FileExists` 会整段跳过，makensis 不会因缺文件失败。
;
; **升级安装只 add、不删**：这里没有 RMDir，用户已勾过 / 已从市场装的插件不会被清掉。
;
; 音乐为什么多一段 librespot：它的本机播放引擎是个独立 exe（上游不发 Windows 二进制，
; 那份是我们自己构建后挂到插件仓库 Release 上的，见 build-plugins.ps1 顶部说明）。
; 走**市场**安装时由清单的 `dependencies[]` 自动下载；走**安装包**安装没有那一步，
; 所以这里顺手把本机已构建好的那份一起放进去 —— 否则装完是个「找不到 librespot」的残废插件。

Section /o "Clipboard history" SecExtClipboard
  !if /FileExists "ext-plugins\clipboard-history\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\clipboard-history"
    File /r "ext-plugins\clipboard-history\*"
    SetOutPath "$INSTDIR"
  !endif
SectionEnd

Section /o "OCR (PaddleOCR, offline)" SecExtOcr
  !if /FileExists "ext-plugins\ocr\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\ocr"
    File /r "ext-plugins\ocr\*"
    SetOutPath "$INSTDIR"
  !endif
SectionEnd

Section /o "File converter (ffmpeg)" SecExtConvert
  !if /FileExists "ext-plugins\convert\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\convert"
    File /r "ext-plugins\convert\*"
    SetOutPath "$INSTDIR"
  !endif
SectionEnd

Section /o "Music & lyrics (librespot)" SecExtMusic
  !if /FileExists "ext-plugins\music\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\music"
    File /r "ext-plugins\music\*"
    SetOutPath "$INSTDIR"
  !endif
  !if /FileExists "deps\librespot\bin\librespot.exe"
    SetOutPath "$INSTDIR\Modules\music\bin"
    File "deps\librespot\bin\librespot.exe"
    SetOutPath "$INSTDIR"
  !endif
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
