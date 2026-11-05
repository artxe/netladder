param(
    [string]$Destination = (Join-Path $PSScriptRoot "..\vendor\windivert")
)

$ErrorActionPreference = "Stop"
$version = "2.2.2"
$url = "https://github.com/basil00/WinDivert/releases/download/v$version/WinDivert-$version-A.zip"
$zip = Join-Path $env:TEMP "WinDivert-$version-A.zip"
$extract = Join-Path $env:TEMP "WinDivert-$version-A"

Write-Host "Downloading WinDivert $version..."
Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip

if (Test-Path -LiteralPath $extract) {
    Remove-Item -LiteralPath $extract -Recurse -Force
}
Expand-Archive -LiteralPath $zip -DestinationPath $extract

$source = Join-Path $extract "WinDivert-$version-A\x64"
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
foreach ($file in @("WinDivert.dll", "WinDivert.lib", "WinDivert64.sys")) {
    Copy-Item -LiteralPath (Join-Path $source $file) -Destination $Destination -Force
}
$license = Join-Path $extract "WinDivert-$version-A\LICENSE"
Copy-Item -LiteralPath $license -Destination (Join-Path $Destination "WinDivert-LICENSE.txt") -Force

Write-Host "Ready: $(Resolve-Path $Destination)"
Write-Host "Keep the DLL and SYS files next to netladder.exe."
