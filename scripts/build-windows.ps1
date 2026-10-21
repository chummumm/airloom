param([ValidateSet('Debug','Release')][string]$Configuration = 'Release')
$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $ProjectRoot
try {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust with the MSVC toolchain first.' }
    if (-not (Get-Command msbuild -ErrorAction SilentlyContinue)) { throw 'Run in a Visual Studio 2022 Developer PowerShell with C++ desktop tools and Windows SDK 26100.' }
    & cargo fetch --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust dependency download failed.' }
    $RustArgs = @('build','-p','airloom-core','--target','x86_64-pc-windows-msvc','--locked')
    $RustProfile = 'debug'
    if ($Configuration -eq 'Release') { $RustArgs += '--release'; $RustProfile = 'release' }
    & cargo @RustArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust core build failed.' }
    & nuget restore native/Airloom/Airloom.sln -NonInteractive
    if ($LASTEXITCODE -ne 0) { throw 'Native NuGet restore failed.' }
    & msbuild native/Airloom/Airloom.sln /m /p:Configuration=$Configuration /p:Platform=x64 /verbosity:minimal
    if ($LASTEXITCODE -ne 0) { throw 'WinUI application build failed.' }
    $OutputPath = Join-Path $ProjectRoot "native/Airloom/bin/x64/$Configuration"
    if (-not (Test-Path (Join-Path $OutputPath 'Airloom.exe'))) { throw 'Native executable not found at expected output path.' }
    $VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $VsInstall = & $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $CrtPath = Get-ChildItem (Join-Path $VsInstall 'VC/Redist/MSVC') -Directory |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64/Microsoft.VC143.CRT' } |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $CrtPath) { throw 'Redistributable Visual C++ x64 runtime files not found.' }
    Copy-Item (Join-Path $CrtPath '*.dll') $OutputPath -Force
    Copy-Item "target/x86_64-pc-windows-msvc/$RustProfile/airloom_core.dll" $OutputPath -Force
    Copy-Item LICENSE $OutputPath -Force
    Copy-Item THIRD_PARTY_NOTICES.md $OutputPath -Force
    & python scripts/collect-licenses.py --output (Join-Path $OutputPath 'THIRD_PARTY_LICENSES.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Dependency license collection failed.' }
    New-Item -ItemType Directory -Path dist -Force | Out-Null
    Compress-Archive -Path "$OutputPath/*" -DestinationPath dist/airloom-windows-x64-portable.zip -Force
    Write-Host "Built native app: $OutputPath/Airloom.exe"
} finally { Pop-Location }

