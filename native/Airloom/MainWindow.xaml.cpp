#include "pch.h"
#include "MainWindow.xaml.h"
#include "StartupTrace.h"
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

#include <cstdio>
#include <iterator>
#include <winrt/Windows.ApplicationModel.DataTransfer.h>
#include <winrt/Windows.Foundation.Numerics.h>

using namespace winrt;
using namespace Windows::Foundation;
using namespace Windows::Foundation::Numerics;
using namespace Windows::Data::Json;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;
using namespace Microsoft::UI::Xaml::Controls::Primitives;
using namespace Microsoft::UI::Xaml::Media;
using namespace Microsoft::UI::Xaml::Shapes;
using airloom::native::Device;
using airloom::native::Preferences;
using airloom::native::Snapshot;

namespace
{
    struct RenderGuard
    {
        explicit RenderGuard(bool& flag) : m_flag(flag) { m_flag = true; }
        ~RenderGuard() { m_flag = false; }
        bool& m_flag;
    };

    JsonObject MakeCommand(wchar_t const* type)
    {
        JsonObject command;
        command.Insert(L"type", JsonValue::CreateStringValue(type));
        return command;
    }

    bool IsActive(hstring const& state)
    {
        return state == L"streaming" || state == L"connecting" ||
            state == L"reconnecting" || state == L"auth_required";
    }

    bool HasVolumeTarget(Snapshot const& snapshot)
    {
        return std::any_of(snapshot.devices.begin(), snapshot.devices.end(), [&](Device const& device)
            {
                return snapshot.IsSelected(device.id) && (IsActive(device.state) ||
                    std::any_of(device.members.begin(), device.members.end(), [&](auto const& member)
                        { return IsActive(member.state); }));
            });
    }

    bool HasConnectingVolumeTarget(Snapshot const& snapshot)
    {
        auto const connecting = [](hstring const& state)
            { return state == L"connecting" || state == L"reconnecting" || state == L"auth_required"; };
        return std::any_of(snapshot.devices.begin(), snapshot.devices.end(), [&](Device const& device)
            {
                return snapshot.IsSelected(device.id) && (connecting(device.state) ||
                    std::any_of(device.members.begin(), device.members.end(), [&](auto const& member)
                        { return connecting(member.state); }));
            });
    }

    bool HighContrastEnabled()
    {
        HIGHCONTRASTW settings{sizeof(settings)};
        return SystemParametersInfoW(SPI_GETHIGHCONTRAST, sizeof(settings), &settings, 0) &&
            (settings.dwFlags & HCF_HIGHCONTRASTON) != 0;
    }

    Windows::UI::Color SystemColor(int index)
    {
        auto const color = GetSysColor(index);
        return Windows::UI::Color{255, GetRValue(color), GetGValue(color), GetBValue(color)};
    }

    bool SameDevices(Snapshot const& a, Snapshot const& b)
    {
        return a.selectedDeviceIds == b.selectedDeviceIds && a.devices.size() == b.devices.size() &&
            std::equal(a.devices.begin(), a.devices.end(), b.devices.begin(), [](Device const& left, Device const& right)
                {
                    return left.id == right.id && left.name == right.name && left.model == right.model &&
                        left.address == right.address && left.passwordRequired == right.passwordRequired && left.online == right.online &&
                        left.selected == right.selected && left.state == right.state && left.statusMessage == right.statusMessage &&
                        left.errorMessage == right.errorMessage && left.kind == right.kind &&
                        left.expectedMembers == right.expectedMembers && left.groupComplete == right.groupComplete &&
                        left.members.size() == right.members.size() &&
                        std::equal(left.members.begin(), left.members.end(), right.members.begin(), [](auto const& memberLeft, auto const& memberRight)
                            { return memberLeft.id == memberRight.id && memberLeft.name == memberRight.name &&
                                memberLeft.online == memberRight.online && memberLeft.state == memberRight.state &&
                                memberLeft.statusMessage == memberRight.statusMessage; });
                });
    }

    bool SameSources(Snapshot const& a, Snapshot const& b)
    {
        return a.sourceMode == b.sourceMode && a.selectedSourceId == b.selectedSourceId && a.sources.size() == b.sources.size() &&
            std::equal(a.sources.begin(), a.sources.end(), b.sources.begin(), [](auto const& left, auto const& right)
                { return left.id == right.id && left.name == right.name && left.kind == right.kind; });
    }

    hstring Decibels(double level)
    {
        if (level <= 0.00001) return L"−∞ dB";
        wchar_t formatted[32]{};
        swprintf_s(formatted, L"%.0f dB", 20.0 * std::log10(level));
        return formatted;
    }

    hstring DurationText(uint64_t seconds)
    {
        wchar_t formatted[48]{};
        if (seconds >= 3600)
            swprintf_s(formatted, L"%llu:%02llu:%02llu", seconds / 3600, (seconds / 60) % 60, seconds % 60);
        else
            swprintf_s(formatted, L"%02llu:%02llu", seconds / 60, seconds % 60);
        return formatted;
    }

    hstring SampleRateLabel(uint32_t sampleRate)
    {
        if (sampleRate == 0) return L"—";
        wchar_t formatted[32]{};
        if (sampleRate % 1000 == 0)
            swprintf_s(formatted, L"%u kHz", sampleRate / 1000);
        else
            swprintf_s(formatted, L"%.1f kHz", sampleRate / 1000.0);
        return formatted;
    }

    // Counts are read at a glance; group thousands the way Windows does.
    hstring CountText(uint64_t value)
    {
        std::wstring digits = std::to_wstring(value);
        std::wstring grouped;
        grouped.reserve(digits.size() + digits.size() / 3);
        auto const lead = digits.size() % 3;
        for (size_t index = 0; index < digits.size(); ++index)
        {
            if (index != 0 && (index + 3 - lead) % 3 == 0) grouped.push_back(L',');
            grouped.push_back(digits[index]);
        }
        return hstring{grouped};
    }

    // Segoe Fluent Icons volume glyphs, chosen like the Windows volume flyout.
    wchar_t const* VolumeGlyphFor(double percent)
    {
        if (percent <= 0.0) return L"";
        if (percent < 34.0) return L"";
        if (percent < 67.0) return L"";
        return L"";
    }

    void SetAccessibleName(DependencyObject const& element, hstring const& name)
    {
        Automation::AutomationProperties::SetName(element, name);
    }

    void SetText(TextBlock const& target, hstring const& value)
    {
        if (target.Text() != value) target.Text(value);
    }

    Style AppStyle(wchar_t const* key)
    {
        return Application::Current().Resources().Lookup(box_value(key)).as<Style>();
    }

    // Windows 11's decelerating curve for things that arrive.
    Microsoft::UI::Composition::CompositionEasingFunction Decelerate(Microsoft::UI::Composition::Compositor const& compositor)
    {
        return compositor.CreateCubicBezierEasingFunction(float2{0.1f, 0.9f}, float2{0.2f, 1.0f});
    }

    // Segoe Fluent Icons used by elements built in code.
    constexpr wchar_t GlyphSpeaker[] = L"\xE7F5";
    constexpr wchar_t GlyphDesktop[] = L"\xE7F4";
    constexpr wchar_t GlyphApp[] = L"\xECAA";
    constexpr wchar_t GlyphWarning[] = L"\xE7BA";
    constexpr wchar_t GlyphLock[] = L"\xE72E";
    constexpr wchar_t GlyphAdd[] = L"\xE710";
    constexpr wchar_t GlyphPlay[] = L"\xE768";
    constexpr wchar_t GlyphStop[] = L"\xE71A";
    constexpr wchar_t GlyphChevronDown[] = L"\xE70D";
    constexpr wchar_t GlyphChevronUp[] = L"\xE70E";

    struct RoutePoint
    {
        double x{};
        double y{};
    };

    // A route leaves the source level, bends once and arrives level at its speaker.
    RoutePoint RoutePointAt(double startX, double startY, double endX, double endY, double t)
    {
        double const span = endX - startX;
        double const x1 = startX + span * 0.5;
        double const x2 = endX - span * 0.42;
        double const u = 1.0 - t;
        double const a = u * u * u;
        double const b = 3.0 * u * u * t;
        double const c = 3.0 * u * t * t;
        double const d = t * t * t;
        return RoutePoint{a * startX + b * x1 + c * x2 + d * endX, a * startY + b * startY + c * endY + d * endY};
    }

    double RouteLength(double startX, double startY, double endX, double endY)
    {
        double length{};
        auto previous = RoutePointAt(startX, startY, endX, endY, 0.0);
        for (int step = 1; step <= 32; ++step)
        {
            auto const point = RoutePointAt(startX, startY, endX, endY, step / 32.0);
            length += std::hypot(point.x - previous.x, point.y - previous.y);
            previous = point;
        }
        return length;
    }

    // Shape dash lengths are multiples of the stroke thickness.
    DoubleCollection Dashes(double dash, double gap)
    {
        DoubleCollection pattern;
        pattern.Append(dash);
        pattern.Append(gap);
        return pattern;
    }

    // A connected route is drawn from the computer to the speaker once (500 ms,
    // decelerating), then stays solid.
    Media::Animation::Storyboard DrawPath(Shapes::Path const& path, double length)
    {
        double const units = std::max(1.0, length / 2.0) + 2.0;
        path.StrokeDashArray(Dashes(units, units));
        path.StrokeDashCap(PenLineCap::Flat);
        path.StrokeDashOffset(units);
        Media::Animation::DoubleAnimation draw;
        draw.From(Windows::Foundation::IReference<double>(units));
        draw.To(Windows::Foundation::IReference<double>(0.0));
        draw.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(500)));
        draw.EnableDependentAnimation(true);
        Media::Animation::ExponentialEase ease;
        ease.Exponent(6.0);
        ease.EasingMode(Media::Animation::EasingMode::EaseOut);
        draw.EasingFunction(ease);
        Media::Animation::Storyboard::SetTarget(draw, path);
        Media::Animation::Storyboard::SetTargetProperty(draw, L"StrokeDashOffset");
        Media::Animation::Storyboard storyboard;
        storyboard.Children().Append(draw);
        storyboard.Completed([path](auto const&, auto const&)
            {
                path.ClearValue(Shapes::Shape::StrokeDashArrayProperty());
                path.ClearValue(Shapes::Shape::StrokeDashOffsetProperty());
            });
        storyboard.Begin();
        return storyboard;
    }

    // A connecting route marches its dashes toward the speaker.
    Media::Animation::Storyboard MarchPath(Shapes::Path const& path)
    {
        Media::Animation::DoubleAnimation march;
        march.From(Windows::Foundation::IReference<double>(0.0));
        march.To(Windows::Foundation::IReference<double>(-7.0));
        march.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(700)));
        march.RepeatBehavior(Media::Animation::RepeatBehaviorHelper::Forever());
        march.EnableDependentAnimation(true);
        Media::Animation::Storyboard::SetTarget(march, path);
        Media::Animation::Storyboard::SetTargetProperty(march, L"StrokeDashOffset");
        Media::Animation::Storyboard storyboard;
        storyboard.Children().Append(march);
        storyboard.Begin();
        return storyboard;
    }
}

namespace winrt::Airloom::implementation
{
    MainWindow::MainWindow()
    {
        airloom::native::TraceStartup("Window: initialize XAML");
        InitializeComponent();
        airloom::native::TraceStartup("Window: XAML ready");
        InitializeWindow();
        airloom::native::TraceStartup("Window: native window ready");
        InitializePreferences();
        airloom::native::TraceStartup("Window: preferences ready");

        m_pollTimer = DispatcherTimer();
        m_pollTimer.Interval(std::chrono::microseconds(33333));
        m_pollTimer.Tick([weak = get_weak()](auto const&, auto const&)
            {
                if (auto self = weak.get()) self->PollEngine();
            });
        m_volumeTimer = DispatcherTimer();
        m_volumeTimer.Interval(std::chrono::milliseconds(180));
        m_volumeTimer.Tick([weak = get_weak()](auto const&, auto const&)
            {
                if (auto self = weak.get())
                {
                    self->FlushVolume();
                }
            });

        // Slider handles pointer events internally. Observe the handled events too,
        // without capturing the pointer or changing the control's native behavior.
        VolumeSlider().AddHandler(UIElement::PointerPressedEvent(), box_value(Input::PointerEventHandler(
            [weak = get_weak()](auto const&, auto const&)
            { if (auto self = weak.get()) self->m_volumePointerDown = true; })), true);
        auto finishVolume = Input::PointerEventHandler([weak = get_weak()](auto const&, auto const&)
            { if (auto self = weak.get()) self->FinishVolumeInteraction(); });
        VolumeSlider().AddHandler(UIElement::PointerReleasedEvent(), box_value(finishVolume), true);
        VolumeSlider().AddHandler(UIElement::PointerCaptureLostEvent(), box_value(finishVolume), true);
        VolumeSlider().AddHandler(UIElement::PointerCanceledEvent(), box_value(finishVolume), true);

        Closed([weak = get_weak()](auto const&, auto const&)
            {
                if (auto self = weak.get())
                {
                    if (self->m_pollTimer) self->m_pollTimer.Stop();
                    if (self->m_volumeTimer) self->m_volumeTimer.Stop();
                    self->m_engine.Shutdown();
                    self->m_desktop.Detach();
                }
            });
    }

    MainWindow::~MainWindow()
    {
        if (m_pollTimer) m_pollTimer.Stop();
        if (m_volumeTimer) m_volumeTimer.Stop();
    }

    void MainWindow::Start(bool background)
    {
        m_background = background;
        Activate();
    }

    void MainWindow::InitializeWindow()
    {
        airloom::native::TraceStartup("Window: obtain HWND");
        Microsoft::UI::Xaml::Window projectedWindow = *this;
        auto nativeWindow = projectedWindow.as<IWindowNative>();
        check_hresult(nativeWindow->get_WindowHandle(&m_hwnd));
        Title(L"Airloom");
        airloom::native::TraceStartup("Window: configure titlebar");
        ExtendsContentIntoTitleBar(true);
        SetTitleBar(TitleDragRegion());
        // A 48px title bar with matching caption buttons, as in Windows' own apps.
        AppWindow().TitleBar().PreferredHeightOption(Microsoft::UI::Windowing::TitleBarHeightOption::Tall);

        auto const dpi = GetDpiForWindow(m_hwnd);
        auto const width = MulDiv(1120, static_cast<int>(dpi), 96);
        auto const height = MulDiv(780, static_cast<int>(dpi), 96);
        MONITORINFO monitor{sizeof(monitor)};
        GetMonitorInfoW(MonitorFromWindow(m_hwnd, MONITOR_DEFAULTTONEAREST), &monitor);
        auto const usableWidth = static_cast<int>(monitor.rcWork.right - monitor.rcWork.left);
        auto const usableHeight = static_cast<int>(monitor.rcWork.bottom - monitor.rcWork.top);
        auto const actualWidth = std::min(width, usableWidth);
        auto const actualHeight = std::min(height, usableHeight);
        airloom::native::TraceStartup("Window: resize");
        SetWindowPos(m_hwnd, nullptr, monitor.rcWork.left + (usableWidth - actualWidth) / 2,
            monitor.rcWork.top + (usableHeight - actualHeight) / 2,
            actualWidth, actualHeight, SWP_NOZORDER | SWP_NOACTIVATE);

        airloom::native::TraceStartup("Window: attach desktop");
        m_desktop.Attach(m_hwnd,
            [this] { ShowWindow(); },
            [this] { ExitNow(); });
        airloom::native::TraceStartup("Window: apply theme");
        ApplyTheme();
    }

    void MainWindow::InitializePreferences()
    {
        m_highContrast = HighContrastEnabled();
        m_hiddenDevices = Preferences::HiddenDevices();
        ThemePicker().SelectedIndex(static_cast<int32_t>(Preferences::Theme()));
        m_visualizerEnabled = Preferences::Visualizer();
        VisualizerToggle().IsOn(m_visualizerEnabled);
        StartupToggle().IsOn(Preferences::StartsWithWindows());
        auto const theme = Preferences::Theme();
        // The window content owns the theme, so Mica follows the same choice.
        WindowRoot().RequestedTheme(theme == 1 ? ElementTheme::Light : theme == 2 ? ElementTheme::Dark : ElementTheme::Default);
        m_preferencesReady = true;
        ApplyTheme();
    }

