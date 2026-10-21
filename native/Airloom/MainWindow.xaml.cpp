#include "pch.h"
#include "MainWindow.xaml.h"
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

#include <cstdio>
#include <iterator>
#include <winrt/Windows.ApplicationModel.DataTransfer.h>

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
                        left.errorMessage == right.errorMessage;
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

    void SetText(TextBlock const& target, hstring const& value)
    {
        if (target.Text() != value) target.Text(value);
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
        Microsoft::UI::Xaml::Window projectedWindow = *this;
        auto nativeWindow = projectedWindow.as<IWindowNative>();
        check_hresult(nativeWindow->get_WindowHandle(&m_hwnd));
        Title(L"Airloom");
        ExtendsContentIntoTitleBar(true);
        SetTitleBar(TitleDragRegion());

        auto const dpi = GetDpiForWindow(m_hwnd);
        auto const width = MulDiv(1140, static_cast<int>(dpi), 96);
        auto const height = MulDiv(810, static_cast<int>(dpi), 96);
        MONITORINFO monitor{sizeof(monitor)};
        GetMonitorInfoW(MonitorFromWindow(m_hwnd, MONITOR_DEFAULTTONEAREST), &monitor);
        auto const usableWidth = static_cast<int>(monitor.rcWork.right - monitor.rcWork.left);
        auto const usableHeight = static_cast<int>(monitor.rcWork.bottom - monitor.rcWork.top);
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
        m_highContrast = HighContrastEnabled();
        m_hiddenDevices = Preferences::HiddenDevices();
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
        RenderUpdate();
        m_pollTimer.Start();
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
        // Theme dictionaries are resolved by XAML for static elements. Dynamic device cards
        // obtain the matching resource dictionary explicitly when the actual theme changes.
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
        bool const busy = next.state == L"connecting" || next.state == L"reconnecting";
        bool const auth = next.authRequestId != 0;
        bool const discovering = next.state == L"discovering";
        bool const sourceError = next.errorCode == L"capture_sources";
        auto const device = next.SelectedDevice();
        bool const hasSource = std::any_of(next.sources.begin(), next.sources.end(), [&next](auto const& source)
            { return source.id == next.selectedSourceId; });
        bool const devicesChanged = !m_devicesRendered || m_forceDeviceRender || !SameDevices(m_snapshot, next);
        if (devicesChanged)
        {
            RenderDevices(next);
            if (m_settingsOpen) RenderDeviceVisibility(next);
            else m_visibilityRendered = false;
        }
        if (!m_sourcesRendered || !SameSources(m_snapshot, next)) RenderSources(next);
        SourcePicker().IsEnabled(!next.sources.empty());
        auto const visibleCount = static_cast<size_t>(std::count_if(next.devices.begin(), next.devices.end(), [this](Device const& entry)
            { return !IsDeviceHidden(entry.id); }));
        DeviceCount().Text(visibleCount == 0 ? L"在同一个网络中寻找" :
            to_hstring(visibleCount) + L" 台设备 · 已选 " + to_hstring(next.selectedDeviceIds.size()) + L" 台");
        bool const hiddenOnly = visibleCount == 0 && !m_hiddenDevices.empty();
        EmptyDeviceTitle().Text(discovering ? L"正在寻找附近的扬声器" : hiddenOnly ? L"设备已收好" : L"还没有发现扬声器");
        EmptyDeviceDescription().Text(hiddenOnly ? L"在设置中恢复已隐藏的设备，\n或重新发现附近的扬声器。" :
            L"让电脑和 HomePod 连接同一网络，\n然后点右上角重新发现。");
        DiscoveryRing().IsActive(discovering);
        DiscoveryRing().Visibility(discovering ? Visibility::Visible : Visibility::Collapsed);

        hstring badge = L"等待连接";
        hstring eyebrow = L"尚未连接";
        hstring description = device ? L"使用当前声音来源连接已选扬声器。" : L"在左侧选择一台或多台扬声器，然后开始连接。";
        hstring action = L"开始连接";
        if (discovering) { badge = L"正在发现"; description = L"正在寻找同一网络中的 AirPlay 扬声器。"; }
        if (streaming) { badge = L"正在传输"; eyebrow = L"正在向 " + to_hstring(streamingCount) + L" 台扬声器发送音频"; action = L"全部断开"; }
        if (next.state == L"connecting") { badge = L"正在连接"; eyebrow = L"正在建立连接"; description = L"正在与扬声器建立连接…"; action = L"取消连接"; }
        if (next.state == L"reconnecting") { badge = L"正在重连"; eyebrow = L"连接恢复中"; description = L"连接暂时中断，正在尝试恢复…"; action = L"停止重连"; }
        if (auth) { badge = streaming ? L"传输中 · 待验证" : L"需要验证"; description = L"所选设备需要访问密码或配对码。"; action = next.authKind == L"pin" ? L"输入配对码" : L"输入访问密码"; }
        if (next.state == L"error") { badge = L"连接未完成"; eyebrow = L"连接需要处理"; description = L"查看上方提示后，再试一次。"; action = L"重新连接"; }
        // Keep an actionable failure in one place. Capture-source errors belong
        // beside the source control; session/network errors retain the banner.
        if (!next.statusMessage.empty() && next.errorMessage.empty()) description = next.statusMessage;
        if (sourceError)
        {
            badge = L"等待音频输出";
            eyebrow = L"等待声音来源";
            description = device ? L"完成声音来源设置后，可连接已选扬声器。" : L"在左侧选择一台或多台扬声器。";
            action = L"开始连接";
        }

        hstring targets;
        for (auto const& entry : next.devices)
        {
            if (!next.IsSelected(entry.id) || IsDeviceHidden(entry.id)) continue;
            if (!targets.empty()) targets = targets + L" · ";
            targets = targets + entry.name;
        }
        SetText(StatusBadge(), badge);
        SetText(SessionEyebrow(), eyebrow);
        SetText(TargetName(), targets.empty() ? L"选择扬声器" : targets);
        SetText(StatusDescription(), description);
        ToolTipService::SetToolTip(StatusDescription(), box_value(description));
        ToolTipService::SetToolTip(TargetName(), box_value(targets));
        StatusDot().Fill(Brush(streaming ? L"AudioBrush" : busy || auth ? L"BrandBrush" : L"MutedBrush"));
        SetText(ConnectLabel(), action);
        bool const selectedOnline = std::any_of(next.devices.begin(), next.devices.end(), [&next](Device const& entry)
            { return next.IsSelected(entry.id) && entry.online; });
        ConnectButton().IsEnabled(active || (selectedOnline && hasSource));
        ConnectGlyph().Glyph(auth ? L"\uE72E" : streaming ? L"\uE71A" : L"\uE768");
        ConnectGlyph().Visibility(busy && m_motionAllowed ? Visibility::Collapsed : Visibility::Visible);
        ConnectSpinner().Visibility(busy && m_motionAllowed ? Visibility::Visible : Visibility::Collapsed);
        ConnectSpinner().IsActive(busy && m_motionAllowed);
        CancelAuthButton().Visibility(auth ? Visibility::Visible : Visibility::Collapsed);
        CancelAuthButton().Content(box_value(L"取消此设备"));
        ForgetButton().IsEnabled(device != nullptr && !active);
        ForgetLabel().Text(device ? L"忘记“" + device->name + L"”凭据" : L"忘记设备凭据");
        ToolTipService::SetToolTip(ForgetButton(), box_value(ForgetLabel().Text()));
        // Volume is editable before connection. Local input remains authoritative
        // through the debounce AND acknowledgement, not just until command enqueue.
        VolumeSlider().IsEnabled(m_engine.Available());
        if (m_volumeAwaitingAck && !m_volumePending)
        {
            if (std::abs(next.volume - m_pendingVolume) < 0.0005)
                m_volumeAwaitingAck = false;
            else if (!m_volumePointerDown && std::chrono::steady_clock::now() >= m_volumeAckDeadline)
            {
                m_volumeAwaitingAck = false;
                ShowLocalError(L"音量调整尚未确认，已恢复显示当前音量。请重试。");
            }
        }
        if (!m_volumePending && !m_volumeAwaitingAck && !m_volumePointerDown)
        {
            auto const value = std::round(next.volume * 100.0);
            if (VolumeSlider().Value() != value) VolumeSlider().Value(value);
            SetText(VolumeLabel(), to_hstring(static_cast<int>(value)) + L"%");
        }
        LocalMuteToggle().IsEnabled(hasSource);
        LocalMuteToggle().IsOn(next.muteLocal);
        TestToneButton().IsEnabled(streaming);
        TestToneButton().Visibility(sourceError ? Visibility::Collapsed : Visibility::Visible);
        SourceRetryButton().Visibility(sourceError ? Visibility::Visible : Visibility::Collapsed);
        SourceRetryButton().IsEnabled(m_engine.Available());
        CopyDiagnosticsButton().IsEnabled(!next.diagnosticsJson.empty() && next.diagnosticsJson != L"{}");
        SetText(TestToneLabel(), next.testToneActive ? L"停止试播" : L"试播 3 秒");
        SetText(CaptureStatusText(), sourceError ? next.errorMessage : next.testToneActive ? L"正在播放测试音，结束后恢复电脑声音。" :
            !next.captureMessage.empty() ? next.captureMessage : L"连接后显示实际声音采集状态。");
        CaptureStatusText().MaxLines(sourceError ? 0 : 2);
        ToolTipService::SetToolTip(CaptureStatusText(), box_value(CaptureStatusText().Text()));
        CaptureStatusText().Foreground(Brush(sourceError ? L"DangerBrush" : next.captureState == L"signal" ? L"AudioBrush" :
            next.captureState == L"error" ? L"DangerBrush" :
            next.captureState == L"silent" || next.captureState == L"quiet" ? L"WarningBrush" : L"MutedBrush"));

        if (m_desktop.HasTrayIcon())
        {
            FooterMessage().Text(next.muteLocal
                ? L"电脑静音会影响同一输出上的其他应用；如果采集无声，可先关闭此选项。"
                : L"关闭窗口后继续在托盘运行。要结束传输，请选择“全部断开”或退出 Airloom。");
        }
        if (next.state != m_snapshot.state || next.selectedDeviceIds != m_snapshot.selectedDeviceIds)
        {
            AnimateStateChange();
            m_desktop.SetTooltip(L"Airloom · " + badge + (streaming ? L" · " + to_hstring(streamingCount) + L" 台" : L""));
        }
        RenderAudio(next);
        RenderError(next);
    }

    void MainWindow::RenderDevices(Snapshot const& next)
    {
        std::vector<hstring> visibleIds;
        for (auto const& device : next.devices)
            if (!IsDeviceHidden(device.id)) visibleIds.push_back(device.id);
        m_deviceRows.erase(std::remove_if(m_deviceRows.begin(), m_deviceRows.end(), [&visibleIds](auto const& row)
            { return std::find(visibleIds.begin(), visibleIds.end(), row.id) == visibleIds.end(); }), m_deviceRows.end());

        auto children = DeviceCards().Children();
        uint32_t position{};
        for (auto const& device : next.devices)
        {
            if (IsDeviceHidden(device.id)) continue;
            auto found = std::find_if(m_deviceRows.begin(), m_deviceRows.end(), [&device](auto const& row)
                { return row.id == device.id; });
            if (found == m_deviceRows.end())
            {
                DeviceRow created;
                created.id = device.id;
                created.button = Button();
                created.button.HorizontalAlignment(HorizontalAlignment::Stretch);
                created.button.HorizontalContentAlignment(HorizontalAlignment::Stretch);
                created.button.Padding(Thickness{10, 10, 10, 10});
                created.button.Height(72);
                created.button.CornerRadius(CornerRadius{8});
                Automation::AutomationProperties::SetAutomationId(created.button, L"Receiver:" + device.id);

                Grid contents;
                contents.ColumnSpacing(10);
                ColumnDefinition iconColumn, textColumn, markerColumn;
                iconColumn.Width(GridLength{18, GridUnitType::Pixel});
                textColumn.Width(GridLength{1, GridUnitType::Star});
                markerColumn.Width(GridLength{14, GridUnitType::Pixel});
                contents.ColumnDefinitions().Append(iconColumn);
                contents.ColumnDefinitions().Append(textColumn);
                contents.ColumnDefinitions().Append(markerColumn);
                created.icon = FontIcon();
                created.icon.Glyph(L"\uE767");
                created.icon.FontSize(16);
                contents.Children().Append(created.icon);
                StackPanel labels;
                Grid::SetColumn(labels, 1);
                labels.Spacing(5);
                labels.VerticalAlignment(VerticalAlignment::Center);
                created.name = TextBlock();
                created.name.FontSize(14);
                created.name.FontWeight(Windows::UI::Text::FontWeights::SemiBold());
                created.name.TextTrimming(TextTrimming::CharacterEllipsis);
                created.detail = TextBlock();
                created.detail.FontSize(12);
                created.detail.TextTrimming(TextTrimming::CharacterEllipsis);
                labels.Children().Append(created.name);
                labels.Children().Append(created.detail);
                contents.Children().Append(labels);
                created.marker = FontIcon();
                Grid::SetColumn(created.marker, 2);
                created.marker.FontSize(12);
                contents.Children().Append(created.marker);
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
                MenuFlyout actions;
                created.retry = MenuFlyoutItem();
                created.retry.Text(L"重新连接此设备");
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
                created.hide.Click([weak = get_weak(), id = device.id](auto const&, auto const&)
                    {
                        if (auto self = weak.get())
                        {
                            if (auto current = self->m_snapshot.FindDevice(id)) self->SetDeviceHidden(id, current->name, true);
                        }
                    });
                actions.Items().Append(created.hide);
                created.button.ContextFlyout(actions);
                m_deviceRows.push_back(std::move(created));
                found = std::prev(m_deviceRows.end());
            }

            auto& row = *found;
            bool const selected = next.IsSelected(device.id);
            row.button.BorderThickness(Thickness{m_highContrast ? 1.0 : 0.0});
            row.button.BorderBrush(Brush(L"ControlOutlineBrush"));
            row.button.Background(Brush(selected ? L"BrandSoftBrush" : L"AppCanvasBrush"));
            row.button.IsEnabled(device.online || selected);
            SetText(row.name, device.name);
            row.name.Foreground(Brush(selected ? L"BrandBrush" : L"InkBrush"));
            row.icon.Foreground(Brush(selected ? L"BrandBrush" : L"MutedBrush"));
            hstring stateLabel = !device.online ? L"暂时离线" : L"可连接";
            if (device.state == L"streaming") stateLabel = L"正在传输";
            else if (device.state == L"connecting") stateLabel = L"正在连接…";
            else if (device.state == L"reconnecting") stateLabel = L"正在重连…";
            else if (device.state == L"auth_required") stateLabel = L"等待密码或配对码";
            else if (device.state == L"error") stateLabel = L"连接失败 · 右键重试";
            else if (selected) stateLabel = L"已选择";
            else if (device.passwordRequired) stateLabel = L"需要密码";
            SetText(row.detail, stateLabel);
            row.detail.Foreground(Brush(m_highContrast && selected ? L"BrandBrush" :
                device.state == L"streaming" ? L"AudioBrush" : device.state == L"error" ? L"DangerBrush" : L"MutedBrush"));
            row.marker.Glyph(selected ? L"\uE73E" : L"\uE72E");
            row.marker.Foreground(Brush(selected ? L"BrandBrush" : L"MutedBrush"));
            // Hidden keeps the same marker slot for every state.
            row.marker.Opacity(selected || device.passwordRequired ? 1.0 : 0.0);
            row.retry.Visibility(selected && device.state != L"streaming" ? Visibility::Visible : Visibility::Collapsed);
            row.hide.Text(selected ? L"断开并隐藏设备" : L"隐藏设备");
            SetAccessibleName(row.button, device.name + (selected ? L"，已选择，" : L"，未选择，") + stateLabel);
            ToolTipService::SetToolTip(row.button, box_value(!device.errorMessage.empty() ? device.errorMessage :
                device.name + (device.model.empty() ? L"" : L" · " + device.model)));

            // Preserve the same native Button and focus object across snapshots.
            // Only insert/remove/move when the actual device identity list changes.
            uint32_t current{};
            if (!children.IndexOf(row.button, current)) children.InsertAt(position, row.button);
            else if (current != position)
            {
                children.RemoveAt(current);
                children.InsertAt(position, row.button);
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
        bool const streaming = next.StreamingCount() > 0;
        double const left = streaming ? next.leftLevel : 0.0;
        double const right = streaming ? next.rightLevel : 0.0;
        LeftMeter().Value(left);
        RightMeter().Value(right);
        SetText(LeftDb(), Decibels(left));
        SetText(RightDb(), Decibels(right));
        SetText(SampleRateText(), SampleRateLabel(next.sampleRate));
        SetText(ElapsedText(), streaming || next.elapsedSeconds > 0 ? DurationText(next.elapsedSeconds) : L"—");
        SetText(PacketsText(), to_hstring(next.packetsSent));

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
        bool const engineError = !next.errorMessage.empty() && next.errorCode != L"capture_sources";
        ErrorBanner().IsOpen(localError || engineError);
        // Settings owns maintenance actions; a source/receiver error is already
        // explained on the home view and must not displace the update controls.
        SettingsErrorBanner().IsOpen(localError);
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
        ErrorBanner().IsOpen(true);
        SettingsErrorBanner().Message(message);
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
        if (m_snapshot.authRequestId != 0) ShowAuthentication(true);
        else Send(IsActive(m_snapshot.state) ? L"disconnect" : L"connect");
    }

    void MainWindow::OnCancelAuth(IInspectable const&, RoutedEventArgs const&)
    {
        if (m_snapshot.authRequestId == 0) return;
        auto command = MakeCommand(L"disconnect");
        command.Insert(L"request_id", JsonValue::CreateNumberValue(static_cast<double>(m_snapshot.authRequestId)));
        Send(command);
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
        SetText(VolumeLabel(), to_hstring(static_cast<int>(std::round(args.NewValue()))) + L"%");
        m_volumeTimer.Stop();
        m_volumeTimer.Start();
    }

    void MainWindow::FlushVolume()
    {
        m_volumeTimer.Stop();
        if (!m_volumePending || m_exiting) return;
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
        password.Header(box_value(!m_snapshot.authDeviceName.empty() ? m_snapshot.authDeviceName :
            device ? device->name : pin ? L"配对码" : L"访问密码"));
        password.PlaceholderText(pin ? L"配对码" : L"访问密码");
        password.PasswordRevealMode(PasswordRevealMode::Peek);
        password.MaxLength(1024);
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

    void MainWindow::SetDeviceHidden(hstring const& id, hstring const& name, bool hidden)
    {
        if (hidden == IsDeviceHidden(id)) return;
        if (hidden && m_snapshot.IsSelected(id))
        {
            auto command = MakeCommand(L"toggle_device");
            command.Insert(L"device_id", JsonValue::CreateStringValue(id));
            command.Insert(L"selected", JsonValue::CreateBooleanValue(false));
            if (!Send(command)) return;
        }
        auto updated = m_hiddenDevices;
        updated.erase(std::remove_if(updated.begin(), updated.end(), [&id](auto const& entry)
            { return entry.id == id; }), updated.end());
        if (hidden) updated.push_back({id, name});
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
        DeviceVisibilityList().Children().Clear();
        std::vector<airloom::native::HiddenDevice> entries;
        for (auto const& device : next.devices) entries.push_back({device.id, device.name});
        for (auto const& hidden : m_hiddenDevices)
        {
            if (!next.FindDevice(hidden.id)) entries.push_back(hidden);
        }
        auto const visible = std::count_if(next.devices.begin(), next.devices.end(), [this](Device const& device)
            { return !IsDeviceHidden(device.id); });
        VisibilitySummary().Text(to_hstring(visible) + L" 台显示 · " + to_hstring(m_hiddenDevices.size()) + L" 台隐藏");
        VisibilityEmptyText().Visibility(entries.empty() ? Visibility::Visible : Visibility::Collapsed);
        for (auto const& entry : entries)
        {
            bool const hidden = IsDeviceHidden(entry.id);
            auto const discovered = next.FindDevice(entry.id);
            Border row;
            row.Background(Brush(L"InsetBrush"));
            row.CornerRadius(CornerRadius{12});
            row.Padding(Thickness{16, 13, 14, 13});
            Grid layout;
            layout.ColumnSpacing(16);
            ColumnDefinition labelColumn;
            labelColumn.Width(GridLength{1, GridUnitType::Star});
            ColumnDefinition actionColumn;
            actionColumn.Width(GridLength{1, GridUnitType::Auto});
            layout.ColumnDefinitions().Append(labelColumn);
            layout.ColumnDefinitions().Append(actionColumn);
            StackPanel labels;
            labels.Spacing(4);
            labels.VerticalAlignment(VerticalAlignment::Center);
            TextBlock title;
            title.Text(entry.name.empty() ? entry.id : entry.name);
            title.FontSize(14);
            title.FontWeight(Windows::UI::Text::FontWeights::SemiBold());
            title.TextTrimming(TextTrimming::CharacterEllipsis);
            labels.Children().Append(title);
            TextBlock detail;
            detail.FontSize(11);
            detail.Foreground(Brush(L"MutedBrush"));
            detail.TextTrimming(TextTrimming::CharacterEllipsis);
            hstring status = hidden ? L"已隐藏" : L"在首页显示";
            if (!discovered || !discovered->online) status = status + L" · 当前不在线";
            else if (discovered->state == L"streaming") status = status + L" · 正在传输";
            else if (!discovered->model.empty()) status = status + L" · " + discovered->model;
            detail.Text(status);
            labels.Children().Append(detail);
            layout.Children().Append(labels);
            Button toggle;
            Grid::SetColumn(toggle, 1);
            toggle.VerticalAlignment(VerticalAlignment::Center);
            toggle.MinWidth(90);
            toggle.Content(box_value(hidden ? L"恢复显示" : L"隐藏"));
            SetAccessibleName(toggle, (hidden ? L"恢复显示 " : L"隐藏 ") + entry.name);
            toggle.Click([weak = get_weak(), id = entry.id, name = entry.name, hidden](auto const&, auto const&)
                { if (auto self = weak.get()) self->SetDeviceHidden(id, name, !hidden); });
            layout.Children().Append(toggle);
            row.Child(layout);
            DeviceVisibilityList().Children().Append(row);
        }
        m_visibilityRendered = true;
    }

    void MainWindow::ShowSettings(bool visible)
    {
        m_settingsOpen = visible;
        if (visible && !m_visibilityRendered) RenderDeviceVisibility(m_snapshot);
        HomeView().Visibility(visible ? Visibility::Collapsed : Visibility::Visible);
        SettingsView().Visibility(visible ? Visibility::Visible : Visibility::Collapsed);
        if (visible) SettingsBackButton().Focus(FocusState::Programmatic);
        else SettingsButton().Focus(FocusState::Programmatic);
        if (m_motionAllowed)
        {
            auto target = visible ? SettingsContent().as<UIElement>() : PageContent().as<UIElement>();
            auto visual = Hosting::ElementCompositionPreview::GetElementVisual(target);
            auto animation = visual.Compositor().CreateScalarKeyFrameAnimation();
            animation.InsertKeyFrame(0.0f, 0.35f);
            animation.InsertKeyFrame(1.0f, 1.0f);
            animation.Duration(std::chrono::milliseconds(160));
            visual.StartAnimation(L"Opacity", animation);
        }
    }

    void MainWindow::OnOpenSettings(IInspectable const&, RoutedEventArgs const&)
    {
        ShowSettings(true);
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
        CurrentVersionText().Text(L"当前版本 " + hstring(update.currentVersion));
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
            DiagnosticsCopyStatus().Text(L"诊断信息已复制，可粘贴到问题反馈中。");
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
        auto const foreground = m_highContrast ? SystemColor(COLOR_WINDOWTEXT) :
            dark ? Windows::UI::Color{255, 240, 240, 247} : Windows::UI::Color{255, 30, 32, 45};
        titlebar.ButtonForegroundColor(foreground);
        titlebar.ButtonHoverForegroundColor(m_highContrast ? SystemColor(COLOR_HIGHLIGHTTEXT) : foreground);
        titlebar.ButtonPressedForegroundColor(foreground);
        titlebar.ButtonInactiveForegroundColor(m_highContrast ? SystemColor(COLOR_GRAYTEXT) :
            dark ? Windows::UI::Color{255, 153, 156, 173} : Windows::UI::Color{255, 107, 110, 128});
        titlebar.ButtonBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonInactiveBackgroundColor(Windows::UI::Color{0, 0, 0, 0});
        titlebar.ButtonHoverBackgroundColor(m_highContrast ? SystemColor(COLOR_HIGHLIGHT) :
            dark ? Windows::UI::Color{30, 255, 255, 255} : Windows::UI::Color{18, 0, 0, 0});
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
