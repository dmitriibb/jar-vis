# Copy an existing Windows release without building it or removing any files.
# Run from any directory: powershell -ExecutionPolicy Bypass -File <path>/copy-release.ps1
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$tauriDir = Join-Path $PSScriptRoot 'src-tauri'
$destinationDir = Join-Path $PSScriptRoot 'release-app'
$bundleConfig = Get-Content -LiteralPath (Join-Path $tauriDir 'bundle.windows.json') -Raw | ConvertFrom-Json

# Use the same runtime DLL list as the Windows installer. Frontend assets and
# Rust application code are compiled into jar-vis.exe; build libraries aren't needed.
$files = @(
    [pscustomobject]@{
        Source = Join-Path $tauriDir 'target/release/jar-vis.exe'
        Destination = Join-Path $destinationDir 'jar-vis.exe'
    }
    foreach ($resource in $bundleConfig.bundle.resources.PSObject.Properties) {
        [pscustomobject]@{
            Source = Join-Path $tauriDir $resource.Name
            Destination = Join-Path $destinationDir $resource.Value
        }
    }
)

# Check the entire input set before creating or updating the destination.
$missingFiles = @($files | Where-Object { -not (Test-Path -LiteralPath $_.Source -PathType Leaf) })
if ($missingFiles.Count -gt 0) {
    throw ("Release files are missing. Build with 'npm run bundle:windows' in assistant/app first.`n" +
        (($missingFiles | ForEach-Object { $_.Source }) -join "`n"))
}

[long]$totalBytes = 0
foreach ($file in $files) {
    $parentDir = Split-Path -Parent $file.Destination
    New-Item -ItemType Directory -Path $parentDir -Force | Out-Null
    Copy-Item -LiteralPath $file.Source -Destination $file.Destination -Force
    $totalBytes += (Get-Item -LiteralPath $file.Destination).Length
    Write-Host ('Copied {0}' -f (Split-Path -Leaf $file.Destination))
}

Write-Host ("`nCopied {0} files ({1:N2} MB) to {2}" -f $files.Count, ($totalBytes / 1000000), $destinationDir)
Write-Host 'Launch release-app\jar-vis.exe. Existing settings and home-folder data are reused.'
Write-Host 'Keep your models, assistant/agents, assistant/skills, assistant/pets and sessions folders.'
