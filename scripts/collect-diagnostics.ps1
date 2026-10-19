# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

# Launch a new FL Studio process with opt-in plug-in diagnostics.
param([string]$FlStudioPath)
$ErrorActionPreference = 'Stop'
if (Get-Process -Name FL64, FL -ErrorAction SilentlyContinue) {
    throw 'Close all FL Studio windows before running this script.'
}
if (-not $FlStudioPath) {
    $candidates = @(Get-ChildItem -Path "$env:ProgramFiles\Image-Line\FL Studio*\FL64.exe" -ErrorAction SilentlyContinue)
    if ($candidates.Count -eq 1) {
        $FlStudioPath = $candidates[0].FullName
    } else {
        $FlStudioPath = Read-Host 'Enter the full path to FL64.exe'
    }
}
if (-not (Test-Path -LiteralPath $FlStudioPath -PathType Leaf)) {
    throw 'FL Studio executable not found.'
}
$logDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('sample-vsti-diagnostics-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $logDirectory | Out-Null
$previousDirectory = $env:SAMPLE_VSTI_DIAGNOSTICS_DIR
try {
    $env:SAMPLE_VSTI_DIAGNOSTICS_DIR = $logDirectory
    Write-Host "Diagnostics directory: $logDirectory"
    Write-Host 'Load Sample VSTi, play a note, then remove the plug-in and close FL Studio.'
    $process = Start-Process -FilePath $FlStudioPath -PassThru
    $process.WaitForExit()
} finally {
    $env:SAMPLE_VSTI_DIAGNOSTICS_DIR = $previousDirectory
}
Write-Host "Diagnostic reports: $logDirectory"
Get-ChildItem -LiteralPath $logDirectory -Filter '*.txt' | Select-Object -ExpandProperty FullName
