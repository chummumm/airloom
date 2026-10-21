#define AppName "Airloom"
#define AppVersion "0.1.0"
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
OutputBaseFilename=Airloom-0.1.0-setup-x64
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

[Icons]
Name: "{group}\Airloom"; Filename: "{app}\Airloom.exe"
Name: "{autodesktop}\Airloom"; Filename: "{app}\Airloom.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Airloom"; ValueData: """{app}\Airloom.exe"" --background"; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\Airloom.exe"; Description: "Open Airloom"; Flags: nowait postinstall skipifsilent

