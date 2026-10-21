param([switch] $ProbeDisplay)
$ErrorActionPreference = 'Stop'
# The Windows inbox host supplies the .NET Framework UIAutomation assemblies.
# Keep this integration check usable when CI invokes it from PowerShell 7.
if ($PSVersionTable.PSEdition -ne 'Desktop') {
    $DesktopPowerShell = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
    $HostArguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-MTA', '-File', $PSCommandPath)
    if ($ProbeDisplay) { $HostArguments += '-ProbeDisplay' }
    & $DesktopPowerShell @HostArguments
    if ($LASTEXITCODE -ne 0) { throw "Native UI check failed in Windows PowerShell (code $LASTEXITCODE)." }
    return
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;
public static class AirloomUiProbe {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [StructLayout(LayoutKind.Sequential)]
    public struct WindowBounds { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out WindowBounds bounds);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct DisplayMode {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
        public ushort SpecVersion, DriverVersion, Size, DriverExtra;
        public uint Fields;
        public int PositionX, PositionY;
        public uint Orientation, FixedOutput;
        public short Color, Duplex, YResolution, TTOption, Collate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string FormName;
        public ushort LogPixels;
        public uint BitsPerPel, Width, Height, Flags, Frequency;
        public uint ICMMethod, ICMIntent, MediaType, DitherType, Reserved1, Reserved2, PanningWidth, PanningHeight;
        public static DisplayMode Empty() {
            var mode = new DisplayMode();
            mode.Size = (ushort)Marshal.SizeOf(typeof(DisplayMode));
            return mode;
        }
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool EnumDisplaySettings(string name, int index, ref DisplayMode mode);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int ChangeDisplaySettings(ref DisplayMode mode, uint flags);
    private static DisplayMode? originalDisplay;
    public static void PrepareDisplay(int minimumWidth, int minimumHeight) {
        if (GetSystemMetrics(0) >= minimumWidth && GetSystemMetrics(1) >= minimumHeight) return;
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true")
            throw new InvalidOperationException("Native UI verification needs an on-screen desktop of at least " + minimumWidth + " x " + minimumHeight + ". The script does not change a personal desktop.");
        var original = DisplayMode.Empty();
        if (!EnumDisplaySettings(null, -1, ref original)) throw new InvalidOperationException("Cannot inspect the CI display mode.");
        var candidates = new List<DisplayMode>();
        var available = new List<string>();
        for (int index = 0; index < 4096; ++index) {
            var mode = DisplayMode.Empty();
            if (!EnumDisplaySettings(null, index, ref mode)) break;
            available.Add(mode.Width + "x" + mode.Height + "x" + mode.BitsPerPel);
            if (mode.Width >= minimumWidth && mode.Height >= minimumHeight && mode.BitsPerPel == 32) candidates.Add(mode);
        }
        candidates.Sort((a, b) => ((ulong)a.Width * a.Height).CompareTo((ulong)b.Width * b.Height));
        foreach (var candidate in candidates) {
            var mode = candidate;
            if (ChangeDisplaySettings(ref mode, 2) != 0) continue; // CDS_TEST, supported modes only.
            if (ChangeDisplaySettings(ref mode, 0) != 0) continue; // Temporary; no registry write.
            originalDisplay = original;
            for (int attempt = 0; attempt < 40; ++attempt) {
                if (GetSystemMetrics(0) >= minimumWidth && GetSystemMetrics(1) >= minimumHeight) return;
                Thread.Sleep(50);
            }
            RestoreDisplay();
        }
        throw new InvalidOperationException("CI display cannot show the required native window. Available modes: " + string.Join(", ", available));
    }
    public static void RestoreDisplay() {
        if (!originalDisplay.HasValue) return;
        var mode = originalDisplay.Value;
        int result = ChangeDisplaySettings(ref mode, 0);
        if (result != 0) throw new InvalidOperationException("Cannot restore the CI display mode: " + result);
        originalDisplay = null;
    }
    public static string ReadClipboard() {
        string value = null;
        Exception failure = null;
        var thread = new Thread(() => {
            try { value = System.Windows.Forms.Clipboard.GetText(); }
            catch (Exception error) { failure = error; }
        });
        thread.IsBackground = true;
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        if (!thread.Join(3000)) throw new TimeoutException("Clipboard read timed out.");
        if (failure != null) throw failure;
        return value;
    }
}
'@

if ($ProbeDisplay) {
    try {
        [AirloomUiProbe]::PrepareDisplay(1600, 1040)
        Write-Host "Native UI test display available: $([AirloomUiProbe]::GetSystemMetrics(0)) x $([AirloomUiProbe]::GetSystemMetrics(1))."
    } finally { [AirloomUiProbe]::RestoreDisplay() }
    return
}

function Save-NativeScreenshot([string] $Destination) {
    $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $Capture = $Screen
    if ($null -ne $Process -and -not $Process.HasExited) {
        $Bounds = New-Object AirloomUiProbe+WindowBounds
        if ([AirloomUiProbe]::GetWindowRect($Process.MainWindowHandle, [ref]$Bounds)) {
            $WindowBounds = [System.Drawing.Rectangle]::new($Bounds.Left, $Bounds.Top,
                $Bounds.Right - $Bounds.Left, $Bounds.Bottom - $Bounds.Top)
            $Capture = [System.Drawing.Rectangle]::Intersect($Screen, $WindowBounds)
        }
    }
    if ($Capture.Width -le 0 -or $Capture.Height -le 0) { throw 'The native window is outside the capture display.' }
    $Bitmap = New-Object System.Drawing.Bitmap $Capture.Width, $Capture.Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    try {
        $Graphics.CopyFromScreen($Capture.Location, [System.Drawing.Point]::Empty, $Capture.Size)
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

function Wait-NativeId([string] $Id) {
    $Condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty, $Id)
    $ControlDeadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited while waiting for '$Id'." }
        try {
            $Window = [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
            $Control = $Window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $Condition)
            if ($null -ne $Control) { return $Control }
        } catch [System.Windows.Automation.ElementNotAvailableException] { }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $ControlDeadline)
    throw "Native automation ID '$Id' was not available within 10 seconds."
}

function Set-HomeScroll([double] $Percent) {
    $Viewport = Wait-NativeId 'HomeScrollViewer'
    $Scroll = [System.Windows.Automation.ScrollPattern]$Viewport.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
    if ($Scroll.Current.VerticallyScrollable) {
        $Scroll.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, $Percent)
        Start-Sleep -Milliseconds 250
    }
}

function Get-NativeLayout {
    $Geometry = [ordered]@{}
    foreach ($Id in @('DeviceRail', 'PageContent', 'SignalPanel', 'TransportPanel', 'VolumeSlider', 'VolumeValue')) {
        $Control = Wait-NativeId $Id
        $Rect = $Control.Current.BoundingRectangle
        if ($Rect.IsEmpty -or $Rect.Width -le 0 -or $Rect.Height -le 0) { throw "Empty geometry for $Id." }
        $Geometry[$Id] = [ordered]@{ x = $Rect.X; y = $Rect.Y; width = $Rect.Width; height = $Rect.Height }
    }
    return $Geometry
}

function Assert-StableLayout($Expected, $Actual, [string] $Context) {
    foreach ($Id in $Expected.Keys) {
        foreach ($Dimension in @('x', 'y', 'width', 'height')) {
            $Delta = [Math]::Abs([double]$Expected[$Id][$Dimension] - [double]$Actual[$Id][$Dimension])
            if ($Delta -gt 1.0) { throw "$Context moved $Id $Dimension by $Delta px." }
        }
    }
}

function Test-NativeVolumeLayout {
    $Measurements = @()
    $DragVerified = $false
    foreach ($Size in @(@(880, 680), @(1100, 800), @(1600, 1000))) {
        Write-Host "Verifying native volume at $($Size[0]) x $($Size[1]) on $([AirloomUiProbe]::GetSystemMetrics(0)) x $([AirloomUiProbe]::GetSystemMetrics(1))."
        if (-not [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0, $Size[0], $Size[1], 0x0004)) {
            throw "Could not resize the real native window to $($Size[0]) x $($Size[1])."
        }
        Start-Sleep -Milliseconds 500
        # UIA returns empty geometry for clipped controls. Bring the actual
        # transport into view before measuring or sending pointer input.
        Set-HomeScroll 100
        $Slider = Wait-NativeId 'VolumeSlider'
        if (-not $Slider.Current.IsEnabled) { throw 'The idle volume slider must support preparing the next session.' }
        $Range = [System.Windows.Automation.RangeValuePattern]$Slider.GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern)
        $Baseline = $null
        foreach ($Volume in @(0, 9, 10, 99, 100)) {
            $Range.SetValue($Volume)
            # Covers debounce, engine acknowledgement, and several UI polls.
            Start-Sleep -Milliseconds 350
            if ([Math]::Abs($Range.Current.Value - $Volume) -gt 0.01) {
                throw "Volume snapped back after setting $Volume at $($Size[0]) px."
            }
            $Geometry = Get-NativeLayout
            if ($null -eq $Baseline) { $Baseline = $Geometry }
            else { Assert-StableLayout $Baseline $Geometry "Volume $Volume at $($Size[0]) px" }
            $Measurements += [ordered]@{ window_width = $Size[0]; window_height = $Size[1]; volume = $Volume; bounds = $Geometry }
        }
        # Use real pointer input once, while the slider lies on the runner's
        # actual display. Wide-window geometry can exceed that display.
        $Rect = $Slider.Current.BoundingRectangle
        $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        if (-not $DragVerified -and -not $Slider.Current.IsOffscreen -and
            $Rect.Right -lt $Screen.Right -and $Rect.Bottom -lt $Screen.Bottom) {
            $Range.SetValue(25)
            Start-Sleep -Milliseconds 350
            $Baseline = Get-NativeLayout
            $Y = [int]($Rect.Y + $Rect.Height / 2)
            $StartX = [int]($Rect.X + 12 + ($Rect.Width - 24) * 0.25)
            $null = [AirloomUiProbe]::SetForegroundWindow($Process.MainWindowHandle)
            $null = [AirloomUiProbe]::SetCursorPos($StartX, $Y)
            [AirloomUiProbe]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
            try {
                foreach ($Fraction in @(0.35, 0.45, 0.55, 0.65, 0.75)) {
                    $X = [int]($Rect.X + 12 + ($Rect.Width - 24) * $Fraction)
                    $null = [AirloomUiProbe]::SetCursorPos($X, $Y)
                    Start-Sleep -Milliseconds 120
                    Assert-StableLayout $Baseline (Get-NativeLayout) 'Pointer volume drag'
                }
            } finally { [AirloomUiProbe]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) }
            $DraggedValue = $Range.Current.Value
            if ($DraggedValue -lt 60) { throw "Pointer drag did not change the real slider: $DraggedValue." }
            Start-Sleep -Milliseconds 400
            if ([Math]::Abs($Range.Current.Value - $DraggedValue) -gt 0.01) { throw 'Volume snapped back after releasing the pointer.' }
            $DragVerified = $true
        }
        Set-HomeScroll 0
        Save-NativeScreenshot (Join-Path $EvidencePath "native-window-$($Size[0]).png")
    }
    if (-not $DragVerified) { throw 'No real pointer volume drag could be verified on the native display.' }
    $Range.SetValue(50)
    return [ordered]@{ measurements = $Measurements; pointer_drag_verified = $DragVerified }
}

