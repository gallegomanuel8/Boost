; Windows installer for Boost (Inno Setup 6), built by scripts/package-windows.ps1.
; Installs for the current user only, without administrator rights, in the same
; place as install.ps1: %LOCALAPPDATA%\Programs\Boost.
;   ISCC /DAppVersion=<version> /DStage=<folder with the files> /DOutDir=<dist> boost.iss
; Keep this file UTF-8 with BOM: without it ISCC misreads the accented texts.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef Stage
  #error Pass /DStage=<folder with boost.exe, raudo-nmhost.exe, ...>
#endif
#ifndef OutDir
  #define OutDir "."
#endif

[Setup]
AppId={{6F7C2B1E-4D3A-4B8E-9C1F-B0057A11D0E5}
AppName=Boost
AppVersion={#AppVersion}
AppVerName=Boost {#AppVersion}
AppPublisher=Boost
AppPublisherURL=https://github.com/gallegomanuel8/Boost
AppSupportURL=https://github.com/gallegomanuel8/Boost/issues
AppUpdatesURL=https://github.com/gallegomanuel8/Boost/releases
DefaultDirName={localappdata}\Programs\Boost
DisableDirPage=yes
DisableProgramGroupPage=yes
DefaultGroupName=Boost
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#Stage}\LICENSE
SetupIconFile=..\crates\raudo-app\icons\icon.ico
UninstallDisplayIcon={app}\boost.exe
UninstallDisplayName=Boost
OutputDir={#OutDir}
OutputBaseFilename=boost-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Languages]
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
es.InstallFirefoxExt=Instalar la extensión en Firefox
en.InstallFirefoxExt=Install the Firefox extension
es.InstallChromeExt=Instalar la extensión en Chrome
en.InstallChromeExt=Install the extension in Chrome
es.InstallEdgeExt=Instalar la extensión en Edge
en.InstallEdgeExt=Install the extension in Edge
es.InstallBraveExt=Instalar la extensión en Brave
en.InstallBraveExt=Install the extension in Brave
es.InstallTools=Instalar yt-dlp y ffmpeg con winget (necesarios para vídeos de YouTube y MP3)
en.InstallTools=Install yt-dlp and ffmpeg with winget (needed for YouTube videos and MP3)
es.RemoveTools=¿Desinstalar también yt-dlp, FFmpeg y Deno, que instaló Boost?%n%nDéjalos si otros programas los usan.
en.RemoveTools=Also uninstall yt-dlp, FFmpeg and Deno, which Boost installed?%n%nKeep them if other programs use them.
es.OpenChromiumExt=Abrir la carpeta de la extensión para navegadores Chromium
en.OpenChromiumExt=Open the extension folder for Chromium browsers
es.DeleteData=¿Borrar también el historial de descargas, los ajustes y las descargas sin terminar?%n%nLos ficheros ya descargados no se tocan.
en.DeleteData=Also delete the download history, the settings and the unfinished downloads?%n%nFiles already downloaded are not touched.
es.ExtStillInstalled=La extensión de Boost sigue instalada en: %1.%n%nEl desinstalador no puede quitar extensiones de los navegadores. Quítala desde la página de extensiones de cada uno (about:addons en Firefox, brave://extensions en Brave, chrome://extensions en Chrome, edge://extensions en Edge).
en.ExtStillInstalled=The Boost extension is still installed in: %1.%n%nThe uninstaller cannot remove browser extensions. Remove it from each browser's extensions page (about:addons in Firefox, brave://extensions in Brave, chrome://extensions in Chrome, edge://extensions in Edge).
es.OpenFirefoxAddons=¿Abrir ahora los complementos de Firefox para quitarla?
en.OpenFirefoxAddons=Open Firefox's add-ons now to remove it?

[Messages]
es.FinishedLabel=Boost se ha instalado.%n%nFirefox te pedirá confirmación para añadir la extensión.%n%nChrome, Edge y Brave solo instalan directamente extensiones de su tienda: marca tu navegador y se abrirá con los pasos para añadir la de Boost («Modo de desarrollador» → «Cargar descomprimida»). Boost también te guía en Preferencias → Extensiones web.
en.FinishedLabel=Boost has been installed.%n%nFirefox will ask you to confirm adding the extension.%n%nChrome, Edge and Brave only install extensions from their store directly: tick your browser and it opens with the steps to add Boost's ("Developer mode" → "Load unpacked"). Boost also guides you in Preferencias → Extensiones web.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\boost.exe"; DestDir: "{app}"; Flags: ignoreversion; BeforeInstall: MoveAsideIfInUse('boost.exe')
Source: "{#Stage}\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#Stage}\raudo-nmhost.exe"; DestDir: "{app}"; Flags: ignoreversion; BeforeInstall: MoveAsideIfInUse('raudo-nmhost.exe')
Source: "{#Stage}\raudo-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\boost.xpi"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#Stage}\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#Stage}\install-extension.html"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; An extension folder left by an older version could keep files that no longer exist.
Type: filesandordirs; Name: "{app}\extension"
; Executables moved aside while in use by an earlier install (MoveAsideIfInUse).
Type: files; Name: "{app}\*.old"

[Icons]
Name: "{userprograms}\Boost"; Filename: "{app}\boost.exe"; WorkingDir: "{app}"; Comment: "Gestor de descargas"
Name: "{userdesktop}\Boost"; Filename: "{app}\boost.exe"; WorkingDir: "{app}"; Comment: "Gestor de descargas"; Tasks: desktopicon

[Run]
; Browsers find the host, and the boost:// link opens the app.
Filename: "{app}\raudo-nmhost.exe"; Parameters: "register"; Flags: runhidden waituntilterminated; StatusMsg: "Registrando Boost en los navegadores..."
; Before Boost starts, in a console that shows winget's progress.
Filename: "{cmd}"; Parameters: "/c ""{code:ToolsCommand}"""; Description: "{cm:InstallTools}"; Flags: postinstall waituntilterminated skipifsilent; Check: CanInstallTools; BeforeInstall: RecordTools
Filename: "{app}\boost.exe"; Description: "{cm:LaunchProgram,Boost}"; Flags: nowait postinstall skipifsilent
; Firefox opens the signed add-on and asks the user to add it.
Filename: "{code:FirefoxExe}"; Parameters: """{app}\boost.xpi"""; Description: "{cm:InstallFirefoxExt}"; Flags: nowait postinstall skipifsilent; Check: CanInstallFirefoxExt
; Chromium browsers load it by hand (developer mode → Load unpacked) and refuse their
; extensions page on the command line: they open a page with the steps, and the
; folder opens in Explorer to be dragged there.
Filename: "{code:BrowserExe|chrome.exe}"; Parameters: """{code:HelpUrl|chrome}"""; Description: "{cm:InstallChromeExt}"; Flags: nowait postinstall skipifsilent unchecked; Check: HasBrowser('chrome.exe'); BeforeInstall: OpenExtensionFolder
Filename: "{code:BrowserExe|msedge.exe}"; Parameters: """{code:HelpUrl|edge}"""; Description: "{cm:InstallEdgeExt}"; Flags: nowait postinstall skipifsilent unchecked; Check: HasBrowser('msedge.exe'); BeforeInstall: OpenExtensionFolder
Filename: "{code:BrowserExe|brave.exe}"; Parameters: """{code:HelpUrl|brave}"""; Description: "{cm:InstallBraveExt}"; Flags: nowait postinstall skipifsilent unchecked; Check: HasBrowser('brave.exe'); BeforeInstall: OpenExtensionFolder
; Other Chromium browsers (Vivaldi, Opera...), when none of the above is installed.
Filename: "{app}\extension"; Description: "{cm:OpenChromiumExt}"; Flags: postinstall shellexec nowait skipifsilent unchecked; Check: not HasAnyChromium

[UninstallRun]
Filename: "{app}\raudo-nmhost.exe"; Parameters: "unregister"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterHost"
; Unregistered, browsers can no longer restart the host: stop the running ones
; now, or their files stay locked.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM raudo-nmhost.exe"; Flags: runhidden waituntilterminated; RunOnceId: "StopHost"
; "Start with the system" from the preferences (tauri-plugin-autostart).
Filename: "{sys}\reg.exe"; Parameters: "delete ""HKCU\Software\Microsoft\Windows\CurrentVersion\Run"" /v Boost /f"; Flags: runhidden; RunOnceId: "RemoveAutostart"

[UninstallDelete]
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\org.raudo.nmhost.chromium.json"
Type: files; Name: "{app}\org.raudo.nmhost.firefox.json"

[Code]
// Boost (or the browser host) running would keep its files locked.
procedure StopBoost();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM boost.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM raudo-nmhost.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

// A browser restarts the host as soon as its extension reconnects, so it may be
// running again by the time its file is replaced. Windows refuses to overwrite a
// running executable but lets it be renamed: the running copy keeps working from
// the new name, and the next install or the uninstall deletes it.
procedure MoveAsideIfInUse(Name: String);
var
  Path: String;
  Code: Integer;
begin
  Path := ExpandConstant('{app}\' + Name);
  if not FileExists(Path) then
    Exit;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM ' + Name, '', SW_HIDE, ewWaitUntilTerminated, Code);
  if not DeleteFile(Path) then
    RenameFile(Path, Path + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old');
end;

// Browsers register their path under App Paths (per user or for all users).
function BrowserExe(Exe: String): String;
begin
  if not RegQueryStringValue(HKCU, 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\' + Exe, '', Result) then
    if not RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\' + Exe, '', Result) then
      Result := '';
  if (Result <> '') and not FileExists(Result) then
    Result := '';
end;

function HasBrowser(Exe: String): Boolean;
begin
  Result := BrowserExe(Exe) <> '';
end;

function HasAnyChromium: Boolean;
begin
  Result := HasBrowser('chrome.exe') or HasBrowser('msedge.exe') or HasBrowser('brave.exe');
end;

function FirefoxExe(Param: String): String;
begin
  Result := BrowserExe('firefox.exe');
end;

// Boost's helper programs: on PATH (also as just saved in the registry, which this
// process may not have yet), in winget's links or next to Boost.
function ToolFound(Name: String): Boolean;
var
  Paths, Part: String;
begin
  Paths := GetEnv('PATH');
  if RegQueryStringValue(HKCU, 'Environment', 'Path', Part) then
    Paths := Paths + ';' + Part;
  if RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', 'Path', Part) then
    Paths := Paths + ';' + Part;
  Paths := Paths + ';' + ExpandConstant('{localappdata}\Microsoft\WinGet\Links;{commonpf64}\WinGet\Links;{app}');
  Result := FileSearch(Name + '.exe', Paths) <> '';
end;

function WingetExe: String;
begin
  Result := ExpandConstant('{localappdata}\Microsoft\WindowsApps\winget.exe');
  if not FileExists(Result) then
    Result := '';
end;

function WingetInstall(Id: String): String;
begin
  Result := '"' + WingetExe + '" install --id ' + Id + ' --exact --source winget --accept-package-agreements --accept-source-agreements --disable-interactivity & ';
end;

// winget packages to install: only what is missing. yt-dlp.yt-dlp brings its own
// FFmpeg (yt-dlp.FFmpeg) and Deno, the JavaScript runtime YouTube needs.
function ToolPackages: String;
begin
  Result := '';
  if not ToolFound('yt-dlp') then
    Result := 'yt-dlp.yt-dlp'
  else if not ToolFound('ffmpeg') then
    Result := 'yt-dlp.FFmpeg';
end;

function ToolsCommand(Param: String): String;
begin
  Result := WingetInstall(ToolPackages) + 'timeout /t 5';
end;

const
  ToolsKey = 'Software\Boost';

// What this installer had winget install, so that only those are offered for
// removal on uninstall (with its dependencies, as winget reports them then).
procedure RecordTools;
var
  Old, Added: String;
begin
  Added := ToolPackages;
  // A Deno already there satisfies the dependency and stays the user's.
  if Added = 'yt-dlp.yt-dlp' then
  begin
    Added := Added + ' yt-dlp.FFmpeg';
    if not ToolFound('deno') then
      Added := Added + ' DenoLand.Deno';
  end;
  if RegQueryStringValue(HKCU, ToolsKey, 'WingetPackages', Old) and (Old <> '') then
    Added := Old + ' ' + Added;
  RegWriteStringValue(HKCU, ToolsKey, 'WingetPackages', Added);
end;

function CanInstallTools: Boolean;
begin
  Result := (WingetExe <> '') and (not ToolFound('yt-dlp') or not ToolFound('ffmpeg'));
end;

// file:/// URL of the steps page for one browser (?b=chrome|edge|brave).
function HelpUrl(Browser: String): String;
begin
  Result := ExpandConstant('{app}\install-extension.html');
  StringChangeEx(Result, '%', '%25', True);
  StringChangeEx(Result, ' ', '%20', True);
  StringChangeEx(Result, '#', '%23', True);
  StringChangeEx(Result, '\', '/', True);
  Result := 'file:///' + Result + '?b=' + Browser;
end;

var
  FolderOpened: Boolean;

// Once, however many browsers are ticked.
procedure OpenExtensionFolder;
var
  Code: Integer;
begin
  if FolderOpened then
    Exit;
  FolderOpened := True;
  ShellExec('open', ExpandConstant('{app}\extension'), '', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

function FileHas(Path, Needle: String): Boolean;
var
  Text: AnsiString;
begin
  Result := FileExists(Path) and LoadStringFromFile(Path, Text) and (Pos(Needle, String(Text)) > 0);
end;

// Some profile of a Chromium browser lists the extension (unpacked ones are keyed
// by the ID pinned in the manifest).
function ChromiumHasExt(UserData: String): Boolean;
var
  Rec: TFindRec;
  Needle: String;
begin
  Result := False;
  Needle := '"boloeecmdjhgdkcahinhhpakjhiljbhi":{';
  if FindFirst(UserData + '\*', Rec) then
  try
    repeat
      if (Rec.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0) and (Rec.Name <> '.') and (Rec.Name <> '..') then
        if FileHas(UserData + '\' + Rec.Name + '\Secure Preferences', Needle) or FileHas(UserData + '\' + Rec.Name + '\Preferences', Needle) then
        begin
          Result := True;
          Exit;
        end;
    until not FindNext(Rec);
  finally
    FindClose(Rec);
  end;
end;

// Firefox keeps installed add-ons as <profile>\extensions\<id>.xpi.
function FirefoxHasExt: Boolean;
var
  Rec: TFindRec;
  Profiles: String;
begin
  Result := False;
  Profiles := ExpandConstant('{userappdata}\Mozilla\Firefox\Profiles');
  if FindFirst(Profiles + '\*', Rec) then
  try
    repeat
      if (Rec.Name <> '.') and (Rec.Name <> '..') and
         (FileExists(Profiles + '\' + Rec.Name + '\extensions\boost-downloader@boost.app.xpi') or
          FileExists(Profiles + '\' + Rec.Name + '\extensions\raudo@raudo.org.xpi')) then
      begin
        Result := True;
        Exit;
      end;
    until not FindNext(Rec);
  finally
    FindClose(Rec);
  end;
end;

procedure AddName(var List: String; Name: String);
begin
  if List <> '' then
    List := List + ', ';
  List := List + Name;
end;

// Data Boost leaves outside its folder; the history and settings only if asked.
procedure DeleteAppData;
begin
  // WebView2 cache of the window, and the IPC files of a running Boost.
  DelTree(ExpandConstant('{localappdata}\org.boost.app'), True, True, True);
  DelTree(ExpandConstant('{localappdata}\raudo\run'), True, True, True);
  if UninstallSilent then
    Exit;
  if MsgBox(CustomMessage('DeleteData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
  begin
    DelTree(ExpandConstant('{userappdata}\raudo'), True, True, True);
    DelTree(ExpandConstant('{localappdata}\raudo'), True, True, True);
  end;
end;

// The winget packages this installer added (RecordTools), on request only:
// other programs may use them too.
procedure OfferToolsRemoval;
var
  Packages, Command, Id: String;
  Code, Space: Integer;
begin
  if not RegQueryStringValue(HKCU, ToolsKey, 'WingetPackages', Packages) then
    Exit;
  RegDeleteKeyIncludingSubkeys(HKCU, ToolsKey);
  Packages := Trim(Packages);
  if UninstallSilent or (Packages = '') or (WingetExe = '') then
    Exit;
  if MsgBox(CustomMessage('RemoveTools'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) <> IDYES then
    Exit;
  Command := '';
  while Packages <> '' do
  begin
    Space := Pos(' ', Packages);
    if Space = 0 then
    begin
      Id := Packages;
      Packages := '';
    end else begin
      Id := Copy(Packages, 1, Space - 1);
      Packages := Trim(Copy(Packages, Space + 1, Length(Packages)));
    end;
    // Listed again by a later install: removed once.
    if Pos('--id ' + Id + ' ', Command) = 0 then
      Command := Command + '"' + WingetExe + '" uninstall --id ' + Id + ' --exact --source winget --disable-interactivity & ';
  end;
  Exec(ExpandConstant('{cmd}'), '/c "' + Command + 'timeout /t 5"', '', SW_SHOWNORMAL, ewWaitUntilTerminated, Code);
end;

procedure TellAboutExtensions;
var
  Found: String;
  Firefox: Boolean;
  Code: Integer;
begin
  if UninstallSilent then
    Exit;
  Firefox := FirefoxHasExt;
  if Firefox then AddName(Found, 'Firefox');
  if ChromiumHasExt(ExpandConstant('{localappdata}\Google\Chrome\User Data')) then AddName(Found, 'Chrome');
  if ChromiumHasExt(ExpandConstant('{localappdata}\Microsoft\Edge\User Data')) then AddName(Found, 'Edge');
  if ChromiumHasExt(ExpandConstant('{localappdata}\BraveSoftware\Brave-Browser\User Data')) then AddName(Found, 'Brave');
  if ChromiumHasExt(ExpandConstant('{localappdata}\Vivaldi\User Data')) then AddName(Found, 'Vivaldi');
  if Found = '' then
    Exit;
  if not Firefox then
    MsgBox(FmtMessage(CustomMessage('ExtStillInstalled'), [Found]), mbInformation, MB_OK)
  else if (FirefoxExe('') <> '') and
          (MsgBox(FmtMessage(CustomMessage('ExtStillInstalled'), [Found]) + #13#10#13#10 + CustomMessage('OpenFirefoxAddons'), mbConfirmation, MB_YESNO) = IDYES) then
    Exec(FirefoxExe(''), 'about:addons', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

function CanInstallFirefoxExt: Boolean;
var
  Exe: String;
begin
  Exe := FirefoxExe('');
  Result := (Exe <> '') and FileExists(Exe) and FileExists(ExpandConstant('{app}\boost.xpi'));
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopBoost();
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopBoost();
  if CurUninstallStep = usPostUninstall then
  begin
    DeleteAppData;
    OfferToolsRemoval;
    TellAboutExtensions;
  end;
end;
