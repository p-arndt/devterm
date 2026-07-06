; DevTerm — Windows installer (Inno Setup 6).
;
; Build locally:
;   iscc installer\devterm.iss
; Override version / source location (used by CI):
;   iscc /DAppVersion=0.2.0 /DSourceDir="target\x86_64-pc-windows-msvc\release" installer\devterm.iss
;
; Produces  installer\output\DevTerm-<version>-Setup.exe  — a standard wizard installer
; with Start Menu + optional desktop shortcut, an optional "Add to PATH" task, and a
; matching uninstaller in Add/Remove Programs.

#define AppName        "DevTerm"
#define AppExeName     "devterm.exe"
#define AppPublisher   "Patrick Arndt"
#define AppURL         "https://github.com/patrickarndt/devterm"

#ifndef AppVersion
  #define AppVersion   "0.0.0"
#endif

; Where the freshly built devterm.exe lives. Relative paths in [Files] resolve against
; THIS script's folder (installer\), so the default reaches up to the repo's target dir.
; CI overrides this with an absolute path via /DSourceDir=...
#ifndef SourceDir
  #define SourceDir    "..\target\x86_64-pc-windows-msvc\release"
#endif

[Setup]
; Keep this AppId STABLE across releases — it's how Windows recognizes upgrades vs. new installs.
AppId={{577955F8-8E3A-4C26-9907-A7BB84A86C76}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir=output
OutputBaseFilename=DevTerm-{#AppVersion}-Setup
SetupIconFile=..\assets\devterm.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; 64-bit app; install per-machine by default but allow a non-admin per-user install.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "addtopath";   Description: "Add DevTerm to PATH (run ""devterm"" from any terminal)"; GroupDescription: "System integration:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md";               DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{group}\{#AppName}";           Filename: "{app}\{#AppExeName}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}";     Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; "Add to PATH" task: append the install dir to the *user* PATH, only if not already present.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath('{app}')

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; \
    Flags: nowait postinstall skipifsilent

[Code]
// True only when the install dir is not already on the user's PATH (avoids duplicate entries on re-install).
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
  Needle: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Needle := ';' + Uppercase(ExpandConstant(Param)) + ';';
  Result := Pos(Needle, ';' + Uppercase(OrigPath) + ';') = 0;
end;
