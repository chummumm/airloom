$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$EvidencePath = Join-Path $ProjectRoot 'artifacts'
New-Item -ItemType Directory -Path $EvidencePath -Force | Out-Null
$AppPath = Join-Path $ProjectRoot 'native/Airloom/bin/x64/Release/Airloom.exe'
$Process = Start-Process -FilePath $AppPath -WorkingDirectory (Split-Path $AppPath) -PassThru
try {
    $Deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 500
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited during startup (code $($Process.ExitCode))." }
    } while ($Process.MainWindowHandle -eq 0 -and [DateTime]::UtcNow -lt $Deadline)
    if ($Process.MainWindowHandle -eq 0) { throw 'No native window appeared within 20 seconds.' }
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
    @{ result='window-opened'; process='Airloom.exe'; hwnd=$Process.MainWindowHandle.ToInt64(); note='UI launch only; no physical HomePod was connected.' } |
        ConvertTo-Json | Set-Content (Join-Path $EvidencePath 'native-smoke.json') -Encoding utf8
} finally {
    if (-not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
}
