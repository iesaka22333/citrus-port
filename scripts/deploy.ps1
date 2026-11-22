# CitrusPort deployment packaging script for Windows PowerShell.
# Builds (optionally) and packages platform archives into dist/.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent $ScriptDir
Set-Location -LiteralPath $RootDir

$Platform = "native"
$BuildProfile = "release"
$TargetOverride = ""
$OutDir = Join-Path $RootDir "dist"
$SkipBuild = $false
$Offline = $false
$DoClean = $false
$IsWindowsHost = ($env:OS -eq "Windows_NT")
$ScriptArgs = @($args)

function Show-Usage {
    $text = @'
CitrusPort deployment packaging script (Windows PowerShell)

Usage:
  .\scripts\deploy.ps1 [options]

Options:
  --platform <native|windows|linux|all>  Target platform (default: native)
  --profile <debug|release|both>         Build profile (default: release)
  --target <triple>                      Override the Rust target triple.
  --out <directory>                      Output directory (default: dist)
  --skip-build                           Package existing artifacts only.
  --offline                              Pass --offline to cargo when building.
  --clean                                Run cargo clean before building.
  -h, --help                             Show this help.

Outputs:
  Windows: dist/citrus-port-<version>-<target>-<profile>.zip
  Linux:   dist/citrus-port-<version>-<target>-<profile>.tar.gz
  Also writes dist/SHA256SUMS.

Examples:
  .\scripts\deploy.ps1 --platform windows --profile release --offline
  .\scripts\deploy.ps1 --platform linux --profile release --target x86_64-unknown-linux-gnu
'@
    Write-Host $text
}

function Fail {
    param([string]$Message)
    Write-Host "error: $Message" -ForegroundColor Red
    $global:LASTEXITCODE = 2
    exit 2
}

function Require-Value {
    param([string]$Option, [int]$Index)
    if ($Index -ge $script:ScriptArgs.Count -or [string]::IsNullOrEmpty([string]$script:ScriptArgs[$Index])) {
        Fail "$Option requires a value"
    }
}

for ($i = 0; $i -lt $ScriptArgs.Count; $i++) {
    $arg = [string]$ScriptArgs[$i]
    if ($arg -eq "--platform") {
        $i++
        Require-Value "--platform" $i
        $Platform = [string]$ScriptArgs[$i]
    }
    elseif ($arg -like "--platform=*") {
        $Platform = $arg.Substring("--platform=".Length)
    }
    elseif ($arg -eq "--profile" -or $arg -eq "--mode") {
        $i++
        Require-Value $arg $i
        $BuildProfile = [string]$ScriptArgs[$i]
    }
    elseif ($arg -like "--profile=*") {
        $BuildProfile = $arg.Substring("--profile=".Length)
    }
    elseif ($arg -like "--mode=*") {
        $BuildProfile = $arg.Substring("--mode=".Length)
    }
    elseif ($arg -eq "--target") {
        $i++
        Require-Value "--target" $i
        $TargetOverride = [string]$ScriptArgs[$i]
    }
    elseif ($arg -like "--target=*") {
        $TargetOverride = $arg.Substring("--target=".Length)
    }
    elseif ($arg -eq "--out") {
        $i++
        Require-Value "--out" $i
        $OutDir = [string]$ScriptArgs[$i]
    }
    elseif ($arg -like "--out=*") {
        $OutDir = $arg.Substring("--out=".Length)
    }
    elseif ($arg -eq "--skip-build") {
        $SkipBuild = $true
    }
    elseif ($arg -eq "--offline") {
        $Offline = $true
    }
    elseif ($arg -eq "--clean") {
        $DoClean = $true
    }
    elseif ($arg -eq "-h" -or $arg -eq "--help") {
        Show-Usage
        exit 0
    }
    else {
        Fail "Unknown option: $arg"
    }
}

if ($Platform -notin @("native", "linux", "windows", "all")) {
    Fail "--platform must be one of: native, linux, windows, all"
}
if ($BuildProfile -notin @("debug", "release", "both")) {
    Fail "--profile must be one of: debug, release, both"
}
if ($TargetOverride -ne "" -and $Platform -eq "all") {
    Fail "--target cannot be combined with --platform all"
}
if (-not [System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir = Join-Path $RootDir $OutDir
}

function Invoke-NativeCommand {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [switch]$Echo
    )

    $outputLines = @()
    $previousErrorAction = $ErrorActionPreference
    $previousNativeErrorAction = $null
    $hasNativePreference = $null -ne (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue)
    if ($hasNativePreference) {
        $previousNativeErrorAction = $PSNativeCommandUseErrorActionPreference
        $PSNativeCommandUseErrorActionPreference = $false
    }

    # Native stderr is normal for tools such as tar. Keep it from becoming a
    # terminating error because the script sets ErrorActionPreference = Stop.
    $ErrorActionPreference = "Continue"
    $exitCode = 1
    try {
        if ($Echo) {
            & $FilePath @Arguments 2>&1 | Tee-Object -Variable outputLines | Out-Host
        }
        else {
            $outputLines = @(& $FilePath @Arguments 2>&1)
        }
        $exitCode = if ($null -eq $LASTEXITCODE) { 1 } else { [int]$LASTEXITCODE }
    }
    finally {
        $ErrorActionPreference = $previousErrorAction
        if ($hasNativePreference) {
            $PSNativeCommandUseErrorActionPreference = $previousNativeErrorAction
        }
    }

    $text = (@($outputLines) | ForEach-Object { [string]$_ }) -join "`n"
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output = $text
    }
}

