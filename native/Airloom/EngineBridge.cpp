#include "pch.h"
#include "EngineBridge.h"

namespace airloom::native
{
    EngineBridge::EngineBridge()
    {
        std::wstring executable(32768, L'\0');
        auto const length = GetModuleFileNameW(nullptr, executable.data(), static_cast<DWORD>(executable.size()));
        if (length == 0 || length >= executable.size())
        {
            m_loadError = L"无法定位 Airloom 的安装目录。请从安装位置重新打开应用。";
            return;
        }
        executable.resize(length);
        auto const slash = executable.find_last_of(L"\\/");
        auto const libraryPath = executable.substr(0, slash + 1) + L"airloom_core.dll";
        m_library = LoadLibraryExW(libraryPath.c_str(), nullptr,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (!m_library)
        {
            m_loadError = L"音频引擎未能加载。请确认 airloom_core.dll 与 Airloom.exe 位于同一安装目录，或重新安装 Airloom。";
            return;
        }

        auto const create = reinterpret_cast<CreateFn>(GetProcAddress(m_library, "airloom_create"));
        m_destroy = reinterpret_cast<DestroyFn>(GetProcAddress(m_library, "airloom_destroy"));
        m_command = reinterpret_cast<CommandFn>(GetProcAddress(m_library, "airloom_command"));
        m_snapshot = reinterpret_cast<SnapshotFn>(GetProcAddress(m_library, "airloom_snapshot"));
        m_freeString = reinterpret_cast<FreeStringFn>(GetProcAddress(m_library, "airloom_free_string"));
        if (!create || !m_destroy || !m_command || !m_snapshot || !m_freeString)
        {
            m_loadError = L"音频引擎版本不匹配。请重新安装同一版本的 Airloom。";
            Shutdown();
            return;
        }
        m_instance = create();
        if (!m_instance)
        {
            m_loadError = L"音频引擎初始化失败。请重新打开 Airloom。";
            Shutdown();
        }
    }

    EngineBridge::~EngineBridge()
    {
        Shutdown();
    }

    void EngineBridge::Shutdown() noexcept
    {
        if (m_instance && m_destroy)
        {
            m_destroy(std::exchange(m_instance, nullptr));
        }
        if (m_library)
        {
            FreeLibrary(std::exchange(m_library, nullptr));
        }
    }

    winrt::Windows::Data::Json::JsonObject EngineBridge::Snapshot() const
    {
        if (!m_instance)
        {
            throw winrt::hresult_error(E_UNEXPECTED, L"音频引擎尚未就绪。");
        }
        char* raw = m_snapshot(m_instance);
        if (!raw)
        {
            throw winrt::hresult_error(E_FAIL, L"音频引擎未返回状态。");
        }
        std::unique_ptr<char, FreeStringFn> owned(raw, m_freeString);
        // Reject a damaged boundary response instead of scanning indefinitely.
        constexpr size_t maximumSnapshotBytes = 4 * 1024 * 1024;
        auto const length = strnlen_s(raw, maximumSnapshotBytes);
        if (length == maximumSnapshotBytes)
        {
            throw winrt::hresult_invalid_argument(L"音频引擎返回了过大的状态数据。");
        }
        auto const text = winrt::to_hstring(std::string_view(raw, length));
        return winrt::Windows::Data::Json::JsonObject::Parse(text);
    }

    bool EngineBridge::Command(winrt::Windows::Data::Json::JsonObject const& command) const
    {
        if (!m_instance)
        {
            return false;
        }
        std::string utf8 = winrt::to_string(command.Stringify());
        auto const result = m_command(m_instance, utf8.c_str());
        // Authentication values are never logged. Clear the transient UTF-8 copy.
        if (!utf8.empty())
        {
            SecureZeroMemory(utf8.data(), utf8.size());
        }
        return result == 0;
    }
}
