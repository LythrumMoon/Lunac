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
!include "LogicLib.nsh"

; ── 源文件解析基准 ────────────────────────────────────────────────
; makensis 把 File / OutFile 的相对路径按**本脚本所在目录**解析（不是调用方的 CWD），
; 而本脚本在 scripts\、待打包的暂存目录在 ..\release\Lunac —— 显式切过去，
; 这样无论从哪个目录调用 makensis 都能解析到同一份文件，Setup.exe 也固定落在 release\。
!cd ${__FILEDIR__}\..\release

Name "Lunac"
!define PRODUCT_VERSION "0.9.33"
OutFile "Lunac-${PRODUCT_VERSION}-Setup.exe"
InstallDir "$LOCALAPPDATA\Lunac"
; 升级 / 重装时自动定位**上一版的安装目录**（2026-09-30）：从卸载注册表的
; `InstallLocation` 读回来；读不到（全新安装，或旧版本没写过这个值）才用上面的缺省。
; 没有这一条，上次装在 `D:\Lunac` 的用户重装会被装到 `%LOCALAPPDATA%\Lunac`
; —— 变成**两份安装**、新目录里一份旧数据都没有（历史遗留洞，见 ai-spec §6「升级安装」）。
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "InstallLocation"
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

  ; ── 覆盖前先停掉正在运行的实例（2026-09-30）─────────────────────
  ; `lunac.exe` 是常驻托盘的，一直在跑；文件被占用时 `File` 写不进去，弹出
  ; 「Error opening file for writing」。手动装还能点「重试」，**静默更新直接失败** ——
  ; 所以这一条是自更新链路（updater.rs 拉起 `Setup.exe /S`）的前置条件。
  ; `Sleep` 是必须的：taskkill 返回时句柄未必已释放，立刻覆盖照样会失败。
  ; agent.exe 一起杀：它是 lunac 的子进程，但 `/im` 只杀匹配名，不会跟着父进程走。
  DetailPrint "Stopping running Lunac..."
  nsExec::ExecToLog 'taskkill /f /im lunac.exe'
  nsExec::ExecToLog 'taskkill /f /im agent.exe'
  Sleep 1200

  ; Main application
  ; 只有 lunac.exe + 自研 agent.exe —— **不要**把上游 cli.exe 放进来：
  ; 自研 agent 已完全替代它，且它是 Anthropic 版权的编译产物（见 .gitignore），
  ; 本机留存可以，随安装包分发不可以。build-release.ps1 会在打包前后各校验一次。
  File "Lunac\lunac.exe"
  File "Lunac\agent.exe"
  File "Lunac\WebView2Loader.dll"

  ; 技能 / 工具目录：内置技能（真 SKILL.md，会被加载）+ README 与 .example 模板（供用户照抄）。
  ; 判据 = 文件名：`SKILL.md` / `*.json` 会被 agent 当成真实技能 / 工具，`.example` 不会（见 agent-templates\）。
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

  ; PaddleOCR-json 引擎**不再随安装包分发**（2026-09-30）：它现在是 `ocr` 插件清单里的一条
  ; `dependencies[]`（`type = "archive"`），由宿主在装插件时下载解压到 `Modules\ocr\paddle-ocr\`
  ; （见 plugin_market.rs）。安装包里既不放引擎、这里也不再拷贝。
  ; **卸载清理要留着**：装过引擎的老版本用户升级后仍需 `nsis-hooks.nsh` 那份 RMDir 清干净。

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
  ; 给下一次升级 / 重装定位用（见文件头的 `InstallDirRegKey`）—— 少了它，重装就找不到这儿
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayIcon" "$INSTDIR\lunac.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "Publisher" "Lunac"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac" "NoRepair" 1
SectionEnd

; ── 拓展插件（勾选才装，2026-09-29 用户要求）───────────────────────
; 规则：拓展插件**不随安装包默认安装**，但安装包里要有勾选项。下面五段都带 `/o`
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

; 桌宠（2026-09-30 补）：无依赖（形象由用户自己在控制台里导入，见 ai-spec §4.8），
; 所以这一段就是纯拷贝。窗口形态由插件清单的 `window` 段声明，安装器不必知道。
Section /o "Desktop pet" SecExtPet
  !if /FileExists "ext-plugins\pet\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\pet"
    File /r "ext-plugins\pet\*"
    SetOutPath "$INSTDIR"
  !endif
SectionEnd

; 代理（2026-10-02 补）：无依赖。它管系统代理与本机播放（librespot）的代理 —— 代理软件
; 本身由用户自备，插件只是把系统 / librespot 指过去（契约见 ai-spec §4.11）。
Section /o "Proxy (system + librespot)" SecExtProxy
  !if /FileExists "ext-plugins\proxy\lunac-plugin.json"
    SetOutPath "$INSTDIR\Modules\proxy"
    File /r "ext-plugins\proxy\*"
    SetOutPath "$INSTDIR"
  !endif
SectionEnd

; ── 静默安装 = 自更新：装完把用户带回应用（2026-09-30）───────────────
; 交互式安装**不**自动拉起（用户自己点「完成」决定要不要开）。`${Silent}` 由 LogicLib
; 提供（展开成 `IfSilent`，见 NSIS\Include\LogicLib.nsh）；`/S` 由 updater.rs 传入。
; 放在 `.onInstSuccess` 而不是某个 Section 里：它保证**所有** Section 都写完了才执行。
Function .onInstSuccess
  ${If} ${Silent}
    Exec "$INSTDIR\lunac.exe"
  ${EndIf}
FunctionEnd

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
