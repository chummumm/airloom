#include "pch.h"
#include "DesktopIntegration.h"
#include "resource.h"

namespace
{
    constexpr UINT TrayMessage = WM_APP + 42;
    constexpr UINT DeferredExitMessage = WM_APP + 44;
    constexpr UINT_PTR SubclassId = 0xA17;
    constexpr wchar_t SettingsKey[] = L"Software\\Airloom";
    constexpr wchar_t StartupKey[] = L"Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    uint32_t ReadSetting(wchar_t const* name, uint32_t fallback)
    {
        DWORD value{};
        DWORD bytes = sizeof(value);
        return RegGetValueW(HKEY_CURRENT_USER, SettingsKey, name, RRF_RT_REG_DWORD,
            nullptr, &value, &bytes) == ERROR_SUCCESS ? value : fallback;
    }

    void WriteSetting(wchar_t const* name, uint32_t value)
    {
        HKEY key{};
        auto result = RegCreateKeyExW(HKEY_CURRENT_USER, SettingsKey, 0, nullptr, 0,
            KEY_SET_VALUE, nullptr, &key, nullptr);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
        result = RegSetValueExW(key, name, 0, REG_DWORD,
            reinterpret_cast<BYTE const*>(&value), sizeof(value));
        RegCloseKey(key);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
    }
}

namespace airloom::native
{
    DesktopIntegration::~DesktopIntegration()
    {
        Detach();
    }

    bool DesktopIntegration::Attach(HWND window, std::function<void()> show, std::function<void()> exit)
    {
        m_window = window;
        m_show = std::move(show);
        m_exit = std::move(exit);
        m_taskbarCreated = RegisterWindowMessageW(L"TaskbarCreated");
        m_exitRequestMessage = RegisterWindowMessageW(airloom::updates::ExitRequestMessageName);
        SetPropW(window, WindowIdentity, reinterpret_cast<HANDLE>(1));
        m_icon = static_cast<HICON>(LoadImageW(GetModuleHandleW(nullptr), MAKEINTRESOURCEW(IDI_AIRLOOM),
            IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR));
        if (m_icon)
        {
            SendMessageW(window, WM_SETICON, ICON_SMALL, reinterpret_cast<LPARAM>(m_icon));
            SendMessageW(window, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(m_icon));
        }
        m_data.cbSize = sizeof(m_data);
        m_data.hWnd = window;
        m_data.uID = 1;
        m_data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        m_data.uCallbackMessage = TrayMessage;
        m_data.hIcon = m_icon ? m_icon : LoadIconW(nullptr, IDI_APPLICATION);
        wcscpy_s(m_data.szTip, L"Airloom · 等待连接");
        if (!SetWindowSubclass(window, SubclassProcedure, SubclassId, reinterpret_cast<DWORD_PTR>(this)))
        {
            return false;
        }
        if (m_exitRequestMessage)
            SetPropW(window, airloom::updates::GracefulExitCapability, reinterpret_cast<HANDLE>(1));
        return AddIcon();
    }

