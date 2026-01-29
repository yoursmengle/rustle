; Rustle Inno Setup script
; Place a copy of Microsoft Visual C++ Redistributable x64 installer as
; `installer\VC_redist.x64.exe` (or allow the installer to download it from Microsoft).
; Build with Inno Setup (ISCC.exe) or use command-line tools.

#define AppVersion GetFileVersion(SourcePath + "..\\target\\release\\rustle.exe")

[Setup]
AppName=Rustle
AppVersion={#AppVersion}
DefaultDirName={commonpf}\Rustle
DefaultGroupName=Rustle
DisableProgramGroupPage=yes
DisableDirPage=no
Compression=lzma
SolidCompression=yes
PrivilegesRequired=admin
OutputBaseFilename=rustle-setup-{#AppVersion}
ArchitecturesInstallIn64BitMode=x64

; Files to include. Adjust the Source paths if your build output location differs.
[Files]
; The built rustle executable
Source: "{#SourcePath}..\target\release\rustle.exe"; DestDir: "{app}"; Flags: ignoreversion
; External manifest to force run-as-admin (installed next to exe)
Source: "{#SourcePath}rustle.exe.manifest"; DestDir: "{app}"; Flags: ignoreversion
; VC redist installer - always include it with the installer package
Source: "{#SourcePath}vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
; Launch directly (manifest is now asInvoker, so no UAC prompt).
Name: "{group}\Rustle"; Filename: "{app}\rustle.exe"; IconFilename: "{app}\rustle.exe"; Parameters: ""; WorkingDir: "{app}"
Name: "{commondesktop}\Rustle"; Filename: "{app}\rustle.exe"; Tasks: desktopicon; IconFilename: "{app}\rustle.exe"; Parameters: ""; WorkingDir: "{app}"

[Tasks]
Name: desktopicon; Description: "Create a &desktop icon"; GroupDescription: "Additional icons:"; Flags: unchecked
Name: autostart; Description: "Start Rustle with Windows"; GroupDescription: "Additional icons:"

[Registry]
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Rustle"; ValueData: """{app}\rustle.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
; Install VC runtime silently before launching the app (only if not already installed)
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "Installing Visual C++ Runtime..."; Flags: waituntilterminated skipifdoesntexist; Check: not IsVCRedistInstalled
; Launch directly (manifest is asInvoker, so no UAC prompt expected) - use shellexec to avoid console window
Filename: "{app}\rustle.exe"; Description: "Launch Rustle"; Flags: nowait postinstall skipifsilent shellexec; WorkingDir: "{app}"

[Code]
const
  VC_REDIST_URL = 'https://aka.ms/vs/17/release/vc_redist.x64.exe';

function FileExistsInSys(Name: String): Boolean;
begin
  Result := FileExists(ExpandConstant('{sys}\' + Name));
end;

function IsVCRedistInstalled(): Boolean;
var
  rv: Cardinal;
begin
  Result := False;
  // Check known registry key for VC runtimes (x64). This works for VS 2015-2022 redist packages.
  if RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', rv) then
    Result := (rv = 1);
  if (not Result) and RegQueryDWordValue(HKLM, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', rv) then
    Result := (rv = 1);
  // Fallback: check for presence of common runtime DLLs in System32
  if (not Result) then
    Result := FileExistsInSys('vcruntime140.dll') or FileExistsInSys('vcruntime140_1.dll') or FileExistsInSys('ucrtbase.dll');
end;

// URLDownloadToFile wrapper
function URLDownloadToFile(Caller: Integer; URL: String; FileName: String; Reserved: Integer; CallBack: Integer): LongInt;
  external 'URLDownloadToFileA@urlmon.dll stdcall';

function DownloadFile(const Url, Dest: String): Boolean;
var
  hr: LongInt;
begin
  hr := URLDownloadToFile(0, Url, Dest, 0, 0);
  Result := (hr = 0);
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
end;

// Scheduled task no longer needed because the app runs asInvoker without UAC prompt.