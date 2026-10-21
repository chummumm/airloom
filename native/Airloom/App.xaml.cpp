#include "pch.h"
#include "App.xaml.h"
#include "MainWindow.xaml.h"
#include "DesktopIntegration.h"
#include "StartupTrace.h"

namespace winrt::Airloom::implementation
{
    App::App()
    {
        airloom::native::TraceStartup("App: initialize resources");
        InitializeComponent();
        airloom::native::TraceStartup("App: resources ready");
    }

    App::~App()
    {
        if (m_singleInstance) CloseHandle(m_singleInstance);
    }

    void App::OnLaunched(Microsoft::UI::Xaml::LaunchActivatedEventArgs const&)
    {
        airloom::native::TraceStartup("App: launched");
        m_singleInstance = CreateMutexW(nullptr, FALSE, L"Local\\Airloom.SingleInstance.v1");
        auto const mutexError = GetLastError();
        if (m_singleInstance && mutexError == ERROR_ALREADY_EXISTS)
        {
            airloom::native::TraceStartup("App: duplicate instance");
            EnumWindows([](HWND window, LPARAM) -> BOOL
                {
                    if (GetPropW(window, airloom::native::WindowIdentity))
                    {
                        DWORD processId{};
                        GetWindowThreadProcessId(window, &processId);
                        AllowSetForegroundWindow(processId);
                        PostMessageW(window, airloom::native::ShowWindowMessage, 0, 0);
                        return FALSE;
                    }
                    return TRUE;
                }, 0);
            Exit();
            return;
        }

        airloom::native::TraceStartup("App: create main window");
        auto main = winrt::make_self<MainWindow>();
        m_window = main.as<Microsoft::UI::Xaml::Window>();
        airloom::native::TraceStartup("App: activate main window");
        main->Start(airloom::native::Preferences::IsBackgroundLaunch());
        airloom::native::TraceStartup("App: launch complete");
    }
}
