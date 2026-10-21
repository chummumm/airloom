#include "pch.h"
#include "MainWindow.xaml.h"
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

#include <cstdio>

using namespace winrt;
using namespace Windows::Foundation;
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

    bool SameDevices(Snapshot const& a, Snapshot const& b)
    {
        return a.selectedDeviceId == b.selectedDeviceId && a.devices.size() == b.devices.size() &&
            std::equal(a.devices.begin(), a.devices.end(), b.devices.begin(), [](Device const& left, Device const& right)
                {
                    return left.id == right.id && left.name == right.name && left.model == right.model &&
                        left.address == right.address && left.passwordRequired == right.passwordRequired && left.online == right.online;
                });
    }

    bool SameSources(Snapshot const& a, Snapshot const& b)
    {
        return a.selectedSourceId == b.selectedSourceId && a.sources.size() == b.sources.size() &&
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

    void SetAccessibleName(DependencyObject const& element, hstring const& name)
    {
        Automation::AutomationProperties::SetName(element, name);
    }

    Grid SpeakerIcon(Media::Brush const& accent, Media::Brush const& body)
    {
        Grid icon;
        icon.Width(40);
        icon.Height(47);
        Rectangle shell;
        shell.Width(33);
        shell.Height(40);
        shell.RadiusX(13);
        shell.RadiusY(13);
        shell.Fill(body);
        shell.VerticalAlignment(VerticalAlignment::Bottom);
        icon.Children().Append(shell);
        Ellipse top;
        top.Width(31);
        top.Height(9);
        top.Fill(accent);
        top.Opacity(0.65);
        top.VerticalAlignment(VerticalAlignment::Top);
        top.Margin(Thickness{0, 5, 0, 0});
        icon.Children().Append(top);
        return icon;
    }
}

