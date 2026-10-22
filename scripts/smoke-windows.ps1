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
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern short GetAsyncKeyState(int key);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern uint RegisterWindowMessage(string name);
    [DllImport("user32.dll", SetLastError = true)] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    public static void RequestExit(IntPtr window) {
        uint message = RegisterWindowMessage("Airloom.RequestExit.v1");
        if (message == 0 || !PostMessage(window, message, IntPtr.Zero, IntPtr.Zero))
            throw new InvalidOperationException("Cannot request native application exit: " + Marshal.GetLastWin32Error());
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct WindowBounds { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out WindowBounds bounds);
    [StructLayout(LayoutKind.Sequential)]
    public struct CursorPoint { public int X, Y; }
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool GetPhysicalCursorPos(out CursorPoint point);
    [DllImport("user32.dll")] private static extern IntPtr WindowFromPoint(CursorPoint point);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [StructLayout(LayoutKind.Sequential)]
    private struct MouseInput {
        public int X, Y;
        public uint Data, Flags, Time;
        public UIntPtr Extra;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct Input { public uint Type; public MouseInput Mouse; }
    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint count, Input[] inputs, int size);
    private static void SendMouse(int x, int y, uint flags) {
        var input = new Input { Mouse = new MouseInput { X = x, Y = y, Flags = flags } };
        int size = Marshal.SizeOf(typeof(Input));
        if (size != (IntPtr.Size == 8 ? 40 : 28)) throw new InvalidOperationException("Unexpected native INPUT layout.");
        uint sent = SendInput(1, new[] { input }, size);
        if (sent != 1) throw new InvalidOperationException("SendInput accepted " + sent + " events; Win32 error " + Marshal.GetLastWin32Error() + ".");
    }
    public static CursorPoint Cursor() {
        CursorPoint point;
        if (!GetPhysicalCursorPos(out point)) throw new InvalidOperationException("Cannot read the physical pointer: " + Marshal.GetLastWin32Error());
        return point;
    }
    public static CursorPoint MovePointer(int x, int y) {
        int left = GetSystemMetrics(76), top = GetSystemMetrics(77);
        int width = GetSystemMetrics(78), height = GetSystemMetrics(79);
        if (x < left || y < top || x >= left + width || y >= top + height)
            throw new InvalidOperationException("Pointer target is outside the native desktop.");
        int absoluteX = (int)Math.Round((x - left) * 65535.0 / (width - 1));
        int absoluteY = (int)Math.Round((y - top) * 65535.0 / (height - 1));
        // MOVE | MOVE_NOCOALESCE | VIRTUALDESK | ABSOLUTE. Unlike moving
        // the cursor alone, this verifies an event entered the input stream.
        SendMouse(absoluteX, absoluteY, 0xE001);
        for (int attempt = 0; attempt < 20; ++attempt) {
            var actual = Cursor();
            if (Math.Abs(actual.X - x) <= 1 && Math.Abs(actual.Y - y) <= 1) return actual;
            Thread.Sleep(25);
        }
        var final = Cursor();
        throw new InvalidOperationException("Pointer did not reach " + x + "," + y + "; actual " + final.X + "," + final.Y + ".");
    }
    public static void RequirePointOwner(int x, int y, uint expectedProcess) {
        uint actualProcess;
        var window = WindowFromPoint(new CursorPoint { X = x, Y = y });
        GetWindowThreadProcessId(window, out actualProcess);
        if (actualProcess != expectedProcess)
            throw new InvalidOperationException("Pointer target belongs to process " + actualProcess + ", expected " + expectedProcess + ".");
    }
    public static void LeftButton(bool down) { SendMouse(0, 0, down ? 0x0002u : 0x0004u); }
    public static bool LeftButtonIsDown() { return (GetAsyncKeyState(1) & 0x8000) != 0; }
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
    // GET uses BOOL*, while SET takes the BOOL value in pvParam. A zero
    // fWinIni changes only the session, without persisting or broadcasting it.
    // https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-systemparametersinfow
    [DllImport("user32.dll", EntryPoint = "SystemParametersInfoW", ExactSpelling = true, SetLastError = true)]
    private static extern bool GetSystemParameter(uint action, uint parameter, [MarshalAs(UnmanagedType.Bool)] out bool value, uint flags);
    [DllImport("user32.dll", EntryPoint = "SystemParametersInfoW", ExactSpelling = true, SetLastError = true)]
    private static extern bool SetSystemParameter(uint action, uint parameter, IntPtr value, uint flags);
    private static bool? originalClientAreaAnimation;
    public static bool ClientAreaAnimationsEnabled() {
        bool enabled;
        if (!GetSystemParameter(0x1042, 0, out enabled, 0))
            throw new InvalidOperationException("Cannot read client-area animation preference: " + Marshal.GetLastWin32Error());
        return enabled;
    }
    public static void PrepareFixtureMotion() {
        bool original = ClientAreaAnimationsEnabled();
        if (original) return;
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true")
            throw new InvalidOperationException("Active waveform evidence needs animations enabled. The script does not change a personal desktop.");
        originalClientAreaAnimation = original;
        if (!SetSystemParameter(0x1043, 0, new IntPtr(1), 0) || !ClientAreaAnimationsEnabled())
            throw new InvalidOperationException("Cannot temporarily enable CI client-area animations: " + Marshal.GetLastWin32Error());
    }
    public static void RestoreFixtureMotion() {
        if (!originalClientAreaAnimation.HasValue) return;
        bool original = originalClientAreaAnimation.Value;
        if (!SetSystemParameter(0x1043, 0, new IntPtr(original ? 1 : 0), 0) || ClientAreaAnimationsEnabled() != original)
            throw new InvalidOperationException("Cannot restore CI client-area animation preference: " + Marshal.GetLastWin32Error());
        originalClientAreaAnimation = null;
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

# UI Automation reports physical pixels. Keep window/display calculations in
# the same coordinate system before creating any Windows Forms handles.
$null = [AirloomUiProbe]::SetProcessDPIAware()

if ($ProbeDisplay) {
    try {
        [AirloomUiProbe]::PrepareDisplay(1600, 1040)
        Write-Host "Native UI test display available: $([AirloomUiProbe]::GetSystemMetrics(0)) x $([AirloomUiProbe]::GetSystemMetrics(1))."
        if ($env:GITHUB_ACTIONS -eq 'true') {
            $OriginalCursor = [AirloomUiProbe]::Cursor()
            try {
                $ActualCursor = [AirloomUiProbe]::MovePointer(100, 100)
                Write-Host "Native mouse input available at $($ActualCursor.X), $($ActualCursor.Y)."
            } finally { $null = [AirloomUiProbe]::MovePointer($OriginalCursor.X, $OriginalCursor.Y) }
        }
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
    # WinUI's thumb has a -2 px visual outset. Slider's automation bounds
    # legitimately include that moving decoration at its endpoints; compare
    # the containing layout slot, and record the slider's visual bounds separately.
    foreach ($Id in @('DeviceRail', 'PageContent', 'SignalPanel', 'TransportPanel', 'VolumeSliderHost', 'VolumeValue')) {
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
            if ($Delta -gt 1.0) {
                $Before = $Expected[$Id] | ConvertTo-Json -Compress
                $After = $Actual[$Id] | ConvertTo-Json -Compress
                throw "$Context moved $Id $Dimension by $Delta px. Before: $Before. After: $After."
            }
        }
    }
}

function Get-NativeSliderThumb([System.Windows.Automation.AutomationElement] $Slider) {
    # WinUI deliberately exposes HorizontalThumb in Raw view only. Its actual
    # bounds are a reliable pointer target; the whole slider includes padding.
    $Walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $Queue = New-Object 'System.Collections.Generic.Queue[System.Windows.Automation.AutomationElement]'
    $Queue.Enqueue($Slider)
    $Visited = 0
    while ($Queue.Count -gt 0 -and $Visited -lt 128) {
        $Parent = $Queue.Dequeue()
        $Child = $Walker.GetFirstChild($Parent)
        while ($null -ne $Child -and $Visited -lt 128) {
            $Visited++
            if ($Child.Current.ControlType -eq [System.Windows.Automation.ControlType]::Thumb -and
                -not $Child.Current.IsOffscreen -and -not $Child.Current.BoundingRectangle.IsEmpty) { return $Child }
            $Queue.Enqueue($Child)
            $Child = $Walker.GetNextSibling($Child)
        }
    }
    throw 'The visible native slider thumb was not available in the raw automation tree.'
}

function Assert-AdaptiveComposition($Geometry, [double] $WorkspaceWidth) {
    $Signal = $Geometry['SignalPanel']
    $Receivers = $Geometry['DeviceRail']
    $Transport = $Geometry['TransportPanel']
    $HomeBounds = (Wait-NativeId 'HomeView').Current.BoundingRectangle
    if ($Transport.y -lt $Signal.y -or $Transport.x -lt $HomeBounds.Left - 1 -or
        $Transport.x + $Transport.width -gt $HomeBounds.Right + 1 -or
        $Transport.y + $Transport.height -gt $HomeBounds.Bottom + 1) {
        throw 'The persistent transport dock was outside the home workspace.'
    }
    if ($WorkspaceWidth -ge 1050) {
        if ($Receivers.x -lt ($Signal.x + $Signal.width - 1) -or [Math]::Abs($Receivers.y - $Signal.y) -gt 1) {
            throw 'The wide workspace did not place receivers beside the signal stage.'
        }
        return 'wide'
    }
    if ($Signal.y -lt ($Receivers.y + $Receivers.height - 1) -or [Math]::Abs($Receivers.x - $Signal.x) -gt 1) {
        throw 'The compact workspace did not place receivers before the signal stage.'
    }
    return 'compact'
}

function Assert-CompactReceiversVisible {
    $ViewportBounds = (Wait-NativeId 'HomeScrollViewer').Current.BoundingRectangle
    $DockBounds = (Wait-NativeId 'TransportPanel').Current.BoundingRectangle
    $VisibleBottom = [Math]::Min($ViewportBounds.Bottom, $DockBounds.Top)
    $ReceiverPanel = Wait-NativeId 'DeviceRail'
    $HeadingCondition = [System.Windows.Automation.AndCondition]::new(
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, '扬声器'),
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Text))
    $Heading = $ReceiverPanel.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $HeadingCondition)
    if ($null -eq $Heading) { throw 'The compact receiver heading was not exposed in the native UI.' }
    $CandidateControls = @($Heading)
    $ButtonCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
    foreach ($ReceiverControl in $ReceiverPanel.FindAll([System.Windows.Automation.TreeScope]::Descendants, $ButtonCondition)) {
        if ($ReceiverControl.Current.AutomationId.StartsWith('Receiver:', [System.StringComparison]::Ordinal)) {
            $CandidateControls += $ReceiverControl
            break
        }
    }
    # A compact view must expose the heading and the beginning of its first
    # receiver at scroll top; the entire receiver panel need not fit on screen.
    foreach ($CandidateControl in $CandidateControls) {
        $CandidateBounds = $CandidateControl.Current.BoundingRectangle
        if ($CandidateBounds.IsEmpty -or $CandidateBounds.Width -le 0 -or $CandidateBounds.Height -le 0 -or
            $CandidateBounds.Top -lt $ViewportBounds.Top - 1 -or $CandidateBounds.Top -ge $VisibleBottom -or
            $CandidateBounds.Bottom -le $ViewportBounds.Top -or $CandidateBounds.Right -le $ViewportBounds.Left -or
            $CandidateBounds.Left -ge $ViewportBounds.Right) {
            throw "Compact receiver control '$($CandidateControl.Current.Name)' was not visible above the dock at scroll top."
        }
    }
}

