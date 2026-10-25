#define AppName "Airloom"
#define AppVersion "0.4.4"
#define AppPublisher "Airloom contributors"
#define AppURL "https://github.com/chummumm/airloom"

[Setup]
AppId={{932A385B-F82E-4F87-986A-4D33FF37C5D1}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
DefaultDirName={localappdata}\Programs\Airloom
DefaultGroupName=Airloom
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
OutputDir=..\dist
OutputBaseFilename=Airloom-{#AppVersion}-setup-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\Airloom.exe
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "startup"; Description: "Start Airloom when I sign in"; Flags: unchecked

[Files]
Source: "..\native\Airloom\bin\x64\Release\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; This copy is available before replacing any installed files, including on an
; upgrade from a release which does not understand the graceful-exit protocol.
Source: "..\native\Airloom\bin\x64\Release\Airloom.Update.exe"; Flags: dontcopy

[Icons]
Name: "{group}\Airloom"; Filename: "{app}\Airloom.exe"
Name: "{autodesktop}\Airloom"; Filename: "{app}\Airloom.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Airloom"; ValueData: """{app}\Airloom.exe"" --background"; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\Airloom.exe"; Description: "Open Airloom"; Flags: nowait postinstall skipifsilent

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Helper: String;
  ExitCode: Integer;
begin
  Result := '';
  ExtractTemporaryFile('Airloom.Update.exe');
  Helper := ExpandConstant('{tmp}\Airloom.Update.exe');
  if not Exec(Helper, '--request-exit "' + ExpandConstant('{app}\Airloom.exe') + '"',
      '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then
  begin
    Result := 'Cannot check whether Airloom has exited. Please exit Airloom from its tray menu and retry.';
    Exit;
  end;
  Log('Airloom graceful exit result: ' + IntToStr(ExitCode));
  case ExitCode of
    0: Result := '';
    10: Result := 'Airloom is still running. This older version cannot be closed automatically for an update. '
      + 'Right-click the Airloom icon in the taskbar notification area and choose "Exit Airloom", then retry. '
      + 'Closing its main window only hides it to the tray. No program files have been replaced.';
    11: Result := 'Another copy of Airloom is running from a different folder. Exit that copy from its tray menu, then retry.';
    12: Result := 'Airloom did not finish exiting within 15 seconds. No program files have been replaced. '
      + 'Wait for audio to stop, choose "Exit Airloom" from the tray, then retry.';
  else
    Result := 'Airloom may still be starting or exiting. Exit it from the tray, then retry. '
      + 'Setup will not replace files while its running state is uncertain.';
  end;
end;

