; Per-user lagom installer (Inno Setup 6). scripts/package-windows.ps1 builds lagom.exe and
; passes every define below; the version comes from the release tag.
#ifndef Version
  #error Run scripts/package-windows.ps1, which passes /DVersion and the paths.
#endif
#ifdef Sign
  #define SignFlag "sign"
#else
  #define SignFlag ""
#endif
#define AppId "app.lagommail.desktop"

[Setup]
AppId={#AppId}
AppName=lagom
AppVersion={#Version}
AppVerName=lagom {#Version}
AppPublisher=lagom
VersionInfoVersion={#NumericVersion}
VersionInfoProductName=lagom
; Per user: %LOCALAPPDATA%\Programs\lagom, no administrator prompt.
PrivilegesRequired=lowest
DefaultDirName={autopf}\lagom
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile={#Repo}\brand\app-icon\export\lagom.ico
UninstallDisplayIcon={app}\lagom.exe
UninstallDisplayName=lagom
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=lagom-{#Version}-windows-x64-setup
#ifdef Sign
; The ps1 script defines the "lagom" sign tool with /Slagom=...; it signs setup, uninstaller and lagom.exe.
SignTool=lagom
#endif

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion {#SignFlag}
; The toast icon: the AppUserModelId IconUri below.
Source: "{#Repo}\brand\app-icon\export\lagom-512.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bootstrapper}"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: NeedsWebView2

[Icons]
; Toasts and the taskbar match the app by this ID; lagom sets the same one on its process.
Name: "{autoprograms}\lagom"; Filename: "{app}\lagom.exe"; AppUserModelID: "{#AppId}"

[Registry]
; The identity toasts show as. lagom reads DisplayName to know it may post toasts.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\{#AppId}"; ValueType: string; ValueName: "DisplayName"; ValueData: "lagom"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\{#AppId}"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\lagom-512.png"

[Run]
; Windows 11 ships WebView2; Windows 10 may not. Without it lagom shows mail as plain text.
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; StatusMsg: "Installing Microsoft Edge WebView2..."; Check: NeedsWebView2; Flags: waituntilterminated
Filename: "{app}\lagom.exe"; Description: "Open lagom"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; WebView2 keeps its user data folder next to the exe by default (wry passes no other). Mail is
; InPrivate, but the folder still holds the browser profile.
Type: filesandordirs; Name: "{app}\lagom.exe.WebView2"

[Code]
// Microsoft's documented check, the same as lagom's own (apps/desktop/src/main_window/html.rs).
const WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function HasWebView2(Root: Integer): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, WebView2Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

function NeedsWebView2: Boolean;
begin
  Result := not (HasWebView2(HKLM32) or HasWebView2(HKCU));
end;