function Test-NativeVolumeLayout {
    $Measurements = @()
    $DragVerified = $false
    $CompactTopVerified = $false
    $InputProof = $null
    foreach ($Size in @(@(880, 680), @(1100, 800), @(1600, 1000))) {
        Write-Host "Verifying native volume at $($Size[0]) x $($Size[1]) on $([AirloomUiProbe]::GetSystemMetrics(0)) x $([AirloomUiProbe]::GetSystemMetrics(1))."
        if (-not [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0, $Size[0], $Size[1], 0x0004)) {
            throw "Could not resize the real native window to $($Size[0]) x $($Size[1])."
        }
        Start-Sleep -Milliseconds 500
        Set-HomeScroll 0
        $WorkspaceWidth = (Wait-NativeId 'HomeView').Current.BoundingRectangle.Width * 96 /
            [AirloomUiProbe]::GetDpiForWindow($Process.MainWindowHandle)
        if ($WorkspaceWidth -lt 1050) {
            Assert-CompactReceiversVisible
            $CompactTopVerified = $true
        }
        $DockBeforeScroll = (Wait-NativeId 'TransportPanel').Current.BoundingRectangle
        Set-HomeScroll 100
        $DockAfterScroll = (Wait-NativeId 'TransportPanel').Current.BoundingRectangle
        foreach ($Dimension in @('X', 'Y', 'Width', 'Height')) {
            if ([Math]::Abs($DockAfterScroll.$Dimension - $DockBeforeScroll.$Dimension) -gt 1) {
                throw "The transport dock moved when the home workspace scrolled at $($Size[0]) px."
            }
        }
        $Slider = Wait-NativeId 'VolumeSlider'
        if (-not $Slider.Current.IsEnabled) { throw 'The idle volume slider must support preparing the next session.' }
        $Range = [System.Windows.Automation.RangeValuePattern]$Slider.GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern)
        $Baseline = $null
        $TrackStart = $null
        $TrackEnd = $null
        foreach ($Volume in @(0, 9, 10, 99, 100)) {
            $Range.SetValue($Volume)
            # Covers debounce, engine acknowledgement, and several UI polls.
            Start-Sleep -Milliseconds 350
            if ([Math]::Abs($Range.Current.Value - $Volume) -gt 0.01) {
                throw "Volume snapped back after setting $Volume at $($Size[0]) px."
            }
            $Geometry = Get-NativeLayout
            $Composition = Assert-AdaptiveComposition $Geometry $WorkspaceWidth
            if ($null -eq $Baseline) { $Baseline = $Geometry }
            else { Assert-StableLayout $Baseline $Geometry "Volume $Volume at $($Size[0]) px" }
            $Visual = $Slider.Current.BoundingRectangle
            $Measurements += [ordered]@{
                window_width = $Size[0]; window_height = $Size[1]; volume = $Volume; bounds = $Geometry
                workspace_width_dip = $WorkspaceWidth; composition = $Composition
                slider_visual_bounds = [ordered]@{ x = $Visual.X; y = $Visual.Y; width = $Visual.Width; height = $Visual.Height }
            }
            if (-not $DragVerified -and ($Volume -eq 0 -or $Volume -eq 100)) {
                $ThumbBounds = (Get-NativeSliderThumb $Slider).Current.BoundingRectangle
                $Center = $ThumbBounds.X + $ThumbBounds.Width / 2
                if ($Volume -eq 0) { $TrackStart = $Center } else { $TrackEnd = $Center }
            }
        }
        # Drive the actual native thumb using checked mouse input on the
        # visible desktop. UIA only prepares the starting value, never the drag.
        $Rect = $Slider.Current.BoundingRectangle
        $Screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        if (-not $DragVerified -and -not $Slider.Current.IsOffscreen -and
            $Rect.Right -lt $Screen.Right -and $Rect.Bottom -lt $Screen.Bottom) {
            $Range.SetValue(25)
            Start-Sleep -Milliseconds 350
            $null = [AirloomUiProbe]::SetForegroundWindow($Process.MainWindowHandle)
            $Slider.SetFocus()
            $FocusDeadline = [DateTime]::UtcNow.AddSeconds(2)
            while ([AirloomUiProbe]::GetForegroundWindow() -ne $Process.MainWindowHandle -and
                [DateTime]::UtcNow -lt $FocusDeadline) { Start-Sleep -Milliseconds 50 }
            if ([AirloomUiProbe]::GetForegroundWindow() -ne $Process.MainWindowHandle) {
                throw 'The native window could not become foreground for pointer verification.'
            }
            Start-Sleep -Milliseconds 150
            $ThumbBounds = (Get-NativeSliderThumb $Slider).Current.BoundingRectangle
            $Baseline = Get-NativeLayout
            $Y = [int]($ThumbBounds.Y + $ThumbBounds.Height / 2)
            $StartX = [int]($ThumbBounds.X + $ThumbBounds.Width / 2)
            if ($null -eq $TrackStart -or $null -eq $TrackEnd -or $TrackEnd -le $TrackStart) { throw 'Native thumb endpoints did not define a horizontal track.' }
            [AirloomUiProbe]::RequirePointOwner($StartX, $Y, $Process.Id)
            $StartCursor = [AirloomUiProbe]::MovePointer($StartX, $Y)
            Start-Sleep -Milliseconds 150
            $InputProof = [ordered]@{
                method = 'SendInput'; dpi = [AirloomUiProbe]::GetDpiForWindow($Process.MainWindowHandle)
                foreground_verified = $true; point_owner_verified = $true
                thumb_start = [ordered]@{ x = $StartX; y = $Y }
                cursor_start = [ordered]@{ x = $StartCursor.X; y = $StartCursor.Y }
                track_start = $TrackStart; track_end = $TrackEnd; moves = @(); final_value = $null; failure = $null
            }
            Write-Host ("Native pointer target: " + ($InputProof | ConvertTo-Json -Depth 5 -Compress))
            $Pressed = $false
            try {
                [AirloomUiProbe]::LeftButton($true)
                $Pressed = $true
                Start-Sleep -Milliseconds 150
                $InputProof.button_down_verified = [AirloomUiProbe]::LeftButtonIsDown()
                if (-not $InputProof.button_down_verified) { throw 'Injected left-button input did not establish a pressed state.' }
                foreach ($Fraction in @(0.35, 0.45, 0.55, 0.65, 0.75)) {
                    $X = [int]($TrackStart + ($TrackEnd - $TrackStart) * $Fraction)
                    $Cursor = [AirloomUiProbe]::MovePointer($X, $Y)
                    Start-Sleep -Milliseconds 120
                    $InputProof.moves += [ordered]@{ x = $Cursor.X; y = $Cursor.Y; value = $Range.Current.Value }
                    Assert-StableLayout $Baseline (Get-NativeLayout) 'Pointer volume drag'
                }
            } catch {
                $InputProof.failure = $_.Exception.Message
                throw
            } finally {
                if ($Pressed) { [AirloomUiProbe]::LeftButton($false) }
                [System.IO.File]::WriteAllText((Join-Path $EvidencePath 'native-input-probe.json'),
                    ($InputProof | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
            }
            Start-Sleep -Milliseconds 100
            $DraggedValue = $Range.Current.Value
            $InputProof.final_value = $DraggedValue
            [System.IO.File]::WriteAllText((Join-Path $EvidencePath 'native-input-probe.json'),
                ($InputProof | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
            Write-Host ("Native pointer result: " + ($InputProof | ConvertTo-Json -Depth 8 -Compress))
            if ($DraggedValue -lt 60) { throw "Pointer drag did not change the real slider: $DraggedValue. Input trace is in native-input-probe.json." }
            Start-Sleep -Milliseconds 400
            if ([Math]::Abs($Range.Current.Value - $DraggedValue) -gt 0.01) { throw 'Volume snapped back after releasing the pointer.' }
            $DragVerified = $true
        }
        Set-HomeScroll 0
        Save-NativeScreenshot (Join-Path $EvidencePath "native-window-$($Size[0]).png")
    }
    if (-not $DragVerified) { throw 'No real pointer volume drag could be verified on the native display.' }
    if (-not ($Measurements.composition -contains 'wide') -or -not ($Measurements.composition -contains 'compact')) {
        throw 'Both wide and compact native compositions must be exercised.'
    }
    $Range.SetValue(50)
    return [ordered]@{
        measurements = $Measurements; pointer_drag_verified = $DragVerified; pointer_input = $InputProof
        compact_receivers_visible_at_top = $CompactTopVerified
    }
}

function Start-NativeApplication([string] $Executable) {
    $script:Process = Start-Process -FilePath $Executable -WorkingDirectory (Split-Path $Executable) -PassThru
    $Deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 250
        $Process.Refresh()
        if ($Process.HasExited) { throw "Native UI exited during startup (code $($Process.ExitCode))." }
    } while ($Process.MainWindowHandle -eq 0 -and [DateTime]::UtcNow -lt $Deadline)
    if ($Process.MainWindowHandle -eq 0) { throw 'No native window appeared within 20 seconds.' }
    if ($Process.MainWindowTitle -ne 'Airloom') { throw "Unexpected startup window: $($Process.MainWindowTitle)" }
    $null = Wait-NativeId 'HomeView'
    Start-Sleep -Milliseconds 600
}

function Stop-NativeApplication {
    if ($null -eq $Process -or $Process.HasExited) { return }
    [AirloomUiProbe]::RequestExit($Process.MainWindowHandle)
    if (-not $Process.WaitForExit(10000)) { throw 'Native UI did not exit after its explicit exit request.' }
    $script:Process = $null
}

function Set-SettingsScroll([double] $Percent) {
    $Viewport = Wait-NativeId 'SettingsScrollViewer'
    $Scroll = [System.Windows.Automation.ScrollPattern]$Viewport.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
    if ($Scroll.Current.VerticallyScrollable) {
        $Scroll.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, $Percent)
        Start-Sleep -Milliseconds 150
    }
}

function Get-NativeSelectionNames([string] $Id) {
    $Picker = Wait-NativeId $Id
    $Selection = [System.Windows.Automation.SelectionPattern]$Picker.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern)
    return @($Selection.Current.GetSelection() | ForEach-Object { $_.Current.Name })
}

