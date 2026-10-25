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
es.OpenChromiumExt=Abrir la carpeta de la extensión para Chrome, Edge y Brave
en.OpenChromiumExt=Open the extension folder for Chrome, Edge and Brave

[Messages]
es.FinishedLabel=Boost se ha instalado.%n%nFirefox te pedirá confirmación para añadir la extensión.%n%nChrome, Edge y Brave solo instalan extensiones de su tienda: abre la página de extensiones del navegador, activa el «Modo de desarrollador», pulsa «Cargar descomprimida» y elige la carpeta de la extensión. Boost te guía en Preferencias → Extensiones web.
en.FinishedLabel=Boost has been installed.%n%nFirefox will ask you to confirm adding the extension.%n%nChrome, Edge and Brave only install extensions from their store: open the browser's extensions page, turn on "Developer mode", click "Load unpacked" and choose the extension folder. Boost guides you in Preferencias → Extensiones web.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\boost.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#Stage}\raudo-nmhost.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\raudo-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\boost.xpi"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#Stage}\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; An extension folder left by an older version could keep files that no longer exist.
Type: filesandordirs; Name: "{app}\extension"

[Icons]
Name: "{userprograms}\Boost"; Filename: "{app}\boost.exe"; WorkingDir: "{app}"; Comment: "Gestor de descargas"
Name: "{userdesktop}\Boost"; Filename: "{app}\boost.exe"; WorkingDir: "{app}"; Comment: "Gestor de descargas"; Tasks: desktopicon

[Run]
; Browsers find the host, and the boost:// link opens the app.
Filename: "{app}\raudo-nmhost.exe"; Parameters: "register"; Flags: runhidden waituntilterminated; StatusMsg: "Registrando Boost en los navegadores..."
Filename: "{app}\boost.exe"; Description: "{cm:LaunchProgram,Boost}"; Flags: nowait postinstall skipifsilent
; Firefox opens the signed add-on and asks the user to add it.
Filename: "{code:FirefoxExe}"; Parameters: """{app}\boost.xpi"""; Description: "{cm:InstallFirefoxExt}"; Flags: nowait postinstall skipifsilent; Check: CanInstallFirefoxExt
; Chromium browsers load it by hand from this folder (developer mode → Load unpacked).
Filename: "{app}\extension"; Description: "{cm:OpenChromiumExt}"; Flags: postinstall shellexec nowait skipifsilent unchecked

[UninstallRun]
Filename: "{app}\raudo-nmhost.exe"; Parameters: "unregister"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterHost"
; "Start with the system" from the preferences (tauri-plugin-autostart).
Filename: "{sys}\reg.exe"; Parameters: "delete ""HKCU\Software\Microsoft\Windows\CurrentVersion\Run"" /v Boost /f"; Flags: runhidden; RunOnceId: "RemoveAutostart"

[UninstallDelete]
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

// Firefox registers its path under App Paths (per user or for all users).
function FirefoxExe(Param: String): String;
begin
  if not RegQueryStringValue(HKCU, 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\firefox.exe', '', Result) then
    if not RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\firefox.exe', '', Result) then
      Result := '';
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
end;