    void MainWindow::OnLoaded(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_loaded) return;
        airloom::native::TraceStartup("Window: loaded event");
        m_loaded = true;
        BOOL animations = TRUE;
        if (SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &animations, 0))
            m_motionAllowed = animations != FALSE;
        m_deviceTransitions = DeviceCards().ChildrenTransitions();
        m_visibilityTransitions = DeviceVisibilityList().ChildrenTransitions();
        m_motionApplied = !m_motionAllowed;
        ApplyMotionPreference();
        UpdateWorkspaceLayout(LayoutRoot().ActualWidth());
        if (m_engine.Available())
        {
            Send(L"refresh");
            PollEngine();
        }
        else
        {
            ErrorBanner().Title(L"音频引擎未就绪");
            ErrorBanner().Message(m_engine.LoadError());
            ErrorBanner().Visibility(Visibility::Visible);
            ErrorBanner().IsOpen(true);
            RetryButton().IsEnabled(false);
            RefreshButton().IsEnabled(false);
            SourcePicker().IsEnabled(false);
            SourcePicker().PlaceholderText(L"音频引擎未就绪");
            StatusBadge().Text(L"需要安装音频引擎");
            StatusBadge().Foreground(Brush(L"DangerBrush"));
            StatusPill().Background(Brush(L"CriticalWashBrush"));
            StatusDot().Fill(Brush(L"DangerBrush"));
            StatusDescription().Text(L"重新安装后，设备与声音来源会自动显示在这里。");
        }
        RenderUpdate();
        RenderRoute(m_snapshot);
        m_pollTimer.Start();
        airloom::native::TraceStartup("Window: polling ready");
        if (!m_desktop.HasTrayIcon())
        {
            FooterMessage().Text(L"系统托盘暂不可用，关闭窗口会退出 Airloom。");
        }
        if (m_background && m_desktop.HasTrayIcon())
        {
            ::ShowWindow(m_hwnd, SW_HIDE);
        }
    }

    Media::Brush MainWindow::Brush(wchar_t const* resource)
    {
        // Theme dictionaries are resolved by XAML for static elements. Rows built in
        // code obtain the matching dictionary explicitly when the actual theme changes.
        auto dictionaries = Application::Current().Resources().ThemeDictionaries();
        auto key = LayoutRoot().ActualTheme() == ElementTheme::Light ? L"Light" : L"Default";
        if (m_highContrast) key = L"HighContrast";
        if (dictionaries.HasKey(box_value(key)))
        {
            auto dictionary = dictionaries.Lookup(box_value(key)).as<ResourceDictionary>();
            if (dictionary.HasKey(box_value(resource)))
                return dictionary.Lookup(box_value(resource)).as<Media::Brush>();
        }
        return Application::Current().Resources().Lookup(box_value(resource)).as<Media::Brush>();
    }

    void MainWindow::PollEngine()
    {
        if (m_exiting || !m_loaded) return;
        // A hidden, minimized or covered home view has no continuous visuals.
        // Reconcile rows even when the underlying receivers have not changed.
        bool const homeVisible = !m_settingsOpen && IsWindowVisible(m_hwnd) && !IsIconic(m_hwnd);
        if (homeVisible != m_homeMotionVisible)
        {
            m_homeMotionVisible = homeVisible;
            m_forceDeviceRender = true;
            m_routeSignature.clear();
        }
        RenderUpdate();
        if (m_exiting || !m_engine.Available()) return;
        ++m_tickCount;
        // Keep discovery/reconnect current while minimizing work when only the tray is visible.
        if (!IsWindowVisible(m_hwnd) && m_tickCount % 8 != 0) return;
        if (m_tickCount % 30 == 1)
        {
            BOOL enabled = TRUE;
            if (SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &enabled, 0))
                m_motionAllowed = enabled != FALSE;
            if (m_motionApplied != m_motionAllowed) ApplyMotionPreference();
            auto const highContrast = HighContrastEnabled();
            if (m_highContrast != highContrast)
            {
                m_highContrast = highContrast;
                ApplyTheme();
            }
        }
        try
        {
            auto next = Snapshot::Parse(m_engine.Snapshot());
            RenderSnapshot(next);
            m_snapshot = std::move(next);
            if (m_authDialog && (m_snapshot.authRequestId == 0 || m_dialogRequestId != m_snapshot.authRequestId))
            {
                m_authDialog.Hide();
            }
            if (m_snapshot.authRequestId == 0)
            {
                m_authFingerprint.clear();
            }
            else if (IsWindowVisible(m_hwnd) && !m_authDialog && m_authFingerprint != AuthenticationFingerprint())
            {
                ShowAuthentication();
            }
        }
        catch (hresult_error const& error)
        {
            ShowLocalError(L"无法读取音频状态：" + error.message());
        }
        catch (std::exception const&)
        {
            ShowLocalError(L"无法读取音频状态。请重新打开 Airloom。");
        }
    }

    void MainWindow::RenderSnapshot(Snapshot const& next)
    {
        RenderGuard guard(m_rendering);
        bool const active = IsActive(next.state);
        auto const streamingCount = next.StreamingCount();
        bool const streaming = streamingCount > 0;
        // While some speakers already play, a speaker that joins shows its own
        // progress on its row and route; the session itself stays "streaming".
        bool const busy = !streaming && (next.state == L"connecting" || next.state == L"reconnecting");
        bool const auth = next.authRequestId != 0;
        bool const pin = next.authKind == L"pin";
        bool const discovering = next.state == L"discovering";
        bool const sourceError = next.errorCode == L"capture_sources";
        bool const applicationSource = next.sourceMode == L"application";
        auto const device = next.SelectedDevice();
        bool const selectedStereoGroup = std::any_of(next.devices.begin(), next.devices.end(), [&next](Device const& entry)
            { return entry.kind == L"native_stereo" && next.IsSelected(entry.id); });
        auto const selectedSource = std::find_if(next.sources.begin(), next.sources.end(), [&next](auto const& source)
            { return source.id == next.selectedSourceId; });
        bool const hasSource = selectedSource != next.sources.end();
        bool const devicesChanged = !m_devicesRendered || m_forceDeviceRender || !SameDevices(m_snapshot, next) ||
            m_snapshot.authRequestId != next.authRequestId || m_snapshot.authDeviceId != next.authDeviceId ||
            m_snapshot.authKind != next.authKind;
        if (devicesChanged)
        {
            RenderDevices(next);
            if (m_settingsOpen) RenderDeviceVisibility(next);
            else m_visibilityRendered = false;
        }
        if (!m_sourcesRendered || !SameSources(m_snapshot, next) ||
            (m_sourcesPending && !SourcePicker().IsDropDownOpen())) RenderSources(next);
        // Status-only refreshes never rebuild, disable or close an open picker.
        if (!SourcePicker().IsDropDownOpen()) SourcePicker().IsEnabled(!next.sources.empty());
        SourcePicker().PlaceholderText(next.sourcesRefreshing && next.sources.empty() ? L"正在读取…" : applicationSource
            ? (next.sources.empty() ? L"让应用播放声音后会自动出现" : L"选择要推流的应用")
            : (next.sources.empty() ? L"未找到可用的播放设备" : L"选择 Windows 播放设备"));
        SourceModePicker().IsEnabled(m_engine.Available());

        std::vector<Device const*> chosen;
        size_t failedCount{};
        Device const* authDevice{};
        for (auto const& entry : next.devices)
        {
            if (!next.IsSelected(entry.id) || IsDeviceHidden(entry)) continue;
            chosen.push_back(&entry);
            if (entry.state == L"error") ++failedCount;
            if (entry.state == L"auth_required" && (!authDevice || entry.id == next.authDeviceId)) authDevice = &entry;
        }
        auto const visibleCount = static_cast<size_t>(std::count_if(next.devices.begin(), next.devices.end(), [this](Device const& entry)
            { return !IsDeviceHidden(entry); }));
        bool const hasStereoGroup = std::any_of(next.devices.begin(), next.devices.end(), [this](Device const& entry)
            { return entry.kind == L"native_stereo" && !IsDeviceHidden(entry); });
        auto const hiddenCount = next.devices.size() - visibleCount;
        bool const hiddenOnly = visibleCount == 0 && !m_hiddenDevices.empty();
        hstring const unit = hasStereoGroup ? L" 个" : L" 台";
        if (visibleCount == 0)
            SetText(DeviceCount(), discovering || !hiddenOnly ? L"正在同一网络中寻找…" : L"没有可显示的扬声器");
        else
            SetText(DeviceCount(), to_hstring(visibleCount) + (hasStereoGroup ? L" 个接收目标 · 已选 " : L" 台设备 · 已选 ") +
                to_hstring(chosen.size()) + unit + (hiddenCount > 0 ? L" · " + to_hstring(hiddenCount) + L" 个已隐藏" : hstring{}));
        SetText(EmptyDeviceTitle(), discovering ? L"正在寻找附近的扬声器" : hiddenOnly ? L"扬声器都已隐藏" : L"还没有发现扬声器");
        SetText(EmptyDeviceDescription(), hiddenOnly ? L"可在“管理设备”中恢复显示，或重新发现附近的扬声器。" :
            discovering ? L"确认 HomePod 和这台电脑连在同一网络，找到后会自动出现在这里。" : L"将 HomePod 与电脑连接同一网络，然后重新发现。");
        UpdateRadar(discovering && visibleCount == 0);
        SpinGlyph(RefreshGlyph(), discovering, m_discoverySpinRunning);

        hstring const sourceName = hasSource ? selectedSource->name : next.captureSourceName;
        hstring const sourceLabel = applicationSource
            ? (hasSource ? L" " + sourceName + L" 的声音" : hstring(L"应用声音"))
            : hstring(L"系统声音");
        hstring names;
        for (auto const entry : chosen)
        {
            if (!names.empty()) names = names + L"、";
            names = names + entry->name;
        }

        // One sentence each for: who receives (headline), what is happening
        // (subline), and a short state in the pill. Tone: 0 rest, 1 live,
        // 2 busy, 3 attention, 4 problem.
        hstring headline = names;
        hstring description;
        hstring badge = L"未连接";
        hstring action = L"开始投放";
        hstring actionName;
        int tone = 0;
        if (visibleCount == 0)
        {
            headline = hiddenOnly && !discovering ? L"扬声器都已隐藏" : L"还没有发现扬声器";
            description = discovering ? L"正在同一网络中寻找 HomePod 和其他 AirPlay 2 扬声器…" :
                hiddenOnly ? L"可在“管理设备”中恢复显示，或重新发现附近的扬声器。" : L"将 HomePod 与电脑连接同一网络，然后重新发现。";
            if (discovering) { badge = L"正在发现"; tone = 2; }
        }
        else if (chosen.empty())
        {
            headline = L"选择要接收声音的扬声器";
            description = m_wideLayout ? L"在右侧勾选一台或多台，同一份声音会同时送达。" : L"在上方勾选一台或多台，同一份声音会同时送达。";
        }
        else
        {
            description = L"准备就绪 · 开始后会把" + sourceLabel + L"送到这里";
            badge = L"准备就绪";
        }
        if (streaming)
        {
            badge = L"正在投放 · " + to_hstring(streamingCount) + unit;
            tone = 1;
            action = L"停止投放";
            if (next.testToneActive)
            {
                description = selectedStereoGroup ? L"正在测试左右声道：先左，后右（3 秒），随后恢复所选声音。" :
                    L"正在播放测试音，结束后恢复所选声音。";
            }
            else if (authDevice)
            {
                description = authDevice->name + (pin ? L" 需要配对码" : L" 需要访问密码") + L" · 其余设备继续投放";
                tone = 3;
            }
            else if (failedCount > 0)
            {
                description = to_hstring(failedCount) + L" 台连接失败 · 其余设备继续投放";
                tone = 3;
            }
            else
            {
                description = L"正在投放" + sourceLabel + (applicationSource || sourceName.empty() ? hstring{} : L" · 来自 " + sourceName);
            }
        }
        else if (next.state == L"connecting")
        {
            badge = L"正在连接"; tone = 2; description = L"正在建立连接，请稍候…";
            action = L"正在连接…"; actionName = L"正在连接，点击取消";
        }
        else if (next.state == L"reconnecting")
        {
            badge = L"正在重连"; tone = 2; description = L"连接暂时中断，正在尝试恢复…";
            action = L"正在重连…"; actionName = L"正在重连，点击停止";
        }
        if (auth && !streaming)
        {
            hstring const authName = authDevice ? authDevice->name : next.authDeviceName;
            badge = pin ? L"需要配对码" : L"需要密码";
            tone = 3;
            description = (authName.empty() ? hstring(L"所选设备") : authName) + (pin ? L" 需要配对码才能继续" : L" 需要访问密码才能继续");
            action = pin ? L"输入配对码" : L"输入访问密码";
            actionName = hstring{};
        }
        if (next.state == L"error" && !streaming && !auth)
        {
            badge = L"需要处理"; tone = 4; description = L"连接没有成功，查看上方提示后再试一次。";
            action = L"重新连接"; actionName = hstring{};
        }
        // Keep an actionable failure in one place. Capture-source problems belong
        // to the source; session/network errors retain the banner.
        if ((sourceError || (streaming && !hasSource)) && !auth)
        {
            badge = applicationSource ? L"等待选择应用" : sourceError ? L"需要处理" : L"等待音频输出";
            tone = applicationSource || !sourceError ? 3 : 4;
            if (streaming)
                description = applicationSource ? L"还没选择应用 · 扬声器连接保持，正在发送静音" : L"扬声器连接已保留；选择声音来源后继续投放。";
            else if (active)
                description = L"扬声器正在连接；完成声音来源设置后继续投放。";
            else if (!applicationSource && next.sources.empty())
                description = L"连接或启用一个 Windows 播放设备，然后刷新声音来源。";
            else
                description = device ? L"完成声音来源设置后，就能连接已选扬声器。" : L"先选择声音来源和扬声器。";
            if (!active) { action = L"开始投放"; actionName = hstring{}; }
        }
        if (headline.empty()) headline = L"选择要接收声音的扬声器";

        bool const labelChanged = ConnectLabel().Text() != action;
        SetText(StatusBadge(), badge);
        SetText(TargetName(), headline);
        SetText(StatusDescription(), description);
        ToolTipService::SetToolTip(StatusDescription(), box_value(description));
        ToolTipService::SetToolTip(TargetName(), box_value(headline));
        StatusPill().Background(Brush(tone == 1 ? L"SignalWashBrush" : tone == 3 ? L"CautionWashBrush" :
            tone == 4 ? L"CriticalWashBrush" : L"PillRestBrush"));
        StatusBadge().Foreground(Brush(tone == 1 ? L"SignalBrush" : tone == 3 ? L"WarningBrush" :
            tone == 4 ? L"DangerBrush" : L"MutedBrush"));
        StatusDot().Fill(Brush(tone == 1 || tone == 2 ? L"SignalFillBrush" : tone == 3 ? L"WarningBrush" :
            tone == 4 ? L"DangerBrush" : L"FaintBrush"));
        UpdatePulse(tone == 1, tone == 2);
        SetText(ConnectLabel(), action);
        SetAccessibleName(ConnectButton(), actionName.empty() ? action : actionName);
        if (labelChanged && m_devicesRendered) SwapConnectIcon();
        bool const selectedOnline = std::any_of(next.devices.begin(), next.devices.end(), [&next](Device const& entry)
            { return next.IsSelected(entry.id) && entry.online; });
        bool const canConnect = active || (selectedOnline && hasSource);
        ConnectButton().IsEnabled(canConnect);
        ConnectGlyph().Glyph(auth && !streaming ? GlyphLock : streaming || active ? GlyphStop : GlyphPlay);
        bool const animateConnecting = busy && m_motionAllowed && m_homeMotionVisible;
        ConnectGlyph().Visibility(animateConnecting ? Visibility::Collapsed : Visibility::Visible);
        ConnectSpinner().Visibility(animateConnecting ? Visibility::Visible : Visibility::Collapsed);
        ConnectSpinner().IsActive(animateConnecting);
        CancelAuthButton().Visibility(auth ? Visibility::Visible : Visibility::Collapsed);
        ForgetButton().IsEnabled(device != nullptr && !active);
        ForgetLabel().Text(device ? L"忘记“" + device->name + L"”" : L"忘记设备凭据");
        ToolTipService::SetToolTip(ForgetButton(), box_value(device ? L"忘记“" + device->name + L"”的凭据" : hstring(L"先在首页选择一台扬声器")));
        // Only a user adjustment sends volume. Device readings update this control
        // under RenderGuard; local input stays authoritative until acknowledged.
        bool const canAdjustVolume = m_engine.Available() && HasVolumeTarget(next);
        if (!canAdjustVolume || m_snapshot.selectedDeviceIds != next.selectedDeviceIds)
        {
            // A queued gesture belongs to its original receivers, never a future
            // selection. Clear it before disabling releases pointer capture.
            if (m_volumeTimer) m_volumeTimer.Stop();
            m_volumePending = false;
            m_volumeAwaitingAck = false;
            m_volumePointerDown = false;
        }
        VolumeSlider().IsEnabled(canAdjustVolume);
        bool const volumeKnown = canAdjustVolume && next.volumeKnown;
        auto const volumeHeading = device && next.selectedDeviceIds.size() == 1 && device->kind == L"native_stereo"
            ? L"组合音量" : L"扬声器音量";
        SetText(VolumeHeading(), volumeHeading);
        SetAccessibleName(VolumeSlider(), volumeHeading);
        if (m_volumeAwaitingAck && !m_volumePending)
        {
            if (next.volumeKnown && !next.volumeMixed && std::abs(next.volume - m_pendingVolume) < 0.0005)
                m_volumeAwaitingAck = false;
            else if (HasConnectingVolumeTarget(next) || HasConnectingVolumeTarget(m_snapshot))
                m_volumeAckDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
            else if (!m_volumePointerDown && std::chrono::steady_clock::now() >= m_volumeAckDeadline)
            {
                m_volumeAwaitingAck = false;
                ShowLocalError(next.volumeKnown
                    ? L"音量调整尚未确认，已恢复显示当前音量。请重试。"
                    : L"未能确认扬声器音量，请稍后再试。");
            }
        }
        if (!m_volumePending && !m_volumeAwaitingAck && !m_volumePointerDown)
        {
            auto const value = std::round(next.volume * 100.0);
            if (VolumeSlider().Value() != value) VolumeSlider().Value(value);
            SetText(VolumeLabel(), !volumeKnown ? hstring(L"—") : next.volumeMixed ? hstring(L"不同") :
                to_hstring(static_cast<int>(value)) + L"%");
            UpdateVolumeGlyph(value);
        }
        hstring const volumeHelp = !canAdjustVolume ? L"连接扬声器后可调节音量。"
            : m_volumePending || m_volumeAwaitingAck
            ? L"正在调整所选扬声器的音量。"
            : !next.volumeKnown ? L"尚未读取到所选扬声器的当前音量；手动调节会设置所选扬声器的音量。"
            : next.volumeMixed ? L"所选扬声器的音量不同，滑块参考第一台设备；只有手动调节才会统一音量。"
            : L"当前音量来自所选扬声器；手动调节会设置所选扬声器的音量。";
        ToolTipService::SetToolTip(VolumeSlider(), box_value(volumeHelp));
        ToolTipService::SetToolTip(VolumeSliderHost(), box_value(volumeHelp));
        ToolTipService::SetToolTip(VolumeLabel(), box_value(volumeHelp));
        Automation::AutomationProperties::SetHelpText(VolumeSlider(), volumeHelp);
        SetAccessibleName(VolumeLabel(), !volumeKnown && !m_volumePending && !m_volumeAwaitingAck
            ? hstring(L"扬声器音量未读取") : VolumeLabel().Text());
        LocalMuteToggle().IsEnabled(hasSource && !applicationSource);
        LocalMuteLabel().Foreground(Brush(hasSource && !applicationSource ? L"InkBrush" : L"DisabledTextBrush"));
        LocalMuteGlyph().Foreground(Brush(hasSource && !applicationSource ? L"MutedBrush" : L"DisabledTextBrush"));
        ToolTipService::SetToolTip(LocalMuteToggle(), box_value(applicationSource
            ? L"指定应用模式保留本机播放，不改变电脑整体静音状态。"
            : L"静音所选 Windows 输出；断开后恢复。若采集无声，请先关闭。"));
        LocalMuteToggle().IsOn(next.muteLocal);
        // Keep the transport slot stable when source enumeration finishes or fails.
        TestToneButton().IsEnabled(streaming);
        SourceRetryButton().IsEnabled(m_engine.Available() && !next.sourcesRefreshing);
        ToolTipService::SetToolTip(SourceRetryButton(), box_value(next.sourcesRefreshing ? L"正在刷新声音来源…" : L"刷新声音来源列表"));
        SpinGlyph(SourceRetryGlyph(), next.sourcesRefreshing, m_sourceSpinRunning);
        CopyDiagnosticsButton().IsEnabled(!next.diagnosticsJson.empty() && next.diagnosticsJson != L"{}");
        SetText(TestToneLabel(), next.testToneActive ? L"停止试播" : selectedStereoGroup ? L"测试左右声道" : L"试播 3 秒");
        TestToneGlyph().Glyph(next.testToneActive ? GlyphStop : L"\xE8D6");
        SetAccessibleName(TestToneButton(), next.testToneActive ? L"停止试播" :
            selectedStereoGroup ? L"测试左右声道，先左，后右，三秒" : L"播放三秒测试音");
        ToolTipService::SetToolTip(TestToneButton(), box_value(selectedStereoGroup
            ? L"先左，后右，共 3 秒，用于核对组合的左右声道。"
            : L"向已连接的扬声器播放 3 秒测试音，结束后恢复所选声音。"));

        // What the capture actually hears, as its own line under the headline.
        bool const captureProblem = sourceError || next.captureState == L"error";
        SetText(CaptureStatusText(), sourceError ? next.errorMessage :
            next.sourcesRefreshing && next.sources.empty() ? L"正在读取声音来源…" :
            next.testToneActive ? selectedStereoGroup ? L"正在测试左右声道：先左，后右（3 秒），随后恢复所选声音。" :
                L"正在播放测试音，结束后恢复所选声音。" :
            next.captureState != L"idle" && !next.captureMessage.empty() ? next.captureMessage :
            !next.sourceMessage.empty() ? next.sourceMessage : L"连接后显示实际声音采集状态。");
        ToolTipService::SetToolTip(CaptureStatusText(), box_value(CaptureStatusText().Text()));
        CaptureStatusText().Foreground(Brush(captureProblem ? L"DangerBrush" : next.captureState == L"signal" ? L"SignalBrush" :
            next.captureState == L"silent" || next.captureState == L"quiet" ? L"WarningBrush" : L"MutedBrush"));
        CaptureGlyph().Visibility(captureProblem ? Visibility::Visible : Visibility::Collapsed);
        CaptureDot().Visibility(captureProblem ? Visibility::Collapsed : Visibility::Visible);
        CaptureDot().Fill(Brush(next.captureState == L"signal" ? L"SignalFillBrush" :
            next.captureState == L"silent" || next.captureState == L"quiet" ? L"WarningBrush" : L"FaintBrush"));

        // The route's source tile: this computer, or the chosen application.
        RouteSourceGlyph().Glyph(captureProblem ? GlyphWarning : applicationSource ? GlyphApp : GlyphDesktop);
        RouteSourceGlyph().Foreground(Brush(captureProblem ? L"DangerBrush" : applicationSource ? L"SignalBrush" : L"InkBrush"));
        RouteSourceTile().Background(Brush(captureProblem ? L"CriticalWashBrush" : L"AppSurfaceBrush"));
        RouteSourceTile().BorderBrush(Brush(L"NodeStrokeBrush"));
        hstring routeTitle = L"这台电脑";
        hstring routeSubtitle = sourceName;
        if (captureProblem)
        {
            routeTitle = sourceError && !applicationSource ? L"没有输出设备" : L"无法采集声音";
            routeSubtitle = L"查看上方提示";
        }
        else if (applicationSource)
        {
            routeTitle = hasSource ? sourceName : hstring(L"选择应用");
            routeSubtitle = hasSource ? L"指定应用" : L"先让应用播放声音";
        }
        SetText(RouteSourceTitle(), routeTitle);
        SetText(RouteSourceSubtitle(), routeSubtitle);

        // Dock: what is being sent, and to whom.
        DockGlyph().Glyph(applicationSource ? GlyphApp : GlyphDesktop);
        hstring dockTitle = L"系统声音";
        if (sourceError && !applicationSource) dockTitle = L"没有声音来源";
        else if (applicationSource) dockTitle = hasSource ? sourceName : hstring(L"指定应用 · 未选择");
        SetText(DockTitle(), dockTitle);
        hstring dockDetail = L"尚未选择扬声器";
        if (chosen.size() == 1) dockDetail = L"送往 " + chosen.front()->name;
        else if (chosen.size() > 1) dockDetail = L"送往 " + chosen.front()->name + L" 等 " + to_hstring(chosen.size()) + L" 台";
        if (!m_desktop.HasTrayIcon()) dockDetail = L"系统托盘暂不可用，关闭窗口会退出 Airloom。";
        else if (next.muteLocal) dockDetail = L"电脑输出已静音；若声音中断，请关闭电脑静音。";
        SetText(FooterMessage(), dockDetail);
        ToolTipService::SetToolTip(FooterMessage(), box_value(dockDetail));
        SetText(DockElapsed(), streaming ? DurationText(next.elapsedSeconds) : hstring{});

        if (next.state != m_snapshot.state || next.selectedDeviceIds != m_snapshot.selectedDeviceIds)
        {
            if (m_devicesRendered) AnimateStateChange();
            m_desktop.SetTooltip(L"Airloom · " + badge);
        }
        RenderRoute(next);
        RenderAudio(next);
        RenderError(next);
    }

    void MainWindow::RenderDevices(Snapshot const& next)
    {
        std::vector<hstring> visibleIds;
        for (auto const& device : next.devices)
            if (!IsDeviceHidden(device)) visibleIds.push_back(device.id);
        m_deviceRows.erase(std::remove_if(m_deviceRows.begin(), m_deviceRows.end(), [&visibleIds](auto const& row)
            { return std::find(visibleIds.begin(), visibleIds.end(), row.id) == visibleIds.end(); }), m_deviceRows.end());

        auto children = DeviceCards().Children();
        uint32_t position{};
        for (auto const& device : next.devices)
        {
            if (IsDeviceHidden(device)) continue;
            bool const stereoGroup = device.kind == L"native_stereo";
            auto found = std::find_if(m_deviceRows.begin(), m_deviceRows.end(), [&device](auto const& row)
                { return row.id == device.id; });
            if (found == m_deviceRows.end())
            {
                DeviceRow created;
                created.id = device.id;

                // The selectable part of a row is one native button: a check
                // indicator, the speaker's avatar, its name and its live state.
                created.button = Button();
                created.button.Style(AppStyle(L"ReceiverRowStyle"));
                Automation::AutomationProperties::SetAutomationId(created.button, L"Receiver:" + device.id);
                Grid contents;
                contents.ColumnSpacing(12);
                ColumnDefinition checkColumn, avatarColumn, textColumn;
                checkColumn.Width(GridLength{20, GridUnitType::Pixel});
                avatarColumn.Width(GridLength{36, GridUnitType::Pixel});
                textColumn.Width(GridLength{1, GridUnitType::Star});
                contents.ColumnDefinitions().Append(checkColumn);
                contents.ColumnDefinitions().Append(avatarColumn);
                contents.ColumnDefinitions().Append(textColumn);
                created.check = Border();
                created.check.Width(20);
                created.check.Height(20);
                created.check.CornerRadius(CornerRadius{4, 4, 4, 4});
                created.check.VerticalAlignment(VerticalAlignment::Center);
                Automation::AutomationProperties::SetAccessibilityView(created.check, Automation::Peers::AccessibilityView::Raw);
                created.checkGlyph = FontIcon();
                created.checkGlyph.Glyph(L"\xE73E");
                created.checkGlyph.FontSize(12);
                created.checkGlyph.HorizontalAlignment(HorizontalAlignment::Center);
                created.checkGlyph.VerticalAlignment(VerticalAlignment::Center);
                created.check.Child(created.checkGlyph);
                contents.Children().Append(created.check);

                created.avatar = Grid();
                created.avatar.Width(36);
                created.avatar.Height(36);
                created.avatar.VerticalAlignment(VerticalAlignment::Center);
                Grid::SetColumn(created.avatar, 1);
                Automation::AutomationProperties::SetAccessibilityView(created.avatar, Automation::Peers::AccessibilityView::Raw);
                created.avatarFace = Shapes::Ellipse();
                created.avatarFace.Width(36);
                created.avatarFace.Height(36);
                created.avatar.Children().Append(created.avatarFace);
                created.avatarGlyphs = StackPanel();
                created.avatarGlyphs.Orientation(Microsoft::UI::Xaml::Controls::Orientation::Horizontal);
                created.avatarGlyphs.HorizontalAlignment(HorizontalAlignment::Center);
                created.avatarGlyphs.VerticalAlignment(VerticalAlignment::Center);
                for (int glyphIndex = 0; glyphIndex < (stereoGroup ? 2 : 1); ++glyphIndex)
                {
                    FontIcon glyph;
                    glyph.Glyph(GlyphSpeaker);
                    glyph.FontSize(stereoGroup ? 13.0 : 18.0);
                    created.avatarGlyphs.Children().Append(glyph);
                }
                created.avatar.Children().Append(created.avatarGlyphs);
                created.badge = Border();
                created.badge.Width(16);
                created.badge.Height(16);
                created.badge.CornerRadius(CornerRadius{8, 8, 8, 8});
                created.badge.BorderThickness(Thickness{2, 2, 2, 2});
                created.badge.HorizontalAlignment(HorizontalAlignment::Right);
                created.badge.VerticalAlignment(VerticalAlignment::Bottom);
                created.badge.Margin(Thickness{0, 0, -3, -3});
                created.badge.Visibility(Visibility::Collapsed);
                created.badgeGlyph = FontIcon();
                created.badgeGlyph.FontSize(8);
                created.badgeGlyph.HorizontalAlignment(HorizontalAlignment::Center);
                created.badgeGlyph.VerticalAlignment(VerticalAlignment::Center);
                created.badge.Child(created.badgeGlyph);
                created.avatar.Children().Append(created.badge);
                contents.Children().Append(created.avatar);

                StackPanel labels;
                Grid::SetColumn(labels, 2);
                labels.Spacing(2);
                labels.VerticalAlignment(VerticalAlignment::Center);
                created.name = TextBlock();
                created.name.FontSize(14);
                created.name.TextTrimming(TextTrimming::CharacterEllipsis);
                created.name.TextWrapping(TextWrapping::NoWrap);
                labels.Children().Append(created.name);
                StackPanel statusLine;
                statusLine.Orientation(Microsoft::UI::Xaml::Controls::Orientation::Horizontal);
                statusLine.Spacing(6);
                // Three small bars move only while this speaker really receives audio.
                created.equalizer = Grid();
                created.equalizer.Width(10);
                created.equalizer.Height(10);
                created.equalizer.VerticalAlignment(VerticalAlignment::Center);
                created.equalizer.Visibility(Visibility::Collapsed);
                Automation::AutomationProperties::SetAccessibilityView(created.equalizer, Automation::Peers::AccessibilityView::Raw);
                for (int barIndex = 0; barIndex < 3; ++barIndex)
                {
                    Shapes::Rectangle bar;
                    bar.Width(2);
                    bar.Height(10);
                    bar.RadiusX(1);
                    bar.RadiusY(1);
                    bar.HorizontalAlignment(HorizontalAlignment::Left);
                    bar.VerticalAlignment(VerticalAlignment::Bottom);
                    bar.Margin(Thickness{barIndex * 4.0, 0, 0, 0});
                    created.equalizer.Children().Append(bar);
                    created.equalizerBars.push_back(bar);
                }
                statusLine.Children().Append(created.equalizer);
                created.spinner = ProgressRing();
                created.spinner.Width(12);
                created.spinner.Height(12);
                created.spinner.MinWidth(12);
                created.spinner.MinHeight(12);
                created.spinner.IsActive(false);
                created.spinner.Visibility(Visibility::Collapsed);
                created.spinner.VerticalAlignment(VerticalAlignment::Center);
                statusLine.Children().Append(created.spinner);
                created.detail = TextBlock();
                created.detail.FontSize(12);
                created.detail.TextTrimming(TextTrimming::CharacterEllipsis);
                created.detail.TextWrapping(TextWrapping::NoWrap);
                created.detail.VerticalAlignment(VerticalAlignment::Center);
                statusLine.Children().Append(created.detail);
                labels.Children().Append(statusLine);
                contents.Children().Append(labels);
                created.button.Content(contents);
                created.button.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            auto command = MakeCommand(L"toggle_device");
                            command.Insert(L"device_id", JsonValue::CreateStringValue(id));
                            command.Insert(L"selected", JsonValue::CreateBooleanValue(!self->m_snapshot.IsSelected(id)));
                            self->Send(command);
                        }
                    });

                // Per-receiver actions are on the row ("more") and on right-click.
                MenuFlyout actions;
                created.retry = MenuFlyoutItem();
                created.retry.Language(WindowRoot().Language());
                created.retry.Text(L"重新连接此设备");
                FontIcon retryIcon;
                retryIcon.Glyph(L"\xE72C");
                created.retry.Icon(retryIcon);
                created.retry.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            auto command = MakeCommand(L"connect");
                            command.Insert(L"device_id", JsonValue::CreateStringValue(id));
                            self->Send(command);
                        }
                    });
                actions.Items().Append(created.retry);
                created.hide = MenuFlyoutItem();
                created.hide.Language(WindowRoot().Language());
                FontIcon hideIcon;
                hideIcon.Glyph(L"\xED1A");
                created.hide.Icon(hideIcon);
                created.hide.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            if (auto current = self->m_snapshot.FindDevice(id)) self->SetDeviceHidden(id, current->name, true);
                        }
                    });
                actions.Items().Append(created.hide);
                created.button.ContextFlyout(actions);

                // Actions beside the button, never nested in it: one row keeps one
                // selection action, and each extra action is its own native button.
                StackPanel actionsPanel;
                actionsPanel.Orientation(Microsoft::UI::Xaml::Controls::Orientation::Horizontal);
                actionsPanel.Spacing(2);
                actionsPanel.VerticalAlignment(VerticalAlignment::Center);
                actionsPanel.Margin(Thickness{0, 0, 4, 0});
                Grid::SetColumn(actionsPanel, 1);
                created.action = HyperlinkButton();
                created.action.FontSize(12);
                created.action.Padding(Thickness{6, 2, 6, 2});
                created.action.VerticalAlignment(VerticalAlignment::Center);
                created.action.Visibility(Visibility::Collapsed);
                Automation::AutomationProperties::SetAutomationId(created.action, L"ReceiverQuickAction:" + device.id);
                created.action.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            auto current = std::find_if(self->m_deviceRows.begin(), self->m_deviceRows.end(), [&id](auto const& entry)
                                { return entry.id == id; });
                            if (current == self->m_deviceRows.end()) return;
                            if (current->actionKind == L"auth")
                            {
                                if (self->m_snapshot.authRequestId != 0 && self->m_snapshot.authDeviceId == id)
                                    self->ShowAuthentication(true);
                            }
                            else if (current->actionKind == L"retry")
                            {
                                auto command = MakeCommand(L"connect");
                                command.Insert(L"device_id", JsonValue::CreateStringValue(id));
                                self->Send(command);
                            }
                        }
                    });
                actionsPanel.Children().Append(created.action);
                if (stereoGroup)
                {
                    created.memberToggle = Button();
                    created.memberToggle.Style(AppStyle(L"RowIconButtonStyle"));
                    created.memberToggle.VerticalAlignment(VerticalAlignment::Center);
                    Automation::AutomationProperties::SetAutomationId(created.memberToggle, L"ReceiverMembers:" + device.id);
                    created.memberChevron = FontIcon();
                    created.memberChevron.FontSize(12);
                    created.memberToggle.Content(created.memberChevron);
                    created.memberToggle.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                        {
                            if (auto self = weak.get())
                            {
                                auto current = std::find_if(self->m_deviceRows.begin(), self->m_deviceRows.end(), [&id](auto const& entry)
                                    { return entry.id == id; });
                                if (current == self->m_deviceRows.end()) return;
                                current->membersExpanded = !current->membersExpanded;
                                bool const expanded = current->membersExpanded;
                                auto host = current->memberHost;
                                self->RenderDevices(self->m_snapshot);
                                if (expanded) self->AnimateIn(host, 0.0f, -6.0f, 250);
                            }
                        });
                    actionsPanel.Children().Append(created.memberToggle);
                }
                created.more = Button();
                created.more.Style(AppStyle(L"RowIconButtonStyle"));
                created.more.VerticalAlignment(VerticalAlignment::Center);
                FontIcon moreGlyph;
                moreGlyph.Glyph(L"\xE712");
                moreGlyph.FontSize(16);
                created.more.Content(moreGlyph);
                created.more.Flyout(actions);
                ToolTipService::SetToolTip(created.more, box_value(L"更多操作"));
                Automation::AutomationProperties::SetAutomationId(created.more, L"ReceiverActions:" + device.id);
                actionsPanel.Children().Append(created.more);

                created.rowGrid = Grid();
                created.rowGrid.CornerRadius(CornerRadius{6, 6, 6, 6});
                ColumnDefinition mainColumn, actionColumn;
                mainColumn.Width(GridLength{1, GridUnitType::Star});
                actionColumn.Width(GridLength{1, GridUnitType::Auto});
                created.rowGrid.ColumnDefinitions().Append(mainColumn);
                created.rowGrid.ColumnDefinitions().Append(actionColumn);
                created.rowGrid.Children().Append(created.button);
                created.rowGrid.Children().Append(actionsPanel);
                created.root = created.rowGrid;
                if (stereoGroup)
                {
                    // Members fold out under the row, aligned with the name.
                    created.groupSurface = Border();
                    created.groupSurface.CornerRadius(CornerRadius{6, 6, 6, 6});
                    StackPanel groupContents;
                    groupContents.Children().Append(created.rowGrid);
                    created.memberHost = Border();
                    created.memberHost.Margin(Thickness{68, 0, 12, 10});
                    created.memberHost.Padding(Thickness{0, 8, 0, 0});
                    created.memberHost.BorderThickness(Thickness{0, 1, 0, 0});
                    created.memberHost.Visibility(Visibility::Collapsed);
                    StackPanel memberContents;
                    memberContents.Spacing(6);
                    created.memberSummary = TextBlock();
                    created.memberSummary.FontSize(12);
                    memberContents.Children().Append(created.memberSummary);
                    created.memberPanel = StackPanel();
                    created.memberPanel.Spacing(6);
                    memberContents.Children().Append(created.memberPanel);
                    created.memberHost.Child(memberContents);
                    groupContents.Children().Append(created.memberHost);
                    created.groupSurface.Child(groupContents);
                    created.root = created.groupSurface;
                }
                m_deviceRows.push_back(std::move(created));
                found = std::prev(m_deviceRows.end());
            }

            auto& row = *found;
            bool const selected = next.IsSelected(device.id);
            bool const enabled = device.online || selected;
            bool const partial = stereoGroup && !device.groupComplete;
            bool const streamingNow = device.state == L"streaming" && !partial;
            bool const connecting = device.state == L"connecting" || device.state == L"reconnecting";
            bool const needsAuth = device.state == L"auth_required";
            bool const failed = device.state == L"error";
            row.button.IsEnabled(enabled);
            if (row.groupSurface) row.groupSurface.Background(Brush(selected ? L"RowSelectedBrush" : L"RowRestBrush"));
            else row.rowGrid.Background(Brush(selected ? L"RowSelectedBrush" : L"RowRestBrush"));
            // Check indicator drawn to the native CheckBox metrics: 20px, 4px radius,
            // accent fill when on, a strong stroke when off.
            row.check.Background(Brush(selected ? L"CheckOnBrush" : L"CheckRestBrush"));
            row.check.BorderBrush(Brush(L"CheckStrokeBrush"));
            row.check.BorderThickness(selected ? Thickness{0, 0, 0, 0} : Thickness{1, 1, 1, 1});
            row.check.Opacity(enabled ? 1.0 : 0.5);
            row.checkGlyph.Foreground(Brush(L"CheckGlyphBrush"));
            row.checkGlyph.Visibility(selected ? Visibility::Visible : Visibility::Collapsed);
            if (row.rendered && row.selected != selected && selected) PopElement(row.check, 20.0f);
            row.selected = selected;
            row.rendered = true;

            row.avatarFace.Fill(Brush(selected ? L"AvatarSelectedBrush" : L"PillRestBrush"));
            auto const avatarInk = Brush(selected ? L"SignalBrush" : L"MutedBrush");
            for (auto const& child : row.avatarGlyphs.Children())
                if (auto glyph = child.try_as<FontIcon>()) glyph.Foreground(avatarInk);
            row.avatar.Opacity(enabled ? 1.0 : 0.5);
            row.badge.Visibility(needsAuth || failed || partial ? Visibility::Visible : Visibility::Collapsed);
            row.badge.Background(Brush(failed ? L"DangerBrush" : L"WarningBrush"));
            row.badge.BorderBrush(Brush(L"AppSurfaceBrush"));
            row.badgeGlyph.Glyph(needsAuth ? GlyphLock : GlyphWarning);
            row.badgeGlyph.Foreground(Brush(L"AppSurfaceBrush"));

            SetText(row.name, device.name);
            row.name.Foreground(Brush(enabled ? L"InkBrush" : L"DisabledTextBrush"));
            hstring stateLabel = !device.online ? L"暂时离线" : L"可连接";
            if (streamingNow) stateLabel = L"正在投放";
            else if (device.state == L"connecting") stateLabel = L"正在连接…";
            else if (device.state == L"reconnecting") stateLabel = L"正在重连…";
            else if (needsAuth) stateLabel = next.authKind == L"pin" && next.authDeviceId == device.id ? L"需要配对码" : L"需要访问密码";
            else if (failed) stateLabel = L"连接失败 · 可重试";
            else if (selected) stateLabel = L"已选择";
            else if (device.passwordRequired) stateLabel = L"需要访问密码";
            auto const onlineMembers = static_cast<size_t>(std::count_if(device.members.begin(), device.members.end(),
                [](auto const& member) { return member.online; }));
            if (partial)
                stateLabel = L"成员暂缺 · " + to_hstring(onlineMembers) + L" / " + to_hstring(device.expectedMembers) + L" 在线";
            bool const plainState = !streamingNow && !connecting && !needsAuth && !failed && !partial;
            SetText(row.detail, stereoGroup && plainState ? stateLabel + L" · 原生立体声组合" : stateLabel);
            row.detail.Foreground(Brush(m_highContrast && selected ? L"InkBrush" :
                partial || needsAuth ? L"WarningBrush" :
                streamingNow ? L"SignalBrush" : failed ? L"DangerBrush" : L"MutedBrush"));
            row.equalizer.Visibility(streamingNow ? Visibility::Visible : Visibility::Collapsed);
            for (auto const& bar : row.equalizerBars) bar.Fill(Brush(L"SignalBrush"));
            bool const equalize = streamingNow && m_motionAllowed && m_homeMotionVisible;
            if (equalize != row.equalizerRunning)
            {
                row.equalizerRunning = equalize;
                int delay{};
                for (auto const& bar : row.equalizerBars)
                {
                    if (equalize) StartEqualizer(bar, delay);
                    else StopMotion(bar);
                    delay += 300;
                }
            }
            bool const animateConnecting = connecting && m_motionAllowed && m_homeMotionVisible;
            row.spinner.Visibility(animateConnecting ? Visibility::Visible : Visibility::Collapsed);
            row.spinner.IsActive(animateConnecting);

            // The one next step for this speaker, as an inline link.
            row.actionKind = needsAuth && next.authRequestId != 0 && next.authDeviceId == device.id ? hstring(L"auth") :
                failed && selected ? hstring(L"retry") : hstring{};
            row.action.Visibility(row.actionKind.empty() ? Visibility::Collapsed : Visibility::Visible);
            if (!row.actionKind.empty())
            {
                bool const authAction = row.actionKind == L"auth";
                row.action.Content(box_value(authAction ? (next.authKind == L"pin" ? L"输入配对码" : L"输入密码") : L"重试"));
                SetAccessibleName(row.action, (authAction ? (next.authKind == L"pin" ? hstring(L"输入配对码：") : hstring(L"输入密码：")) :
                    hstring(L"重试连接：")) + device.name);
            }
            row.retry.Visibility(selected && device.state != L"streaming" ? Visibility::Visible : Visibility::Collapsed);
            row.retry.Text(stereoGroup ? L"重新连接此组合" : L"重新连接此设备");
            row.hide.Text(stereoGroup ? selected ? L"断开并隐藏组合" : L"隐藏组合" : selected ? L"断开并隐藏设备" : L"隐藏设备");
            SetAccessibleName(row.button, device.name + (selected ? L"，已选择，" : L"，未选择，") + stateLabel +
                (stereoGroup ? L"，原生立体声组合" : L""));
            SetAccessibleName(row.more, L"更多操作：" + device.name);
            ToolTipService::SetToolTip(row.button, box_value(!device.errorMessage.empty() ? device.errorMessage :
                stereoGroup ? hstring(L"Apple“家庭”中已建立的立体声组合，作为一个输出发送。") :
                device.name + (device.model.empty() ? L"" : L" · " + device.model)));
            if (stereoGroup)
            {
                row.memberHost.BorderBrush(Brush(L"DividerBrush"));
                auto const summary = L"成员 · " + to_hstring(onlineMembers) + L" / " + to_hstring(device.expectedMembers) + L" 在线";
                SetText(row.memberSummary, summary);
                row.memberSummary.Foreground(Brush(L"FaintBrush"));
                row.memberChevron.Foreground(Brush(L"MutedBrush"));
                row.memberChevron.Glyph(row.membersExpanded ? GlyphChevronUp : GlyphChevronDown);
                SetAccessibleName(row.memberToggle, (row.membersExpanded ? L"收起 " : L"展开 ") + device.name + L" 的" + summary);
                ToolTipService::SetToolTip(row.memberToggle, box_value(row.membersExpanded ? L"收起成员" : L"展开成员"));
                row.memberHost.Visibility(row.membersExpanded ? Visibility::Visible : Visibility::Collapsed);
                auto memberChildren = row.memberPanel.Children();
                uint32_t memberIndex{};
                for (auto const& member : device.members)
                {
                    Grid memberLayout{nullptr};
                    TextBlock memberName{nullptr}, memberState{nullptr};
                    if (memberIndex < memberChildren.Size())
                    {
                        memberLayout = memberChildren.GetAt(memberIndex).as<Grid>();
                        memberName = memberLayout.Children().GetAt(0).as<TextBlock>();
                        memberState = memberLayout.Children().GetAt(1).as<TextBlock>();
                    }
                    else
                    {
                        memberLayout = Grid();
                        memberLayout.ColumnSpacing(10);
                        ColumnDefinition memberNameColumn, memberStateColumn;
                        memberNameColumn.Width(GridLength{1, GridUnitType::Star});
                        memberStateColumn.Width(GridLength{1, GridUnitType::Auto});
                        memberLayout.ColumnDefinitions().Append(memberNameColumn);
                        memberLayout.ColumnDefinitions().Append(memberStateColumn);
                        memberName = TextBlock();
                        memberName.FontSize(12);
                        memberName.TextTrimming(TextTrimming::CharacterEllipsis);
                        memberState = TextBlock();
                        memberState.FontSize(12);
                        Grid::SetColumn(memberState, 1);
                        memberLayout.Children().Append(memberName);
                        memberLayout.Children().Append(memberState);
                        memberChildren.Append(memberLayout);
                    }
                    hstring memberStatus = !member.online ? L"离线" : member.state == L"streaming" ? L"传输中" :
                        member.state == L"connecting" ? L"连接中" : member.state == L"reconnecting" ? L"重连中" :
                        member.state == L"auth_required" ? L"待输入密码" : member.state == L"error" ? L"连接失败" : L"在线";
                    SetText(memberName, member.name);
                    memberName.Foreground(Brush(L"InkBrush"));
                    SetText(memberState, memberStatus);
                    memberState.Foreground(Brush(!member.online ? L"WarningBrush" :
                        member.state == L"streaming" ? L"SignalBrush" : L"MutedBrush"));
                    Automation::AutomationProperties::SetAutomationId(memberLayout, L"ReceiverMember:" + member.id);
                    Automation::AutomationProperties::SetAccessibilityView(memberLayout, Automation::Peers::AccessibilityView::Content);
                    SetAccessibleName(memberLayout, member.name + L"，" + memberStatus);
                    ToolTipService::SetToolTip(memberLayout, box_value(member.name));
                    ++memberIndex;
                }
                while (memberChildren.Size() > memberIndex) memberChildren.RemoveAtEnd();
            }

            // Preserve the same native Button and focus object across snapshots.
            // Only insert/remove/move when the actual device identity list changes.
            uint32_t current{};
            if (!children.IndexOf(row.root, current)) children.InsertAt(position, row.root);
            else if (current != position)
            {
                children.RemoveAt(current);
                children.InsertAt(position, row.root);
            }
            ++position;
        }
        while (children.Size() > position) children.RemoveAtEnd();
        DevicesEmptyState().Visibility(position == 0 ? Visibility::Visible : Visibility::Collapsed);
        m_devicesRendered = true;
        m_forceDeviceRender = false;
    }

    void MainWindow::RenderSources(Snapshot const& next)
    {
        // Enumeration can finish while the user is pointing at a popup item.
        // Reconcile against the latest snapshot only after that interaction ends.
        // An explicit scope change is the exception: its old popup is obsolete.
        if (m_sourcesRendered && SourcePicker().IsDropDownOpen() && next.sourceMode == m_snapshot.sourceMode)
        {
            m_sourcesPending = true;
            return;
        }
        if (SourcePicker().IsDropDownOpen()) SourcePicker().IsDropDownOpen(false);
        auto const applicationSource = next.sourceMode == L"application";
        auto const modeIndex = applicationSource ? 1 : 0;
        if (SourceModePicker().SelectedIndex() != modeIndex) SourceModePicker().SelectedIndex(modeIndex);
        Automation::AutomationProperties::SetName(SourcePicker(), applicationSource ? L"选择要推流的应用" : L"声音来源，Windows 播放设备");
        ToolTipService::SetToolTip(SourcePicker(), box_value(applicationSource
            ? L"只采集所选应用及其子进程。同一浏览器的标签页属于同一应用。"
            : L"选择正在播放声音的 Windows 输出设备。"));
        auto items = SourcePicker().Items();
        // Remove obsolete IDs before inserting so unchanged items retain their
        // native identity, focus and selection across automatic source changes.
        m_sourceRows.erase(std::remove_if(m_sourceRows.begin(), m_sourceRows.end(), [&next, &items](auto const& row)
            {
                bool const removed = std::none_of(next.sources.begin(), next.sources.end(), [&row](auto const& source)
                    { return source.id == row.id; });
                uint32_t current{};
                if (removed && items.IndexOf(row.item, current)) items.RemoveAt(current);
                return removed;
            }), m_sourceRows.end());
        ComboBoxItem selected{nullptr};
        uint32_t position{};
        for (auto const& source : next.sources)
        {
            auto found = std::find_if(m_sourceRows.begin(), m_sourceRows.end(), [&source](auto const& row)
                { return row.id == source.id; });
            if (found == m_sourceRows.end())
            {
                SourceRow created;
                created.id = source.id;
                created.item = ComboBoxItem();
                created.item.Tag(box_value(source.id));
                m_sourceRows.push_back(std::move(created));
                found = std::prev(m_sourceRows.end());
            }
            auto& row = *found;
            if (row.name != source.name)
            {
                row.name = source.name;
                row.item.Content(box_value(source.name));
                ToolTipService::SetToolTip(row.item, box_value(source.name));
            }
            uint32_t current{};
            if (!items.IndexOf(row.item, current)) items.InsertAt(position, row.item);
            else if (current != position)
            {
                items.RemoveAt(current);
                items.InsertAt(position, row.item);
            }
            if (source.id == next.selectedSourceId) selected = row.item;
            ++position;
        }
        if (SourcePicker().SelectedItem() != selected) SourcePicker().SelectedItem(selected);
        m_sourcesRendered = true;
        m_sourcesPending = false;
    }

    void MainWindow::RenderAudio(Snapshot const& next)
    {
        bool const streaming = next.StreamingCount() > 0;
        double const left = streaming ? next.leftLevel : 0.0;
        double const right = streaming ? next.rightLevel : 0.0;
        // Meters rise immediately and fall back with a short release, the usual
        // level-meter ballistics. With motion off they follow each value exactly.
        double const release = streaming && m_motionAllowed ? 0.78 : 0.0;
        m_displayLeft = std::max(left, m_displayLeft * release);
        m_displayRight = std::max(right, m_displayRight * release);
        LeftMeter().Value(m_displayLeft);
        RightMeter().Value(m_displayRight);
        SetText(LeftDb(), Decibels(left));
        SetText(RightDb(), Decibels(right));
        SetText(SampleRateText(), SampleRateLabel(next.sampleRate));
        SetText(ElapsedText(), streaming || next.elapsedSeconds > 0 ? DurationText(next.elapsedSeconds) : L"—");
        SetText(PacketsText(), CountText(next.packetsSent));
        // Levels always keep their place; transfer figures replace their placeholder
        // when a session starts, so the stage never changes height.
        auto const sessionVisibility = streaming ? Visibility::Visible : Visibility::Collapsed;
        if (StatsFigures().Visibility() != sessionVisibility) StatsFigures().Visibility(sessionVisibility);
        auto const placeholderVisibility = streaming ? Visibility::Collapsed : Visibility::Visible;
        if (StatsPlaceholder().Visibility() != placeholderVisibility) StatsPlaceholder().Visibility(placeholderVisibility);

        auto const width = std::max(1.0, WaveformCanvas().ActualWidth());
        auto const height = std::max(1.0, WaveformCanvas().ActualHeight());
        // Bars stand on a baseline a little below the middle, with a short reflection
        // beneath it, so the instrument reads as a level display rather than a barcode.
        auto const baseline = height * 0.6;
        constexpr size_t waveformBins = 64;
        auto const step = width / static_cast<double>(waveformBins);
        // The resting baseline is a row of dots, one under each bar position.
        auto const dot = std::max(1.0, WaveformBaseline().StrokeThickness());
        WaveformBaseline().X1(step * 0.5);
        WaveformBaseline().X2(width);
        WaveformBaseline().Y1(baseline);
        WaveformBaseline().Y2(baseline);
        auto const dotPeriod = std::max(1.0, step / dot);
        auto const dashes = WaveformBaseline().StrokeDashArray();
        if (!dashes || dashes.Size() != 2 || std::abs(dashes.GetAt(1) - dotPeriod) > 0.001)
        {
            DoubleCollection pattern;
            pattern.Append(0.0);
            pattern.Append(dotPeriod);
            WaveformBaseline().StrokeDashArray(pattern);
        }
        bool const waveformAllowed = m_visualizerEnabled && m_motionAllowed;
        bool const hasSignal = streaming && std::max(left, right) > 0.00005;
        auto const hintVisibility = hasSignal && waveformAllowed ? Visibility::Collapsed : Visibility::Visible;
        if (WaveformHintPanel().Visibility() != hintVisibility) WaveformHintPanel().Visibility(hintVisibility);
        WaveformScaleLabel().Visibility(streaming && waveformAllowed ? Visibility::Visible : Visibility::Collapsed);
        SetText(WaveformHint(), !streaming ? L"连接后，声音将在这里流动" : !m_visualizerEnabled ? L"动态波形已关闭" :
            !m_motionAllowed ? L"已遵循 Windows 动画设置" : L"等待声音 · 播放一首音乐试试");

        // The core provides a 64-bin peak envelope, not signed time-domain samples.
        // Display its actual magnitude on a fixed -60..0 dB scale above the baseline.
        // The shorter, quieter reflection below it is only a display convention; there
        // is no synthetic motion or automatic gain.
        if (m_waveformBars.empty())
        {
            auto color = Brush(L"SignalFillBrush");
            // Even entries are the bars; odd entries are their reflections.
            for (size_t index = 0; index < waveformBins * 2; ++index)
            {
                Line bar;
                bar.Stroke(color);
                bar.StrokeStartLineCap(PenLineCap::Round);
                bar.StrokeEndLineCap(PenLineCap::Round);
                if (index % 2 == 1) bar.Opacity(0.26);
                bar.Visibility(Visibility::Collapsed);
                WaveformCanvas().Children().Append(bar);
                m_waveformBars.push_back(bar);
            }
        }
        auto const thickness = std::clamp(step * 0.42, 2.0, 4.0);
        for (size_t index = 0; index < waveformBins && index * 2 + 1 < m_waveformBars.size(); ++index)
        {
            auto const peak = waveformAllowed && streaming && index < next.waveform.size()
                ? std::clamp(std::abs(next.waveform[index]), 0.0, 1.0) : 0.0;
            auto const magnitude = peak > 0.001 ? std::clamp((20.0 * std::log10(peak) + 60.0) / 60.0, 0.0, 1.0) : 0.0;
            auto& shown = m_displayWave[index];
            shown = waveformAllowed && streaming ? std::max(magnitude, shown * release) : 0.0;
            auto const rise = shown * baseline * 0.92;
            auto const reflection = shown * (height - baseline) * 0.6;
            auto const x = (static_cast<double>(index) + 0.5) * step;
            auto const visible = shown > 0.004 ? Visibility::Visible : Visibility::Collapsed;
            auto const& bar = m_waveformBars[index * 2];
            bar.X1(x); bar.X2(x);
            bar.Y1(baseline - rise); bar.Y2(baseline);
            bar.StrokeThickness(thickness);
            bar.Visibility(visible);
            auto const& mirror = m_waveformBars[index * 2 + 1];
            mirror.X1(x); mirror.X2(x);
            mirror.Y1(baseline); mirror.Y2(baseline + reflection);
            mirror.StrokeThickness(thickness);
            // High contrast keeps only the measured bars.
            mirror.Visibility(m_highContrast ? Visibility::Collapsed : visible);
        }
    }

    void MainWindow::RenderRoute(Snapshot const& next)
    {
        // Continuous motion exists only while it explains something real and
        // Windows allows animation; the drawn state is the same without it.
        bool const animate = m_motionAllowed && !m_highContrast && m_homeMotionVisible;
        if (!animate && m_routeAnimationsAllowed)
        {
            // Collapsing home can zero its canvas size. Stop existing visuals
            // before the geometry guard, so a settings page cannot leave them running.
            StopMotion(RouteSourceHalo());
            RouteSourceHalo().Visibility(Visibility::Collapsed);
            m_sourceHaloRunning = false;
            for (auto& node : m_routeNodes)
            {
                if (node.linkStoryboard) node.linkStoryboard.Stop();
                node.linkStoryboard = nullptr;
                node.link.ClearValue(Shapes::Shape::StrokeDashOffsetProperty());
                if (node.look == L"live") node.link.ClearValue(Shapes::Shape::StrokeDashArrayProperty());
                StopMotion(node.disc);
                StopMotion(node.halo);
                StopMotion(node.ring);
                node.halo.Visibility(Visibility::Collapsed);
                for (auto const& particle : node.particles)
                {
                    Hosting::ElementCompositionPreview::GetElementVisual(particle).StopAnimation(L"Translation");
                    StopMotion(particle);
                }
                StopMotion(node.particleHost);
                node.particleHost.Visibility(Visibility::Collapsed);
                node.haloRunning = node.ringRunning = node.particlesRunning = node.flowing = node.marching = false;
            }
        }
        m_routeAnimationsAllowed = animate;
        double const width = RouteCanvas().ActualWidth();
        double const height = RouteCanvas().ActualHeight();
        if (width < 40.0 || height < 40.0) return;
        bool const streaming = next.StreamingCount() > 0;
        bool const hasSource = std::any_of(next.sources.begin(), next.sources.end(), [&next](auto const& source)
            { return source.id == next.selectedSourceId; });
        // Particles show sound actually leaving: a captured signal or the test tone.
        bool const flowing = streaming && (next.testToneActive ||
            (hasSource && (next.captureState == L"signal" || next.captureState == L"quiet")));

        struct Entry
        {
            hstring id;
            hstring name;
            hstring state;
            hstring look;
            hstring glyph;
            hstring badge;
        };
        std::vector<Entry> entries;
        size_t chosen{};
        for (auto const& device : next.devices)
        {
            if (!next.IsSelected(device.id) || IsDeviceHidden(device)) continue;
            ++chosen;
            if (entries.size() >= 4) continue;
            Entry entry;
            entry.id = device.id;
            entry.name = device.name;
            entry.glyph = device.kind == L"native_stereo" ? L"group" : L"single";
            if (device.kind == L"native_stereo" && !device.groupComplete)
            {
                entry.look = L"warn"; entry.badge = L"warn"; entry.state = L"成员暂缺";
            }
            else if (device.state == L"streaming")
            {
                entry.look = L"live";
                entry.state = next.testToneActive ? L"正在试播" : hasSource ? L"正在投放" : L"已连接 · 静音";
            }
            else if (device.state == L"connecting" || device.state == L"reconnecting")
            {
                entry.look = L"busy";
                entry.state = device.state == L"connecting" ? L"正在连接…" : L"正在重连…";
            }
            else if (device.state == L"auth_required")
            {
                entry.look = L"warn"; entry.badge = L"lock";
                entry.state = next.authKind == L"pin" && next.authDeviceId == device.id ? L"需要配对码" : L"需要访问密码";
            }
            else if (device.state == L"error")
            {
                entry.look = L"err"; entry.badge = L"warn"; entry.state = L"连接失败 · 可重试";
            }
            else
            {
                entry.look = L"idle";
                entry.state = device.online ? L"已选择" : L"暂时离线";
            }
            entries.push_back(std::move(entry));
        }
        if (chosen > entries.size() && !entries.empty())
            entries.back().state = entries.back().state + L" · 另有 " + to_hstring(chosen - entries.size()) + L" 台";
        if (entries.empty())
        {
            bool const anyVisible = std::any_of(next.devices.begin(), next.devices.end(), [this](Device const& device)
                { return !IsDeviceHidden(device); });
            Entry ghost;
            ghost.look = L"ghost";
            ghost.glyph = L"add";
            ghost.name = anyVisible ? L"选择扬声器" : L"寻找扬声器";
            ghost.state = anyVisible ? (m_wideLayout ? L"在右侧勾选" : L"在上方勾选") :
                next.state == L"discovering" ? L"正在发现…" : L"可重新发现";
            entries.push_back(std::move(ghost));
        }

        std::wstring signature = std::to_wstring(static_cast<int>(width)) + L"x" + std::to_wstring(static_cast<int>(height));
        signature += animate ? L"|a" : L"|-";
        signature += flowing ? L"|f" : L"|-";
        signature += m_wideLayout ? L"|w" : L"|c";
        for (auto const& entry : entries)
        {
            for (auto const* part : {&entry.id, &entry.look, &entry.name, &entry.state, &entry.badge, &entry.glyph})
            {
                signature += L'\x1f';
                signature += part->c_str();
            }
        }
        auto const now = std::chrono::steady_clock::now();
        bool const finishParticleFade = std::any_of(m_routeNodes.begin(), m_routeNodes.end(), [&](RouteNode const& node)
            { return node.particlesRunning && !node.flowing && now >= node.particleStopAt; });
        if (signature == m_routeSignature && !finishParticleFade) return;
        m_routeSignature = signature;

        // Geometry follows the design: an 80 px source tile whose right edge
        // starts every curve, and 210 px speaker blocks on the right.
        double const content = std::min(width, m_wideLayout ? 720.0 : 600.0);
        double const left = std::floor((width - content) / 2.0);
        double const sourceY = std::round(height / 2.0);
        double const sourceX = left + 100.0;
        double const nodeX = std::max(sourceX + 66.0, left + content - 210.0);
        double const endX = nodeX - 6.0;
        double const count = static_cast<double>(entries.size());
        double const gap = entries.size() > 1 ? std::clamp((height - 56.0) / (count - 1.0), 56.0, 74.0) : 0.0;
        double const firstY = sourceY - gap * (count - 1.0) / 2.0;
        Canvas::SetLeft(RouteSource(), left);
        Canvas::SetTop(RouteSource(), sourceY - 40.0);

        bool const sourceHalo = flowing && animate;
        if (sourceHalo != m_sourceHaloRunning)
        {
            m_sourceHaloRunning = sourceHalo;
            if (sourceHalo)
            {
                RouteSourceHalo().Visibility(Visibility::Visible);
                StartLoop(RouteSourceHalo(), 40.0f, 40.0f, 1.28f, 0.5f, 2400);
            }
            else
            {
                StopMotion(RouteSourceHalo());
                RouteSourceHalo().Visibility(Visibility::Collapsed);
            }
        }

        auto links = RouteLinkLayer().Children();
        auto particleLayer = RouteParticleLayer().Children();
        auto nodeLayer = RouteNodeLayer().Children();
        auto const detach = [](UIElementCollection const& collection, UIElement const& element)
            {
                uint32_t index{};
                if (element && collection.IndexOf(element, index)) collection.RemoveAt(index);
            };
        m_routeNodes.erase(std::remove_if(m_routeNodes.begin(), m_routeNodes.end(), [&](RouteNode& node)
            {
                bool const keep = std::any_of(entries.begin(), entries.end(), [&node](Entry const& entry)
                    { return entry.id == node.id; });
                if (keep) return false;
                if (node.linkStoryboard) node.linkStoryboard.Stop();
                detach(links, node.link);
                detach(particleLayer, node.particleHost);
                detach(nodeLayer, node.root);
                return true;
            }), m_routeNodes.end());

        for (size_t index = 0; index < entries.size(); ++index)
        {
            auto const& entry = entries[index];
            auto found = std::find_if(m_routeNodes.begin(), m_routeNodes.end(), [&entry](RouteNode const& node)
                { return node.id == entry.id; });
            bool const created = found == m_routeNodes.end();
            if (created)
            {
                RouteNode node;
                node.id = entry.id;
                node.link = Shapes::Path();
                node.link.StrokeThickness(2.0);
                node.link.StrokeStartLineCap(PenLineCap::Round);
                node.link.StrokeEndLineCap(PenLineCap::Round);
                links.Append(node.link);
                node.particleHost = Canvas();
                node.particleHost.Visibility(Visibility::Collapsed);
                particleLayer.Append(node.particleHost);

                node.root = Grid();
                node.root.Width(210.0);
                node.root.Height(52.0);
                node.root.ColumnSpacing(12.0);
                ColumnDefinition discColumn, labelColumn;
                discColumn.Width(GridLength{52, GridUnitType::Pixel});
                labelColumn.Width(GridLength{1, GridUnitType::Star});
                node.root.ColumnDefinitions().Append(discColumn);
                node.root.ColumnDefinitions().Append(labelColumn);
                node.disc = Grid();
                node.disc.Width(52.0);
                node.disc.Height(52.0);
                node.halo = Shapes::Ellipse();
                node.halo.Width(52.0);
                node.halo.Height(52.0);
                node.halo.Visibility(Visibility::Collapsed);
                node.disc.Children().Append(node.halo);
                node.face = Shapes::Ellipse();
                node.face.Width(52.0);
                node.face.Height(52.0);
                node.disc.Children().Append(node.face);
                node.glyphs = StackPanel();
                node.glyphs.Orientation(Microsoft::UI::Xaml::Controls::Orientation::Horizontal);
                node.glyphs.HorizontalAlignment(HorizontalAlignment::Center);
                node.glyphs.VerticalAlignment(VerticalAlignment::Center);
                node.disc.Children().Append(node.glyphs);
                // A connecting speaker's outer ring turns.
                node.ring = Shapes::Ellipse();
                node.ring.Width(62.0);
                node.ring.Height(62.0);
                node.ring.Margin(Thickness{-5, -5, -5, -5});
                node.ring.StrokeThickness(2.0);
                node.ring.StrokeDashArray(Dashes(23.0, 70.0));
                node.ring.StrokeDashCap(PenLineCap::Round);
                node.ring.Visibility(Visibility::Collapsed);
                node.disc.Children().Append(node.ring);
                node.badge = Border();
                node.badge.Width(20.0);
                node.badge.Height(20.0);
                node.badge.CornerRadius(CornerRadius{10, 10, 10, 10});
                node.badge.BorderThickness(Thickness{2, 2, 2, 2});
                node.badge.HorizontalAlignment(HorizontalAlignment::Right);
                node.badge.VerticalAlignment(VerticalAlignment::Bottom);
                node.badge.Margin(Thickness{0, 0, -3, -3});
                node.badge.Visibility(Visibility::Collapsed);
                node.badgeGlyph = FontIcon();
                node.badgeGlyph.FontSize(10.0);
                node.badgeGlyph.HorizontalAlignment(HorizontalAlignment::Center);
                node.badgeGlyph.VerticalAlignment(VerticalAlignment::Center);
                node.badge.Child(node.badgeGlyph);
                node.disc.Children().Append(node.badge);
                node.root.Children().Append(node.disc);
                StackPanel labels;
                Grid::SetColumn(labels, 1);
                labels.VerticalAlignment(VerticalAlignment::Center);
                node.name = TextBlock();
                node.name.FontSize(14.0);
                node.name.FontWeight(Windows::UI::Text::FontWeight{600});
                node.name.TextWrapping(TextWrapping::NoWrap);
                node.name.TextTrimming(TextTrimming::CharacterEllipsis);
                labels.Children().Append(node.name);
                node.state = TextBlock();
                node.state.FontSize(12.0);
                node.state.TextWrapping(TextWrapping::NoWrap);
                node.state.TextTrimming(TextTrimming::CharacterEllipsis);
                labels.Children().Append(node.state);
                node.root.Children().Append(labels);
                nodeLayer.Append(node.root);
                m_routeNodes.push_back(std::move(node));
                found = std::prev(m_routeNodes.end());
            }
            auto& node = *found;

            if (node.glyphKind != entry.glyph)
            {
                node.glyphKind = entry.glyph;
                node.glyphs.Children().Clear();
                int const glyphCount = entry.glyph == L"group" ? 2 : 1;
                for (int glyphIndex = 0; glyphIndex < glyphCount; ++glyphIndex)
                {
                    FontIcon glyph;
                    glyph.Glyph(entry.glyph == L"add" ? GlyphAdd : GlyphSpeaker);
                    glyph.FontSize(entry.glyph == L"group" ? 15.0 : entry.glyph == L"add" ? 18.0 : 22.0);
                    node.glyphs.Children().Append(glyph);
                }
            }

            double const nodeY = std::round(firstY + gap * static_cast<double>(index));
            bool const moved = node.startX != sourceX || node.startY != sourceY || node.endX != endX || node.endY != nodeY;
            if (moved)
            {
                node.startX = sourceX;
                node.startY = sourceY;
                node.endX = endX;
                node.endY = nodeY;
                double const span = endX - sourceX;
                Media::PathFigure figure;
                figure.StartPoint(Point{static_cast<float>(sourceX), static_cast<float>(sourceY)});
                figure.IsFilled(false);
                Media::BezierSegment curve;
                curve.Point1(Point{static_cast<float>(sourceX + span * 0.5), static_cast<float>(sourceY)});
                curve.Point2(Point{static_cast<float>(endX - span * 0.42), static_cast<float>(nodeY)});
                curve.Point3(Point{static_cast<float>(endX), static_cast<float>(nodeY)});
                Media::PathSegmentCollection segments;
                segments.Append(curve);
                figure.Segments(segments);
                Media::PathFigureCollection figures;
                figures.Append(figure);
                Media::PathGeometry geometry;
                geometry.Figures(figures);
                node.link.Data(geometry);
                node.length = RouteLength(sourceX, sourceY, endX, nodeY);
                Canvas::SetLeft(node.root, nodeX);
                Canvas::SetTop(node.root, nodeY - 26.0);
            }

            hstring const previous = node.look;
            bool const lookChanged = previous != entry.look;
            node.look = entry.look;
            bool const live = entry.look == L"live";
            bool const busy = entry.look == L"busy";
            bool const ghost = entry.look == L"ghost";
            bool const attention = entry.look == L"warn";
            bool const problem = entry.look == L"err";
            bool const arrived = live && lookChanged && !previous.empty() && !created && animate;

            // Line language: dotted = chosen but idle, marching dashes = connecting,
            // solid accent = connected, short dashes in caution/critical = needs you.
            // Theme refreshes replace the paint without restarting a route's motion.
            node.link.Stroke(Brush(live ? L"AccentLineBrush" : busy ? L"SignalFillBrush" :
                problem ? L"DangerBrush" : attention ? L"WarningBrush" : L"RouteIdleBrush"));
            node.link.Opacity(live ? 1.0 : busy ? 0.75 : attention || problem ? 0.8 : ghost ? 0.4 : 0.7);
            bool const march = busy && animate;
            if (lookChanged || moved || node.marching != march || (!animate && node.linkStoryboard))
            {
                if (node.linkStoryboard)
                {
                    node.linkStoryboard.Stop();
                    node.linkStoryboard = nullptr;
                }
                node.marching = false;
                node.link.ClearValue(Shapes::Shape::StrokeDashOffsetProperty());
                if (live)
                {
                    node.link.ClearValue(Shapes::Shape::StrokeDashArrayProperty());
                }
                else if (busy)
                {
                    node.link.StrokeDashArray(Dashes(3.0, 4.0));
                    node.link.StrokeDashCap(PenLineCap::Flat);
                }
                else if (attention || problem)
                {
                    node.link.StrokeDashArray(Dashes(2.0, 3.5));
                    node.link.StrokeDashCap(PenLineCap::Flat);
                }
                else
                {
                    node.link.StrokeDashArray(Dashes(0.0, 3.5));
                    node.link.StrokeDashCap(PenLineCap::Round);
                }
                if (arrived) node.linkStoryboard = DrawPath(node.link, node.length);
                else if (march)
                {
                    node.linkStoryboard = MarchPath(node.link);
                    node.marching = true;
                }
            }

            node.face.Fill(Brush(ghost ? L"RowRestBrush" : L"AppSurfaceBrush"));
            node.face.Stroke(Brush(live ? L"SignalFillBrush" : ghost ? L"RouteIdleBrush" : L"NodeStrokeBrush"));
            node.face.StrokeThickness(live ? 2.0 : ghost ? 1.5 : 1.0);
            if (ghost) node.face.StrokeDashArray(Dashes(3.0, 2.5));
            else node.face.ClearValue(Shapes::Shape::StrokeDashArrayProperty());
            auto const ink = Brush(live ? L"SignalBrush" : attention ? L"WarningBrush" : problem ? L"DangerBrush" :
                ghost ? L"FaintBrush" : busy ? L"InkBrush" : L"MutedBrush");
            for (auto const& child : node.glyphs.Children())
                if (auto glyph = child.try_as<FontIcon>()) glyph.Foreground(ink);
            node.badge.Visibility(entry.badge.empty() ? Visibility::Collapsed : Visibility::Visible);
            node.badge.Background(Brush(problem ? L"DangerBrush" : L"WarningBrush"));
            node.badge.BorderBrush(Brush(L"AppSurfaceBrush"));
            node.badgeGlyph.Glyph(entry.badge == L"lock" ? GlyphLock : GlyphWarning);
            node.badgeGlyph.Foreground(Brush(L"AppSurfaceBrush"));
            SetText(node.name, entry.name);
            node.name.Foreground(Brush(ghost ? L"MutedBrush" : L"InkBrush"));
            SetText(node.state, entry.state);
            node.state.Foreground(Brush(live ? L"SignalBrush" : attention ? L"WarningBrush" : problem ? L"DangerBrush" :
                ghost ? L"FaintBrush" : L"MutedBrush"));
            node.halo.Fill(Brush(L"SignalFillBrush"));
            node.ring.Stroke(Brush(L"SignalFillBrush"));

            // A joining speaker pops once (0.86 → 1.06 → 1), then breathes quietly.
            if (arrived) BounceElement(node.disc, 26.0f);
            bool const halo = live && animate;
            if (halo != node.haloRunning)
            {
                node.haloRunning = halo;
                if (halo)
                {
                    node.halo.Visibility(Visibility::Visible);
                    StartLoop(node.halo, 26.0f, 26.0f, 1.75f, 0.24f, 2400, static_cast<int>(index) * 300);
                }
                else
                {
                    StopMotion(node.halo);
                    node.halo.Visibility(Visibility::Collapsed);
                }
            }
            node.ring.Visibility(busy ? Visibility::Visible : Visibility::Collapsed);
            bool const spin = busy && animate;
            if (spin != node.ringRunning)
            {
                node.ringRunning = spin;
                if (spin) StartSpin(node.ring, 31.0f, 31.0f, 1100);
                else StopMotion(node.ring);
            }

            // Particles travel the same curve on the compositor (1.8 s, linear), so
            // they stay smooth whatever the UI thread is doing.
            bool const fading = node.particlesRunning && (node.flowing || now < node.particleStopAt);
            bool const particles = live && animate && (flowing || fading);
            if (particles && (!node.particlesRunning || moved))
            {
                if (node.particles.empty())
                {
                    for (int particleIndex = 0; particleIndex < 5; ++particleIndex)
                    {
                        Shapes::Ellipse particle;
                        particle.Width(6.0);
                        particle.Height(6.0);
                        Canvas::SetLeft(particle, -3.0);
                        Canvas::SetTop(particle, -3.0);
                        node.particleHost.Children().Append(particle);
                        node.particles.push_back(particle);
                    }
                }
                node.particleHost.Visibility(Visibility::Visible);
                auto hostVisual = Hosting::ElementCompositionPreview::GetElementVisual(node.particleHost);
                auto compositor = hostVisual.Compositor();
                hostVisual.StopAnimation(L"Opacity");
                hostVisual.Opacity(0.0f);
                node.flowing = flowing;
                node.particleStopAt = flowing ? std::chrono::steady_clock::time_point{} : now + std::chrono::milliseconds(150);
                if (flowing) FadeVisual(node.particleHost, 1.0f, 250);
                auto const linear = compositor.CreateLinearEasingFunction();
                for (size_t particleIndex = 0; particleIndex < node.particles.size(); ++particleIndex)
                {
                    auto const& particle = node.particles[particleIndex];
                    Hosting::ElementCompositionPreview::SetIsTranslationEnabled(particle, true);
                    auto visual = Hosting::ElementCompositionPreview::GetElementVisual(particle);
                    auto travel = compositor.CreateVector3KeyFrameAnimation();
                    for (int step = 0; step <= 24; ++step)
                    {
                        double const progress = step / 24.0;
                        auto const point = RoutePointAt(node.startX, node.startY, node.endX, node.endY, progress);
                        travel.InsertKeyFrame(static_cast<float>(progress),
                            float3{static_cast<float>(point.x), static_cast<float>(point.y), 0.0f}, linear);
                    }
                    auto fade = compositor.CreateScalarKeyFrameAnimation();
                    fade.InsertKeyFrame(0.0f, 0.0f, linear);
                    fade.InsertKeyFrame(0.12f, 1.0f, linear);
                    fade.InsertKeyFrame(0.86f, 1.0f, linear);
                    fade.InsertKeyFrame(1.0f, 0.0f, linear);
                    auto const delay = std::chrono::milliseconds(static_cast<int>(particleIndex) * 360 + static_cast<int>(index) * 190);
                    travel.Duration(std::chrono::milliseconds(1800));
                    travel.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
                    travel.DelayTime(delay);
                    fade.Duration(std::chrono::milliseconds(1800));
                    fade.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
                    fade.DelayTime(delay);
                    visual.Opacity(0.0f);
                    visual.StartAnimation(L"Translation", travel);
                    visual.StartAnimation(L"Opacity", fade);
                }
                node.particlesRunning = true;
            }
            else if (!particles && node.particlesRunning)
            {
                for (auto const& particle : node.particles)
                {
                    auto visual = Hosting::ElementCompositionPreview::GetElementVisual(particle);
                    visual.StopAnimation(L"Translation");
                    visual.StopAnimation(L"Opacity");
                }
                node.particleHost.Visibility(Visibility::Collapsed);
                node.particlesRunning = false;
                node.flowing = false;
            }
            for (auto const& particle : node.particles) particle.Fill(Brush(L"SignalFillBrush"));
            // Silence (or a source switch) lets the particles fade out in 150 ms and
            // back in 250 ms; the connected line itself stays.
            if (node.particlesRunning && node.flowing != flowing)
            {
                node.flowing = flowing;
                node.particleStopAt = flowing ? std::chrono::steady_clock::time_point{} : now + std::chrono::milliseconds(150);
                FadeVisual(node.particleHost, flowing ? 1.0f : 0.0f, flowing ? 250 : 150);
            }
            if (created && animate)
            {
                AnimateIn(node.root, 0.0f, 6.0f, 250);
                AnimateIn(node.link, 0.0f, 0.0f, 250);
            }
        }
    }

    void MainWindow::RenderError(Snapshot const& next)
    {
        bool const localError = !m_localError.empty() && std::chrono::steady_clock::now() < m_localErrorUntil;
        bool const engineError = !next.errorMessage.empty() && next.errorCode != L"capture_sources";
        // A closed InfoBar still occupies its margin, so it is also collapsed.
        ErrorBanner().IsOpen(localError || engineError);
        ErrorBanner().Visibility(localError || engineError ? Visibility::Visible : Visibility::Collapsed);
        // Settings owns maintenance actions; a source/receiver error is already
        // explained on the home view and must not displace the update controls.
        SettingsErrorBanner().IsOpen(localError);
        SettingsErrorBanner().Visibility(localError ? Visibility::Visible : Visibility::Collapsed);
        SettingsErrorBanner().Message(localError ? m_localError : L"");
        if (localError || engineError)
        {
            ErrorBanner().Title(next.state == L"reconnecting" ? L"正在恢复连接" : L"连接需要处理");
            ErrorBanner().Severity(next.state == L"reconnecting" ? InfoBarSeverity::Warning : InfoBarSeverity::Error);
            ErrorBanner().Message(localError ? m_localError : next.errorMessage);
            RetryButton().IsEnabled(m_engine.Available() && next.state != L"connecting");
        }
    }

    void MainWindow::ShowLocalError(hstring const& message)
    {
        m_localError = message;
        m_localErrorUntil = std::chrono::steady_clock::now() + std::chrono::seconds(8);
        ErrorBanner().Title(L"操作未完成");
        ErrorBanner().Message(message);
        ErrorBanner().Severity(InfoBarSeverity::Error);
        ErrorBanner().Visibility(Visibility::Visible);
        ErrorBanner().IsOpen(true);
        SettingsErrorBanner().Message(message);
        SettingsErrorBanner().Visibility(Visibility::Visible);
        SettingsErrorBanner().IsOpen(true);
    }

    bool MainWindow::Send(JsonObject const& command)
    {
        try
        {
            if (m_engine.Command(command))
            {
                m_localError.clear();
                return true;
            }
            ShowLocalError(L"音频引擎暂时未能接收操作，请稍后重试。");
        }
        catch (hresult_error const&)
        {
            ShowLocalError(L"无法将操作交给音频引擎，请重试。");
        }
        return false;
    }

    bool MainWindow::Send(wchar_t const* type)
    {
        return Send(MakeCommand(type));
    }

    void MainWindow::OnRefresh(IInspectable const&, RoutedEventArgs const&)
    {
        Send(L"refresh");
    }

    void MainWindow::OnRetry(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.authRequestId != 0) ShowAuthentication(true);
        else if (!m_snapshot.selectedDeviceId.empty() && !m_snapshot.selectedSourceId.empty()) Send(L"connect");
        else Send(L"refresh");
    }

    void MainWindow::OnConnect(IInspectable const&, RoutedEventArgs const&)
    {
        // A running route always keeps its stop action, even while a different
        // receiver is waiting for credentials in its own row.
        if (m_snapshot.StreamingCount() > 0) Send(L"disconnect");
        else if (m_snapshot.authRequestId != 0) ShowAuthentication(true);
        else Send(IsActive(m_snapshot.state) ? L"disconnect" : L"connect");
    }

    void MainWindow::OnCancelAuth(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.authRequestId == 0) return;
        auto command = MakeCommand(L"disconnect");
        command.Insert(L"request_id", JsonValue::CreateNumberValue(static_cast<double>(m_snapshot.authRequestId)));
        Send(command);
    }

    void MainWindow::OnRefreshSources(IInspectable const&, RoutedEventArgs const&)
    {
        Send(L"refresh_sources");
    }

    void MainWindow::OnSourceModeChanged(IInspectable const&, SelectionChangedEventArgs const&)
    {
        if (m_rendering || !m_loaded) return;
        auto item = SourceModePicker().SelectedItem().try_as<ComboBoxItem>();
        if (!item) return;
        auto mode = unbox_value<hstring>(item.Tag());
        if (mode == m_snapshot.sourceMode) return;
        auto command = MakeCommand(L"select_source_mode");
        command.Insert(L"mode", JsonValue::CreateStringValue(mode));
        Send(command);
    }

    void MainWindow::OnSourceChanged(IInspectable const&, SelectionChangedEventArgs const&)
    {
        if (m_rendering || !m_loaded) return;
        auto item = SourcePicker().SelectedItem().try_as<ComboBoxItem>();
        if (!item) return;
        auto const sourceId = unbox_value<hstring>(item.Tag());
        if (sourceId == m_snapshot.selectedSourceId) return;
        if (std::none_of(m_snapshot.sources.begin(), m_snapshot.sources.end(), [&sourceId](auto const& source)
            { return source.id == sourceId; }))
        {
            // The popup may still contain an application that exited while it
            // was open. Drop its stale action and reconcile once it closes.
            m_sourcesPending = true;
            return;
        }
        auto command = MakeCommand(L"select_source");
        command.Insert(L"source_id", JsonValue::CreateStringValue(sourceId));
        Send(command);
    }

    void MainWindow::OnVolumeChanged(IInspectable const&, RangeBaseValueChangedEventArgs const& args)
    {
        if (m_rendering || !m_loaded || !m_volumeTimer || !VolumeSlider().IsEnabled() || !HasVolumeTarget(m_snapshot)) return;
        m_pendingVolume = std::clamp(args.NewValue() / 100.0, 0.0, 1.0);
        m_volumePending = true;
        SetText(VolumeLabel(), to_hstring(static_cast<int>(std::round(args.NewValue()))) + L"%");
        SetAccessibleName(VolumeLabel(), VolumeLabel().Text());
        UpdateVolumeGlyph(args.NewValue());
        m_volumeTimer.Stop();
        m_volumeTimer.Start();
    }

    void MainWindow::UpdateVolumeGlyph(double value)
    {
        // Fixed 20px slot: the glyph changes, the layout does not.
        hstring const glyph = VolumeGlyphFor(value);
        if (VolumeGlyph().Glyph() != glyph) VolumeGlyph().Glyph(glyph);
    }

    void MainWindow::FlushVolume()
    {
        m_volumeTimer.Stop();
        if (!m_volumePending || m_exiting) return;
        if (!VolumeSlider().IsEnabled() || !HasVolumeTarget(m_snapshot))
        {
            m_volumePending = false;
            m_volumeAwaitingAck = false;
            return;
        }
        auto command = MakeCommand(L"set_volume");
        command.Insert(L"volume", JsonValue::CreateNumberValue(m_pendingVolume));
        bool const accepted = Send(command);
        m_volumePending = false;
        m_volumeAwaitingAck = accepted;
        m_volumeAckDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    }

    void MainWindow::FinishVolumeInteraction()
    {
        if (!m_volumePointerDown) return;
        m_volumePointerDown = false;
        // Commit the last pointer position immediately; keyboard and automation
        // changes still use the same debounce and acknowledgement path.
        FlushVolume();
    }

    void MainWindow::OnMuteLocalChanged(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_rendering || !m_loaded) return;
        auto command = MakeCommand(L"set_mute_local");
        command.Insert(L"enabled", JsonValue::CreateBooleanValue(LocalMuteToggle().IsOn()));
        Send(command);
    }

    void MainWindow::OnForgetCredentials(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.selectedDeviceId.empty()) return;
        auto command = MakeCommand(L"forget_credentials");
        command.Insert(L"device_id", JsonValue::CreateStringValue(m_snapshot.selectedDeviceId));
        Send(command);
    }

    hstring MainWindow::AuthenticationFingerprint() const
    {
        return to_hstring(m_snapshot.authRequestId) + L"\n" + m_snapshot.authDeviceId + L"\n" + m_snapshot.authKind + L"\n" +
            m_snapshot.authMessage + L"\n" + m_snapshot.errorMessage;
    }

    fire_and_forget MainWindow::ShowAuthentication(bool explicitlyRequested)
    {
        auto lifetime = get_strong();
        if (m_authDialog || m_exiting || !LayoutRoot().XamlRoot() || m_snapshot.authRequestId == 0) co_return;
        auto const fingerprint = AuthenticationFingerprint();
        if (!explicitlyRequested && m_authFingerprint == fingerprint) co_return;
        m_authFingerprint = fingerprint;
        bool const pin = m_snapshot.authKind == L"pin";
        auto const requestId = m_snapshot.authRequestId;
        auto const authDeviceId = m_snapshot.authDeviceId;
        auto const device = m_snapshot.FindDevice(authDeviceId);

        ContentDialog dialog;
        m_authDialog = dialog;
        m_dialogRequestId = requestId;
        dialog.XamlRoot(LayoutRoot().XamlRoot());
        // Dialogs live in the popup layer and do not inherit the window root's language tag.
        dialog.Language(WindowRoot().Language());
        dialog.RequestedTheme(LayoutRoot().ActualTheme());
        dialog.Style(Application::Current().Resources().Lookup(box_value(L"DefaultContentDialogStyle")).as<Style>());
        dialog.Title(box_value(pin ? L"输入设备配对码" : L"输入扬声器访问密码"));
        dialog.PrimaryButtonText(L"继续连接");
        if (!pin) dialog.SecondaryButtonText(L"使用配对码");
        dialog.CloseButtonText(L"取消");
        dialog.DefaultButton(ContentDialogButton::Primary);

        StackPanel content;
        content.Spacing(12);
        content.MaxWidth(400);
        TextBlock description;
        description.TextWrapping(TextWrapping::Wrap);
        description.Text(!m_snapshot.authMessage.empty() ? m_snapshot.authMessage :
            pin ? L"输入设备显示或播报的配对码，再继续连接。" : L"输入在 Apple“家庭”中为扬声器设置的 AirPlay 访问密码。");
        content.Children().Append(description);
        PasswordBox password;
        password.Header(box_value(!m_snapshot.authDeviceName.empty() ? m_snapshot.authDeviceName :
            device ? device->name : pin ? L"配对码" : L"访问密码"));
        password.PlaceholderText(pin ? L"配对码" : L"访问密码");
        password.PasswordRevealMode(PasswordRevealMode::Peek);
        password.MaxLength(1024);
        SetAccessibleName(password, pin ? L"设备配对码" : L"AirPlay 访问密码");
        content.Children().Append(password);
        TextBlock validation;
        validation.Text(pin ? L"请先输入配对码。" : L"请先输入访问密码。");
        validation.Foreground(Brush(L"DangerBrush"));
        validation.FontSize(12);
        validation.Visibility(Visibility::Collapsed);
        content.Children().Append(validation);
        CheckBox remember;
        remember.Content(box_value(L"在这台电脑上记住此设备"));
        remember.IsChecked(false);
        content.Children().Append(remember);
        TextBlock note;
        note.Text(L"凭据不会显示在连接状态或日志中，可在设置中忘记。");
        note.TextWrapping(TextWrapping::Wrap);
        note.FontSize(12);
        note.Foreground(Brush(L"MutedBrush"));
        content.Children().Append(note);
        dialog.Content(content);
        dialog.PrimaryButtonClick([password, validation, pin](auto const&, ContentDialogButtonClickEventArgs const& args)
            {
                auto const value = password.Password();
                auto const bytes = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.c_str(),
                    static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
                if (value.empty() || bytes <= 0 || bytes > 1024 || std::wstring_view(value).find(L'\0') != std::wstring_view::npos)
                {
                    validation.Text(value.empty() ? pin ? L"请先输入配对码。" : L"请先输入访问密码。" : L"凭据格式无效或长度超过设备协议限制（1024 字节）。");
                    validation.Visibility(Visibility::Visible);
                    args.Cancel(true);
                    password.Focus(FocusState::Programmatic);
                }
            });
        dialog.Opened([password](auto const&, auto const&) { password.Focus(FocusState::Programmatic); });

        try
        {
            auto const result = co_await dialog.ShowAsync();
            // A dialog belongs to one engine challenge. A late button click must not
            // authenticate, cancel, or re-pair a newer connection attempt.
            if (!m_exiting && m_snapshot.authRequestId != 0 &&
                m_snapshot.authRequestId == requestId && m_snapshot.authDeviceId == authDeviceId)
            {
                if (result == ContentDialogResult::Primary)
                {
                    auto command = MakeCommand(L"authenticate");
                    command.Insert(L"value", JsonValue::CreateStringValue(password.Password()));
                    command.Insert(L"remember", JsonValue::CreateBooleanValue(remember.IsChecked().Value()));
                    command.Insert(L"request_id", JsonValue::CreateNumberValue(static_cast<double>(requestId)));
                    Send(command);
                }
                else if (result == ContentDialogResult::Secondary)
                {
                    auto command = MakeCommand(L"pair");
                    command.Insert(L"request_id", JsonValue::CreateNumberValue(static_cast<double>(requestId)));
                    Send(command);
                }
                else
                {
                    auto command = MakeCommand(L"disconnect");
                    command.Insert(L"request_id", JsonValue::CreateNumberValue(static_cast<double>(requestId)));
                    Send(command);
                }
            }
        }
        catch (hresult_error const&)
        {
            if (!m_exiting) ShowLocalError(L"验证窗口未能打开。请点击“输入访问密码”重试。");
        }
        password.Password(L"");
        m_authDialog = nullptr;
    }

    bool MainWindow::IsDeviceHidden(hstring const& id) const
    {
        return std::any_of(m_hiddenDevices.begin(), m_hiddenDevices.end(), [&id](auto const& device)
            { return device.id == id; });
    }

    bool MainWindow::IsDeviceHidden(Device const& device) const
    {
        if (IsDeviceHidden(device.id)) return true;
        // A physical receiver hidden before discovery learned its native group
        // must not silently reappear as part of a new logical receiver card.
        return device.kind == L"native_stereo" && std::any_of(device.members.begin(), device.members.end(),
            [this](auto const& member) { return IsDeviceHidden(member.id); });
    }

    void MainWindow::SetDeviceHidden(hstring const& id, hstring const& name, bool hidden)
    {
        auto const device = m_snapshot.FindDevice(id);
        if (hidden == (device ? IsDeviceHidden(*device) : IsDeviceHidden(id))) return;
        if (hidden && m_snapshot.IsSelected(id))
        {
            auto command = MakeCommand(L"toggle_device");
            command.Insert(L"device_id", JsonValue::CreateStringValue(id));
            command.Insert(L"selected", JsonValue::CreateBooleanValue(false));
            if (!Send(command)) return;
        }
        std::vector<hstring> memberIds;
        if (device && device->kind == L"native_stereo")
            for (auto const& member : device->members) memberIds.push_back(member.id);
        for (auto const& stored : m_hiddenDevices)
            if (stored.id == id)
                for (auto const& memberId : stored.memberIds)
                    if (std::find(memberIds.begin(), memberIds.end(), memberId) == memberIds.end()) memberIds.push_back(memberId);
        auto updated = m_hiddenDevices;
        updated.erase(std::remove_if(updated.begin(), updated.end(), [&id, &memberIds](auto const& entry)
            {
                if (entry.id == id) return true;
                return std::find(memberIds.begin(), memberIds.end(), entry.id) != memberIds.end();
            }), updated.end());
        if (hidden)
        {
            // Persist the association on the group record, so restoring an
            // offline group after restart also clears its member preferences.
            updated.push_back({id, name, memberIds});
            // Retain the user's choice if Apple Home later dissolves the group
            // or a temporary discovery record exposes only physical receivers.
            if (device && device->kind == L"native_stereo")
                for (auto const& member : device->members) updated.push_back({member.id, member.name});
        }
        try
        {
            Preferences::SaveHiddenDevices(updated);
            m_hiddenDevices = std::move(updated);
            m_forceDeviceRender = true;
            m_visibilityRendered = false;
            RenderDevices(m_snapshot);
            if (m_settingsOpen) RenderDeviceVisibility(m_snapshot);
        }
        catch (hresult_error const&)
        {
            ShowLocalError(L"未能保存设备显示设置，请重试。");
        }
    }

    void MainWindow::RenderDeviceVisibility(Snapshot const& next)
    {
        std::vector<airloom::native::HiddenDevice> entries;
        for (auto const& device : next.devices) entries.push_back({device.id, device.name});
        for (auto const& hidden : m_hiddenDevices)
        {
            bool const groupedMember = std::any_of(next.devices.begin(), next.devices.end(), [&hidden](auto const& device)
                { return device.kind == L"native_stereo" && std::any_of(device.members.begin(), device.members.end(),
                    [&hidden](auto const& member) { return member.id == hidden.id; }); });
            if (!next.FindDevice(hidden.id) && !groupedMember) entries.push_back(hidden);
        }
        auto const visible = std::count_if(next.devices.begin(), next.devices.end(), [this](Device const& device)
            { return !IsDeviceHidden(device); });
        auto const hiddenCount = std::count_if(entries.begin(), entries.end(), [this, &next](auto const& entry)
            { auto const device = next.FindDevice(entry.id); return device ? IsDeviceHidden(*device) : IsDeviceHidden(entry.id); });
        SetText(VisibilitySummary(), entries.empty() ? hstring(L"隐藏不需要的扬声器；离线设备也可恢复显示。") :
            to_hstring(visible) + L" 个在首页显示 · " + to_hstring(hiddenCount) + L" 个已隐藏");
        VisibilityEmptyRow().Visibility(entries.empty() ? Visibility::Visible : Visibility::Collapsed);
        m_visibilityRows.erase(std::remove_if(m_visibilityRows.begin(), m_visibilityRows.end(), [&entries](auto const& row)
            { return std::none_of(entries.begin(), entries.end(), [&row](auto const& entry) { return entry.id == row.id; }); }),
            m_visibilityRows.end());
        auto children = DeviceVisibilityList().Children();
        uint32_t position{};
        for (auto const& entry : entries)
        {
            auto const discovered = next.FindDevice(entry.id);
            bool const hidden = discovered ? IsDeviceHidden(*discovered) : IsDeviceHidden(entry.id);
            auto found = std::find_if(m_visibilityRows.begin(), m_visibilityRows.end(), [&entry](auto const& row)
                { return row.id == entry.id; });
            if (found == m_visibilityRows.end())
            {
                // Settings-expander items: a divider above, text aligned with the
                // header's title column, the action on the right.
                VisibilityRow created;
                created.id = entry.id;
                created.root = Border();
                created.root.BorderThickness(Thickness{0, 1, 0, 0});
                created.root.Padding(Thickness{56, 10, 16, 10});
                created.root.MinHeight(56);
                Grid layout;
                layout.ColumnSpacing(16);
                ColumnDefinition labelColumn, actionColumn;
                labelColumn.Width(GridLength{1, GridUnitType::Star});
                actionColumn.Width(GridLength{1, GridUnitType::Auto});
                layout.ColumnDefinitions().Append(labelColumn);
                layout.ColumnDefinitions().Append(actionColumn);
                StackPanel labels;
                labels.Spacing(2);
                labels.VerticalAlignment(VerticalAlignment::Center);
                created.title = TextBlock();
                created.title.FontSize(14);
                created.title.TextTrimming(TextTrimming::CharacterEllipsis);
                created.title.TextWrapping(TextWrapping::NoWrap);
                labels.Children().Append(created.title);
                created.detail = TextBlock();
                created.detail.FontSize(12);
                created.detail.TextTrimming(TextTrimming::CharacterEllipsis);
                created.detail.TextWrapping(TextWrapping::NoWrap);
                labels.Children().Append(created.detail);
                layout.Children().Append(labels);
                created.toggle = Button();
                Grid::SetColumn(created.toggle, 1);
                created.toggle.VerticalAlignment(VerticalAlignment::Center);
                created.toggle.MinWidth(88);
                created.toggle.Style(AppStyle(L"CompactButtonStyle"));
                Automation::AutomationProperties::SetAutomationId(created.toggle, L"DeviceVisibility:" + entry.id);
                created.toggle.Click([weak = get_weak(), id = entry.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            auto current = self->m_snapshot.FindDevice(id);
                            if (current) self->SetDeviceHidden(id, current->name, !self->IsDeviceHidden(*current));
                            else
                            {
                                auto stored = std::find_if(self->m_hiddenDevices.begin(), self->m_hiddenDevices.end(), [&id](auto const& device)
                                    { return device.id == id; });
                                if (stored != self->m_hiddenDevices.end()) self->SetDeviceHidden(id, stored->name, false);
                            }
                        }
                    });
                layout.Children().Append(created.toggle);
                created.root.Child(layout);
                m_visibilityRows.push_back(std::move(created));
                found = std::prev(m_visibilityRows.end());
            }
            auto& row = *found;
            row.name = entry.name;
            row.root.BorderBrush(Brush(L"DividerBrush"));
            SetText(row.title, entry.name.empty() ? entry.id : entry.name);
            row.title.Foreground(Brush(hidden ? L"MutedBrush" : L"InkBrush"));
            row.detail.Foreground(Brush(L"MutedBrush"));
            hstring status = hidden ? L"已隐藏" : L"在首页显示";
            if (discovered && discovered->kind == L"native_stereo") status = status + L" · 原生立体声组合";
            if (!discovered || !discovered->online) status = status + L" · 当前不在线";
            else if (discovered->state == L"streaming") status = status + L" · 正在传输";
            else if (!discovered->model.empty()) status = status + L" · " + discovered->model;
            SetText(row.detail, status);
            row.toggle.Content(box_value(hidden ? L"恢复显示" : L"隐藏"));
            SetAccessibleName(row.toggle, (hidden ? L"恢复显示 " : L"隐藏 ") + entry.name);
            uint32_t current{};
            if (!children.IndexOf(row.root, current)) children.InsertAt(position, row.root);
            else if (current != position)
            {
                children.RemoveAt(current);
                children.InsertAt(position, row.root);
            }
            ++position;
        }
        while (children.Size() > position) children.RemoveAtEnd();
        m_visibilityRendered = true;
    }

    void MainWindow::ShowSettings(bool visible, bool focusDevices)
    {
        bool const changed = m_settingsOpen != visible;
        m_settingsOpen = visible;
        if (visible && !m_visibilityRendered) RenderDeviceVisibility(m_snapshot);
        HomeView().Visibility(visible ? Visibility::Collapsed : Visibility::Visible);
        SettingsView().Visibility(visible ? Visibility::Visible : Visibility::Collapsed);
        // Windows 11 places "back" in the title bar of a sub-page.
        SettingsBackButton().Visibility(visible ? Visibility::Visible : Visibility::Collapsed);
        SettingsButton().Visibility(visible ? Visibility::Collapsed : Visibility::Visible);
        TitleSection().Visibility(visible ? Visibility::Visible : Visibility::Collapsed);
        TitleDragRegion().Padding(Thickness{visible ? 8.0 : 16.0, 0, 0, 0});
        // The focus target has only just become visible; move focus after layout.
        DispatcherQueue().TryEnqueue(Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [weak = get_weak(), visible]
            {
                if (auto self = weak.get())
                {
                    if (self->m_settingsOpen != visible) return;
                    if (visible) self->SettingsBackButton().Focus(FocusState::Programmatic);
                    else self->SettingsButton().Focus(FocusState::Programmatic);
                }
            });
        if (changed)
        {
            // Drill in (96 % → 100 %, 320 ms); back out (102 % → 100 %, 250 ms).
            if (visible) AnimateZoomIn(SettingsSheet(), 0.96f, 320);
            else AnimateZoomIn(HomeSheet(), 1.02f, 250);
            if (visible) AnimateIn(SettingsBackButton(), -8.0f, 0.0f, 240);
        }
        if (visible && focusDevices)
        {
            DispatcherQueue().TryEnqueue(Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [weak = get_weak()]
                {
                    if (auto self = weak.get())
                    {
                        if (!self->m_settingsOpen) return;
                        self->VisibilityCard().StartBringIntoView();
                        self->FlashVisibilityCard();
                    }
                });
        }
    }

    void MainWindow::OnOpenSettings(IInspectable const&, RoutedEventArgs const&)
    {
        ShowSettings(true);
    }

    void MainWindow::OnManageDevices(IInspectable const&, RoutedEventArgs const&)
    {
        ShowSettings(true, true);
    }

    void MainWindow::OnCloseSettings(IInspectable const&, RoutedEventArgs const&)
    {
        ShowSettings(false);
    }

    void MainWindow::OnTestTone(IInspectable const&, RoutedEventArgs const&)
    {
        auto command = MakeCommand(L"test_tone");
        command.Insert(L"enabled", JsonValue::CreateBooleanValue(!m_snapshot.testToneActive));
        Send(command);
    }

    void MainWindow::RenderUpdate()
    {
        auto const update = m_updater.Snapshot();
        if (update.exitRequested) { ExitNow(); return; }
        if (m_updateRendered && update.state == m_lastUpdate.state && update.currentVersion == m_lastUpdate.currentVersion &&
            update.availableVersion == m_lastUpdate.availableVersion && update.message == m_lastUpdate.message &&
            update.progress == m_lastUpdate.progress) return;
        m_lastUpdate = update;
        m_updateRendered = true;
        CurrentVersionText().Text(L"Airloom " + hstring(update.currentVersion));
        UpdateStatusText().Text(hstring(update.message));
        bool const checking = update.state == L"checking";
        bool const downloading = update.state == L"downloading";
        bool const launching = update.state == L"launching";
        bool const busy = checking || downloading || launching;
        CheckUpdatesButton().IsEnabled(!busy);
        CheckUpdatesButton().Content(box_value(checking ? L"正在检查…" : L"检查更新"));
        bool const canInstall = update.state == L"available";
        InstallUpdateButton().Visibility(canInstall ? Visibility::Visible : Visibility::Collapsed);
        InstallUpdateButton().Content(box_value(L"下载并安装 " + hstring(update.availableVersion)));
        CancelUpdateButton().Visibility(checking || downloading ? Visibility::Visible : Visibility::Collapsed);
        UpdateProgress().Visibility(busy ? Visibility::Visible : Visibility::Collapsed);
        UpdateProgress().IsIndeterminate(checking || launching);
        UpdateProgress().Value(std::clamp(update.progress, 0.0, 1.0) * 100.0);
    }

    void MainWindow::OnCheckUpdates(IInspectable const&, RoutedEventArgs const&)
    {
        m_updater.Check();
        RenderUpdate();
    }

    void MainWindow::OnInstallUpdate(IInspectable const&, RoutedEventArgs const&)
    {
        m_updater.Install();
        RenderUpdate();
    }

    void MainWindow::OnCancelUpdate(IInspectable const&, RoutedEventArgs const&)
    {
        m_updater.Cancel();
        RenderUpdate();
    }

    void MainWindow::OnCopyDiagnostics(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.diagnosticsJson.empty() || m_snapshot.diagnosticsJson == L"{}") return;
        try
        {
            // The core exports a separate allowlisted diagnostics object. Never
            // copy the raw snapshot, device identities, addresses or auth data.
            Windows::ApplicationModel::DataTransfer::DataPackage package;
            package.SetText(m_snapshot.diagnosticsJson);
            Windows::ApplicationModel::DataTransfer::Clipboard::SetContent(package);
            Windows::ApplicationModel::DataTransfer::Clipboard::Flush();
            DiagnosticsCopyStatus().Text(L"已复制，可粘贴到问题反馈中。不包含设备名称、地址或凭据。");
        }
        catch (hresult_error const&)
        {
            ShowLocalError(L"未能写入剪贴板，请稍后重试。");
        }
    }

    void MainWindow::OnThemeChanged(IInspectable const&, SelectionChangedEventArgs const&)
    {
        if (!m_preferencesReady) return;
        auto const selected = ThemePicker().SelectedIndex();
        if (selected < 0) return;
        WindowRoot().RequestedTheme(selected == 1 ? ElementTheme::Light : selected == 2 ? ElementTheme::Dark : ElementTheme::Default);
        try { Preferences::SaveTheme(static_cast<uint32_t>(selected)); }
        catch (hresult_error const&) { ShowLocalError(L"主题已应用，但未能保存设置。"); }
        ApplyTheme();
    }

    void MainWindow::OnActualThemeChanged(FrameworkElement const&, IInspectable const&)
    {
        if (m_hwnd) ApplyTheme();
    }

    void MainWindow::ApplyTheme()
    {
        if (!m_hwnd) return;
        bool const dark = LayoutRoot().ActualTheme() == ElementTheme::Dark;
        auto titlebar = AppWindow().TitleBar();
        auto const foreground = m_highContrast ? SystemColor(COLOR_WINDOWTEXT) :
            dark ? Windows::UI::Color{255, 255, 255, 255} : Windows::UI::Color{255, 27, 27, 27};
        titlebar.ButtonForegroundColor(foreground);
        titlebar.ButtonHoverForegroundColor(m_highContrast ? SystemColor(COLOR_HIGHLIGHTTEXT) : foreground);
        titlebar.ButtonPressedForegroundColor(foreground);
        titlebar.ButtonInactiveForegroundColor(m_highContrast ? SystemColor(COLOR_GRAYTEXT) :
            dark ? Windows::UI::Color{255, 138, 138, 138} : Windows::UI::Color{255, 141, 141, 141});
        titlebar.ButtonBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonInactiveBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonHoverBackgroundColor(m_highContrast ? SystemColor(COLOR_HIGHLIGHT) :
            dark ? Windows::UI::Color{15, 255, 255, 255} : Windows::UI::Color{9, 0, 0, 0});
        titlebar.ButtonPressedBackgroundColor(m_highContrast ? SystemColor(COLOR_HIGHLIGHT) :
            dark ? Windows::UI::Color{10, 255, 255, 255} : Windows::UI::Color{6, 0, 0, 0});
        for (auto const& bar : m_waveformBars) bar.Stroke(Brush(L"SignalFillBrush"));
        // Refresh code-created brushes without changing route state or replaying
        // connection animations. Every route element refreshes its paint together.
        m_routeSignature.clear();
        m_forceDeviceRender = true;
        m_visibilityRendered = false;
    }

    void MainWindow::OnVisualizerChanged(IInspectable const&, RoutedEventArgs const&)
    {
        if (!m_preferencesReady) return;
        m_visualizerEnabled = VisualizerToggle().IsOn();
        try { Preferences::SaveVisualizer(m_visualizerEnabled); }
        catch (hresult_error const&) { ShowLocalError(L"波形设置已应用，但未能保存设置。"); }
        RenderAudio(m_snapshot);
    }

    void MainWindow::OnStartupChanged(IInspectable const&, RoutedEventArgs const&)
    {
        if (!m_preferencesReady) return;
        try
        {
            Preferences::SetStartsWithWindows(StartupToggle().IsOn());
        }
        catch (hresult_error const&)
        {
            m_preferencesReady = false;
            StartupToggle().IsOn(Preferences::StartsWithWindows());
            m_preferencesReady = true;
            ShowLocalError(L"Windows 未能保存登录启动设置，请重试。");
        }
    }

    // ---- Motion -------------------------------------------------------------
    // Every animation below explains a change: a page arriving, a session state
    // changing, a selection, a loading action or live output. All of them are
    // composition animations on transform/opacity, and none runs when Windows
    // "Animation effects" is off; the state itself is always shown immediately.

    void MainWindow::AnimateIn(UIElement const& element, float fromX, float fromY, int milliseconds, float fromOpacity)
    {
        if (!m_motionAllowed || !element) return;
        Hosting::ElementCompositionPreview::SetIsTranslationEnabled(element, true);
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        auto const ease = Decelerate(compositor);
        auto const duration = std::chrono::milliseconds(milliseconds);
        if (fromX != 0.0f || fromY != 0.0f)
        {
            auto translation = compositor.CreateVector3KeyFrameAnimation();
            translation.InsertKeyFrame(0.0f, float3{fromX, fromY, 0.0f});
            translation.InsertKeyFrame(1.0f, float3{0.0f, 0.0f, 0.0f}, ease);
            translation.Duration(duration);
            visual.StartAnimation(L"Translation", translation);
        }
        auto opacity = compositor.CreateScalarKeyFrameAnimation();
        opacity.InsertKeyFrame(0.0f, fromOpacity);
        opacity.InsertKeyFrame(1.0f, 1.0f, ease);
        opacity.Duration(duration);
        visual.StartAnimation(L"Opacity", opacity);
    }

    void MainWindow::AnimateZoomIn(UIElement const& element, float fromScale, int milliseconds)
    {
        // Settings drills in from 96 %; home returns from 102 %. Both views fill
        // the area under the title bar, so that is the scale centre.
        if (!m_motionAllowed || !element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        auto const width = static_cast<float>(LayoutRoot().ActualWidth());
        auto const height = static_cast<float>(std::max(0.0, LayoutRoot().ActualHeight() - 48.0));
        visual.CenterPoint(float3{width * 0.5f, height * 0.5f, 0.0f});
        auto const ease = Decelerate(compositor);
        auto const duration = std::chrono::milliseconds(milliseconds);
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{fromScale, fromScale, 1.0f});
        scale.InsertKeyFrame(1.0f, float3{1.0f, 1.0f, 1.0f}, ease);
        scale.Duration(duration);
        auto opacity = compositor.CreateScalarKeyFrameAnimation();
        opacity.InsertKeyFrame(0.0f, 0.0f);
        opacity.InsertKeyFrame(1.0f, 1.0f, ease);
        opacity.Duration(duration);
        visual.StartAnimation(L"Scale", scale);
        visual.StartAnimation(L"Opacity", opacity);
    }

    void MainWindow::PopElement(UIElement const& element, float size)
    {
        if (!m_motionAllowed || !element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        visual.CenterPoint(float3{size * 0.5f, size * 0.5f, 0.0f});
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{0.7f, 0.7f, 1.0f});
        scale.InsertKeyFrame(1.0f, float3{1.0f, 1.0f, 1.0f}, Decelerate(compositor));
        scale.Duration(std::chrono::milliseconds(220));
        visual.StartAnimation(L"Scale", scale);
    }

    void MainWindow::UpdatePulse(bool live, bool busy)
    {
        // Live: the dot breathes outward. Busy: it blinks. Only while the window
        // is on screen and Windows allows animation; otherwise a still dot.
        int const mode = !m_motionAllowed || m_highContrast || !m_homeMotionVisible ? 0 : live ? 1 : busy ? 2 : 0;
        if (mode == m_pulseMode) return;
        m_pulseMode = mode;
        auto pulse = Hosting::ElementCompositionPreview::GetElementVisual(StatusPulse());
        auto dot = Hosting::ElementCompositionPreview::GetElementVisual(StatusDot());
        pulse.StopAnimation(L"Scale");
        pulse.StopAnimation(L"Opacity");
        dot.StopAnimation(L"Opacity");
        dot.Opacity(1.0f);
        StatusPulse().Visibility(mode == 1 ? Visibility::Visible : Visibility::Collapsed);
        if (mode == 1)
        {
            StartLoop(StatusPulse(), 4.0f, 4.0f, 2.6f, 0.5f, 1600);
        }
        else if (mode == 2)
        {
            auto compositor = dot.Compositor();
            auto blink = compositor.CreateScalarKeyFrameAnimation();
            blink.InsertKeyFrame(0.0f, 1.0f);
            blink.InsertKeyFrame(0.5f, 0.3f);
            blink.InsertKeyFrame(1.0f, 1.0f);
            blink.Duration(std::chrono::milliseconds(1000));
            blink.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
            dot.StartAnimation(L"Opacity", blink);
        }
    }

    void MainWindow::StartLoop(UIElement const& element, float centerX, float centerY, float toScale, float fromOpacity,
        int milliseconds, int delayMilliseconds)
    {
        if (!element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        visual.CenterPoint(float3{centerX, centerY, 0.0f});
        auto const ease = compositor.CreateCubicBezierEasingFunction(float2{0.0f, 0.0f}, float2{0.2f, 1.0f});
        auto const duration = std::chrono::milliseconds(milliseconds);
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{1.0f, 1.0f, 1.0f});
        scale.InsertKeyFrame(1.0f, float3{toScale, toScale, 1.0f}, ease);
        scale.Duration(duration);
        scale.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
        auto fade = compositor.CreateScalarKeyFrameAnimation();
        fade.InsertKeyFrame(0.0f, fromOpacity);
        fade.InsertKeyFrame(1.0f, 0.0f, ease);
        fade.Duration(duration);
        fade.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
        if (delayMilliseconds > 0)
        {
            scale.DelayTime(std::chrono::milliseconds(delayMilliseconds));
            fade.DelayTime(std::chrono::milliseconds(delayMilliseconds));
            visual.Opacity(0.0f);
        }
        visual.StartAnimation(L"Scale", scale);
        visual.StartAnimation(L"Opacity", fade);
    }

    void MainWindow::StartSpin(UIElement const& element, float centerX, float centerY, int milliseconds)
    {
        if (!element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        visual.CenterPoint(float3{centerX, centerY, 0.0f});
        auto rotation = compositor.CreateScalarKeyFrameAnimation();
        rotation.InsertKeyFrame(0.0f, 0.0f);
        rotation.InsertKeyFrame(1.0f, 360.0f, compositor.CreateLinearEasingFunction());
        rotation.Duration(std::chrono::milliseconds(milliseconds));
        rotation.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
        visual.StartAnimation(L"RotationAngleInDegrees", rotation);
    }

    void MainWindow::StartEqualizer(UIElement const& element, int delayMilliseconds)
    {
        if (!element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        // Each bar grows from its foot.
        visual.CenterPoint(float3{1.0f, 10.0f, 0.0f});
        auto const ease = compositor.CreateCubicBezierEasingFunction(float2{0.42f, 0.0f}, float2{0.58f, 1.0f});
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{1.0f, 0.3f, 1.0f});
        scale.InsertKeyFrame(0.5f, float3{1.0f, 1.0f, 1.0f}, ease);
        scale.InsertKeyFrame(1.0f, float3{1.0f, 0.3f, 1.0f}, ease);
        scale.Duration(std::chrono::milliseconds(900));
        scale.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
        if (delayMilliseconds > 0) scale.DelayTime(std::chrono::milliseconds(delayMilliseconds));
        visual.StartAnimation(L"Scale", scale);
    }

    void MainWindow::StopMotion(UIElement const& element)
    {
        if (!element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        visual.StopAnimation(L"Scale");
        visual.StopAnimation(L"Opacity");
        visual.StopAnimation(L"RotationAngleInDegrees");
        visual.Scale(float3{1.0f, 1.0f, 1.0f});
        visual.Opacity(1.0f);
        visual.RotationAngleInDegrees(0.0f);
    }

    void MainWindow::FadeVisual(UIElement const& element, float opacity, int milliseconds)
    {
        if (!element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        if (!m_motionAllowed)
        {
            visual.StopAnimation(L"Opacity");
            visual.Opacity(opacity);
            return;
        }
        auto compositor = visual.Compositor();
        auto fade = compositor.CreateScalarKeyFrameAnimation();
        fade.InsertKeyFrame(1.0f, opacity, compositor.CreateLinearEasingFunction());
        fade.Duration(std::chrono::milliseconds(milliseconds));
        visual.StartAnimation(L"Opacity", fade);
    }

    void MainWindow::BounceElement(UIElement const& element, float center)
    {
        if (!m_motionAllowed || !element) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(element);
        auto compositor = visual.Compositor();
        visual.CenterPoint(float3{center, center, 0.0f});
        auto const ease = Decelerate(compositor);
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{0.86f, 0.86f, 1.0f});
        scale.InsertKeyFrame(0.6f, float3{1.06f, 1.06f, 1.0f}, ease);
        scale.InsertKeyFrame(1.0f, float3{1.0f, 1.0f, 1.0f}, ease);
        scale.Duration(std::chrono::milliseconds(450));
        visual.StartAnimation(L"Scale", scale);
    }

    void MainWindow::SwapConnectIcon()
    {
        // One slot, three states: the icon scales in (0.5 → 1, 167 ms) while the
        // label cross-fades; the button itself never changes size.
        if (!m_motionAllowed) return;
        auto icon = Hosting::ElementCompositionPreview::GetElementVisual(ConnectIcon());
        auto compositor = icon.Compositor();
        auto const ease = Decelerate(compositor);
        icon.CenterPoint(float3{8.0f, 8.0f, 0.0f});
        auto scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0.0f, float3{0.5f, 0.5f, 1.0f});
        scale.InsertKeyFrame(1.0f, float3{1.0f, 1.0f, 1.0f}, ease);
        scale.Duration(std::chrono::milliseconds(167));
        auto fade = compositor.CreateScalarKeyFrameAnimation();
        fade.InsertKeyFrame(0.0f, 0.0f);
        fade.InsertKeyFrame(1.0f, 1.0f, ease);
        fade.Duration(std::chrono::milliseconds(167));
        icon.StartAnimation(L"Scale", scale);
        icon.StartAnimation(L"Opacity", fade);
        auto label = Hosting::ElementCompositionPreview::GetElementVisual(ConnectLabel());
        auto labelFade = compositor.CreateScalarKeyFrameAnimation();
        labelFade.InsertKeyFrame(0.0f, 0.2f);
        labelFade.InsertKeyFrame(1.0f, 1.0f, compositor.CreateLinearEasingFunction());
        labelFade.Duration(std::chrono::milliseconds(120));
        label.StartAnimation(L"Opacity", labelFade);
    }

    void MainWindow::UpdateRadar(bool searching)
    {
        // Three staggered rings while discovery is really running and the list
        // is empty; they stop as soon as the first speaker arrives.
        bool const run = searching && m_motionAllowed && !m_highContrast && m_homeMotionVisible;
        if (run == m_radarRunning) return;
        m_radarRunning = run;
        int delay{};
        for (UIElement ring : {RadarRing1().as<UIElement>(), RadarRing2().as<UIElement>(), RadarRing3().as<UIElement>()})
        {
            if (run)
            {
                ring.Visibility(Visibility::Visible);
                StartLoop(ring, 22.0f, 22.0f, 2.6f, 0.45f, 2400, delay);
            }
            else
            {
                StopMotion(ring);
                ring.Visibility(Visibility::Collapsed);
            }
            delay += 800;
        }
    }

    void MainWindow::SpinGlyph(FontIcon const& glyph, bool spin, bool& running)
    {
        bool const run = spin && m_motionAllowed && m_homeMotionVisible;
        if (run == running) return;
        running = run;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(glyph);
        if (!run)
        {
            visual.StopAnimation(L"RotationAngleInDegrees");
            visual.RotationAngleInDegrees(0.0f);
            return;
        }
        auto compositor = visual.Compositor();
        auto const width = static_cast<float>(std::max(1.0, glyph.ActualWidth()));
        auto const height = static_cast<float>(std::max(1.0, glyph.ActualHeight()));
        visual.CenterPoint(float3{width * 0.5f, height * 0.5f, 0.0f});
        auto rotation = compositor.CreateScalarKeyFrameAnimation();
        rotation.InsertKeyFrame(0.0f, 0.0f);
        rotation.InsertKeyFrame(1.0f, 360.0f, compositor.CreateLinearEasingFunction());
        rotation.Duration(std::chrono::milliseconds(900));
        rotation.IterationBehavior(Microsoft::UI::Composition::AnimationIterationBehavior::Forever);
        visual.StartAnimation(L"RotationAngleInDegrees", rotation);
    }

    void MainWindow::ApplyMotionPreference()
    {
        if (m_motionApplied == m_motionAllowed) return;
        m_motionApplied = m_motionAllowed;
        // Native list transitions follow the same Windows preference.
        DeviceCards().ChildrenTransitions(m_motionAllowed ? m_deviceTransitions : nullptr);
        DeviceVisibilityList().ChildrenTransitions(m_motionAllowed ? m_visibilityTransitions : nullptr);
        // Session figures slide in when a session starts; they never animate out.
        for (UIElement element : {StatsFigures().as<UIElement>()})
        {
            if (!m_motionAllowed)
            {
                Hosting::ElementCompositionPreview::SetImplicitShowAnimation(element, nullptr);
                continue;
            }
            Hosting::ElementCompositionPreview::SetIsTranslationEnabled(element, true);
            auto compositor = Hosting::ElementCompositionPreview::GetElementVisual(element).Compositor();
            auto const ease = Decelerate(compositor);
            auto opacity = compositor.CreateScalarKeyFrameAnimation();
            opacity.Target(L"Opacity");
            opacity.InsertKeyFrame(0.0f, 0.0f);
            opacity.InsertKeyFrame(1.0f, 1.0f, ease);
            opacity.Duration(std::chrono::milliseconds(300));
            auto translation = compositor.CreateVector3KeyFrameAnimation();
            translation.Target(L"Translation");
            translation.InsertKeyFrame(0.0f, float3{0.0f, 10.0f, 0.0f});
            translation.InsertKeyFrame(1.0f, float3{0.0f, 0.0f, 0.0f}, ease);
            translation.Duration(std::chrono::milliseconds(300));
            auto group = compositor.CreateAnimationGroup();
            group.Add(opacity);
            group.Add(translation);
            Hosting::ElementCompositionPreview::SetImplicitShowAnimation(element, group);
        }
        if (!m_motionAllowed)
        {
            UpdatePulse(false, false);
            UpdateRadar(false);
            SpinGlyph(RefreshGlyph(), false, m_discoverySpinRunning);
            SpinGlyph(SourceRetryGlyph(), false, m_sourceSpinRunning);
        }
        // Route halos, particles and row equalizers reconcile on the next render.
        m_routeSignature.clear();
        m_forceDeviceRender = true;
    }

    void MainWindow::FlashVisibilityCard()
    {
        // Arriving from "管理设备": the speaker visibility card glows once in
        // the accent wash so the eye lands on it.
        if (!m_motionAllowed) return;
        Media::Animation::DoubleAnimation glow;
        glow.From(Windows::Foundation::IReference<double>(0.0));
        glow.To(Windows::Foundation::IReference<double>(1.0));
        glow.Duration(DurationHelper::FromTimeSpan(std::chrono::milliseconds(450)));
        glow.AutoReverse(true);
        Media::Animation::ExponentialEase ease;
        ease.Exponent(4.0);
        ease.EasingMode(Media::Animation::EasingMode::EaseOut);
        glow.EasingFunction(ease);
        Media::Animation::Storyboard::SetTarget(glow, VisibilityHighlight());
        Media::Animation::Storyboard::SetTargetProperty(glow, L"Opacity");
        if (m_highlightStoryboard) m_highlightStoryboard.Stop();
        m_highlightStoryboard = Media::Animation::Storyboard();
        m_highlightStoryboard.Children().Append(glow);
        m_highlightStoryboard.Begin();
    }

    void MainWindow::AnimateStateChange()
    {
        if (!m_motionAllowed || !IsWindowVisible(m_hwnd)) return;
        AnimateIn(TargetName(), 0.0f, 8.0f, 280, 0.0f);
        AnimateIn(StatusDescription(), 0.0f, 6.0f, 340, 0.0f);
        PopElement(StatusDot(), 8.0f);
    }

    void MainWindow::UpdateWorkspaceLayout(double width)
    {
        // XAML layout reports effective pixels. Use the actual client content,
        // not the HWND's outer frame or an assumed monitor scale.
        bool const wide = width >= 1050.0;
        if (wide != m_wideLayout) m_routeSignature.clear();
        m_wideLayout = wide;
        WideWorkspaceTrigger().IsActive(wide);
        CompactWorkspaceTrigger().IsActive(!wide);
    }

    void MainWindow::OnLayoutRootSizeChanged(IInspectable const&, SizeChangedEventArgs const& args)
    {
        if (m_loaded) UpdateWorkspaceLayout(args.NewSize().Width);
    }

    void MainWindow::OnWaveformSizeChanged(IInspectable const&, SizeChangedEventArgs const&)
    {
        if (m_loaded) RenderAudio(m_snapshot);
    }

    void MainWindow::OnRouteSizeChanged(IInspectable const&, SizeChangedEventArgs const&)
    {
        if (m_loaded) RenderRoute(m_snapshot);
    }

    void MainWindow::ShowWindow()
    {
        m_desktop.Show();
        Activate();
        PollEngine();
    }

    void MainWindow::ExitNow()
    {
        if (m_exiting) return;
        m_exiting = true;
        m_desktop.BeginExit();
        m_updater.PrepareForExit();
        if (m_pollTimer) m_pollTimer.Stop();
        if (m_volumeTimer) m_volumeTimer.Stop();
        if (m_authDialog) m_authDialog.Hide();
        if (m_engine.Available()) Send(L"disconnect");
        m_engine.Shutdown();
        m_desktop.Detach();
        Close();
        Application::Current().Exit();
    }

    void MainWindow::OnExitClick(IInspectable const&, RoutedEventArgs const&)
    {
        ExitNow();
    }
}