function Find-NativeOption([string] $Name) {
    $NameCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    $KindCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::ListItem)
    $Condition = [System.Windows.Automation.AndCondition]::new($NameCondition, $KindCondition)
    $Window = [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
    $Option = $Window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $Condition)
    if ($null -ne $Option) { return $Option }
    # A native ComboBox popup can have a separate HWND; remain process-scoped.
    $ProcessCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $Process.Id)
    $Roots = [System.Windows.Automation.AutomationElement]::RootElement.FindAll(
        [System.Windows.Automation.TreeScope]::Children, $ProcessCondition)
    foreach ($Root in $Roots) {
        $Option = $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $Condition)
        if ($null -ne $Option) { return $Option }
    }
    return $null
}

function Select-NativeOption([string] $Id, [string] $Name) {
    $Picker = Wait-NativeId $Id
    if (-not $Picker.Current.IsEnabled -or $Picker.Current.IsOffscreen -or $Picker.Current.BoundingRectangle.IsEmpty) {
        throw "The native picker '$Id' is not enabled and visible."
    }
    $Expansion = [System.Windows.Automation.ExpandCollapsePattern]$Picker.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
    $Expansion.Expand()
    $Deadline = [DateTime]::UtcNow.AddSeconds(3)
    $Option = $null
    do {
        $Option = Find-NativeOption $Name
        if ($null -ne $Option) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $Deadline)
    if ($null -eq $Option) { throw "The native option '$Name' was not exposed after opening '$Id'." }
    $Selection = [System.Windows.Automation.SelectionItemPattern]$Option.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
    $Selection.Select()
    $Expansion.Collapse()
    Start-Sleep -Milliseconds 350
    $SelectedNames = @(Get-NativeSelectionNames $Id)
    if ($SelectedNames.Count -ne 1 -or $SelectedNames[0] -ne $Name) { throw "Native picker '$Id' did not retain '$Name'." }
}

