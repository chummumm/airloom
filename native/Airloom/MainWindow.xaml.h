#pragma once

#include "MainWindow.g.h"
#include "DesktopIntegration.h"
#include "EngineBridge.h"
#include "Models.h"

namespace winrt::Airloom::implementation
{
    struct MainWindow : MainWindowT<MainWindow>
    {
        MainWindow();
        ~MainWindow();
        void Start(bool background);

        void OnLoaded(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnActualThemeChanged(Microsoft::UI::Xaml::FrameworkElement const&, Windows::Foundation::IInspectable const&);
        void OnRefresh(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnRetry(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnConnect(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnCancelAuth(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnSourceChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::SelectionChangedEventArgs const&);
        void OnVolumeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const&);
        void OnMuteLocalChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnThemeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::SelectionChangedEventArgs const&);
        void OnVisualizerChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnStartupChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnForgetCredentials(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnExitClick(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void OnWaveformSizeChanged(Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);

    private:
        void InitializeWindow();
        void InitializePreferences();
        void PollEngine();
        void RenderSnapshot(airloom::native::Snapshot const& snapshot);
        void RenderDevices(airloom::native::Snapshot const& snapshot);
        void RenderSources(airloom::native::Snapshot const& snapshot);
        void RenderAudio(airloom::native::Snapshot const& snapshot);
        void RenderError(airloom::native::Snapshot const& snapshot);
        void ApplyTheme();
        void AnimateStateChange();
        void ShowWindow();
        void ExitNow();
        void ShowLocalError(hstring const& message);
        bool Send(Windows::Data::Json::JsonObject const& command);
        bool Send(wchar_t const* type);
        fire_and_forget ShowAuthentication(bool explicitlyRequested = false);
        hstring AuthenticationFingerprint() const;
        Microsoft::UI::Xaml::Media::Brush Brush(wchar_t const* resource) const;

        airloom::native::EngineBridge m_engine;
        airloom::native::DesktopIntegration m_desktop;
        airloom::native::Snapshot m_snapshot;
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
        bool m_devicesRendered{};
        bool m_sourcesRendered{};
        bool m_forceDeviceRender{};
        bool m_volumePending{};
        double m_pendingVolume{0.75};
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
