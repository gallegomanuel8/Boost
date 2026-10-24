; Windows installer for Boost (Inno Setup 6), built by scripts/package-windows.ps1.
; Installs for the current user only, without administrator rights, in the same
; place as install.ps1: %LOCALAPPDATA%\Programs\Boost.
;   ISCC /DAppVersion=<version> /DStage=<folder with the files> /DOutDir=<dist> boost.iss

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