namespace winrt::Airloom::implementation
{
    MainWindow::MainWindow()
    {
        InitializeComponent();
        InitializeWindow();
        InitializePreferences();

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
                    self->m_volumeTimer.Stop();
                    self->m_volumePending = false;
                    auto command = MakeCommand(L"set_volume");
                    command.Insert(L"volume", JsonValue::CreateNumberValue(self->m_pendingVolume));
                    self->Send(command);
                }
            });

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
        auto nativeWindow = this->as<IWindowNative>();
        check_hresult(nativeWindow->get_WindowHandle(&m_hwnd));
        Title(L"Airloom");
        ExtendsContentIntoTitleBar(true);
        SetTitleBar(TitleDragRegion());

        auto const dpi = GetDpiForWindow(m_hwnd);
        auto const width = MulDiv(1140, static_cast<int>(dpi), 96);
        auto const height = MulDiv(810, static_cast<int>(dpi), 96);
        MONITORINFO monitor{sizeof(monitor)};
        GetMonitorInfoW(MonitorFromWindow(m_hwnd, MONITOR_DEFAULTTONEAREST), &monitor);
        auto const usableWidth = monitor.rcWork.right - monitor.rcWork.left;
        auto const usableHeight = monitor.rcWork.bottom - monitor.rcWork.top;
        auto const actualWidth = std::min(width, usableWidth);
        auto const actualHeight = std::min(height, usableHeight);
        SetWindowPos(m_hwnd, nullptr, monitor.rcWork.left + (usableWidth - actualWidth) / 2,
            monitor.rcWork.top + (usableHeight - actualHeight) / 2,
            actualWidth, actualHeight, SWP_NOZORDER | SWP_NOACTIVATE);

        m_desktop.Attach(m_hwnd,
            [this] { ShowWindow(); },
            [this] { ExitNow(); });
        ApplyTheme();
    }

    void MainWindow::InitializePreferences()
    {
        ThemePicker().SelectedIndex(static_cast<int32_t>(Preferences::Theme()));
        m_visualizerEnabled = Preferences::Visualizer();
        VisualizerToggle().IsOn(m_visualizerEnabled);
        StartupToggle().IsOn(Preferences::StartsWithWindows());
        auto const theme = Preferences::Theme();
        LayoutRoot().RequestedTheme(theme == 1 ? ElementTheme::Light : theme == 2 ? ElementTheme::Dark : ElementTheme::Default);
        m_preferencesReady = true;
        ApplyTheme();
    }

    void MainWindow::OnLoaded(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_loaded) return;
        m_loaded = true;
        if (m_engine.Available())
        {
            Send(L"refresh");
            PollEngine();
            m_pollTimer.Start();
        }
        else
        {
            ErrorBanner().Title(L"音频引擎未就绪");
            ErrorBanner().Message(m_engine.LoadError());
            ErrorBanner().IsOpen(true);
            RetryButton().IsEnabled(false);
            RefreshButton().IsEnabled(false);
            SourcePicker().IsEnabled(false);
            SourcePicker().PlaceholderText(L"音频引擎未就绪");
            StatusBadge().Text(L"需要安装音频引擎");
            StatusDescription().Text(L"重新安装后，设备与声音来源会自动显示在这里。");
        }
        if (!m_desktop.HasTrayIcon())
        {
            FooterMessage().Text(L"系统托盘暂不可用，关闭窗口会退出 Airloom。");
        }
        if (m_background && m_desktop.HasTrayIcon())
        {
            ::ShowWindow(m_hwnd, SW_HIDE);
        }
    }

    Media::Brush MainWindow::Brush(wchar_t const* resource) const
    {
        // Theme dictionaries are resolved by XAML for static elements. Dynamic device cards
        // obtain the matching resource dictionary explicitly when the actual theme changes.
        auto dictionaries = Application::Current().Resources().ThemeDictionaries();
        auto key = LayoutRoot().ActualTheme() == ElementTheme::Light ? L"Light" : L"Default";
        Windows::UI::ViewManagement::AccessibilitySettings accessibility;
        if (accessibility.HighContrast()) key = L"HighContrast";
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
        if (m_exiting || !m_loaded || !m_engine.Available()) return;
        ++m_tickCount;
        // Keep discovery/reconnect current while minimizing work when only the tray is visible.
        if (!IsWindowVisible(m_hwnd) && m_tickCount % 8 != 0) return;
        if (m_tickCount % 30 == 1)
        {
            BOOL enabled = TRUE;
            if (SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &enabled, 0))
                m_motionAllowed = enabled != FALSE;
        }
        try
        {
            auto next = Snapshot::Parse(m_engine.Snapshot());
            RenderSnapshot(next);
            m_snapshot = std::move(next);
            if (m_authDialog && (m_snapshot.state != L"auth_required" || m_dialogRequestId != m_snapshot.authRequestId))
            {
                m_authDialog.Hide();
            }
            if (m_snapshot.state != L"auth_required")
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
        bool const streaming = next.state == L"streaming";
        bool const busy = next.state == L"connecting" || next.state == L"reconnecting";
        bool const auth = next.state == L"auth_required";
        bool const discovering = next.state == L"discovering";
        auto const device = next.SelectedDevice();
        if (!m_devicesRendered || m_forceDeviceRender || !SameDevices(m_snapshot, next)) RenderDevices(next);
        if (!m_sourcesRendered || !SameSources(m_snapshot, next)) RenderSources(next);
        DeviceCards().IsEnabled(!active);
        SourcePicker().IsEnabled(!active && !next.sources.empty());
        DeviceCount().Text(next.devices.empty() ? L"在同一个网络中寻找" : to_hstring(next.devices.size()) + L" 台设备 · 选择声音的目的地");
        EmptyDeviceTitle().Text(discovering ? L"正在寻找附近的扬声器" : L"还没有发现扬声器");
        DiscoveryRing().IsActive(discovering);
        DiscoveryRing().Visibility(discovering ? Visibility::Visible : Visibility::Collapsed);

        hstring badge = L"等待连接";
        hstring eyebrow = L"你的声音，即将出发";
        hstring description = device ? L"准备好了，就开始连接。" : L"在左侧选择设备，然后开始连接。";
        hstring action = L"开始连接";
        if (discovering) { badge = L"正在发现"; description = L"正在寻找同一网络中的 AirPlay 扬声器。"; }
        if (streaming) { badge = L"正在传输"; eyebrow = L"声音正在这里流动"; description = L"现在播放电脑上的音乐，听见整个房间。"; action = L"断开连接"; }
        if (next.state == L"connecting") { badge = L"正在连接"; eyebrow = L"让声音准备就绪"; description = L"正在与扬声器建立连接…"; action = L"取消连接"; }
        if (next.state == L"reconnecting") { badge = L"正在重连"; eyebrow = L"正在找回声音"; description = L"连接暂时中断，正在尝试恢复…"; action = L"停止重连"; }
        if (auth) { badge = L"需要验证"; eyebrow = L"再一步，就能听见"; description = L"这台设备需要访问密码或配对码。"; action = next.authKind == L"pin" ? L"输入配对码" : L"输入访问密码"; }
        if (next.state == L"error") { badge = L"连接未完成"; eyebrow = L"稍作调整，再出发"; description = L"查看上方提示后，再试一次。"; action = L"重新连接"; }
        if (!next.statusMessage.empty()) description = next.statusMessage;

        StatusBadge().Text(badge);
        SessionEyebrow().Text(eyebrow);
        TargetName().Text(device ? device->name : L"选择一台扬声器");
        StatusDescription().Text(description);
        StatusDot().Fill(Brush(streaming ? L"AudioBrush" : busy || auth ? L"BrandBrush" : L"MutedBrush"));
        ConnectLabel().Text(action);
        ConnectButton().IsEnabled(active || (device && device->online && !next.selectedSourceId.empty()));
        ConnectGlyph().Glyph(streaming ? L"\uE71A" : auth ? L"\uE72E" : L"\uE768");
        ConnectGlyph().Visibility(busy && m_motionAllowed ? Visibility::Collapsed : Visibility::Visible);
        ConnectSpinner().Visibility(busy && m_motionAllowed ? Visibility::Visible : Visibility::Collapsed);
        ConnectSpinner().IsActive(busy && m_motionAllowed);
        CancelAuthButton().Visibility(auth ? Visibility::Visible : Visibility::Collapsed);
        ForgetButton().IsEnabled(device != nullptr && !active);
        VolumeSlider().IsEnabled(device != nullptr);
        if (!m_volumePending)
        {
            VolumeSlider().Value(std::round(next.volume * 100.0));
            VolumeLabel().Text(to_hstring(static_cast<int>(std::round(next.volume * 100.0))) + L"%");
        }
        LocalMuteToggle().IsEnabled(!next.selectedSourceId.empty());
        LocalMuteToggle().IsOn(next.muteLocal);

        if (m_desktop.HasTrayIcon())
        {
            FooterMessage().Text(next.muteLocal
                ? L"电脑静音会影响使用同一输出设备的其他应用；退出后恢复由音频引擎管理。"
                : L"无需新装虚拟声卡 · 声源需是 Windows 中已存在的播放设备");
        }
        if (next.state != m_snapshot.state)
        {
            AnimateStateChange();
            m_desktop.SetTooltip(L"Airloom · " + badge + (device ? L" · " + device->name : L""));
        }
        RenderAudio(next);
        RenderError(next);
    }

    void MainWindow::RenderDevices(Snapshot const& next)
    {
        DeviceCards().Children().Clear();
        for (auto const& device : next.devices)
        {
            bool const selected = device.id == next.selectedDeviceId;
            Button card;
            card.HorizontalAlignment(HorizontalAlignment::Stretch);
            card.HorizontalContentAlignment(HorizontalAlignment::Stretch);
            card.Padding(Thickness{12, 12, 12, 12});
            card.CornerRadius(CornerRadius{14});
            card.BorderThickness(Thickness{1});
            card.BorderBrush(Brush(selected ? L"BrandBrush" : L"HairlineBrush"));
            card.Background(Brush(selected ? L"BrandSoftBrush" : L"InsetBrush"));
            card.IsEnabled(device.online);
            SetAccessibleName(card, device.name + (selected ? L"，已选择" : L"，选择扬声器") + (device.passwordRequired ? L"，需要验证" : L""));
            ToolTipService::SetToolTip(card, box_value(device.address));

            Grid contents;
            contents.ColumnSpacing(11);
            ColumnDefinition iconColumn;
            iconColumn.Width(GridLength{40, GridUnitType::Pixel});
            ColumnDefinition textColumn;
            textColumn.Width(GridLength{1, GridUnitType::Star});
            ColumnDefinition checkColumn;
            checkColumn.Width(GridLength{16, GridUnitType::Pixel});
            contents.ColumnDefinitions().Append(iconColumn);
            contents.ColumnDefinitions().Append(textColumn);
            contents.ColumnDefinitions().Append(checkColumn);
            contents.Children().Append(SpeakerIcon(Brush(L"BrandBrush"), Brush(L"SpeakerBodyBrush")));

            StackPanel labels;
            Grid::SetColumn(labels, 1);
            labels.Spacing(5);
            labels.VerticalAlignment(VerticalAlignment::Center);
            TextBlock name;
            name.Text(device.name);
            name.FontSize(14);
            name.FontWeight(Windows::UI::Text::FontWeights::SemiBold());
            name.TextTrimming(TextTrimming::CharacterEllipsis);
            labels.Children().Append(name);
            TextBlock detail;
            detail.FontSize(11);
            detail.Foreground(Brush(L"MutedBrush"));
            detail.TextTrimming(TextTrimming::CharacterEllipsis);
            detail.Text(!device.online ? L"暂时离线" : device.model.empty() ? L"AirPlay 扬声器" : device.model);
            labels.Children().Append(detail);
            contents.Children().Append(labels);

            if (selected || device.passwordRequired)
            {
                FontIcon marker;
                Grid::SetColumn(marker, 2);
                marker.Glyph(selected ? L"\uE73E" : L"\uE72E");
                marker.FontSize(selected ? 13 : 11);
                marker.Foreground(Brush(selected ? L"BrandBrush" : L"MutedBrush"));
                contents.Children().Append(marker);
            }
            card.Content(contents);
            card.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                {
                    if (auto self = weak.get())
                    {
                        auto command = MakeCommand(L"select_device");
                        command.Insert(L"device_id", JsonValue::CreateStringValue(id));
                        self->Send(command);
                    }
                });
            DeviceCards().Children().Append(card);
        }
        DevicesEmptyState().Visibility(next.devices.empty() ? Visibility::Visible : Visibility::Collapsed);
        m_devicesRendered = true;
        m_forceDeviceRender = false;
    }

    void MainWindow::RenderSources(Snapshot const& next)
    {
        SourcePicker().Items().Clear();
        int32_t selected = -1;
        int32_t index = 0;
        for (auto const& source : next.sources)
        {
            ComboBoxItem item;
            item.Content(box_value(source.name));
            item.Tag(box_value(source.id));
            ToolTipService::SetToolTip(item, box_value(source.name));
            SourcePicker().Items().Append(item);
            if (source.id == next.selectedSourceId) selected = index;
            ++index;
        }
        SourcePicker().SelectedIndex(selected);
        SourcePicker().PlaceholderText(next.sources.empty() ? L"未找到可用的播放设备" : L"选择 Windows 播放设备");
        m_sourcesRendered = true;
    }

    void MainWindow::RenderAudio(Snapshot const& next)
    {
        bool const streaming = next.state == L"streaming";
        double const left = streaming ? next.leftLevel : 0.0;
        double const right = streaming ? next.rightLevel : 0.0;
        LeftMeter().Value(left);
        RightMeter().Value(right);
        LeftDb().Text(Decibels(left));
        RightDb().Text(Decibels(right));
        SampleRateText().Text(SampleRateLabel(next.sampleRate));
        ElapsedText().Text(streaming || next.elapsedSeconds > 0 ? DurationText(next.elapsedSeconds) : L"—");
        PacketsText().Text(next.packetsSent > 0 ? to_hstring(next.packetsSent) : L"—");

        auto const width = std::max(1.0, WaveformCanvas().ActualWidth());
        auto const height = std::max(1.0, WaveformCanvas().ActualHeight());
        auto const center = height * 0.5;
        WaveformBaseline().X2(width);
        WaveformBaseline().Y1(center);
        WaveformBaseline().Y2(center);
        bool const waveformAllowed = m_visualizerEnabled && m_motionAllowed;
        bool const hasSignal = streaming && std::max(left, right) > 0.00005;
        WaveformLine().Visibility(waveformAllowed && streaming ? Visibility::Visible : Visibility::Collapsed);
        WaveformHint().Visibility(hasSignal && waveformAllowed ? Visibility::Collapsed : Visibility::Visible);
        WaveformHint().Text(!streaming ? L"连接后显示实时音频" : !m_visualizerEnabled ? L"动态波形已关闭 · 电平仍在实时更新" : !m_motionAllowed ? L"已遵循 Windows 动画设置 · 电平仍在更新" : L"等待声音 · 试着播放一首音乐");
        if (!waveformAllowed || !streaming) return;

        PointCollection points;
        if (next.waveform.size() >= 2)
        {
            auto const count = next.waveform.size();
            for (size_t index = 0; index < count; ++index)
            {
                points.Append(Point{static_cast<float>(width * static_cast<double>(index) / static_cast<double>(count - 1)),
                    static_cast<float>(center - next.waveform[index] * (height * 0.43))});
            }
        }
        else
        {
            points.Append(Point{0, static_cast<float>(center)});
            points.Append(Point{static_cast<float>(width), static_cast<float>(center)});
        }
        WaveformLine().Points(points);
    }

    void MainWindow::RenderError(Snapshot const& next)
    {
        bool const localError = !m_localError.empty() && std::chrono::steady_clock::now() < m_localErrorUntil;
        bool const engineError = !next.errorMessage.empty();
        ErrorBanner().IsOpen(localError || engineError);
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
        ErrorBanner().IsOpen(true);
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
        if (m_snapshot.state == L"auth_required") ShowAuthentication(true);
        else if (!m_snapshot.selectedDeviceId.empty() && !m_snapshot.selectedSourceId.empty()) Send(L"connect");
        else Send(L"refresh");
    }

    void MainWindow::OnConnect(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.state == L"auth_required") ShowAuthentication(true);
        else Send(IsActive(m_snapshot.state) ? L"disconnect" : L"connect");
    }

    void MainWindow::OnCancelAuth(IInspectable const&, RoutedEventArgs const&)
    {
        Send(L"disconnect");
    }

    void MainWindow::OnSourceChanged(IInspectable const&, SelectionChangedEventArgs const&)
    {
        if (m_rendering || !m_loaded) return;
        auto item = SourcePicker().SelectedItem().try_as<ComboBoxItem>();
        if (!item) return;
        auto command = MakeCommand(L"select_source");
        command.Insert(L"source_id", JsonValue::CreateStringValue(unbox_value<hstring>(item.Tag())));
        Send(command);
    }

    void MainWindow::OnVolumeChanged(IInspectable const&, RangeBaseValueChangedEventArgs const& args)
    {
        if (m_rendering || !m_loaded || !m_volumeTimer) return;
        m_pendingVolume = std::clamp(args.NewValue() / 100.0, 0.0, 1.0);
        m_volumePending = true;
        VolumeLabel().Text(to_hstring(static_cast<int>(std::round(args.NewValue()))) + L"%");
        m_volumeTimer.Stop();
        m_volumeTimer.Start();
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
        if (m_authDialog || m_exiting || !LayoutRoot().XamlRoot() || m_snapshot.state != L"auth_required") co_return;
        auto const fingerprint = AuthenticationFingerprint();
        if (!explicitlyRequested && m_authFingerprint == fingerprint) co_return;
        m_authFingerprint = fingerprint;
        bool const pin = m_snapshot.authKind == L"pin";
        auto const requestId = m_snapshot.authRequestId;
        auto const authDeviceId = m_snapshot.authDeviceId;
        auto const device = m_snapshot.SelectedDevice();

        ContentDialog dialog;
        m_authDialog = dialog;
        m_dialogRequestId = requestId;
        dialog.XamlRoot(LayoutRoot().XamlRoot());
        dialog.RequestedTheme(LayoutRoot().ActualTheme());
        dialog.Title(box_value(pin ? L"输入设备配对码" : L"输入扬声器访问密码"));
        dialog.PrimaryButtonText(L"继续连接");
        if (!pin) dialog.SecondaryButtonText(L"使用配对码");
        dialog.CloseButtonText(L"取消");
        dialog.DefaultButton(ContentDialogButton::Primary);

        StackPanel content;
        content.Spacing(16);
        content.MaxWidth(400);
        TextBlock description;
        description.TextWrapping(TextWrapping::Wrap);
        description.Text(!m_snapshot.authMessage.empty() ? m_snapshot.authMessage :
            pin ? L"输入设备显示或播报的配对码，再继续连接。" : L"输入在 Apple 家庭设置中为扬声器设置的 AirPlay 访问密码。");
        content.Children().Append(description);
        PasswordBox password;
        password.Header(box_value(device ? device->name : pin ? L"配对码" : L"访问密码"));
        password.PlaceholderText(pin ? L"配对码" : L"访问密码");
        password.PasswordRevealMode(PasswordRevealMode::Peek);
        password.MaxLength(512);
        SetAccessibleName(password, pin ? L"设备配对码" : L"AirPlay 访问密码");
        content.Children().Append(password);
        TextBlock validation;
        validation.Text(pin ? L"请先输入配对码。" : L"请先输入访问密码。");
        validation.Foreground(SolidColorBrush(Windows::UI::Color{255, 196, 65, 65}));
        validation.FontSize(12);
        validation.Visibility(Visibility::Collapsed);
        content.Children().Append(validation);
        CheckBox remember;
        remember.Content(box_value(L"在这台电脑上记住此设备"));
        remember.IsChecked(false);
        content.Children().Append(remember);
        TextBlock note;
        note.Text(L"凭据不会显示在连接状态或日志中。可在设置中忘记此设备。" );
        note.TextWrapping(TextWrapping::Wrap);
        note.FontSize(12);
        note.Foreground(Brush(L"MutedBrush"));
        content.Children().Append(note);
        dialog.Content(content);
        dialog.PrimaryButtonClick([password, validation](auto const&, ContentDialogButtonClickEventArgs const& args)
            {
                if (password.Password().empty())
                {
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
            if (!m_exiting && m_snapshot.state == L"auth_required" &&
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

    void MainWindow::OnThemeChanged(IInspectable const&, SelectionChangedEventArgs const&)
    {
        if (!m_preferencesReady) return;
        auto const selected = ThemePicker().SelectedIndex();
        if (selected < 0) return;
        LayoutRoot().RequestedTheme(selected == 1 ? ElementTheme::Light : selected == 2 ? ElementTheme::Dark : ElementTheme::Default);
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
        auto const foreground = dark ? Windows::UI::Color{255, 240, 240, 247} : Windows::UI::Color{255, 30, 32, 45};
        titlebar.ButtonForegroundColor(foreground);
        titlebar.ButtonBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonInactiveBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonHoverBackgroundColor(dark ? Windows::UI::Color{30, 255, 255, 255} : Windows::UI::Color{18, 0, 0, 0});
        m_forceDeviceRender = true;
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

    void MainWindow::AnimateStateChange()
    {
        if (!m_motionAllowed || !IsWindowVisible(m_hwnd)) return;
        auto visual = Hosting::ElementCompositionPreview::GetElementVisual(TargetName());
        auto animation = visual.Compositor().CreateScalarKeyFrameAnimation();
        animation.InsertKeyFrame(0.0f, 0.45f);
        animation.InsertKeyFrame(1.0f, 1.0f);
        animation.Duration(std::chrono::milliseconds(180));
        visual.StartAnimation(L"Opacity", animation);
    }

    void MainWindow::OnWaveformSizeChanged(IInspectable const&, SizeChangedEventArgs const&)
    {
        if (m_loaded) RenderAudio(m_snapshot);
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
