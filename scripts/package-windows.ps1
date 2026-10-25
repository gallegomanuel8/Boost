# Builds the Windows installer of a release into dist\, natively on Windows:
#   boost-<version>-windows-x86_64-setup.exe   (Inno Setup, per-user, no admin)
# Needs Rust (x86_64-pc-windows-gnu with MinGW gcc, or MSVC) and Inno Setup 6.
# The signed Firefox add-on (web-ext-artifacts\boost-<ext version>.xpi, from
# scripts/sign-extension.sh or the GitHub release) goes in when it exists.
#   powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 [-SkipBuild]
param([switch]$SkipBuild)
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root
$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$ExtVersion = (Get-Content extension\manifest.json -Raw | ConvertFrom-Json).version
$Xpi = "web-ext-artifacts\boost-$ExtVersion.xpi"
$Dist = Join-Path $Root "dist"
$Stage = Join-Path $Dist "work\boost-$Version-windows-x86_64"

$Iscc = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue | ForEach-Object Source),
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $Iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }

# MinGW's windres cannot handle spaces in the build folder path.
if (-not $env:CARGO_TARGET_DIR -and $Root -match ' ') {
    $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA "boost-build\target"
}
$Target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "target" }

if (-not $SkipBuild) {
    Write-Host "==> building"
    cargo build --release -p raudo-app -p raudo-nmhost -p raudo-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

Write-Host "==> staging"
Remove-Item (Join-Path $Dist "work") -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
foreach ($f in "boost.exe", "raudo-nmhost.exe", "raudo-cli.exe") {
    Copy-Item (Join-Path $Target "release\$f") $Stage
}
# GNU builds load WebView2 through this DLL, which Windows does not ship.
$Loader = Join-Path $Target "release\WebView2Loader.dll"
if (Test-Path $Loader) { Copy-Item $Loader $Stage }
Copy-Item extension "$Stage\extension" -Recurse
Copy-Item LICENSE, README.md $Stage
# Steps page the installer opens in Chrome, Edge or Brave.
Copy-Item scripts\install-extension.html $Stage
if (Test-Path $Xpi) {
    Copy-Item $Xpi "$Stage\boost.xpi"
} else {
    Write-Host "    (no signed ${Xpi}: the Firefox add-on is not included)"
}

Write-Host "==> installer"
& $Iscc /Q "/DAppVersion=$Version" "/DStage=$Stage" "/DOutDir=$Dist" scripts\boost.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

Remove-Item (Join-Path $Dist "work") -Recurse -Force
Get-ChildItem $Dist -Filter "boost-$Version-windows-*setup.exe" | ForEach-Object {
    "{0}  {1:N1} MB  sha256 {2}" -f $_.Name, ($_.Length / 1MB), (Get-FileHash $_.FullName).Hash.ToLower()
}
