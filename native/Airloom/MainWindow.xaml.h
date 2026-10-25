#pragma once

#include "MainWindow.g.h"
#include "DesktopIntegration.h"
#include "EngineBridge.h"
#include "Models.h"
#include "Updater.h"

namespace winrt::Airloom::implementation
{
    struct MainWindow : MainWindowT<MainWindow>
    {
        MainWindow();
        ~MainWindow();
        void Start(bool background);

        void OnLoaded(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnLayoutRootSizeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);
        void OnActualThemeChanged(Microsoft::UI::Xaml::FrameworkElement const&, Windows::Foundation::IInspectable const&);
        void OnRefresh(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnRetry(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnConnect(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCancelAuth(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnRefreshSources(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnSourceModeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::SelectionChangedEventArgs const&);
        void OnSourceChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::SelectionChangedEventArgs const&);
        void OnVolumeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const&);
        void OnMuteLocalChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnThemeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::SelectionChangedEventArgs const&);
        void OnVisualizerChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnStartupChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnForgetCredentials(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnOpenSettings(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCloseSettings(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnManageDevices(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnTestTone(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCheckUpdates(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnInstallUpdate(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCancelUpdate(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCopyDiagnostics(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnExitClick(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnWaveformSizeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);
        void OnRouteSizeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);

    private:
        void InitializeWindow();
        void InitializePreferences();
        void UpdateWorkspaceLayout(double width);
        void PollEngine();
        void RenderSnapshot(airloom::native::Snapshot const& snapshot);
        void RenderDevices(airloom::native::Snapshot const& snapshot);
        void RenderSources(airloom::native::Snapshot const& snapshot);
        void RenderAudio(airloom::native::Snapshot const& snapshot);
        void RenderRoute(airloom::native::Snapshot const& snapshot);
        void RenderError(airloom::native::Snapshot const& snapshot);
        void RenderDeviceVisibility(airloom::native::Snapshot const& snapshot);
        void RenderUpdate();
        void FlushVolume();
        void FinishVolumeInteraction();
        void ShowSettings(bool visible, bool focusDevices = false);
        bool IsDeviceHidden(hstring const& id) const;
        bool IsDeviceHidden(airloom::native::Device const& device) const;
        void SetDeviceHidden(hstring const& id, hstring const& name, bool hidden);
        void ApplyTheme();
        void AnimateStateChange();
        void AnimateIn(Microsoft::UI::Xaml::UIElement const& element, float fromX, float fromY, int milliseconds, float fromOpacity = 0.0f);
        void AnimateZoomIn(Microsoft::UI::Xaml::UIElement const& element, float fromScale, int milliseconds);
        void PopElement(Microsoft::UI::Xaml::UIElement const& element, float size);
        void BounceElement(Microsoft::UI::Xaml::UIElement const& element, float center);
        void SwapConnectIcon();
        void StartLoop(Microsoft::UI::Xaml::UIElement const& element, float centerX, float centerY, float toScale, float fromOpacity,
            int milliseconds, int delayMilliseconds = 0);
        void StartSpin(Microsoft::UI::Xaml::UIElement const& element, float centerX, float centerY, int milliseconds);
        void StartEqualizer(Microsoft::UI::Xaml::UIElement const& element, int delayMilliseconds);
        void StopMotion(Microsoft::UI::Xaml::UIElement const& element);
        void FadeVisual(Microsoft::UI::Xaml::UIElement const& element, float opacity, int milliseconds);
        void UpdatePulse(bool live, bool busy);
        void UpdateRadar(bool searching);
        void SpinGlyph(Microsoft::UI::Xaml::Controls::FontIcon const& glyph, bool spin, bool& running);
        void ApplyMotionPreference();
        void FlashVisibilityCard();
        void UpdateVolumeGlyph(double value);
        void ShowWindow();
        void ExitNow();
        void ShowLocalError(hstring const& message);
        bool Send(Windows::Data::Json::JsonObject const& command);
        bool Send(wchar_t const* type);
        fire_and_forget ShowAuthentication(bool explicitlyRequested = false);
        hstring AuthenticationFingerprint() const;
        Microsoft::UI::Xaml::Media::Brush Brush(wchar_t const* resource);

        struct DeviceRow
        {
            hstring id;
            Microsoft::UI::Xaml::UIElement root{nullptr};
            Microsoft::UI::Xaml::Controls::Grid rowGrid{nullptr};
            Microsoft::UI::Xaml::Controls::Button button{nullptr};
            Microsoft::UI::Xaml::Controls::Button more{nullptr};
            Microsoft::UI::Xaml::Controls::Border check{nullptr};
            Microsoft::UI::Xaml::Controls::FontIcon checkGlyph{nullptr};
            Microsoft::UI::Xaml::Controls::Grid avatar{nullptr};
            Microsoft::UI::Xaml::Shapes::Ellipse avatarFace{nullptr};
            Microsoft::UI::Xaml::Controls::StackPanel avatarGlyphs{nullptr};
            Microsoft::UI::Xaml::Controls::Border badge{nullptr};
            Microsoft::UI::Xaml::Controls::FontIcon badgeGlyph{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock name{nullptr};
            Microsoft::UI::Xaml::Controls::Grid equalizer{nullptr};
            std::vector<Microsoft::UI::Xaml::Shapes::Rectangle> equalizerBars;
            Microsoft::UI::Xaml::Controls::ProgressRing spinner{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock detail{nullptr};
            Microsoft::UI::Xaml::Controls::HyperlinkButton action{nullptr};
            Microsoft::UI::Xaml::Controls::MenuFlyoutItem retry{nullptr};
            Microsoft::UI::Xaml::Controls::MenuFlyoutItem hide{nullptr};
            Microsoft::UI::Xaml::Controls::Border groupSurface{nullptr};
            Microsoft::UI::Xaml::Controls::Button memberToggle{nullptr};
            Microsoft::UI::Xaml::Controls::FontIcon memberChevron{nullptr};
            Microsoft::UI::Xaml::Controls::Border memberHost{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock memberSummary{nullptr};
            Microsoft::UI::Xaml::Controls::StackPanel memberPanel{nullptr};
            hstring actionKind;
            bool membersExpanded{};
            bool equalizerRunning{};
            bool rendered{};
            bool selected{};
        };

        // One speaker on the route: its curve, flowing particles and node.
        struct RouteNode
        {
            hstring id;
            Microsoft::UI::Xaml::Shapes::Path link{nullptr};
            Microsoft::UI::Xaml::Controls::Canvas particleHost{nullptr};
            std::vector<Microsoft::UI::Xaml::Shapes::Ellipse> particles;
            Microsoft::UI::Xaml::Controls::Grid root{nullptr};
            Microsoft::UI::Xaml::Controls::Grid disc{nullptr};
            Microsoft::UI::Xaml::Shapes::Ellipse halo{nullptr};
            Microsoft::UI::Xaml::Shapes::Ellipse face{nullptr};
            Microsoft::UI::Xaml::Shapes::Ellipse ring{nullptr};
            Microsoft::UI::Xaml::Controls::StackPanel glyphs{nullptr};
            Microsoft::UI::Xaml::Controls::Border badge{nullptr};
            Microsoft::UI::Xaml::Controls::FontIcon badgeGlyph{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock name{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock state{nullptr};
            Microsoft::UI::Xaml::Media::Animation::Storyboard linkStoryboard{nullptr};
            hstring look;
            hstring glyphKind;
            double startX{-1.0};
            double startY{-1.0};
            double endX{-1.0};
            double endY{-1.0};
            double length{};
            bool haloRunning{};
            bool ringRunning{};
            bool particlesRunning{};
            bool flowing{};
            bool marching{};
            std::chrono::steady_clock::time_point particleStopAt{};
        };

        struct SourceRow
        {
            hstring id;
            hstring name;
            Microsoft::UI::Xaml::Controls::ComboBoxItem item{nullptr};
        };

        struct VisibilityRow
        {
            hstring id;
            hstring name;
            Microsoft::UI::Xaml::Controls::Border root{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock title{nullptr};
            Microsoft::UI::Xaml::Controls::TextBlock detail{nullptr};
            Microsoft::UI::Xaml::Controls::Button toggle{nullptr};
        };

        airloom::native::EngineBridge m_engine;
        airloom::native::DesktopIntegration m_desktop;
        airloom::native::UpdateService m_updater;
        airloom::native::Snapshot m_snapshot;
        std::vector<airloom::native::HiddenDevice> m_hiddenDevices;
        std::vector<DeviceRow> m_deviceRows;
        std::vector<SourceRow> m_sourceRows;
        std::vector<VisibilityRow> m_visibilityRows;
        std::vector<RouteNode> m_routeNodes;
        std::wstring m_routeSignature;
        std::vector<Microsoft::UI::Xaml::Shapes::Line> m_waveformBars;
        // Display-only release ballistics for real engine peaks; values are never invented.
        std::vector<double> m_displayWave = std::vector<double>(64, 0.0);
        double m_displayLeft{};
        double m_displayRight{};
        Microsoft::UI::Xaml::Media::Animation::TransitionCollection m_deviceTransitions{nullptr};
        Microsoft::UI::Xaml::Media::Animation::TransitionCollection m_visibilityTransitions{nullptr};
        Microsoft::UI::Xaml::Media::Animation::Storyboard m_highlightStoryboard{nullptr};
        airloom::native::UpdateSnapshot m_lastUpdate;
        Microsoft::UI::Xaml::DispatcherTimer m_pollTimer{nullptr};
        Microsoft::UI::Xaml::DispatcherTimer m_volumeTimer{nullptr};
        Microsoft::UI::Xaml::Controls::ContentDialog m_authDialog{nullptr};
        HWND m_hwnd{};
        bool m_rendering{};
        bool m_preferencesReady{};
        bool m_loaded{};
        bool m_exiting{};
        bool m_background{};
        bool m_visualizerEnabled{true};
        bool m_motionAllowed{true};
        bool m_highContrast{};
        bool m_devicesRendered{};
        bool m_sourcesRendered{};
        bool m_sourcesPending{};
        bool m_forceDeviceRender{};
        bool m_settingsOpen{};
        bool m_homeMotionVisible{};
        bool m_routeAnimationsAllowed{true};
        bool m_visibilityRendered{};
        bool m_volumePending{};
        bool m_volumeAwaitingAck{};
        bool m_volumePointerDown{};
        bool m_updateRendered{};
        bool m_motionApplied{};
        int m_pulseMode{};
        bool m_wideLayout{};
        bool m_radarRunning{};
        bool m_sourceHaloRunning{};
        bool m_sourceSpinRunning{};
        bool m_discoverySpinRunning{};
        double m_pendingVolume{0.75};
        std::chrono::steady_clock::time_point m_volumeAckDeadline;
        uint64_t m_tickCount{};
        uint64_t m_dialogRequestId{};
        hstring m_authFingerprint;
        hstring m_localError;
        std::chrono::steady_clock::time_point m_localErrorUntil;
    };
}

namespace winrt::Airloom::factory_implementation
{
    struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow>
    {
    };
}