function Select-NativeTheme([string] $Name) {
    $Picker = Wait-NativeId 'ThemePicker'
    # Theme controls can be below the fold at the minimum native window size.
    foreach ($Percent in @(0, 25, 50, 75, 100)) {
        Set-SettingsScroll $Percent
        if (-not $Picker.Current.IsOffscreen -and -not $Picker.Current.BoundingRectangle.IsEmpty) { break }
    }
    Select-NativeOption 'ThemePicker' $Name
    Set-SettingsScroll 0
}

function Assert-ActiveFixtureWaveform {
    $Signal = Wait-NativeId 'SignalPanel'
    if ($Signal.Current.IsOffscreen -or $Signal.Current.BoundingRectangle.IsEmpty) {
        throw 'The populated audio instrument is not visible for waveform evidence.'
    }
    $TextCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Text)
    $InactiveHints = @('连接后显示实时声音', '连接后，声音将在这里流动', '动态波形已关闭',
        '已遵循 Windows 动画设置', '等待声音 · 播放一首音乐试试')
    $MotionDeadline = [DateTime]::UtcNow.AddSeconds(3)
    do {
        $VisibleHints = @($Signal.FindAll([System.Windows.Automation.TreeScope]::Descendants, $TextCondition) |
            Where-Object { -not $_.Current.IsOffscreen -and $InactiveHints -contains $_.Current.Name })
        if ([AirloomUiProbe]::ClientAreaAnimationsEnabled() -and $VisibleHints.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $MotionDeadline)
    throw 'The populated fixture still shows an inactive or reduced-motion waveform hint.'
}

function Test-NativeThemes([string] $Prefix, [bool] $VerifyActiveWaveform = $false) {
    $Themes = @()
    $ThemeErrors = @()
    foreach ($Theme in @(@('浅色', 'light'), @('深色', 'dark'))) {
        $ThemeFailed = $false
        try {
            Invoke-NativeButton (Wait-NativeId 'SettingsButton')
            $null = Wait-NativeButton '返回声音投放'
            Select-NativeTheme $Theme[0]
            Save-NativeScreenshot (Join-Path $EvidencePath "$Prefix-settings-$($Theme[1]).png")
            Invoke-NativeButton (Wait-NativeId 'SettingsBackButton')
            Start-Sleep -Milliseconds 250
            Set-HomeScroll 0
            if ($VerifyActiveWaveform) { Assert-ActiveFixtureWaveform }
            Save-NativeScreenshot (Join-Path $EvidencePath "$Prefix-home-$($Theme[1]).png")
            $Themes += $Theme[1]
        } catch {
            $ThemeFailed = $true
            $ThemeErrors += "$Prefix $($Theme[1]): $($_.Exception.Message)"
            try { Save-NativeScreenshot (Join-Path $EvidencePath "$Prefix-$($Theme[1])-failure.png") } catch { }
        } finally {
            # Restore the page even after a selection/assertion failure, so the
            # other theme and the independent fixture can still produce evidence.
            if ($ThemeFailed -and $null -ne $Process -and -not $Process.HasExited) {
                try {
                    $BackButton = Wait-NativeId 'SettingsBackButton'
                    if (-not $BackButton.Current.IsOffscreen) {
                        $Picker = Wait-NativeId 'ThemePicker'
                        $Expansion = [System.Windows.Automation.ExpandCollapsePattern]$Picker.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
                        $Expansion.Collapse()
                        Invoke-NativeButton $BackButton
                        Start-Sleep -Milliseconds 250
                    }
                } catch { $ThemeErrors += "$Prefix $($Theme[1]) navigation: $($_.Exception.Message)" }
            }
        }
    }
    return [ordered]@{
        verified = ($ThemeErrors.Count -eq 0 -and $Themes.Count -eq 2); themes = $Themes; errors = $ThemeErrors
        active_waveform_mode_verified = ($VerifyActiveWaveform -and $ThemeErrors.Count -eq 0 -and $Themes.Count -eq 2)
    }
}

function Build-VisualFixture([string] $OutputDirectory) {
    $VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $VsInstall = & $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $VsInstall) { throw 'Visual C++ installation not found for the isolated UI fixture.' }
    $DeveloperCommand = Join-Path $VsInstall 'Common7/Tools/VsDevCmd.bat'
    $Source = Join-Path $ProjectRoot 'scripts/fixtures/UiFixture.cpp'
    $CommandFile = Join-Path $OutputDirectory 'build-fixture.cmd'
    $Dll = Join-Path $OutputDirectory 'airloom_core.dll'
    $ObjectFile = Join-Path $OutputDirectory 'UiFixture.obj'
    $ImportLibrary = Join-Path $OutputDirectory 'UiFixture.lib'
    $Commands = "@echo off`r`ncall `"$DeveloperCommand`" -no_logo -arch=x64 -host_arch=x64`r`nif errorlevel 1 exit /b 1`r`n" +
        "cl.exe /nologo /std:c++20 /EHsc /W4 /WX /MT /utf-8 /LD `"$Source`" /Fo`"$ObjectFile`" /Fe`"$Dll`" /link /IMPLIB:`"$ImportLibrary`"`r`n"
    [System.IO.File]::WriteAllText($CommandFile, $Commands, [System.Text.UTF8Encoding]::new($false))
    & $env:ComSpec /d /c $CommandFile
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $Dll)) { throw 'Isolated visual fixture build failed.' }
}

function Read-FixtureDiagnostics {
    Invoke-NativeButton (Wait-NativeId 'SettingsButton')
    $null = Wait-NativeButton '返回声音投放'
    try {
        Invoke-NativeButton (Wait-NativeId 'CopyDiagnosticsButton')
        Start-Sleep -Milliseconds 200
        $Diagnostics = [AirloomUiProbe]::ReadClipboard() | ConvertFrom-Json
        if ($Diagnostics.fixture -ne $true -or $null -eq $Diagnostics.fixture_state) {
            throw 'The isolated process did not identify its synthetic command evidence.'
        }
        return $Diagnostics
    } finally {
        Invoke-NativeButton (Wait-NativeId 'SettingsBackButton')
        Start-Sleep -Milliseconds 250
        Set-HomeScroll 0
    }
}

