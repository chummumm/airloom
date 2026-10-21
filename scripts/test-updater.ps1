$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$TestOutput = Join-Path $ProjectRoot 'artifacts/updater-tests/'
$TestIntermediate = Join-Path $ProjectRoot 'native/Updater/obj/tests/'
$Helper = Join-Path $ProjectRoot 'native/Airloom/bin/x64/Release/Airloom.Update.exe'
if (-not (Test-Path -LiteralPath $Helper)) { throw 'Build the native updater first.' }
& msbuild (Join-Path $ProjectRoot 'native/Updater/Airloom.Update.vcxproj') /m /p:Configuration=Release /p:Platform=x64 /p:UpdaterSource=UpdaterTests.cpp /p:UpdaterSubsystem=Console /p:TargetName=Airloom.Update.Tests "/p:OutDir=$TestOutput" "/p:IntDir=$TestIntermediate" /verbosity:minimal
if ($LASTEXITCODE -ne 0) { throw 'Updater test harness build failed.' }
& (Join-Path $TestOutput 'Airloom.Update.Tests.exe') $Helper
if ($LASTEXITCODE -ne 0) { throw 'Updater lifecycle checks failed.' }
