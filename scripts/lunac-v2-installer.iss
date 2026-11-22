; Lunac v2 installer -- Inno Setup 6.7+ (dark themed wizard)
;
; Why Inno Setup:
;   - Inno 6.7 has a built-in dark wizard style that repaints the page body,
;     the navigation buttons and the title bar (NSIS's native button bar cannot
;     be fully themed without a custom UI plugin).
;   - Custom pages / checkboxes are first-class Pascal Script, so the install
;     folder and the "start on boot" option live on the same page.
;
; Build:
;   ISCC.exe /DMyAppVersion=1.0.1 scripts\lunac-v2-installer.iss
;
; The PowerShell build script falls back to scripts\lunac-v2-installer.nsi when
; ISCC.exe is not installed.

#ifndef MyAppVersion
  #define MyAppVersion "1.0.1"
#endif
#ifndef MyAppName
  #define MyAppName "Lunac"
#endif
#ifndef MyAppId
  #define MyAppId "Lunac.Desktop.AI"
#endif
#ifndef MyOutputBaseFilename
  #define MyOutputBaseFilename "Lunac-" + MyAppVersion + "-Setup"
#endif
; 1 = normal install (launch app, stop running instance on uninstall);
; 0 = test build (no launch, no process kills, separate shortcut names).
#ifndef MyRunApp
  #define MyRunApp 1
#endif
; 1 = read the old NSIS InstallLocation; 0 = test build (always use DefaultDirName).
#ifndef MyMigrateOldDir
  #define MyMigrateOldDir 1
#endif

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=Lunac
AppPublisherURL=https://github.com/LythrumMoon/Lunac
AppSupportURL=https://github.com/LythrumMoon/Lunac
DefaultDirName={localappdata}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
PrivilegesRequired=lowest
OutputDir=..\release
OutputBaseFilename={#MyOutputBaseFilename}
SetupIconFile=..\app-flutter\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\lunac.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern dark includetitlebar
WizardImageFile=installer-welcome.bmp
WizardSmallImageFile=installer-small.bmp
WizardImageBackColor=$001B1816
WizardBackColor=$001B1816
WizardImageStretch=no
UsePreviousAppDir=yes
CloseApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[InstallDelete]
; Migrate from the NSIS installer: its uninstaller is replaced by Inno's.
Type: files; Name: "{app}\uninstall.exe"

[Files]
Source: "..\release\LunacV2\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\lunac.exe"
Name: "{autoprograms}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\lunac.exe"; Check: DesktopIconChecked

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\lunac.exe"" --background"; Flags: uninsdeletevalue; Check: AutoStartChecked

#if MyRunApp
[Run]
Filename: "{app}\lunac.exe"; Description: "Open {#MyAppName} now"; Flags: postinstall nowait skipifsilent
Filename: "{app}\lunac.exe"; Parameters: "--background"; Flags: nowait runhidden; Check: IsSilentMode
#endif

#if MyRunApp
[UninstallRun]
Filename: "{cmd}"; Parameters: "/c taskkill /f /im lunac.exe & taskkill /f /im agent.exe"; Flags: runhidden; RunOnceId: "StopLunac"
#endif

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]
var
  AutoStartCheckBox: TNewCheckBox;
  DesktopIconCheckBox: TNewCheckBox;

procedure InitializeWizard;
var
  OldDir: String;
begin
  // Migrate the directory chosen by the old NSIS installer, if present.
  // Skip when the wrapper already passed an explicit /DIR (old updater /D=).
  if (Pos('/DIR', Uppercase(GetCmdTail)) = 0) and ({#MyMigrateOldDir} = 1) and
     RegQueryStringValue(HKCU,
       'Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac',
       'InstallLocation', OldDir) and (OldDir <> '') then
    WizardForm.DirEdit.Text := OldDir;

  AutoStartCheckBox := TNewCheckBox.Create(WizardForm);
  AutoStartCheckBox.Parent := WizardForm.DirEdit.Parent;
  AutoStartCheckBox.Left := WizardForm.DirEdit.Left;
  AutoStartCheckBox.Top := WizardForm.DirEdit.Top + WizardForm.DirEdit.Height + ScaleY(30);
  AutoStartCheckBox.Width := WizardForm.DirEdit.Parent.ClientWidth - AutoStartCheckBox.Left;
  AutoStartCheckBox.Height := ScaleY(18);
  AutoStartCheckBox.Caption := 'Start {#MyAppName} on boot (runs in the tray at login)';
  AutoStartCheckBox.Checked := False;

  DesktopIconCheckBox := TNewCheckBox.Create(WizardForm);
  DesktopIconCheckBox.Parent := WizardForm.DirEdit.Parent;
  DesktopIconCheckBox.Left := AutoStartCheckBox.Left;
  DesktopIconCheckBox.Top := AutoStartCheckBox.Top + ScaleY(24);
  DesktopIconCheckBox.Width := AutoStartCheckBox.Width;
  DesktopIconCheckBox.Height := ScaleY(18);
  DesktopIconCheckBox.Caption := 'Create a desktop shortcut';
  DesktopIconCheckBox.Checked := True;
end;

function AutoStartChecked: Boolean;
begin
  // Silent installs do not create the wizard; keep the existing Run value
  // instead of deleting it.
  Result := (AutoStartCheckBox <> nil) and AutoStartCheckBox.Checked;
end;

function DesktopIconChecked: Boolean;
begin
  Result := (DesktopIconCheckBox = nil) or DesktopIconCheckBox.Checked;
end;

function IsSilentMode: Boolean;
begin
  Result := WizardSilent;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep = ssInstall) and ({#MyRunApp} = 1) then
  begin
    Exec('taskkill', '/f /im lunac.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Exec('taskkill', '/f /im agent.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Sleep(1200);
  end;

  if CurStep = ssPostInstall then
  begin
    if (AutoStartCheckBox <> nil) and (not AutoStartCheckBox.Checked) then
      RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', '{#MyAppName}');
    if (DesktopIconCheckBox <> nil) and (not DesktopIconCheckBox.Checked) then
      DeleteFile(ExpandConstant('{autodesktop}\{#MyAppName}.lnk'));

    // One-time migration from the old NSIS uninstall entry. Only for the real
    // Lunac build; test builds use their own AppId and shortcuts.
    if CompareText('{#MyAppName}', 'Lunac') = 0 then
      RegDeleteKeyIncludingSubkeys(HKCU,
        'Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunac');
  end;
end;