function Invoke-FixtureConnectPointer([string] $Theme) {
    $Button = Wait-NativeId 'ConnectButton'
    if (-not $Button.Current.IsEnabled -or $Button.Current.IsOffscreen) {
        throw 'The primary action must be enabled and visible before physical input.'
    }
    $Bounds = $Button.Current.BoundingRectangle
    $Scale = [AirloomUiProbe]::GetDpiForWindow($Process.MainWindowHandle) / 96.0
    if ($Bounds.IsEmpty -or [Math]::Abs($Bounds.Height / $Scale - 44) -gt 1) {
        throw 'The primary action did not retain its 44 DIP native button height.'
    }
    $OriginalCursor = [AirloomUiProbe]::Cursor()
    $ButtonPressed = $false
    try {
        $null = [AirloomUiProbe]::SetForegroundWindow($Process.MainWindowHandle)
        Start-Sleep -Milliseconds 200
        if ([AirloomUiProbe]::GetForegroundWindow() -ne $Process.MainWindowHandle) {
            throw 'The fixture window did not become foreground for physical button input.'
        }
        $PageBounds = (Wait-NativeId 'PageContent').Current.BoundingRectangle
        $null = [AirloomUiProbe]::MovePointer([int]($PageBounds.X + 8), [int]($PageBounds.Y + 8))
        (Wait-NativeId 'SettingsButton').SetFocus()
        Start-Sleep -Milliseconds 150
        Save-NativeScreenshot (Join-Path $EvidencePath "fixture-button-normal-$Theme.png")
        $Button.SetFocus()
        Start-Sleep -Milliseconds 150
        if (-not $Button.Current.HasKeyboardFocus) { throw 'The primary action did not accept native UIA focus.' }
        Save-NativeScreenshot (Join-Path $EvidencePath "fixture-button-focus-$Theme.png")
        $CenterX = [int]($Bounds.X + $Bounds.Width / 2)
        $CenterY = [int]($Bounds.Y + $Bounds.Height / 2)
        [AirloomUiProbe]::RequirePointOwner($CenterX, $CenterY, [uint32]$Process.Id)
        $null = [AirloomUiProbe]::MovePointer($CenterX, $CenterY)
        Start-Sleep -Milliseconds 200
        Save-NativeScreenshot (Join-Path $EvidencePath "fixture-button-hover-$Theme.png")
        [AirloomUiProbe]::LeftButton($true)
        $ButtonPressed = $true
        Start-Sleep -Milliseconds 150
        if (-not [AirloomUiProbe]::LeftButtonIsDown()) { throw 'The physical button press was not observed.' }
        foreach ($Dimension in @('X', 'Y', 'Width', 'Height')) {
            if ([Math]::Abs($Button.Current.BoundingRectangle.$Dimension - $Bounds.$Dimension) -gt 1) {
                throw "The primary action moved during pointer interaction ($Dimension)."
            }
        }
        Save-NativeScreenshot (Join-Path $EvidencePath "fixture-button-pressed-$Theme.png")
        [AirloomUiProbe]::LeftButton($false)
        $ButtonPressed = $false
        Start-Sleep -Milliseconds 400
        return [ordered]@{
            pointer_method = 'Checked SendInput hover, left-button down, and left-button up'
            focus_method = 'UIA SetFocus with HasKeyboardFocus verification'
            focus_verified = $true; physical_press_verified = $true; stable_bounds_verified = $true
            height_dip = $Bounds.Height / $Scale
        }
    } finally {
        if ($ButtonPressed) { [AirloomUiProbe]::LeftButton($false) }
        $null = [AirloomUiProbe]::MovePointer($OriginalCursor.X, $OriginalCursor.Y)
    }
}

function Send-FixtureEvent([string] $Action) {
    $script:FixtureEventIndex += 1
    $EventPath = Join-Path $script:FixtureEventDirectory "event-$($script:FixtureEventIndex).txt"
    $AcknowledgementPath = Join-Path $script:FixtureEventDirectory "ack-$($script:FixtureEventIndex).txt"
    $StagingPath = "$EventPath.pending"
    [System.IO.File]::WriteAllText($StagingPath, $Action, [System.Text.UTF8Encoding]::new($false))
    # Publish an immutable file, so the DLL cannot read half of an event.
    Move-Item -LiteralPath $StagingPath -Destination $EventPath
    $Deadline = [DateTime]::UtcNow.AddSeconds(3)
    do {
        if ((Test-Path -LiteralPath $AcknowledgementPath) -and
            [System.IO.File]::ReadAllText($AcknowledgementPath) -eq $Action) {
            Start-Sleep -Milliseconds 250
            return
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $Deadline)
    throw "The isolated fixture did not acknowledge automatic event '$Action'."
}

function Assert-NativeIdAbsent([string] $Id) {
    $Condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty, $Id)
    $Window = [System.Windows.Automation.AutomationElement]::FromHandle($Process.MainWindowHandle)
    if ($null -ne $Window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $Condition)) {
        throw "Unexpected native control '$Id' remained in the current view."
    }
}

function Test-SourceEventFixture([string] $Theme, $BeforeEvents) {
    $Picker = Wait-NativeId 'SourcePicker'
    $Expansion = [System.Windows.Automation.ExpandCollapsePattern]$Picker.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
    $Selection = [System.Windows.Automation.SelectionPattern]$Picker.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern)
    $Expansion.Expand()
    Start-Sleep -Milliseconds 150
    $SelectedOptions = @($Selection.Current.GetSelection())
    if ($SelectedOptions.Count -ne 1) { throw 'Automatic source test requires one explicitly selected application.' }
    $ChosenOption = $SelectedOptions[0]
    $ChosenIdentity = $ChosenOption.GetRuntimeId() -join ','
    $ChosenOption.SetFocus()
    Start-Sleep -Milliseconds 100
    if (-not $ChosenOption.Current.HasKeyboardFocus) { throw 'The selected source popup item did not receive keyboard focus.' }
    try {
        foreach ($FixtureAction in @('source_refreshing', 'applications_changed')) {
            Send-FixtureEvent $FixtureAction
            $CurrentOptions = @($Selection.Current.GetSelection())
            if ($Expansion.Current.ExpandCollapseState -ne [System.Windows.Automation.ExpandCollapseState]::Expanded -or
                $CurrentOptions.Count -ne 1 -or ($CurrentOptions[0].GetRuntimeId() -join ',') -ne $ChosenIdentity -or
                -not $ChosenOption.Current.HasKeyboardFocus -or
                $ChosenOption.Current.Name -ne 'Microsoft Edge · 测试应用') {
                throw "Automatic event '$FixtureAction' replaced the selected item, stole focus, changed selection, or closed the source popup."
            }
            $ShouldBeRefreshing = $FixtureAction -eq 'source_refreshing'
            if ((Wait-NativeId 'SourceRetryButton').Current.IsEnabled -eq $ShouldBeRefreshing) {
                throw "Automatic event '$FixtureAction' did not render the source refresh status independently."
            }
        }
        if ($null -eq (Find-NativeOption 'VLC media player · 测试应用') -or
            $null -ne (Find-NativeOption 'Audacity · 新发现测试应用')) {
            throw 'The source popup applied structural changes before the interaction finished.'
        }
        Save-NativeScreenshot (Join-Path $EvidencePath "fixture-source-events-dropdown-$Theme.png")
    } finally { $Expansion.Collapse() }
    Start-Sleep -Milliseconds 250
    $Expansion.Expand()
    Start-Sleep -Milliseconds 150
    try {
        $CurrentOptions = @($Selection.Current.GetSelection())
        if ($null -eq (Find-NativeOption 'Audacity · 新发现测试应用') -or
            $null -ne (Find-NativeOption 'VLC media player · 测试应用') -or
            $CurrentOptions.Count -ne 1 -or ($CurrentOptions[0].GetRuntimeId() -join ',') -ne $ChosenIdentity) {
            throw 'Closing the source popup did not reconcile the new list while reusing the selected native item.'
        }
    } finally { $Expansion.Collapse() }
    $AfterEvents = Read-FixtureDiagnostics
    if ($AfterEvents.fixture_state.source_id -ne $BeforeEvents.fixture_state.source_id -or
        $AfterEvents.fixture_state.streaming_receivers -ne 2 -or
        $AfterEvents.fixture_state.automatic_events -ne ($BeforeEvents.fixture_state.automatic_events + 2) -or
        $AfterEvents.fixture_state.source_selection_commands -ne $BeforeEvents.fixture_state.source_selection_commands -or
        $AfterEvents.fixture_state.playback_commands -ne $BeforeEvents.fixture_state.playback_commands) {
        throw 'Source-list-only events selected another application or issued a playback command.'
    }
    return [ordered]@{
        verified = $true; synthetic_events = $true; real_notifications_claim = $false
        refreshing_preserves_selection_verified = $true; popup_interaction_preserved_verified = $true
        incremental_item_identity_verified = $true; deferred_list_update_verified = $true
        no_playback_commands_verified = $true; event_count = 2
    }
}

