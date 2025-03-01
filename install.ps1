<#
.SYNOPSIS
    Installer script for circe on Windows
.DESCRIPTION
    Downloads and installs circe binary for Windows
.PARAMETER Version
    The version to install (default: latest)
.PARAMETER BinDir
    The directory to install circe to (default: $env:USERPROFILE\.circe\bin)
.PARAMETER TempDir
    The temporary directory to use for downloads (default: $env:TEMP)
.EXAMPLE
    # Install latest version
    iwr -useb https://raw.githubusercontent.com/fossas/circe/main/install.ps1 | iex
.EXAMPLE
    # Install specific version
    $v="v0.5.0"; iwr -useb https://raw.githubusercontent.com/fossas/circe/main/install.ps1 | iex
.EXAMPLE
    # Install to a specific directory
    $BinDir="C:\tools"; iwr -useb https://raw.githubusercontent.com/fossas/circe/main/install.ps1 | iex
.NOTES
    For versions v0.4.0 and earlier, please use the installer attached to the specific
    GitHub release: https://github.com/fossas/circe/releases/tag/vX.Y.Z
#>

param(
    [string]$Version = "",
    [string]$BinDir = "",
    [string]$TempDir = ""
)

# Set error action preference
$ErrorActionPreference = "Stop"

# Define colors for output
function Write-InfoMsg {
    param([string]$Message)
    Write-Host $Message -ForegroundColor Green
}

function Write-WarnMsg {
    param([string]$Message)
    Write-Host "Warning: $Message" -ForegroundColor Yellow
}

function Write-ErrorMsg {
    param([string]$Message)
    Write-Host "Error: $Message" -ForegroundColor Red
    exit 1
}

# Set defaults if not provided
if ([string]::IsNullOrEmpty($BinDir)) {
    $BinDir = Join-Path $env:USERPROFILE ".circe\bin"
}

if ([string]::IsNullOrEmpty($TempDir)) {
    $TempDir = $env:TEMP
}

# Create working directory
$WorkDir = Join-Path $TempDir "circe-install-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null

# Detect platform
$Arch = "x86_64" # Default to x86_64, most Windows installs
if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    Write-WarnMsg "ARM64 Windows is not officially supported yet"
    $Arch = "aarch64"
}
$Platform = "$Arch-pc-windows-msvc"
Write-InfoMsg "Detected platform: $Platform"

# Check if version is specified and warn about older versions
if (![string]::IsNullOrEmpty($Version)) {
    if ($Version -eq "v0.4.0" -or $Version -lt "v0.4.0") {
        Write-WarnMsg "You're installing version $Version, which may not be compatible with this installer."
        Write-WarnMsg "For versions v0.4.0 and earlier, please use the installer attached to the GitHub release:"
        Write-WarnMsg "https://github.com/fossas/circe/releases/tag/$Version"
        Write-WarnMsg "Continuing anyway, but installation may fail."
        
        $confirmation = Read-Host "Do you want to continue anyway? [y/N]"
        if ($confirmation -ne "y" -and $confirmation -ne "Y") {
            Write-Host "Installation cancelled"
            exit 1
        }
    }
}

try {
    # Get latest version if not specified
    if ([string]::IsNullOrEmpty($Version)) {
        Write-InfoMsg "Getting latest version..."
        $Result = Invoke-RestMethod -Uri "https://api.github.com/repos/fossas/circe/releases/latest" -Method Get
        $Version = $Result.tag_name
        Write-InfoMsg "Latest version is $Version"
    }

    # Construct URLs
    $ArchiveName = "circe-$Platform.zip"
    $DownloadUrl = "https://github.com/fossas/circe/releases/download/$Version/$ArchiveName"
    $ChecksumsUrl = "https://github.com/fossas/circe/releases/download/$Version/checksums.txt"

    # Download files
    Write-InfoMsg "Downloading $DownloadUrl"
    $ArchivePath = Join-Path $WorkDir $ArchiveName
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath
    
    Write-InfoMsg "Downloading checksums from $ChecksumsUrl"
    $ChecksumsPath = Join-Path $WorkDir "checksums.txt"
    Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $ChecksumsPath

    # Verify checksum
    Write-InfoMsg "Verifying checksum..."
    $Checksums = Get-Content $ChecksumsPath
    $ExpectedChecksum = ($Checksums | Where-Object { $_ -match $ArchiveName -and $_ -match "([a-fA-F0-9]{64})" } | 
                        ForEach-Object { $Matches[1] })[0]
    
    if ([string]::IsNullOrEmpty($ExpectedChecksum)) {
        Write-ErrorMsg "Couldn't find checksum for $ArchiveName"
    }

    $ActualChecksum = (Get-FileHash -Algorithm SHA256 -Path $ArchivePath).Hash.ToLower()
    
    if ($ActualChecksum -ne $ExpectedChecksum.ToLower()) {
        Write-ErrorMsg "Checksum verification failed! Expected: $ExpectedChecksum, got: $ActualChecksum"
    }
    
    # Extract archive
    Write-InfoMsg "Extracting archive..."
    $ExtractPath = Join-Path $WorkDir "extracted"
    New-Item -ItemType Directory -Path $ExtractPath -Force | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractPath
    
    # Create bin directory if it doesn't exist
    if (-not (Test-Path $BinDir)) {
        New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
        Write-InfoMsg "Created directory: $BinDir"
    }
    
    # Find and copy binary
    $BinaryPath = Get-ChildItem -Path $ExtractPath -Recurse -Filter "circe.exe" | Select-Object -First 1 -ExpandProperty FullName
    if ([string]::IsNullOrEmpty($BinaryPath)) {
        Write-ErrorMsg "Could not find circe.exe in the extracted archive"
    }
    
    Copy-Item -Path $BinaryPath -Destination (Join-Path $BinDir "circe.exe") -Force
    Write-InfoMsg "Installed circe to $(Join-Path $BinDir "circe.exe")"
    
    # Check if bin dir is in PATH
    $UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if (-not $UserPath.Contains($BinDir)) {
        Write-WarnMsg "$BinDir is not in your PATH"
        $AddToPath = Read-Host "Do you want to add it to your PATH? [y/N]"
        if ($AddToPath -eq "y" -or $AddToPath -eq "Y") {
            $NewPath = "$UserPath;$BinDir"
            [Environment]::SetEnvironmentVariable("PATH", $NewPath, "User")
            Write-InfoMsg "Added $BinDir to your PATH (requires a new terminal session to take effect)"
        } else {
            Write-WarnMsg "You'll need to add $BinDir to your PATH manually to use circe from any location"
        }
    }
    
    Write-InfoMsg "Installation complete! Run 'circe --help' to get started."
} 
catch {
    Write-ErrorMsg "Installation failed: $_"
}
finally {
    # Clean up
    if (Test-Path $WorkDir) {
        Remove-Item -Recurse -Force $WorkDir
    }
}