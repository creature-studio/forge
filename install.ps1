# Install or update the Windows x64 release of Forge.
# Usage: irm https://creature-studio.github.io/forge/install.ps1 | iex
# Overrides: FORGE_VERSION=v0.4.0, FORGE_INSTALL_DIR=C:\path\to\bin
$ErrorActionPreference = 'Stop'

$repo = 'creature-studio/forge'
$asset = 'forge-x86_64-pc-windows-msvc.zip'

if (-not [Environment]::Is64BitOperatingSystem -or
    [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne 'X64') {
    throw 'Forge currently publishes native Windows builds for Windows x64 only.'
}

# Windows PowerShell 5.1 may otherwise negotiate an obsolete TLS protocol.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$tag = $env:FORGE_VERSION
if ([string]::IsNullOrWhiteSpace($tag)) {
    Write-Host 'Checking latest Forge release...'
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers @{ 'User-Agent' = 'forge-windows-installer' }
    $tag = [string]$release.tag_name
}
if ($tag -match '^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
    $tag = "v$tag"
}
if ($tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw "Invalid FORGE_VERSION: $tag"
}

$installDir = $env:FORGE_INSTALL_DIR
if ([string]::IsNullOrWhiteSpace($installDir)) {
    $installDir = Join-Path $env:LOCALAPPDATA 'Programs\Forge\bin'
}
$installDir = [IO.Path]::GetFullPath($installDir)

$tempDir = Join-Path ([IO.Path]::GetTempPath()) ("forge-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
try {
    $base = "https://github.com/$repo/releases/download/$tag"
    $archive = Join-Path $tempDir $asset
    $checksums = Join-Path $tempDir 'SHA256SUMS'
    Write-Host "Downloading Forge $tag ($asset)..."
    Invoke-WebRequest -Uri "$base/$asset" -OutFile $archive -UseBasicParsing
    Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $checksums -UseBasicParsing

    $escapedAsset = [regex]::Escape($asset)
    $pattern = "(?m)^([a-fA-F0-9]{64})\s+\*?$escapedAsset\s*$"
    $match = [regex]::Match([IO.File]::ReadAllText($checksums), $pattern)
    if (-not $match.Success) { throw "$asset not found in release SHA256SUMS" }
    $expected = $match.Groups[1].Value
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    if ($actual -ine $expected) { throw "Checksum mismatch for $asset" }
    Write-Host 'SHA-256 verified.'

    $expanded = Join-Path $tempDir 'expanded'
    Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
    $exe = Join-Path $expanded 'forge.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw 'Release archive does not contain forge.exe'
    }

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    $destination = Join-Path $installDir 'forge.exe'
    try {
        Copy-Item -LiteralPath $exe -Destination $destination -Force -ErrorAction Stop
    } catch {
        throw "Cannot install to $destination. Close any running Forge process and retry. $($_.Exception.Message)"
    }
    Write-Host "Installed Forge $tag to $destination"

    # The user PATH is enough: installation never requires administrator access.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $currentParts = @(($userPath + ';' + $machinePath) -split ';' | Where-Object { $_ })
    if (-not ($currentParts | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })) {
        $newUserPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $installDir } else { "$userPath;$installDir" }
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
        Write-Host 'Added Forge to your user PATH (open a new terminal to use it).'
    }
    # Also make the binary accessible in this PowerShell process if run directly.
    if (-not (@($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })) {
        $env:Path = "$env:Path;$installDir"
    }
    Write-Host 'Run forge from PowerShell or Windows Terminal.'
}
finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