function Test-ApplicationSourceFixture([string] $Theme) {
    Set-HomeScroll 0
    $LocalMute = Wait-NativeId 'LocalMuteToggle'
    $MutePattern = [System.Windows.Automation.TogglePattern]$LocalMute.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    if (-not $LocalMute.Current.IsEnabled) { throw 'Endpoint mode unexpectedly disabled local mute.' }
    if ($MutePattern.Current.ToggleState -ne [System.Windows.Automation.ToggleState]::On) { $MutePattern.Toggle() }
    Start-Sleep -Milliseconds 350
    $BeforeSwitch = Read-FixtureDiagnostics
    if ($BeforeSwitch.mute_local -ne $true -or $BeforeSwitch.fixture_state.streaming_receivers -ne 2) {
        throw 'The initial two-receiver fixture did not acknowledge endpoint mute before mode switching.'
    }

    Select-NativeOption 'SourceModePicker' '指定应用'
    $SelectedNames = @(Get-NativeSelectionNames 'SourcePicker')
    if ($SelectedNames.Count -ne 0 -or (Wait-NativeId 'ConnectButton').Current.IsEnabled -or
        (Wait-NativeId 'TestToneButton').Current.IsEnabled) {
        throw 'Application mode selected a source or enabled playback before explicit application choice.'
    }
    $LocalMute = Wait-NativeId 'LocalMuteToggle'
    $MutePattern = [System.Windows.Automation.TogglePattern]$LocalMute.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    if ($LocalMute.Current.IsEnabled -or $MutePattern.Current.ToggleState -ne [System.Windows.Automation.ToggleState]::Off) {
        throw 'Application mode did not disable and clear local output mute.'
    }
    $AwaitingSelection = Read-FixtureDiagnostics
    if ($AwaitingSelection.capture.source_mode -ne 'application' -or $AwaitingSelection.fixture_state.source_id -ne '' -or
        $AwaitingSelection.fixture_state.source_count -ne 3 -or $AwaitingSelection.fixture_state.streaming_receivers -ne 0 -or
        $AwaitingSelection.session_state -ne 'idle' -or $AwaitingSelection.capture.state -ne 'idle' -or
        $AwaitingSelection.mute_local -ne $false) {
        throw 'Mode switching did not reach an idle, unselected application fixture through the real native command path.'
    }
    Save-NativeScreenshot (Join-Path $EvidencePath "fixture-application-awaiting-selection-$Theme.png")

    $BrowserName = 'Microsoft Edge · 测试应用'
    $BrowserId = 'process:4200:133700000000000000'
    Select-NativeOption 'SourcePicker' $BrowserName
    if (-not (Wait-NativeId 'ConnectButton').Current.IsEnabled -or (Wait-NativeId 'TestToneButton').Current.IsEnabled) {
        throw 'Explicit application selection did not become ready without automatically starting playback.'
    }
    $ButtonProof = Invoke-FixtureConnectPointer $Theme
    Assert-ActiveFixtureWaveform
    $PlayingApplication = Read-FixtureDiagnostics
    if ($PlayingApplication.capture.source_mode -ne 'application' -or $PlayingApplication.fixture_state.source_id -ne $BrowserId -or
        $PlayingApplication.fixture_state.streaming_receivers -ne 2 -or $PlayingApplication.capture.state -ne 'signal' -or
        $PlayingApplication.mute_local -ne $false) {
        throw 'The physical primary-button click did not start the explicitly selected synthetic application.'
    }

    Invoke-NativeButton (Wait-NativeId 'SourceRetryButton')
    Start-Sleep -Milliseconds 350
    $SelectedNames = @(Get-NativeSelectionNames 'SourcePicker')
    $AfterRefresh = Read-FixtureDiagnostics
    if ($SelectedNames.Count -ne 1 -or $SelectedNames[0] -ne $BrowserName -or
        $AfterRefresh.fixture_state.source_id -ne $BrowserId -or $AfterRefresh.fixture_state.streaming_receivers -ne 2 -or
        $AfterRefresh.fixture_state.refresh_count -ne ($PlayingApplication.fixture_state.refresh_count + 1) -or
        (Wait-NativeId 'LocalMuteToggle').Current.IsEnabled) {
        throw 'Refreshing application sources changed the explicit selection/playback, skipped the core command, or enabled local mute.'
    }
    $SourceEventProof = Test-SourceEventFixture $Theme $AfterRefresh
    $ApplicationThemes = Test-NativeThemes 'fixture-application' $true
    $script:FixtureThemeErrors = @($script:FixtureThemeErrors) + @($ApplicationThemes.errors)

    Select-NativeOption 'SourceModePicker' '系统声音'
    $ReturnedToSystem = Read-FixtureDiagnostics
    if ($ReturnedToSystem.capture.source_mode -ne 'endpoint' -or $ReturnedToSystem.fixture_state.streaming_receivers -ne 0 -or
        $ReturnedToSystem.session_state -ne 'idle' -or $ReturnedToSystem.capture.state -ne 'idle' -or
        $ReturnedToSystem.test_tone_active -ne $false -or $ReturnedToSystem.mute_local -ne $false -or
        @($ReturnedToSystem.receivers).Count -ne 2 -or (Wait-NativeId 'TestToneButton').Current.IsEnabled) {
        throw 'Returning to system capture did not stop application playback while preserving the receiver selection.'
    }
    return [ordered]@{
        verified = $ApplicationThemes.verified; themes = $ApplicationThemes.themes; errors = $ApplicationThemes.errors
        explicit_selection_verified = $true; mode_switch_stops_playback_verified = $true
        source_refresh_retains_selection_verified = $true; local_mute_disabled_verified = $true
        active_waveform_mode_verified = $ApplicationThemes.active_waveform_mode_verified
        button_pointer_interaction_verified = $true; button_input = $ButtonProof
        source_event_ui = $SourceEventProof
        synthetic_application_count = 3; selected_application = $BrowserName
    }
}