function Wait-NativeUpdateCheck {
    $TextCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Text)
    $UpdateDeadline = [DateTime]::UtcNow.AddSeconds(35)
    $LastStatus = ''
    $LastTextNames = @()
    do {
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited during update check (code $($Process.ExitCode))." }
        try {
            $Window = [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
            $TextControls = $Window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $TextCondition)
            $TextNames = @()
            foreach ($TextControl in $TextControls) {
                $Name = $TextControl.Current.Name
                if ([string]::IsNullOrWhiteSpace($Name)) { continue }
                $TextNames += $Name
                if ($TextControl.Current.AutomationId -eq 'UpdateStatusText') { $LastStatus = $Name }
                # These are the actual TextBlock accessibility names after the
                # WinHTTP request and GitHub release JSON parsing complete.
                if ($Name -eq '当前已是最新版本。' -or
                    $Name.StartsWith('新版本已准备好', [System.StringComparison]::Ordinal)) { return $Name }
            }
            if ($TextNames.Count -gt 0) { $LastTextNames = $TextNames }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # Reacquire controls if the updating provider invalidated an element.
        }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $UpdateDeadline)
    $Details = if ($LastStatus) { $LastStatus } else { $LastTextNames -join ' | ' }
    throw "Update check did not return a successful native status within 35 seconds. Last status/settings text: $Details. GitHub metadata or network failures must be resolved before this check can pass."
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
$Process = $null
try {
    # The large adaptive layout must be physically on-screen, not hanging off
    # a 1024 x 768 desktop. Only this disposable CI display is adjusted.
    [AirloomUiProbe]::PrepareDisplay(1600, 1040)
    Start-Sleep -Milliseconds 500
    $Process = Start-Process -FilePath $AppPath -WorkingDirectory (Split-Path $AppPath) -PassThru
    $Deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 500
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited during startup (code $($Process.ExitCode))." }
    } while ($Process.MainWindowHandle -eq 0 -and [DateTime]::UtcNow -lt $Deadline)
    if ($Process.MainWindowHandle -eq 0) { throw 'No native window appeared within 20 seconds.' }
    if ($Process.MainWindowTitle -ne 'Airloom') { throw "Unexpected startup window: $($Process.MainWindowTitle)" }
    Start-Sleep -Seconds 3
    $VolumeProof = Test-NativeVolumeLayout
    # Capture a fully visible compact window on the runner's actual desktop.
    $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $null = [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0,
        [Math]::Min(1100, $Screen.Width), [Math]::Min(800, $Screen.Height), 0x0004)
    Start-Sleep -Milliseconds 500
    Set-HomeScroll 0
    $SettingsButton = Wait-NativeButton '设置'
    Save-NativeScreenshot (Join-Path $EvidencePath 'native-window.png')
    Invoke-NativeButton $SettingsButton
    $BackButton = Wait-NativeButton '返回声音投放'
    # The update card can be below the fold on a small runner desktop; invoke
    # its accessible control directly. This checks metadata only, never install.
    $UpdateButton = Wait-NativeButton '检查更新' $false
    Invoke-NativeButton $UpdateButton
    $UpdateStatus = Wait-NativeUpdateCheck
    $null = Wait-NativeButton '检查更新' $false
    Start-Sleep -Milliseconds 400
    Save-NativeScreenshot (Join-Path $EvidencePath 'settings-window.png')
    $CopyDiagnostics = Wait-NativeId 'CopyDiagnosticsButton'
    Invoke-NativeButton $CopyDiagnostics
    Start-Sleep -Milliseconds 250
    $CopiedDiagnostics = [AirloomUiProbe]::ReadClipboard() | ConvertFrom-Json
    if ($CopiedDiagnostics.schema_version -ne 1 -or -not $CopiedDiagnostics.app_version -or
        $null -eq $CopiedDiagnostics.capture -or $null -eq $CopiedDiagnostics.receivers) {
        throw 'Copied diagnostics did not contain the expected allowlisted schema.'
    }
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
        update_check_verified = $true
        update_status = $UpdateStatus
        returned_home = $true
        volume_layout_verified = $true
        pointer_drag_verified = $VolumeProof.pointer_drag_verified
        volume_measurements = $VolumeProof.measurements
        diagnostics_copy_verified = $true
        desktop_width = [AirloomUiProbe]::GetSystemMetrics(0)
        desktop_height = [AirloomUiProbe]::GetSystemMetrics(1)
        note = 'Real WinUI launch from the shipped archive; on-screen volume geometry verified at 880/1100/1600 pixels with 0/9/10/99/100 values and a real pointer drag. Scrollable controls are brought into view before measurement. Screenshots at all three sizes, settings, live GitHub update metadata, and diagnostics clipboard verified. No physical HomePod was connected.'
    } | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText((Join-Path $EvidencePath 'native-smoke.json'), $Result,
        [System.Text.UTF8Encoding]::new($false))
} catch {
    try { Save-NativeScreenshot (Join-Path $EvidencePath 'native-smoke-failure.png') } catch { }
    throw
} finally {
    if ($null -ne $Process -and -not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
    [AirloomUiProbe]::RestoreDisplay()
}
