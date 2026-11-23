# Offline integration tests of the Windows install script, run on Windows CI.
$ErrorActionPreference = 'Stop'
$installer = Join-Path (Split-Path $PSScriptRoot -Parent) 'install.ps1'
$assetName = 'forge-x86_64-pc-windows-msvc.zip'
$root = Join-Path ([IO.Path]::GetTempPath()) ('forge-windows-install-test-' + [Guid]::NewGuid().ToString('N'))
$source = Join-Path $root 'release'
$bin = Join-Path $root 'installed bin'
$originalVersion = $env:FORGE_VERSION
$originalDir = $env:FORGE_INSTALL_DIR
$originalUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$originalProcessPath = $env:Path

function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [switch]$UseBasicParsing)
    $name = [IO.Path]::GetFileName(([Uri]$Uri).AbsolutePath)
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $OutFile -Force
}

function Invoke-RestMethod {
    param([string]$Uri, [hashtable]$Headers)
    if ($Uri -notlike '*/releases/latest') { throw "Unexpected URL: $Uri" }
    return @{ tag_name = 'v9.8.7' }
}

try {
    New-Item -ItemType Directory -Path $source -Force | Out-Null
    $payload = Join-Path $source 'forge.exe'
    [IO.File]::WriteAllBytes($payload, [Text.Encoding]::UTF8.GetBytes('test-forge.exe'))
    $archive = Join-Path $source $assetName
    Compress-Archive -LiteralPath $payload -DestinationPath $archive -Force
    Remove-Item -LiteralPath $payload
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    $sums = Join-Path $source 'SHA256SUMS'
    [IO.File]::WriteAllText($sums, "$hash  $assetName" + [Environment]::NewLine)

    $env:FORGE_INSTALL_DIR = $bin
    Remove-Item Env:FORGE_VERSION -ErrorAction SilentlyContinue
    . $installer

    $installed = Join-Path $bin 'forge.exe'
    if (-not (Test-Path -LiteralPath $installed)) { throw 'Installer did not create forge.exe' }
    if ([IO.File]::ReadAllText($installed) -ne 'test-forge.exe') { throw 'Installed binary does not match archive' }

    # Failed checksum must never overwrite an existing installation.
    [IO.File]::WriteAllText($sums, ("0" * 64) + "  $assetName" + [Environment]::NewLine)
    $rejected = $false
    try { . $installer } catch { $rejected = $_.Exception.Message -like '*Checksum mismatch*' }
    if (-not $rejected) { throw 'Installer accepted an invalid checksum' }
    if ([IO.File]::ReadAllText($installed) -ne 'test-forge.exe') { throw 'Corrupt download changed the installed binary' }

    Write-Host 'PASS Windows installer: latest release, SHA-256, ZIP extraction, PATH, corrupt archive protection'
}
finally {
    if ($null -eq $originalVersion) { Remove-Item Env:FORGE_VERSION -ErrorAction SilentlyContinue }
    else { $env:FORGE_VERSION = $originalVersion }
    if ($null -eq $originalDir) { Remove-Item Env:FORGE_INSTALL_DIR -ErrorAction SilentlyContinue }
    else { $env:FORGE_INSTALL_DIR = $originalDir }
    [Environment]::SetEnvironmentVariable('Path', $originalUserPath, 'User')
    $env:Path = $originalProcessPath
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