    bool DesktopIntegration::AddIcon()
    {
        m_added = Shell_NotifyIconW(NIM_ADD, &m_data) != FALSE;
        if (m_added)
        {
            m_data.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &m_data);
        }
        return m_added;
    }

    void DesktopIntegration::Detach() noexcept
    {
        if (m_added)
        {
            Shell_NotifyIconW(NIM_DELETE, &m_data);
            m_added = false;
        }
        if (m_window)
        {
            RemoveWindowSubclass(m_window, SubclassProcedure, SubclassId);
            RemovePropW(m_window, WindowIdentity);
            RemovePropW(m_window, airloom::updates::GracefulExitCapability);
            m_window = nullptr;
        }
        if (m_icon)
        {
            DestroyIcon(std::exchange(m_icon, nullptr));
        }
    }

    void DesktopIntegration::Show() const
    {
        if (!m_window) return;
        ShowWindow(m_window, IsIconic(m_window) ? SW_RESTORE : SW_SHOW);
        SetForegroundWindow(m_window);
    }

    void DesktopIntegration::Hide()
    {
        // Never hide an app whose tray icon could not be registered.
        if (!m_window || !m_added || m_exitPending) return;
        ShowWindow(m_window, SW_HIDE);
        if (!m_balloonShown)
        {
            auto notification = m_data;
            notification.uFlags = NIF_INFO;
            wcscpy_s(notification.szInfoTitle, L"Airloom 继续在后台运行");
            wcscpy_s(notification.szInfo, L"单击托盘图标打开窗口；右键选择“退出 Airloom”可停止传输并退出。");
            notification.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
            Shell_NotifyIconW(NIM_MODIFY, &notification);
            m_balloonShown = true;
        }
    }

    void DesktopIntegration::SetTooltip(winrt::hstring const& text)
    {
        if (!m_added) return;
        wcsncpy_s(m_data.szTip, text.c_str(), _TRUNCATE);
        auto update = m_data;
        update.uFlags = NIF_TIP;
        Shell_NotifyIconW(NIM_MODIFY, &update);
    }

    void DesktopIntegration::ShowMenu()
    {
        POINT cursor{};
        GetCursorPos(&cursor);
        HMENU menu = CreatePopupMenu();
        if (!menu) return;
        AppendMenuW(menu, MF_STRING, 1, L"打开 Airloom");
        AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(menu, MF_STRING, 2, L"退出 Airloom");
        SetMenuDefaultItem(menu, 1, FALSE);
        SetForegroundWindow(m_window);
        auto const selected = TrackPopupMenuEx(menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            cursor.x, cursor.y, m_window, nullptr);
        DestroyMenu(menu);
        PostMessageW(m_window, WM_NULL, 0, 0);
        if (selected == 1 && m_show) m_show();
        if (selected == 2 && m_exit) m_exit();
    }

    void DesktopIntegration::RequestExit()
    {
        if (m_exitPending) return;
        m_exitPending = true;
        // Leave the synchronous installer/session notification before destroying
        // the XAML window. An intervening WM_CLOSE must never hide this shutdown.
        if (!PostMessageW(m_window, DeferredExitMessage, 0, 0) && m_exit) m_exit();
    }

    LRESULT CALLBACK DesktopIntegration::SubclassProcedure(HWND window, UINT message, WPARAM wParam,
        LPARAM lParam, UINT_PTR, DWORD_PTR instance)
    {
        auto self = reinterpret_cast<DesktopIntegration*>(instance);
        if (self->m_taskbarCreated != 0 && message == self->m_taskbarCreated)
        {
            if (self->m_exitPending) return 0;
            self->m_added = false;
            if (!self->AddIcon()) self->Show();
            return 0;
        }
        if (self->m_exitRequestMessage != 0 && message == self->m_exitRequestMessage)
        {
            self->RequestExit();
            return 0;
        }
        if (message == DeferredExitMessage)
        {
            if (self->m_exit) self->m_exit();
            return 0;
        }
        // Restart Manager queries readiness first. A cancelled session end must
        // leave both playback and the ordinary close-to-tray preference intact.
        if (message == WM_QUERYENDSESSION) return TRUE;
        if (message == WM_ENDSESSION)
        {
            if (wParam)
            {
                if ((lParam & ENDSESSION_CLOSEAPP) != 0) self->RequestExit();
                else
                {
                    // Windows can terminate a real logoff/shutdown as soon as
                    // this returns. Restore owned audio/mute state synchronously.
                    self->m_exitPending = true;
                    if (self->m_exit) self->m_exit();
                }
            }
            return 0;
        }
        if (message == ShowWindowMessage)
        {
            if (self->m_show) self->m_show();
            return 0;
        }
        if (message == TrayMessage)
        {
            auto const event = LOWORD(lParam);
            if ((event == NIN_SELECT || event == NIN_KEYSELECT) && self->m_show)
            {
                self->m_show();
            }
            else if (event == WM_CONTEXTMENU)
            {
                self->ShowMenu();
            }
            return 0;
        }
        if (message == WM_CLOSE && self->m_exitPending) return 0;
        if (message == WM_CLOSE && self->m_added)
        {
            self->Hide();
            return 0;
        }
        if (message == WM_GETMINMAXINFO)
        {
            auto info = reinterpret_cast<MINMAXINFO*>(lParam);
            auto const dpi = GetDpiForWindow(window);
            info->ptMinTrackSize.x = MulDiv(880, static_cast<int>(dpi), 96);
            info->ptMinTrackSize.y = MulDiv(680, static_cast<int>(dpi), 96);
            return 0;
        }
        return DefSubclassProc(window, message, wParam, lParam);
    }

    uint32_t Preferences::Theme()
    {
        return std::min(ReadSetting(L"Theme", 0), 2u);
    }

    bool Preferences::Visualizer()
    {
        return ReadSetting(L"Visualizer", 1) != 0;
    }

    void Preferences::SaveTheme(uint32_t theme)
    {
        WriteSetting(L"Theme", std::min(theme, 2u));
    }

    void Preferences::SaveVisualizer(bool enabled)
    {
        WriteSetting(L"Visualizer", enabled ? 1u : 0u);
    }

    std::vector<HiddenDevice> Preferences::HiddenDevices()
    {
        // Store names with identities so an offline hidden device can still be restored.
        DWORD bytes{};
        if (RegGetValueW(HKEY_CURRENT_USER, SettingsKey, L"HiddenDevices", RRF_RT_REG_SZ,
            nullptr, nullptr, &bytes) != ERROR_SUCCESS || bytes < sizeof(wchar_t) ||
            bytes > 262144 || bytes % sizeof(wchar_t) != 0)
            return {};
        std::wstring stored(bytes / sizeof(wchar_t), L'\0');
        if (RegGetValueW(HKEY_CURRENT_USER, SettingsKey, L"HiddenDevices", RRF_RT_REG_SZ,
            nullptr, stored.data(), &bytes) != ERROR_SUCCESS)
            return {};
        auto const terminator = stored.find(L'\0');
        if (terminator != std::wstring::npos) stored.resize(terminator);
        winrt::Windows::Data::Json::JsonArray array;
        if (!winrt::Windows::Data::Json::JsonArray::TryParse(stored, array)) return {};
        std::vector<HiddenDevice> result;
        for (auto const& item : array)
        {
            using winrt::Windows::Data::Json::JsonValueType;
            if (item.ValueType() != JsonValueType::Object || result.size() >= 256) continue;
            auto const object = item.GetObject();
            if (!object.HasKey(L"id") || !object.HasKey(L"name")) continue;
            auto const id = object.GetNamedValue(L"id");
            auto const name = object.GetNamedValue(L"name");
            if (id.ValueType() != JsonValueType::String || name.ValueType() != JsonValueType::String) continue;
            auto const identifier = id.GetString();
            if (identifier.empty() || identifier.size() > 1024 || name.GetString().size() > 1024) continue;
            if (std::any_of(result.begin(), result.end(), [&identifier](HiddenDevice const& device)
                { return device.id == identifier; })) continue;
            result.push_back({identifier, name.GetString()});
        }
        return result;
    }

    void Preferences::SaveHiddenDevices(std::vector<HiddenDevice> const& devices)
    {
        using namespace winrt::Windows::Data::Json;
        if (devices.size() > 256) throw winrt::hresult_invalid_argument(L"隐藏设备列表已达到上限。");
        JsonArray array;
        for (auto const& device : devices)
        {
            if (device.id.empty() || device.id.size() > 1024 || device.name.size() > 1024)
                throw winrt::hresult_invalid_argument(L"设备标识或名称格式无效。");
            JsonObject item;
            item.Insert(L"id", JsonValue::CreateStringValue(device.id));
            item.Insert(L"name", JsonValue::CreateStringValue(device.name));
            array.Append(item);
        }
        auto const value = array.Stringify();
        if ((value.size() + 1) * sizeof(wchar_t) > 262144)
            throw winrt::hresult_invalid_argument(L"隐藏设备列表过大。");
        HKEY key{};
        auto result = RegCreateKeyExW(HKEY_CURRENT_USER, SettingsKey, 0, nullptr, 0,
            KEY_SET_VALUE, nullptr, &key, nullptr);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
        result = RegSetValueExW(key, L"HiddenDevices", 0, REG_SZ,
            reinterpret_cast<BYTE const*>(value.c_str()),
            static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t)));
        RegCloseKey(key);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
    }

    bool Preferences::StartsWithWindows()
    {
        DWORD bytes{};
        return RegGetValueW(HKEY_CURRENT_USER, StartupKey, L"Airloom", RRF_RT_REG_SZ,
            nullptr, nullptr, &bytes) == ERROR_SUCCESS && bytes > sizeof(wchar_t);
    }

    void Preferences::SetStartsWithWindows(bool enabled)
    {
        HKEY key{};
        auto result = RegCreateKeyExW(HKEY_CURRENT_USER, StartupKey, 0, nullptr, 0,
            KEY_SET_VALUE, nullptr, &key, nullptr);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
        if (enabled)
        {
            std::wstring executable(32768, L'\0');
            auto const length = GetModuleFileNameW(nullptr, executable.data(), static_cast<DWORD>(executable.size()));
            if (length == 0 || length >= executable.size())
            {
                RegCloseKey(key);
                throw winrt::hresult_error(E_FAIL, L"无法获取应用安装位置。");
            }
            executable.resize(length);
            auto const command = L"\"" + executable + L"\" --background";
            result = RegSetValueExW(key, L"Airloom", 0, REG_SZ,
                reinterpret_cast<BYTE const*>(command.c_str()),
                static_cast<DWORD>((command.size() + 1) * sizeof(wchar_t)));
        }
        else
        {
            result = RegDeleteValueW(key, L"Airloom");
            if (result == ERROR_FILE_NOT_FOUND) result = ERROR_SUCCESS;
        }
        RegCloseKey(key);
        if (result != ERROR_SUCCESS) winrt::throw_hresult(HRESULT_FROM_WIN32(result));
    }

    bool Preferences::IsBackgroundLaunch()
    {
        int count{};
        auto arguments = CommandLineToArgvW(GetCommandLineW(), &count);
        if (!arguments) return false;
        bool background{};
        for (int index = 1; index < count; ++index)
        {
            if (wcscmp(arguments[index], L"--background") == 0) background = true;
        }
        LocalFree(arguments);
        return background;
    }
}
