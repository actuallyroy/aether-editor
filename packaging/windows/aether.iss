; Inno Setup script for the Aether Windows installer.
;
; Compiled in CI (.github/workflows/release.yml). Two values are passed on the
; ISCC command line:
;   /DMyAppVersion=X.Y.Z          version (from the release tag)
;   /DSourceExe=<path-to-aether.exe>
;
; Produces installer\aether-windows-setup-x86_64.exe — a wizard that installs
; Aether to Program Files, adds Start Menu (and optional desktop) shortcuts,
; can put aether on PATH, and registers a proper uninstaller in Add/Remove
; Programs. The bare aether.exe is still shipped separately for the in-app
; self-updater and for users who want a portable executable.

#define MyAppName "Aether"
#define MyAppPublisher "actuallyroy"
#define MyAppURL "https://github.com/actuallyroy/aether-editor"
#define MyAppExeName "aether.exe"

[Setup]
; Stable AppId so upgrades replace the prior install instead of stacking.
AppId={{A37E5B2C-1D4F-4E8A-9C3B-7E2F6A1D9C04}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=installer
OutputBaseFilename=aether-windows-setup-x86_64
SetupIconFile=aether.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
PrivilegesRequired=admin

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add Aether to PATH (run ""aether"" from any terminal)"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "aether.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\aether.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\aether.ico"; Tasks: desktopicon

[Registry]
; Append {app} to the system PATH only when the optional task is chosen and the
; directory is not already present.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
{ True if Dir is not already a ';'-delimited entry in the system PATH. }
function NeedsAddPath(Dir: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKLM,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;
