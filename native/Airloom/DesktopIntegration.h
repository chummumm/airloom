#pragma once

#include "pch.h"

namespace airloom::native
{
    inline constexpr UINT ShowWindowMessage = WM_APP + 43;
    inline constexpr wchar_t WindowIdentity[] = L"Airloom.MainWindow.v1";

    class DesktopIntegration final
    {
    public:
        DesktopIntegration() = default;
        ~DesktopIntegration();
        DesktopIntegration(DesktopIntegration const&) = delete;
        DesktopIntegration& operator=(DesktopIntegration const&) = delete;

        bool Attach(HWND window, std::function<void()> show, std::function<void()> exit);
        void Detach() noexcept;
        void Show() const;
        void Hide();
        void SetTooltip(winrt::hstring const& text);
        bool HasTrayIcon() const noexcept { return m_added; }

    private:
        static LRESULT CALLBACK SubclassProcedure(HWND window, UINT message, WPARAM wParam,
            LPARAM lParam, UINT_PTR subclassId, DWORD_PTR instance);
        bool AddIcon();
        void ShowMenu();

        HWND m_window{};
        HICON m_icon{};
        NOTIFYICONDATAW m_data{};
        UINT m_taskbarCreated{};
        bool m_added{};
        bool m_balloonShown{};
        std::function<void()> m_show;
        std::function<void()> m_exit;
    };

    struct HiddenDevice
    {
        winrt::hstring id;
        winrt::hstring name;
    };

    class Preferences final
    {
    public:
        static uint32_t Theme(); // 0 system, 1 light, 2 dark
        static bool Visualizer();
        static void SaveTheme(uint32_t theme);
        static void SaveVisualizer(bool enabled);
        static std::vector<HiddenDevice> HiddenDevices();
        static void SaveHiddenDevices(std::vector<HiddenDevice> const& devices);
        static bool StartsWithWindows();
        static void SetStartsWithWindows(bool enabled);
        static bool IsBackgroundLaunch();
    };
}
