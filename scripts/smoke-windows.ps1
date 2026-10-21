$ErrorActionPreference = 'Stop'
# The Windows inbox host supplies the .NET Framework UIAutomation assemblies.
# Keep this integration check usable when CI invokes it from PowerShell 7.
if ($PSVersionTable.PSEdition -ne 'Desktop') {
    $DesktopPowerShell = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
    & $DesktopPowerShell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -MTA -File $PSCommandPath
    if ($LASTEXITCODE -ne 0) { throw "Native UI check failed in Windows PowerShell (code $LASTEXITCODE)." }
    return
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

function Save-NativeScreenshot([string] $Destination) {
    $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $Bitmap = New-Object System.Drawing.Bitmap $Screen.Width, $Screen.Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    try {
        $Graphics.CopyFromScreen($Screen.Location, [System.Drawing.Point]::Empty, $Screen.Size)
        $Bitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $Graphics.Dispose(); $Bitmap.Dispose() }
}

function Wait-NativeButton([string] $Name, [bool] $RequireVisible = $true) {
    $NameCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    $TypeCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Button)
    $Condition = [System.Windows.Automation.AndCondition]::new($NameCondition, $TypeCondition)
    $ControlDeadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited while waiting for '$Name' (code $($Process.ExitCode))." }
        try {
            # Limit searches to this launched app, never the whole desktop tree.
            $Window = [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
            $Control = $Window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $Condition)
            if ($null -ne $Control -and $Control.Current.IsEnabled -and
                (-not $RequireVisible -or -not $Control.Current.IsOffscreen)) { return $Control }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # A page transition can invalidate a provider; reacquire from the HWND.
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $ControlDeadline)
    throw "Expected enabled native button '$Name' was not available within 10 seconds."
}

function Invoke-NativeButton([System.Windows.Automation.AutomationElement] $Button) {
    $Pattern = $Button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    if ($null -eq $Pattern) { throw "Button '$($Button.Current.Name)' does not support InvokePattern." }
    ([System.Windows.Automation.InvokePattern] $Pattern).Invoke()
}

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
    $SettingsButton = Wait-NativeButton '设置'
    Save-NativeScreenshot (Join-Path $EvidencePath 'native-window.png')
    Invoke-NativeButton $SettingsButton
    $BackButton = Wait-NativeButton '返回声音投放'
    # The update card can be below the fold on a small runner desktop. Its
    # presence and InvokePattern still verify the real settings view was loaded.
    $UpdateButton = Wait-NativeButton '检查更新' $false
    $UpdatePattern = $UpdateButton.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    if ($UpdatePattern -isnot [System.Windows.Automation.InvokePattern]) {
        throw 'The settings update button cannot be invoked.'
    }
    Start-Sleep -Milliseconds 400
    Save-NativeScreenshot (Join-Path $EvidencePath 'settings-window.png')
    Invoke-NativeButton $BackButton
    $null = Wait-NativeButton '设置'
    $Process.Refresh()
    if ($Process.HasExited) { throw 'Native UI exited after returning from settings.' }
    $Result = @{
        result = 'window-opened'
        distribution = 'portable-zip'
        process = 'Airloom.exe'
        hwnd = $Process.MainWindowHandle.ToInt64()
        settings_verified = $true
        returned_home = $true
        note = 'Real UI launch from a clean extracted distribution; settings opened and returned using UI Automation. No update was downloaded and no physical HomePod was connected.'
    } | ConvertTo-Json
    [System.IO.File]::WriteAllText((Join-Path $EvidencePath 'native-smoke.json'), $Result,
        [System.Text.UTF8Encoding]::new($false))
} catch {
    try { Save-NativeScreenshot (Join-Path $EvidencePath 'native-smoke-failure.png') } catch { }
    throw
} finally {
    if (-not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
}
