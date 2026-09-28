; Lunac NSIS installer hooks
; 2026-09 决策（修订）：全部缓存与业务/插件数据存放在 exe 安装根目录下。
; 卸载时做“双清理”，保证干净卸载：
;   1. exe 安装目录内运行时生成的数据子目录（ModuleData/temp/skills/tools/config/paddle-ocr）
;   2. 兼容旧版本遗留的 %LOCALAPPDATA%\Lunac(-dev) 数据目录

!macro NSIS_HOOK_POSTUNINSTALL
  ; ── 1. exe 安装根内的运行时数据 ──
  RMDir /r "$INSTDIR\ModuleData"
  RMDir /r "$INSTDIR\temp"
  RMDir /r "$INSTDIR\skills"
  RMDir /r "$INSTDIR\tools"
  ; 插件目录 2026-09-28 由 plugins\ 改名 Modules\ —— 旧目录也一并清（升级过来的人可能还留着）
  RMDir /r "$INSTDIR\Modules"
  RMDir /r "$INSTDIR\plugins"
  RMDir /r "$INSTDIR\config"
  RMDir /r "$INSTDIR\paddle-ocr"

  ; ── 2. 旧版本遗留的 LOCALAPPDATA 数据目录 ──
  RMDir /r "$LOCALAPPDATA\Lunac"
  RMDir /r "$LOCALAPPDATA\Lunac-dev"
!macroend
