$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$EvidencePath = Join-Path $ProjectRoot 'artifacts'
New-Item -ItemType Directory -Path $EvidencePath -Force | Out-Null
$ArchivePath = Join-Path $ProjectRoot 'dist/airloom-windows-x64-portable.zip'
if (-not (Test-Path $ArchivePath)) { throw 'Build the portable distribution before running the startup check.' }
# Launch the shipped files from a clean directory, so build-tree resources
# cannot accidentally hide a missing DLL or XAML resource in the archive.
$SmokeRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('Airloom-Smoke-' + [guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $SmokeRoot
$AppPath = Join-Path $SmokeRoot 'Airloom.exe'
$Process = Start-Process -FilePath $AppPath -WorkingDirectory (Split-Path $AppPath) -PassThru
try {
    $Deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 500
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited during startup (code $($Process.ExitCode))." }
    } while ($Process.MainWindowHandle -eq 0 -and [DateTime]::UtcNow -lt $Deadline)
    if ($Process.MainWindowHandle -eq 0) { throw 'No native window appeared within 20 seconds.' }
    if ($Process.MainWindowTitle -ne 'Airloom') { throw "Unexpected startup window: $($Process.MainWindowTitle)" }
    Start-Sleep -Seconds 3
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $Bitmap = New-Object System.Drawing.Bitmap $Screen.Width, $Screen.Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    try {
        $Graphics.CopyFromScreen($Screen.Location, [System.Drawing.Point]::Empty, $Screen.Size)
        $Bitmap.Save((Join-Path $EvidencePath 'native-window.png'), [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $Graphics.Dispose(); $Bitmap.Dispose() }
    @{ result='window-opened'; distribution='portable-zip'; process='Airloom.exe'; hwnd=$Process.MainWindowHandle.ToInt64(); note='UI launch from a clean extracted distribution; no physical HomePod was connected.' } |
        ConvertTo-Json | Set-Content (Join-Path $EvidencePath 'native-smoke.json') -Encoding utf8
} finally {
    if (-not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
}