function Test-NativeStereoFixture([string] $Theme) {
    $GroupId = 'stereo:12345678-1234-5678-9abc-123456789abc'
    # First hide a physical member through the real settings UI, then let the
    # fixture report that the same two identities belong to a native group.
    Invoke-NativeButton (Wait-NativeId 'Receiver:fixture-living-room')
    Start-Sleep -Milliseconds 250
    Invoke-NativeButton (Wait-NativeId 'SettingsButton')
    Set-SettingsScroll 100
    Invoke-NativeButton (Wait-NativeId 'DeviceVisibility:fixture-study')
    Start-Sleep -Milliseconds 250
    Invoke-NativeButton (Wait-NativeId 'SettingsBackButton')
    Send-FixtureEvent 'native_stereo'
    Assert-NativeIdAbsent "Receiver:$GroupId"
    Assert-NativeIdAbsent 'Receiver:fixture-living-room'
    Assert-NativeIdAbsent 'Receiver:fixture-study'
    Invoke-NativeButton (Wait-NativeId 'SettingsButton')
    Set-SettingsScroll 100
    $RestoreGroup = Wait-NativeId "DeviceVisibility:$GroupId"
    if (-not $RestoreGroup.Current.Name.StartsWith('恢复显示 ')) {
        throw 'A previously hidden physical member reappeared as an unhidden native group.'
    }
    Assert-NativeIdAbsent 'DeviceVisibility:fixture-study'
    Invoke-NativeButton $RestoreGroup
    Start-Sleep -Milliseconds 250
    $HiddenPreference = Get-ItemProperty -LiteralPath $ThemeSettingsPath -Name HiddenDevices
    $RemainingHidden = @($HiddenPreference.HiddenDevices | ConvertFrom-Json)
    if (@($RemainingHidden | Where-Object { $_.id -in @($GroupId, 'fixture-living-room', 'fixture-study') }).Count -ne 0) {
        throw 'Restoring the native group did not clear both group and member hidden identities.'
    }
    Invoke-NativeButton (Wait-NativeId 'SettingsBackButton')
    Set-HomeScroll 0
    $GroupButton = Wait-NativeId "Receiver:$GroupId"
    $GroupIdentity = $GroupButton.GetRuntimeId() -join ','
    if (-not $GroupButton.Current.Name.Contains('原生立体声组合')) {
        throw 'The native stereo card omitted its native-group accessibility label.'
    }
    Invoke-NativeButton $GroupButton
    Start-Sleep -Milliseconds 250
    Invoke-NativeButton (Wait-NativeId 'ConnectButton')
    Start-Sleep -Milliseconds 350
    if ((Wait-NativeId 'VolumeHeading').Current.Name -ne '组合音量' -or
        -not (Wait-NativeId 'TestToneButton').Current.Name.Contains('测试左右声道')) {
        throw 'Selecting a native group did not expose group volume and the channel-verification test tone.'
    }
    Invoke-NativeButton (Wait-NativeId "ReceiverMembers:$GroupId")
    Start-Sleep -Milliseconds 200
    foreach ($MemberId in @('fixture-living-room', 'fixture-study')) {
        $Member = Wait-NativeId "ReceiverMember:$MemberId"
        if ($Member.Current.IsOffscreen -or -not $Member.Current.Name.Contains('传输中')) {
            throw "The expanded group did not expose the connected member '$MemberId'."
        }
    }
    $GroupSlider = Wait-NativeId 'VolumeSlider'
    $Range = [System.Windows.Automation.RangeValuePattern]$GroupSlider.GetCurrentPattern(
        [System.Windows.Automation.RangeValuePattern]::Pattern)
    $Range.SetValue(73)
    Start-Sleep -Milliseconds 350
    $GroupDiagnostics = Read-FixtureDiagnostics
    if ($GroupDiagnostics.fixture_state.native_stereo -ne $true -or
        $GroupDiagnostics.fixture_state.group_complete -ne $true -or
        $GroupDiagnostics.fixture_state.streaming_targets -ne 1 -or
        $GroupDiagnostics.fixture_state.streaming_receivers -ne 2 -or
        [Math]::Abs($GroupDiagnostics.fixture_state.volume - 0.73) -gt 0.001) {
        throw 'The logical group or its existing global-volume command did not match the synthetic two-member state.'
    }
    $GroupThemes = Test-NativeThemes 'fixture-native-stereo' $true
    $script:FixtureThemeErrors = @($script:FixtureThemeErrors) + @($GroupThemes.errors)
    Send-FixtureEvent 'stereo_partial'
    $GroupButton = Wait-NativeId "Receiver:$GroupId"
    if (($GroupButton.GetRuntimeId() -join ',') -ne $GroupIdentity -or
        -not $GroupButton.Current.Name.Contains('成员暂缺') -or $GroupButton.Current.Name.Contains('正在传输') -or
        (Wait-NativeId 'TestToneButton').Current.IsEnabled -or
        -not (Wait-NativeId 'ReceiverMember:fixture-study').Current.Name.Contains('离线') -or
        (Wait-NativeId 'ReceiverMember:fixture-study').Current.IsOffscreen) {
        throw 'A missing member replaced the group card, collapsed its details, or left the partial group labelled as streaming.'
    }
    Save-NativeScreenshot (Join-Path $EvidencePath "fixture-native-stereo-partial-$Theme.png")
    $PartialDiagnostics = Read-FixtureDiagnostics
    if ($PartialDiagnostics.fixture_state.group_complete -ne $false -or
        $PartialDiagnostics.fixture_state.group_state -eq 'streaming' -or
        $PartialDiagnostics.fixture_state.streaming_targets -ne 0 -or
        $PartialDiagnostics.fixture_state.streaming_receivers -ne 1) {
        throw 'The partial-group fixture did not preserve the surviving member while rejecting a complete streaming group.'
    }
    return [ordered]@{
        verified = $GroupThemes.verified; themes = $GroupThemes.themes; errors = $GroupThemes.errors
        synthetic_data = $true; real_stereo_sync_claim = $false
        logical_group_card_verified = $true; member_status_verified = $true
        partial_group_not_streaming_verified = $true; group_volume_verified = $true
        hidden_member_migration_verified = $true; selected_group_id = $GroupId
    }
}

