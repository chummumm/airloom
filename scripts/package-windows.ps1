$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$InnoPath = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
if (-not (Test-Path $InnoPath)) {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) { throw 'Install Inno Setup 6 to build the installer.' }
    & choco install innosetup --yes --no-progress
    if ($LASTEXITCODE -ne 0) { throw 'Could not install Inno Setup.' }
}
& $InnoPath (Join-Path $ProjectRoot 'installer/Airloom.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }

