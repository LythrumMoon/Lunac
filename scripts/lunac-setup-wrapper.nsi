; Lunac setup compatibility wrapper -- NSIS 3.x
;
; The visible installer is the Inno Setup dark wizard (scripts\lunac-v2-installer.iss).
; This tiny wrapper exists for one reason: old Lunac builds auto-update by running
; `Setup.exe /S /D=<dir>` (NSIS flags). Inno Setup only understands `/SILENT` and
; `/DIR=`. The wrapper is silent, embeds the Inno installer, translates the flags
; and waits for it, so both old updaters and manual installs work with one filename.
;
; Build (from build-release-v2.ps1):
;   makensis /DINNER_SETUP=<inner-inno.exe> /DOUTER_SETUP=<final.exe> ^
;            /DPRODUCT_VERSION=<x.y.z> scripts\lunac-setup-wrapper.nsi

!include "StrFunc.nsh"
!include "LogicLib.nsh"
${Using:StrFunc} StrStr

!ifndef INNER_SETUP
  !error "INNER_SETUP is required (the compiled Inno Setup exe)"
!endif
!ifndef OUTER_SETUP
  !error "OUTER_SETUP is required (the final Setup.exe path)"
!endif
!ifndef PRODUCT_VERSION
  !define PRODUCT_VERSION "1.0.1"
!endif

Name "Lunac"
OutFile "${OUTER_SETUP}"
InstallDir "$LOCALAPPDATA\Lunac"
RequestExecutionLevel user
SetCompressor lzma
SilentInstall silent
ShowInstDetails nevershow

Var InnoArgs
Var InnerExe
Var Flag

Section "Install"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=$PLUGINSDIR\LunacInnoSetup.exe "${INNER_SETUP}"
  StrCpy $InnerExe "$PLUGINSDIR\LunacInnoSetup.exe"

  StrCpy $InnoArgs "/NORESTART /SUPPRESSMSGBOXES"

  ; `/S` (old NSIS updater) -> Inno `/SILENT` (progress window, no wizard).
  ${StrStr} $Flag "$CMDLINE" "/S"
  ${If} $Flag != ""
    StrCpy $InnoArgs "$InnoArgs /SILENT"
  ${EndIf}

  ; Old `/D=<dir>` (NSIS) is consumed into $INSTDIR by NSIS itself; the default
  ; value means no /D= was given, so let Inno use its own migration/default.
  StrCpy $0 "$LOCALAPPDATA\Lunac"
  ${If} $INSTDIR != $0
    StrCpy $InnoArgs "$InnoArgs /DIR=$\"$INSTDIR$\""
  ${EndIf}

  DetailPrint "Starting Lunac setup..."
  ExecWait '"$InnerExe" $InnoArgs' $Flag
  SetErrorLevel $Flag
SectionEnd