function Test-PopulatedVisualFixture([string] $FixtureDirectory) {
    $PriorEnabled = $env:AIRLOOM_UI_FIXTURE
    $PriorEvents = $env:AIRLOOM_UI_FIXTURE_EVENTS
    $script:FixtureEventDirectory = Join-Path $FixtureDirectory 'fixture-events'
    $script:FixtureEventIndex = 0
    New-Item -ItemType Directory -Path $script:FixtureEventDirectory -Force | Out-Null
    try {
        $env:AIRLOOM_UI_FIXTURE = '1'
        $env:AIRLOOM_UI_FIXTURE_EVENTS = $script:FixtureEventDirectory
        Start-NativeApplication (Join-Path $FixtureDirectory 'Airloom.exe')
    } finally {
        $env:AIRLOOM_UI_FIXTURE = $PriorEnabled
        $env:AIRLOOM_UI_FIXTURE_EVENTS = $PriorEvents
    }
    $null = [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0, 1100, 800, 0x0004)
    Start-Sleep -Milliseconds 400
    foreach ($Id in @('Receiver:fixture-living-room', 'Receiver:fixture-study', 'Receiver:fixture-bedroom')) {
        $null = Wait-NativeId $Id
    }
    $ThemeProof = Test-NativeThemes 'fixture-playing' $true
    $script:FixtureThemeErrors = @($ThemeProof.errors)
    $FixtureTheme = if ($ThemeProof.themes -contains 'dark') { 'dark' } else { 'theme-unverified' }
    $null = [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0, 880, 680, 0x0004)
    Start-Sleep -Milliseconds 400
    Set-HomeScroll 0
    Assert-CompactReceiversVisible
    Save-NativeScreenshot (Join-Path $EvidencePath "fixture-playing-home-compact-$FixtureTheme.png")
    $null = [AirloomUiProbe]::SetWindowPos($Process.MainWindowHandle, [IntPtr]::Zero, 0, 0, 1100, 800, 0x0004)
    Start-Sleep -Milliseconds 400
    $Slider = Wait-NativeId 'VolumeSlider'
    $Range = [System.Windows.Automation.RangeValuePattern]$Slider.GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern)
    $Baseline = Get-NativeLayout
    foreach ($Volume in @(9, 100, 56)) {
        $Range.SetValue($Volume)
        Start-Sleep -Milliseconds 350
        if ([Math]::Abs($Range.Current.Value - $Volume) -gt 0.01) { throw 'Populated fixture volume snapped back.' }
        Assert-StableLayout $Baseline (Get-NativeLayout) 'Populated fixture volume'
    }
    # This click enters the same native password dialog used for real receivers.
    # Cancelling its request stops only that route. Production deliberately
    # keeps its selection, while the other two receivers continue playing.
    Invoke-NativeButton (Wait-NativeId 'Receiver:fixture-bedroom')
    $null = Wait-NativeButton '继续连接'
    Save-NativeScreenshot (Join-Path $EvidencePath "fixture-auth-$FixtureTheme.png")
    Invoke-NativeButton (Wait-NativeButton '取消')
    Start-Sleep -Milliseconds 400
    $Targets = (Wait-NativeId 'TargetName').Current.Name
    if (-not $Targets.Contains('客厅 HomePod') -or -not $Targets.Contains('书房 HomePod mini') -or -not $Targets.Contains('卧室')) {
        throw 'Cancelling one fixture authentication request changed the other selected receivers.'
    }
    $FixtureDiagnostics = Read-FixtureDiagnostics
    $CancelledRoutes = @($FixtureDiagnostics.receivers | Where-Object { $_.slot -eq 3 -and $_.state -eq 'idle' })
    if ($FixtureDiagnostics.fixture_state.auth_pending -ne $false -or $CancelledRoutes.Count -ne 1 -or
        $FixtureDiagnostics.fixture_state.streaming_receivers -ne 2) {
        throw 'Cancelling one authentication request failed to stop just its route and preserve the other two streams.'
    }
    Invoke-NativeButton (Wait-NativeId 'Receiver:fixture-bedroom')
    Start-Sleep -Milliseconds 350
    $ApplicationProof = Test-ApplicationSourceFixture $FixtureTheme
    $StereoProof = Test-NativeStereoFixture $FixtureTheme
    return [ordered]@{
        verified = ($ThemeProof.verified -and $ApplicationProof.verified -and $StereoProof.verified); data_source = 'CI-only replacement DLL in a separate temporary archive copy'
        synthetic_data = $true; real_audio_or_receiver_claim = $false
        themes = $ThemeProof.themes; errors = @($ThemeProof.errors) + @($ApplicationProof.errors) + @($StereoProof.errors); receivers = 3; initially_streaming = 2
        password_dialog_verified = $true; per_receiver_cancel_verified = $true
        populated_volume_retained = $true; compact_receivers_visible_at_top = $true
        active_waveform_mode_verified = $ThemeProof.active_waveform_mode_verified
        application_source_ui_verified = $ApplicationProof.verified
        button_pointer_interaction_verified = $ApplicationProof.button_pointer_interaction_verified
        application_source_ui = $ApplicationProof
        source_event_ui_verified = $ApplicationProof.source_event_ui.verified
        source_event_ui = $ApplicationProof.source_event_ui
        native_stereo_ui_verified = $StereoProof.verified
        native_stereo_ui = $StereoProof
    }
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
$FixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('Airloom-UiFixture-' + [guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $SmokeRoot
$AppPath = Join-Path $SmokeRoot 'Airloom.exe'
$ThemeSettingsPath = 'HKCU:\Software\Airloom'
$SavedTheme = Get-ItemProperty -LiteralPath $ThemeSettingsPath -Name Theme -ErrorAction SilentlyContinue
$SavedHiddenDevices = Get-ItemProperty -LiteralPath $ThemeSettingsPath -Name HiddenDevices -ErrorAction SilentlyContinue
$ShippedCoreHash = (Get-FileHash -LiteralPath (Join-Path $SmokeRoot 'airloom_core.dll') -Algorithm SHA256).Hash
$StartupTracePath = [System.IO.Path]::GetFullPath((Join-Path $EvidencePath 'native-startup.txt'))
$PriorStartupTrace = $env:AIRLOOM_STARTUP_TRACE
$Process = $null
try {
    $env:AIRLOOM_STARTUP_TRACE = $StartupTracePath
    # The large adaptive layout must be physically on-screen, not hanging off
    # a 1024 x 768 desktop. Only this disposable CI display is adjusted.
    [AirloomUiProbe]::PrepareDisplay(1600, 1040)
    Start-Sleep -Milliseconds 500
    Start-NativeApplication $AppPath
    $ShippedWindowHandle = $Process.MainWindowHandle.ToInt64()
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
    $ThemeProof = Test-NativeThemes 'native'
    Stop-NativeApplication

    # Populated screenshots run the same WinUI binary with a synthetic core in
    # another temporary directory. The shipped archive/core stay byte-identical;
    # no fixture switch, receivers or signals are added to production code.
    New-Item -ItemType Directory -Path $FixtureRoot -Force | Out-Null
    Copy-Item -Path (Join-Path $SmokeRoot '*') -Destination $FixtureRoot -Recurse -Force
    $FixtureThemeErrors = @()
    try {
        Build-VisualFixture $FixtureRoot
        # Only the isolated populated fixture uses a temporary CI animation
        # preference; the production executable still honors Windows normally.
        [AirloomUiProbe]::PrepareFixtureMotion()
        $FixtureProof = Test-PopulatedVisualFixture $FixtureRoot
    } catch {
        $FixtureProof = [ordered]@{
            verified = $false; synthetic_data = $true; real_audio_or_receiver_claim = $false
            errors = @($FixtureThemeErrors) + @("Populated fixture: $($_.Exception.Message)")
        }
        if ($null -ne $Process -and -not $Process.HasExited) {
            try { Save-NativeScreenshot (Join-Path $EvidencePath 'fixture-failure.png') } catch { }
        }
    } finally {
        try { Stop-NativeApplication } finally { [AirloomUiProbe]::RestoreFixtureMotion() }
    }
    if ((Get-FileHash -LiteralPath (Join-Path $SmokeRoot 'airloom_core.dll') -Algorithm SHA256).Hash -ne $ShippedCoreHash) {
        throw 'The populated visual fixture changed the real core used for shipped-archive verification.'
    }
    $VisualErrors = @($ThemeProof.errors) + @($FixtureProof.errors)
    $Result = @{
        result = $(if ($VisualErrors.Count -eq 0) { 'window-opened' } else { 'verification-failed' })
        distribution = 'portable-zip'
        process = 'Airloom.exe'
        hwnd = $ShippedWindowHandle
        settings_verified = $true
        update_check_verified = $true
        update_status = $UpdateStatus
        returned_home = $true
        volume_layout_verified = $true
        adaptive_layout_verified = $true
        compact_receivers_visible_at_top = $VolumeProof.compact_receivers_visible_at_top
        pointer_drag_verified = $VolumeProof.pointer_drag_verified
        pointer_input = $VolumeProof.pointer_input
        volume_measurements = $VolumeProof.measurements
        diagnostics_copy_verified = $true
        themes_verified = $ThemeProof.verified
        themes = $ThemeProof.themes
        fixture_states_verified = $FixtureProof.verified
        fixture_evidence = $FixtureProof
        verification_errors = $VisualErrors
        desktop_width = [AirloomUiProbe]::GetSystemMetrics(0)
        desktop_height = [AirloomUiProbe]::GetSystemMetrics(1)
        note = 'Checks run the real WinUI executable from the shipped archive. Verification booleans and verification_errors record which stages passed; any verification error fails CI after independent visual stages finish. Separately named fixture-* screenshots use a CI-only replacement DLL in an isolated copy for populated states, application-source controls, button input, and password-dialog checks. Fixture data is synthetic, was not shipped, and is not evidence of audio capture or transport. No physical HomePod was connected to CI.'
    } | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText((Join-Path $EvidencePath 'native-smoke.json'), $Result,
        [System.Text.UTF8Encoding]::new($false))
    if ($VisualErrors.Count -gt 0) { throw ('Native visual verification failed: ' + ($VisualErrors -join ' | ')) }
} catch {
    Write-Host "Native startup trace: $StartupTracePath"
    if (Test-Path -LiteralPath $StartupTracePath -PathType Leaf) {
        Get-Content -LiteralPath $StartupTracePath -ErrorAction Continue | Write-Host
    } else { Write-Host 'No startup trace was created before this failure.' }
    if ($null -ne $Process -and -not $Process.HasExited) {
        try { Save-NativeScreenshot (Join-Path $EvidencePath 'native-smoke-failure.png') } catch { }
    }
    throw
} finally {
    $env:AIRLOOM_STARTUP_TRACE = $PriorStartupTrace
    if ($null -ne $Process -and -not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $FixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
    if ($null -ne $SavedTheme) {
        New-ItemProperty -LiteralPath $ThemeSettingsPath -Name Theme -Value $SavedTheme.Theme -PropertyType DWord -Force | Out-Null
    } else {
        Remove-ItemProperty -LiteralPath $ThemeSettingsPath -Name Theme -ErrorAction SilentlyContinue
    }
    if ($null -ne $SavedHiddenDevices) {
        New-ItemProperty -LiteralPath $ThemeSettingsPath -Name HiddenDevices -Value $SavedHiddenDevices.HiddenDevices -PropertyType String -Force | Out-Null
    } else {
        Remove-ItemProperty -LiteralPath $ThemeSettingsPath -Name HiddenDevices -ErrorAction SilentlyContinue
    }
    try { [AirloomUiProbe]::RestoreFixtureMotion() } finally { [AirloomUiProbe]::RestoreDisplay() }
}
