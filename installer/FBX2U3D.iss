#define MyAppName "FBX2U3D"
#define MyAppExeName "fbx2u3d.exe"
#define MyAppPublisher "MysticFrog"
#define MyAppURL "https://github.com/MysticFrog/FBX2U3D"

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#ifndef StagingDir
  #error StagingDir must be provided to ISCC.
#endif

[Setup]
AppId={{6D91E060-758A-44C3-A2CC-49BCE4BCE282}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=FBX2U3D-Setup-{#MyAppVersion}
SetupIconFile=..\FBX2U3D-MAC.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add {#MyAppName} to your Windows PATH"; GroupDescription: "Shell integration:"; Flags: unchecked
Name: "contextmenu"; Description: "Add Explorer right-click quick convert for .fbx files"; GroupDescription: "Shell integration:"; Flags: unchecked

[Files]
Source: "{#StagingDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\quick-convert.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\quick-convert.vbs"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\FBX2U3D.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\LICENSE_COMPATIBILITY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\Intel-U3D-SDK-LICENSE-APACHE-2.0.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\u3d-sdk\*"; DestDir: "{app}\u3d-sdk"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autoprograms}\{#MyAppName}\README"; Filename: "{app}\README.md"

[Registry]
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.fbx\shell\FBX2U3DQuickConvert"; ValueType: string; ValueName: ""; ValueData: "Quick convert to U3D"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.fbx\shell\FBX2U3DQuickConvert"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.fbx\shell\FBX2U3DQuickConvert\command"; ValueType: string; ValueName: ""; ValueData: """{sys}\wscript.exe"" ""{app}\quick-convert.vbs"" ""%1"""; Tasks: contextmenu; Flags: uninsdeletekey

[Code]
const
  EnvironmentKey = 'Environment';

function PathContains(const Paths, Value: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Value) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToUserPath(const Value: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKCU, EnvironmentKey, 'Path', Paths) then
    Paths := '';

  if not PathContains(Paths, Value) then begin
    if Paths = '' then
      Paths := Value
    else
      Paths := Paths + ';' + Value;
    RegWriteExpandStringValue(HKCU, EnvironmentKey, 'Path', Paths);
  end;
end;

procedure RemoveFromUserPath(const Value: string);
var
  Paths: string;
  UpdatedPaths: string;
begin
  if not RegQueryStringValue(HKCU, EnvironmentKey, 'Path', Paths) then
    exit;

  UpdatedPaths := ';' + Paths + ';';
  StringChangeEx(UpdatedPaths, ';' + Value + ';', ';', True);
  while Pos(';;', UpdatedPaths) > 0 do
    StringChangeEx(UpdatedPaths, ';;', ';', True);

  if (Length(UpdatedPaths) > 0) and (UpdatedPaths[1] = ';') then
    Delete(UpdatedPaths, 1, 1);

  if (Length(UpdatedPaths) > 0) and (UpdatedPaths[Length(UpdatedPaths)] = ';') then
    Delete(UpdatedPaths, Length(UpdatedPaths), 1);

  RegWriteExpandStringValue(HKCU, EnvironmentKey, 'Path', UpdatedPaths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToUserPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromUserPath(ExpandConstant('{app}'));
end;