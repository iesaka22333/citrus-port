# CitrusPort build and test script for Windows PowerShell.
# Run with --help for usage.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent $ScriptDir
Set-Location -LiteralPath $RootDir

$Platform = "native"
$BuildProfile = "debug"
$TargetOverride = ""
$RunTests = $false
$TestOnly = $false
$IncludeIgnored = $false
$Offline = $false
$DoClean = $false
$Report = Join-Path $RootDir "test-results.txt"
$ReportInitialized = $false
$OverallExit = 0
$IsWindowsHost = ($env:OS -eq "Windows_NT")
$ScriptArgs = @($args)

function Show-Usage {
    $text = @'
CitrusPort build and test script (Windows PowerShell)

Usage:
  .\scripts\build.ps1 [options]

Options:
  --platform <native|windows|linux|all>  Target platform (default: native)
  --profile <debug|release|both>         Build profile (default: debug)
                                         --mode is accepted as an alias.
  --target <triple>                      Override the Rust target triple.
                                         Only valid with a single platform.
  --test                                 Run tests for each selected platform/profile.
  --test-only                            Run tests without building first.
  --ignored                              Also run #[ignore] tests (performance samples).
  --offline                              Pass --offline to cargo.
  --report <path>                        Test report path (default: test-results.txt).
  --clean                                Run cargo clean before building/testing.
  -h, --help                             Show this help.

Examples:
  .\scripts\build.ps1 --platform windows --profile debug
  .\scripts\build.ps1 --platform windows --profile both --test --offline
  .\scripts\build.ps1 --platform linux --profile release --target x86_64-unknown-linux-gnu
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
    elseif ($arg -eq "--test") {
        $RunTests = $true
    }
    elseif ($arg -eq "--test-only") {
        $RunTests = $true
        $TestOnly = $true
    }
    elseif ($arg -eq "--ignored") {
        $IncludeIgnored = $true
    }
    elseif ($arg -eq "--offline") {
        $Offline = $true
    }
    elseif ($arg -eq "--report") {
        $i++
        Require-Value "--report" $i
        $Report = [string]$ScriptArgs[$i]
    }
    elseif ($arg -like "--report=*") {
        $Report = $arg.Substring("--report=".Length)
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
if (-not [System.IO.Path]::IsPathRooted($Report)) {
    $Report = Join-Path $RootDir $Report
}

function Get-HostTriple {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        return "unknown"
    }
    $line = (& rustc -vV 2>$null) |
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

function Ensure-Target {
    param([string]$Target)
    if ($Target -eq "") {
        return
    }
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        $installed = @(& rustup target list --installed 2>$null)
        if ($installed -notcontains $Target) {
            Fail "Rust target '$Target' is not installed. Run: rustup target add $Target"
        }
    }
    else {
        Write-Warning "rustup not found; cannot verify target '$Target'."
    }
    if ($Target -like "*windows*" -and -not (Get-Command x86_64-w64-mingw32-gcc -ErrorAction SilentlyContinue)) {
        Write-Warning "x86_64-w64-mingw32-gcc not found; Windows cross-linking may fail."
    }
}

function Invoke-CargoStep {
    param([string[]]$CargoArgs)
    Write-Host "==> cargo $($CargoArgs -join ' ')"
    $cargoLines = @()
    $previousColor = $env:CARGO_TERM_COLOR
    $env:CARGO_TERM_COLOR = "never"
    try {
        & cargo @CargoArgs 2>&1 | Tee-Object -Variable cargoLines
        $code = if ($null -eq $LASTEXITCODE) { 1 } else { [int]$LASTEXITCODE }
    }
    finally {
        if ($null -eq $previousColor) {
            Remove-Item Env:CARGO_TERM_COLOR -ErrorAction SilentlyContinue
        }
        else {
            $env:CARGO_TERM_COLOR = $previousColor
        }
    }
    return [pscustomobject]@{
        ExitCode = $code
        Output = (($cargoLines | Out-String -Width 4096))
    }
}

function Get-TestCounts {
    param([string]$Text)
    $passed = 0
    $failed = 0
    $ignored = 0
    $measured = 0
    foreach ($line in ($Text -split "`r?`n")) {
        if ($line -match '^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured') {
            $passed += [int]$Matches[2]
            $failed += [int]$Matches[3]
            $ignored += [int]$Matches[4]
            $measured += [int]$Matches[5]
        }
    }
    return [pscustomobject]@{
        Passed = $passed
        Failed = $failed
        Ignored = $ignored
        Measured = $measured
    }
}

function Initialize-Report {
    if ($script:ReportInitialized) {
        return
    }
    $parent = Split-Path -Parent $script:Report
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $rustVersion = if (Get-Command rustc -ErrorAction SilentlyContinue) { & rustc --version } else { "unknown" }
    $cargoVersion = if (Get-Command cargo -ErrorAction SilentlyContinue) { & cargo --version } else { "unknown" }
    $osDescription = [System.Environment]::OSVersion.VersionString
    $header = @(
        "CitrusPort Test Report",
        "======================",
        "Generated (UTC): $((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))",
        "Host OS: $osDescription",
        "Repository: $script:RootDir",
        "PowerShell: $($PSVersionTable.PSVersion)",
        "Cargo: $cargoVersion",
        "Rustc: $rustVersion",
        ""
    )
    Set-Content -Path $script:Report -Value $header -Encoding UTF8
    $script:ReportInitialized = $true
}

function Add-TestReport {
    param(
        [string]$PlatformName,
        [string]$Target,
        [string]$ProfileName,
        [string]$Command,
        [object]$Result
    )

    $counts = Get-TestCounts $Result.Output
    $status = if ($Result.ExitCode -ne 0 -or $counts.Failed -gt 0) { "FAIL" } else { "PASS" }
    $targetLabel = if ($Target -ne "") { $Target } else { Get-HostTriple }

    Initialize-Report

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("------------------------------------------------------------")
    $lines.Add("Platform: $PlatformName")
    $lines.Add("Target: $targetLabel")
    $lines.Add("Profile: $ProfileName")
    $lines.Add("Command: $Command")
    $lines.Add("Exit code: $($Result.ExitCode)")
    $lines.Add("Status: $status")
    $lines.Add("Summary: $($counts.Passed) passed, $($counts.Failed) failed, $($counts.Ignored) ignored, $($counts.Measured) measured")
    $lines.Add("Test result lines:")
    foreach ($line in ($Result.Output -split "`r?`n")) {
        if ($line -match 'Running |test result: |error(\[|:)') {
            $lines.Add("  $line")
        }
    }
    $lines.Add("")
    $lines.Add("Raw output:")
    foreach ($line in ($Result.Output -split "`r?`n")) {
        $lines.Add("  $line")
    }
    $lines.Add("")
    Add-Content -Path $script:Report -Value $lines.ToArray() -Encoding UTF8
}

function Invoke-TestRun {
    param(
        [string]$PlatformName,
        [string]$Target,
        [string]$ProfileLabel,
        [string[]]$CargoArgs
    )
    $test = Invoke-CargoStep -CargoArgs $CargoArgs
    Add-TestReport -PlatformName $PlatformName -Target $Target -ProfileName $ProfileLabel `
        -Command ("cargo " + ($CargoArgs -join " ")) -Result $test
    if ($test.ExitCode -ne 0) {
        $script:OverallExit = 1
    }
}

$specs = @(Get-TargetSpecs)
$profiles = @(Get-Profiles)

if ($DoClean) {
    Write-Host "==> cargo clean"
    & cargo clean
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

foreach ($spec in $specs) {
    $parts = $spec -split '\|', 2
    $platformName = $parts[0]
    $target = $parts[1]
    Ensure-Target $target

    foreach ($profileName in $profiles) {
        if (-not $TestOnly) {
            $cargoArgs = @("build", "--locked")
            if ($profileName -eq "release") {
                $cargoArgs += "--release"
            }
            if ($target -ne "") {
                $cargoArgs += @("--target", $target)
            }
            if ($Offline) {
                $cargoArgs += "--offline"
            }
            $build = Invoke-CargoStep -CargoArgs $cargoArgs
            if ($build.ExitCode -ne 0) {
                Write-Host "error: build failed with exit code $($build.ExitCode)." -ForegroundColor Red
                $global:LASTEXITCODE = $build.ExitCode
                exit $build.ExitCode
            }
        }

        if ($RunTests) {
            $baseArgs = @("test", "--locked")
            if ($profileName -eq "release") {
                $baseArgs += "--release"
            }
            if ($target -ne "") {
                $baseArgs += @("--target", $target)
            }
            if ($Offline) {
                $baseArgs += "--offline"
            }

            Invoke-TestRun -PlatformName $platformName -Target $target `
                -ProfileLabel $profileName -CargoArgs $baseArgs

            if ($IncludeIgnored) {
                $ignoredArgs = $baseArgs + @("--", "--ignored", "--nocapture")
                Invoke-TestRun -PlatformName $platformName -Target $target `
                    -ProfileLabel "$profileName (ignored tests)" -CargoArgs $ignoredArgs
            }
        }
    }
}

$global:LASTEXITCODE = $script:OverallExit
exit $script:OverallExit
