# ==============================================================================
# Klip Windows Production Installer & Environment Setup
# Zero-friction setup: installs binary, configures persistent PATH & Auto-Start
# ==============================================================================
[CmdletBinding()]
param (
    [string]$SourcePath = "",
    [switch]$SkipAutoStart = $false
)

$ErrorActionPreference = "Stop"

Write-Host @"
 _    _ _       
| | _| (_)_ __  
| |/ / | | '_ \ 
|   <| | | |_) |
|_|\_\_|_| .__/ 
         |_|    
End-to-End Encrypted Clipboard Sync
"@ -ForegroundColor Cyan

# 1. Setup Destination Path
$InstallDir = Join-Path $env:LOCALAPPDATA "Klip\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Write-Host "[✓] Created installation directory: $InstallDir" -ForegroundColor Green
}

$KlipExe = Join-Path $InstallDir "klip.exe"

# 2. Locate or Download Binary
$BinaryFound = $false

if ($SourcePath -ne "" -and (Test-Path $SourcePath)) {
    Write-Host "[INFO] Copying provided binary from $SourcePath..." -ForegroundColor Cyan
    Copy-Item -Path $SourcePath -Destination $KlipExe -Force
    $BinaryFound = $true
} elseif ($MyInvocation.MyCommand.Path) {
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $RepoRoot = Split-Path -Parent $ScriptDir
    $ReleaseCandidate = Join-Path $RepoRoot "target\release\klip.exe"
    $DebugCandidate = Join-Path $RepoRoot "target\debug\klip.exe"

    if (Test-Path $ReleaseCandidate) {
        Write-Host "[INFO] Using compiled release binary from $ReleaseCandidate" -ForegroundColor Cyan
        Copy-Item -Path $ReleaseCandidate -Destination $KlipExe -Force
        $BinaryFound = $true
    } elseif (Test-Path $DebugCandidate) {
        Write-Host "[INFO] Using compiled debug binary from $DebugCandidate" -ForegroundColor Cyan
        Copy-Item -Path $DebugCandidate -Destination $KlipExe -Force
        $BinaryFound = $true
    } elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "[INFO] Building klip for Windows from source using cargo..." -ForegroundColor Cyan
        Push-Location $RepoRoot
        try {
            cargo build --release --bin klip
            if (Test-Path $ReleaseCandidate) {
                Copy-Item -Path $ReleaseCandidate -Destination $KlipExe -Force
                $BinaryFound = $true
            }
        } finally {
            Pop-Location
        }
    }
}

if (-not $BinaryFound) {
    Write-Host "[INFO] Downloading latest Klip release binary from GitHub..." -ForegroundColor Cyan
    $DownloadUrl = "https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-windows-x86_64.exe"

    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
        $ReleasesApi = "https://api.github.com/repos/Tribal-Chief-001/Klip/releases"
        $Releases = Invoke-RestMethod -Uri $ReleasesApi -UseBasicParsing -Headers @{ "User-Agent" = "Klip-Installer" } -ErrorAction SilentlyContinue
        if ($Releases -and $Releases.Count -gt 0) {
            $LatestRelease = $Releases[0]
            $Asset = $LatestRelease.assets | Where-Object { $_.name -like "*windows*.exe" } | Select-Object -First 1
            if ($Asset -and $Asset.browser_download_url) {
                $DownloadUrl = $Asset.browser_download_url
            }
        }
    } catch {
        # Fallback to direct release download URL
    }

    Write-Host "[INFO] Downloading binary from $DownloadUrl..." -ForegroundColor Cyan
    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $KlipExe -UseBasicParsing
        if (Test-Path $KlipExe) {
            $BinaryFound = $true
        }
    } catch {
        Write-Warning "Could not download GitHub release binary: $_"
    }
}

if (-not (Test-Path $KlipExe)) {
    Write-Error "Failed to install klip.exe. Please ensure a binary or cargo compiler is available."
    exit 1
}

Write-Host "[✓] Installed klip.exe to $KlipExe" -ForegroundColor Green

# 3. Add to User PATH persistently
$UserPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
$PathEntries = $UserPath -split ';' | Where-Object { $_ -ne "" }

if ($PathEntries -notcontains $InstallDir) {
    Write-Host "[INFO] Adding $InstallDir to persistent User PATH..." -ForegroundColor Cyan
    $NewPath = ($PathEntries + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $NewPath, [EnvironmentVariableTarget]::User)
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "[✓] User PATH updated" -ForegroundColor Green
} else {
    Write-Host "[✓] PATH already contains $InstallDir" -ForegroundColor Green
}

# 4. Configure Auto-start via Windows Registry
if (-not $SkipAutoStart) {
    Write-Host "[INFO] Registering Klip in Windows startup registry..." -ForegroundColor Cyan
    $RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    $RunValue = "`"$KlipExe`" start"
    Set-ItemProperty -Path $RunKey -Name "Klip" -Value $RunValue
    Write-Host "[✓] Registered startup key: Klip -> $RunValue" -ForegroundColor Green
}

# 5. Launch Background Daemon & Verify
Write-Host "[INFO] Starting Klip background daemon..." -ForegroundColor Cyan
Start-Process -FilePath $KlipExe -ArgumentList "start" -WindowStyle Hidden
Start-Sleep -Seconds 2

Write-Host "[INFO] Querying status..." -ForegroundColor Cyan
try {
    & $KlipExe status
} catch {
    Write-Warning "Daemon is initializing in the background. You can check status anytime with: klip status"
}

Write-Host ""
Write-Host "[✓] Klip installed and configured successfully!" -ForegroundColor Green
Write-Host "  - CLI Piping:     Get-Content file.txt | klip copy   &&   klip paste"
Write-Host "  - Interactive UI: klip"
Write-Host "  - Pairing:        klip pair"
Write-Host "  - Status:         klip status"
Write-Host "  - Stop Daemon:    klip stop"
