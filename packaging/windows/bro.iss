; bro for Windows: a per-user installer (no admin prompt) that puts bro.exe, bro-app.exe and bro-cli.cmd in
; %LOCALAPPDATA%\Programs\bro (where install.ps1 installs too), adds a "bro" Start menu entry that opens the
; desktop app, optionally a desktop shortcut, and puts bro on your PATH for terminals (bro opens the desktop
; app, bro-cli the terminal UI).
;
; Build: ISCC /DAppVersion=2.0.22 /DBinDir=<folder with bro.exe, bro-app.exe, bro-cli.cmd, README.md, LICENSE> ^
;             /DOutputName=bro-v2.0.22-x86_64-pc-windows-msvc-setup /O<out dir> packaging\windows\bro.iss
;
; Updating over a running bro works: the running executables are renamed to *.old first (Windows lets a running
; file be renamed, not replaced) and bro deletes them the next time it starts, like install.ps1 does.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef OutputName
  #define OutputName "bro-setup"
#endif

[Setup]
AppId={{6E3B2C8A-6F1D-4C55-9B0E-2B8B7F3D1A42}
AppName=bro
AppVersion={#AppVersion}
AppVerName=bro {#AppVersion}
AppPublisher=JustSuperHuman
AppPublisherURL=https://github.com/JustSuperHuman/bro-cli
AppSupportURL=https://github.com/JustSuperHuman/bro-cli/issues
AppUpdatesURL=https://github.com/JustSuperHuman/bro-cli/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription=bro installer
DefaultDirName={localappdata}\Programs\bro
DefaultGroupName=bro
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
SetupIconFile=..\..\crates\bro-tui\assets\bro.ico
UninstallDisplayIcon={app}\bro-app.exe
UninstallDisplayName=bro
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile={#BinDir}\LICENSE
; bro is updated while it runs (see PrepareToInstall), so never ask to close it
CloseApplications=no
RestartApplications=no

[Tasks]
Name: "addtopath"; Description: "Add bro to my PATH, so ""bro"" and ""bro-cli"" work in any terminal"
Name: "desktopicon"; Description: "Put a bro shortcut on my desktop"; Flags: unchecked

[Files]
Source: "{#BinDir}\bro.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\bro-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\bro-cli.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\bro"; Filename: "{app}\bro-app.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "Open bro, the workspace for your coding agents"
Name: "{autodesktop}\bro"; Filename: "{app}\bro-app.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "Open bro, the workspace for your coding agents"; Tasks: desktopicon

[Run]
Filename: "{app}\bro-app.exe"; Description: "Open bro now"; WorkingDir: "{%USERPROFILE}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: files; Name: "{app}\*.old"

[Code]
const
  EnvKey = 'Environment';

{ The user PATH, split on ';' and compared case-insensitively. }
function PathHas(Paths, Dir: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(Dir: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Paths) then
    Paths := '';
  if PathHas(Paths, Dir) then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: string);
var
  Paths, Rest, Part, Kept: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Paths) then
    exit;
  Rest := Paths;
  Kept := '';
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Part := Rest;
      Rest := '';
    end else begin
      Part := Copy(Rest, 1, P - 1);
      Rest := Copy(Rest, P + 1, Length(Rest));
    end;
    if (Part <> '') and (CompareText(Part, Dir) <> 0) then
    begin
      if Kept <> '' then
        Kept := Kept + ';';
      Kept := Kept + Part;
    end;
  end;
  if Kept <> Paths then
    RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Kept);
end;

{ A running bro.exe / bro-app.exe can't be replaced but can be renamed: move it aside so the new one goes in. }
procedure MoveAside(Name: string);
var
  Path, Old: string;
begin
  Path := ExpandConstant('{app}\' + Name);
  if not FileExists(Path) then
    exit;
  Old := Path + '.old';
  if FileExists(Old) and not DeleteFile(Old) then
    Old := Path + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old';
  RenameFile(Path, Old);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  MoveAside('bro.exe');
  MoveAside('bro-app.exe');
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
