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
ChangesEnvironment=yes
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
; The OFL asks for its text beside every copy of the fonts compiled into lagom.exe.
Source: "{#Repo}\apps\desktop\assets\fonts\OFL-*.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
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
const
  WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  PathReceipt = 'Software\lagom\Installer';

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

function ExpandEnvironmentStrings(Source, Dest: String; Size: Cardinal): Cardinal;
  external 'ExpandEnvironmentStringsW@kernel32.dll stdcall';

function ExpandedEntry(Entry: String): String;
var
  Size, Written: Cardinal;
begin
  if Entry = '' then begin
    Result := '';
    Exit;
  end;
  Size := ExpandEnvironmentStrings(Entry, '', 0);
  if Size = 0 then RaiseException('Could not expand the user PATH.');
  SetLength(Result, Size);
  Written := ExpandEnvironmentStrings(Entry, Result, Size);
  if (Written = 0) or (Written > Size) then RaiseException('Could not expand the user PATH.');
  SetLength(Result, Written - 1);
  Result := RemoveBackslash(RemoveQuotes(Trim(Result)));
end;

function FindPathEntry(Value, Dir: String; Exact: Boolean; var Start, Count: Integer): Boolean;
var
  Finish: Integer;
  Entry: String;
  Quoted: Boolean;
begin
  Start := 1;
  while Start <= Length(Value) do begin
    Finish := Start;
    Quoted := False;
    while Finish <= Length(Value) do begin
      if Value[Finish] = '"' then Quoted := not Quoted;
      if (Value[Finish] = ';') and not Quoted then Break;
      Finish := Finish + 1;
    end;
    Count := Finish - Start;
    Entry := Copy(Value, Start, Count);
    if Exact then Result := Entry = Dir
    else Result := CompareText(ExpandedEntry(Entry), Dir) = 0;
    if Result then Exit;
    Start := Finish + 1;
  end;
  Result := False;
end;

function UserPath: String;
begin
  Result := '';
  if RegValueExists(HKCU, 'Environment', 'Path') then
    if not RegQueryStringValue(HKCU, 'Environment', 'Path', Result) then
      RaiseException('Could not read the user PATH.');
end;

function RegGetValue(Root: Integer; Subkey, Name: String; Flags: Cardinal;
  var Kind: Cardinal; Data, Size: LongWord): Longint;
  external 'RegGetValueW@advapi32.dll stdcall';

procedure SavePath(Value: String);
var
  Kind: Cardinal;
  Written: Boolean;
begin
  Kind := 2;
  if RegValueExists(HKCU, 'Environment', 'Path') then
    if RegGetValue(HKCU, 'Environment', 'Path', $10000006, Kind, 0, 0) <> 0 then
      RaiseException('Could not read the user PATH type.');
  if Kind = 1 then Written := RegWriteStringValue(HKCU, 'Environment', 'Path', Value)
  else Written := RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Value);
  if not Written then
    RaiseException('Could not update the user PATH.');
end;

procedure RemoveInstalledPath;
var
  Value, Dir: String;
  Start, Count: Integer;
begin
  if not RegQueryStringValue(HKCU, PathReceipt, 'PathEntry', Dir) then Exit;
  // Older receipts omitted the quotes around directories containing semicolons.
  if (Pos(';', Dir) > 0) and (Copy(Dir, 1, 1) <> '"') then Dir := '"' + Dir + '"';
  Value := UserPath;
  if FindPathEntry(Value, Dir, True, Start, Count) then begin
    if Start > 1 then begin
      Start := Start - 1;
      Count := Count + 1;
    end else if Count < Length(Value) then Count := Count + 1;
    Delete(Value, Start, Count);
    SavePath(Value);
  end;
  RegDeleteValue(HKCU, PathReceipt, 'PathEntry');
  RegDeleteKeyIfEmpty(HKCU, PathReceipt);
end;

procedure CurStepChanged(Step: TSetupStep);
var
  Dir, Value, Previous: String;
  Start, Count: Integer;
begin
  if Step <> ssPostInstall then Exit;
  Dir := ExpandConstant('{app}');
  if RegQueryStringValue(HKCU, PathReceipt, 'PathEntry', Previous) then
    if CompareText(RemoveQuotes(Previous), Dir) <> 0 then RemoveInstalledPath;
  Value := UserPath;
  if FindPathEntry(Value, Dir, False, Start, Count) then Exit;
  if Value <> '' then Value := Value + ';';
  if Pos(';', Dir) > 0 then Dir := '"' + Dir + '"';
  if not RegWriteStringValue(HKCU, PathReceipt, 'PathEntry', Dir) then
    RaiseException('Could not record the user PATH entry.');
  try
    SavePath(Value + Dir);
  except
    RegDeleteValue(HKCU, PathReceipt, 'PathEntry');
    RaiseException(GetExceptionMessage);
  end;
end;

procedure CurUninstallStepChanged(Step: TUninstallStep);
begin
  if Step = usUninstall then RemoveInstalledPath;
end;
