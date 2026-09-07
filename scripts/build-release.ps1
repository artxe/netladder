$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$windivert = Join-Path $root "vendor\windivert"
$driver = Join-Path $windivert "WinDivert64.sys"
$windivertLicense = Join-Path $windivert "WinDivert-LICENSE.txt"
$readme = Join-Path $root "README.md"
$license = Join-Path $root "LICENSE"

function Copy-IfChanged {
    param(
        [string]$Source,
        [string]$Destination
    )

    if (Test-Path -LiteralPath $Destination) {
        $sourceHash = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash
        $destinationHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
        if ($sourceHash -eq $destinationHash) {
            return
        }
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

if (-not (Test-Path -LiteralPath $driver) -or
    -not (Test-Path -LiteralPath (Join-Path $windivert "WinDivert.dll")) -or
    -not (Test-Path -LiteralPath $windivertLicense)) {
    & (Join-Path $PSScriptRoot "setup-windivert.ps1")
}

Push-Location $root
try {
    $env:WINDIVERT_PATH = $windivert
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }
    $dist = Join-Path $root "dist"
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $oldExecutable = Join-Path $dist "netlimit.exe"
    if (Test-Path -LiteralPath $oldExecutable) {
        Remove-Item -LiteralPath $oldExecutable -Force
    }
    $builtExecutable = Join-Path $root "target\release\netladder.exe"
    $releaseExecutable = Join-Path $dist "netladder.exe"
    try {
        Copy-Item -LiteralPath $builtExecutable -Destination $releaseExecutable -Force
    } catch [System.IO.IOException] {
        $releaseExecutable = Join-Path $dist "netladder-updated.exe"
        Copy-Item -LiteralPath $builtExecutable -Destination $releaseExecutable -Force
        Write-Warning "netladder.exe is running. Close it, then use netladder-updated.exe."
    }
    Copy-IfChanged -Source (Join-Path $windivert "WinDivert.dll") -Destination (Join-Path $dist "WinDivert.dll")
    Copy-IfChanged -Source $driver -Destination (Join-Path $dist "WinDivert64.sys")
    Copy-IfChanged -Source $windivertLicense -Destination (Join-Path $dist "WinDivert-LICENSE.txt")
    Copy-IfChanged -Source $readme -Destination (Join-Path $dist "README.md")
    Copy-IfChanged -Source $license -Destination (Join-Path $dist "LICENSE")
    $archive = Join-Path $dist "netladder-windows-x64.zip"
    $archiveFiles = @(
        $builtExecutable
        (Join-Path $windivert "WinDivert.dll")
        $driver
        $readme
        $license
        $windivertLicense
    )
    Compress-Archive -LiteralPath $archiveFiles -DestinationPath $archive -CompressionLevel Optimal -Force
    Write-Host "Release ready: $dist"
} finally {
    Pop-Location
}
