# Signs the Firefox extension on addons.mozilla.org (unlisted: signed, not published);
# Windows counterpart of sign-extension.sh. Leaves web-ext-artifacts\boost-<version>.xpi.
# Keys: https://addons.mozilla.org/developers/addon/api/key/
# They are read from the terminal (or WEB_EXT_API_KEY / WEB_EXT_API_SECRET), never stored.
#   powershell -ExecutionPolicy Bypass -File scripts\sign-extension.ps1
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

$Key = $env:WEB_EXT_API_KEY
if (-not $Key) { $Key = Read-Host "Emisor JWT (user:...)" }
$Secret = $env:WEB_EXT_API_SECRET
if (-not $Secret) {
    $s = Read-Host "Secreto JWT" -AsSecureString
    $Secret = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($s))
}
# Pasted values often carry stray spaces or newlines.
$Key = $Key -replace '\s', ''
$Secret = $Secret -replace '\s', ''
if ($Key -notmatch '^user:\d+:\d+$') { throw "El emisor debe tener la forma user:NNNN:NNN" }
if ($Secret -notmatch '^[0-9a-f]{64}$') { throw "El secreto debe tener 64 caracteres hexadecimales (se leyeron $($Secret.Length))." }
$env:WEB_EXT_API_KEY = $Key
$env:WEB_EXT_API_SECRET = $Secret

$Version = (Get-Content extension\manifest.json -Raw | ConvertFrom-Json).version
$Out = Join-Path $Root "web-ext-artifacts"
npx -y web-ext@10.7.0 sign -s extension --channel unlisted --artifacts-dir $Out
if ($LASTEXITCODE -ne 0) { throw "web-ext sign failed" }

$Xpi = Get-ChildItem $Out -Filter "*-$Version.xpi" | Where-Object Name -ne "boost-$Version.xpi" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($Xpi) { Move-Item $Xpi.FullName (Join-Path $Out "boost-$Version.xpi") -Force }
Write-Host "Firmado: $Out\boost-$Version.xpi"
