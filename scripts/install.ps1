# Installs Boost for the current user (no administrator rights):
#   %LOCALAPPDATA%\Programs\Boost\boost.exe (+ raudo-nmhost.exe, raudo-cli.exe)
#   Start menu shortcut, browser registration (Native Messaging + boost:// link)
# Run from the unzipped Windows package:
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-Uninstall]
param([switch]$Uninstall)
$ErrorActionPreference = "Stop"

$Dest = Join-Path $env:LOCALAPPDATA "Programs\Boost"
$Shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "Boost.lnk"

Get-Process boost -ErrorAction SilentlyContinue | Stop-Process -Force

if ($Uninstall) {
    if (Test-Path "$Dest\raudo-nmhost.exe") { & "$Dest\raudo-nmhost.exe" unregister | Out-Null }
    Remove-Item $Shortcut -ErrorAction SilentlyContinue
    Remove-Item $Dest -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Boost desinstalado (el historial de descargas en %APPDATA%\raudo se conserva)."
    exit 0
}

$Here = $PSScriptRoot
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
foreach ($f in "boost.exe", "WebView2Loader.dll", "raudo-nmhost.exe", "raudo-cli.exe", "boost.xpi") {
    if (Test-Path "$Here\$f") { Copy-Item "$Here\$f" $Dest -Force }
}
if (Test-Path "$Here\extension") {
    Remove-Item "$Dest\extension" -Recurse -Force -ErrorAction SilentlyContinue
    Copy-Item "$Here\extension" "$Dest\extension" -Recurse
}

# Browsers find the host, and the boost:// link opens the app.
& "$Dest\raudo-nmhost.exe" register | Out-Null

$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($Shortcut)
$lnk.TargetPath = "$Dest\boost.exe"
$lnk.WorkingDirectory = $Dest
$lnk.Description = "Gestor de descargas"
$lnk.Save()

Write-Host "Boost instalado en $Dest"
Write-Host "Abrelo desde el menu Inicio. Extension para Chrome/Edge/Brave: carga descomprimida desde $Dest\extension"
