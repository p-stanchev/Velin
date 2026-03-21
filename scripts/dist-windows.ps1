param(
    [string]$OutputDir = "dist"
)

$ErrorActionPreference = "Stop"

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($Arguments -join ' ')"
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $repoRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is required but was not found in PATH."
}

if (-not (Get-Command cargo-wix -ErrorAction SilentlyContinue)) {
    throw "cargo-wix is required. Install it with: cargo install cargo-wix"
}

$wixMain = Join-Path $repoRoot "crates\velin-app\wix\main.wxs"
if (-not (Test-Path $wixMain)) {
    Write-Host "Initializing WiX configuration..."
    Invoke-CheckedCommand cargo @("wix", "init", "-p", "velin-app", "--product-name", "Velin", "--manufacturer", "p-stanchev")
}

$versionLine = Select-String -Path (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "(.+)"$' | Select-Object -First 1
if (-not $versionLine) {
    throw "Could not determine workspace version from Cargo.toml."
}
$version = $versionLine.Matches[0].Groups[1].Value

$absoluteOutputDir = Join-Path $repoRoot $OutputDir
New-Item -ItemType Directory -Force -Path $absoluteOutputDir | Out-Null

Write-Host "Building release binary..."
Invoke-CheckedCommand cargo @("build", "-p", "velin-app", "--release")

$binaryPath = Join-Path $repoRoot "target\release\velin-app.exe"
$portablePath = Join-Path $absoluteOutputDir "Velin-$version-x64.exe"
Copy-Item -Force $binaryPath $portablePath

$installerPath = Join-Path $absoluteOutputDir "Velin-$version-x64.msi"
Write-Host "Building MSI installer at $installerPath ..."
Invoke-CheckedCommand cargo @("wix", "-p", "velin-app", "--nocapture", "--output", $installerPath)

Write-Host "Done: $installerPath"
Write-Host "Portable binary: $portablePath"
