; Inno Setup script for Quotty. Built by tools\build-installer.ps1, which
; passes the version from Cargo.toml:
;   ISCC.exe /DAppVersion=1.0.0 installer\quotty.iss
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppName "Quotty"
#define AppPublisher "Brent"
#define AppURL "https://github.com/confeden/Quotty"
#define AppExe "quotty.exe"

[Setup]
AppId={{7C2F1E64-9A3D-4B58-9C51-0B7E6D2A4F13}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
VersionInfoVersion={#AppVersion}
; Per-user install: no admin prompt, and autostart/settings live per user anyway.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName} {#AppVersion}
OutputDir=..\dist
OutputBaseFilename={#AppName}-Setup-{#AppVersion}
SetupIconFile=..\assets\quotty.ico
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "{cm:AutoStartTask}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The strip has no taskbar window, so let the uninstaller close it itself.
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im {#AppExe}"; Flags: runhidden; RunOnceId: "StopQuotty"

[UninstallDelete]
Type: files; Name: "{userstartup}\{#AppName}.lnk"
Type: files; Name: "{userdesktop}\{#AppName}.lnk"

[CustomMessages]
ru.AutoStartTask=Запускать Quotty при входе в Windows
en.AutoStartTask=Start Quotty when Windows starts

[Code]
// A running copy would keep quotty.exe locked during an upgrade.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im {#AppExe}', '',
       SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := '';
end;