function Get-HostTriple {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        return "unknown"
    }
    $result = Invoke-NativeCommand -FilePath "rustc" -Arguments @("-vV")
    if ($result.ExitCode -ne 0) {
        return "unknown"
    }
    $line = ($result.Output -split "`r?`n") |
        Where-Object { $_ -match '^host:\s*' } |
        Select-Object -First 1
    if ($null -eq $line) {
        return "unknown"
    }
    return ($line -replace '^host:\s*', '').Trim()
}

function Get-TargetSpecs {
    $linuxTarget = if ($IsWindowsHost) { "x86_64-unknown-linux-gnu" } else { "" }
    $windowsTarget = if ($IsWindowsHost) { "" } else { "x86_64-pc-windows-gnu" }

    $specs = @()
    switch ($Platform) {
        "native" { $specs = @("native|") }
        "linux" { $specs = @("linux|$linuxTarget") }
        "windows" { $specs = @("windows|$windowsTarget") }
        "all" { $specs = @("linux|$linuxTarget", "windows|$windowsTarget") }
    }

    if ($TargetOverride -ne "") {
        $specs = @("$Platform|$TargetOverride")
    }
    return $specs
}

function Get-Profiles {
    switch ($BuildProfile) {
        "debug" { return @("debug") }
        "release" { return @("release") }
        "both" { return @("debug", "release") }
    }
    return @()
}

function Get-ArchiveKind {
    param([string]$PlatformName, [string]$Target)
    if ($Target -ne "") {
        if ($Target -like "*windows*") { return "zip" }
        if ($Target -like "*linux*") { return "tar.gz" }
    }
    switch ($PlatformName) {
        "windows" { return "zip" }
        "linux" { return "tar.gz" }
        "native" {
            if ($IsWindowsHost) { return "zip" }
            return "tar.gz"
        }
    }
    Fail "unknown platform '$PlatformName'"
}

function Get-ProjectVersion {
    $inPackage = $false
    foreach ($line in Get-Content -LiteralPath (Join-Path $RootDir "Cargo.toml")) {
        if ($line -match '^\s*\[package\]') {
            $inPackage = $true
            continue
        }
        if ($line -match '^\s*\[') {
            $inPackage = $false
            continue
        }
        if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    return "0.0.0"
}

if (-not $SkipBuild) {
    $buildArgs = @("--platform", $Platform, "--profile", $BuildProfile)
    if ($TargetOverride -ne "") {
        $buildArgs += @("--target", $TargetOverride)
    }
    if ($Offline) {
        $buildArgs += "--offline"
    }
    if ($DoClean) {
        $buildArgs += "--clean"
    }
    & (Join-Path $ScriptDir "build.ps1") @buildArgs
    if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

$version = Get-ProjectVersion
$specs = @(Get-TargetSpecs)
$profiles = @(Get-Profiles)

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$checksumFile = Join-Path $OutDir "SHA256SUMS"
Set-Content -Path $checksumFile -Value "" -Encoding ASCII

foreach ($spec in $specs) {
    $parts = $spec -split '\|', 2
    $platformName = $parts[0]
    $target = $parts[1]
    $kind = Get-ArchiveKind $platformName $target
    $label = if ($target -ne "") { $target } else { Get-HostTriple }

    foreach ($profileName in $profiles) {
        $binaryName = if ($kind -eq "zip") { "citrus-port.exe" } else { "citrus-port" }
        $binaryPath = Join-Path $RootDir "target"
        if ($target -ne "") {
            $binaryPath = Join-Path $binaryPath $target
        }
        $binaryPath = Join-Path $binaryPath $profileName
        $binaryPath = Join-Path $binaryPath $binaryName

        if (-not (Test-Path -LiteralPath $binaryPath)) {
            Fail "expected binary not found: $binaryPath. Build it first or omit --skip-build."
        }

        $packageName = "citrus-port-$version-$label-$profileName"
        $stage = [System.IO.Path]::Combine(
            [System.IO.Path]::GetTempPath(),
            ("citrus-port-pkg-" + [Guid]::NewGuid().ToString("N"))
        )
        $packageDir = Join-Path $stage $packageName
        New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
        Copy-Item -LiteralPath $binaryPath -Destination $packageDir
        Copy-Item -LiteralPath (Join-Path $RootDir "README.md") -Destination $packageDir
        Copy-Item -LiteralPath (Join-Path $RootDir "LICENSE") -Destination $packageDir

        if ($kind -eq "zip") {
            $archive = Join-Path $OutDir "$packageName.zip"
            Compress-Archive -Path $packageDir -DestinationPath $archive -Force
        }
        else {
            if (-not (Get-Command tar -ErrorAction SilentlyContinue)) {
                Fail "tar is required to create .tar.gz packages on Windows"
            }
            $archive = Join-Path $OutDir "$packageName.tar.gz"
            $tar = Invoke-NativeCommand -FilePath "tar" `
                -Arguments @("-czf", $archive, "-C", $stage, $packageName) -Echo
            if ($tar.ExitCode -ne 0) {
                Fail "tar packaging failed with exit code $($tar.ExitCode)"
            }
        }

        $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
        Add-Content -Path $checksumFile -Value "$hash  $(Split-Path -Leaf $archive)" -Encoding ASCII
        Write-Host "==> Created $archive"
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
}

Write-Host "==> Checksums written to $checksumFile"
