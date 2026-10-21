#pragma once

#include "UpdateCommon.h"
#include "ExitProtocol.h"

namespace airloom::updates
{
    struct ApplicationWindow
    {
        HWND window{};
        Handle process;
        bool legacy{};
        bool otherInstance{};
        bool cannotInspect{};
    };

    inline ApplicationWindow FindApplicationWindow(fs::path const& application)
    {
        struct Search
        {
            std::wstring expectedPath;
            ApplicationWindow found;
        } search{application.lexically_normal().wstring(), {}};
        auto enumerated = EnumWindows([](HWND window, LPARAM parameter) -> BOOL {
            if (!GetPropW(window, WindowIdentity)) return TRUE;
            auto& search = *reinterpret_cast<Search*>(parameter);
            DWORD processId{};
            GetWindowThreadProcessId(window, &processId);
            Handle process(OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, processId));
            if (!process)
            {
                search.found.cannotInspect = true;
                return TRUE;
            }
            std::array<wchar_t, 32768> path{};
            DWORD length = static_cast<DWORD>(path.size());
            if (!QueryFullProcessImageNameW(process.value, 0, path.data(), &length))
            {
                search.found.cannotInspect = true;
                return TRUE;
            }
            if (CompareStringOrdinal(path.data(), static_cast<int>(length), search.expectedPath.c_str(),
                static_cast<int>(search.expectedPath.size()), TRUE) != CSTR_EQUAL)
            {
                search.found.otherInstance = true;
                return TRUE;
            }
            search.found.window = window;
            search.found.process = std::move(process);
            search.found.legacy = GetPropW(window, GracefulExitCapability) != reinterpret_cast<HANDLE>(1);
            return FALSE;
        }, reinterpret_cast<LPARAM>(&search));
        if (!enumerated && !search.found.window) search.found.cannotInspect = true;
        return std::move(search.found);
    }

    inline ApplicationExitResult RequestApplicationExit(fs::path const& application)
    {
        Require(application.is_absolute() && application.filename() == L"Airloom.exe", "Invalid application path");
        auto target = FindApplicationWindow(application);
        if (!target.window)
        {
            if (target.cannotInspect) return ApplicationExitResult::CannotInspect;
            if (target.otherInstance) return ApplicationExitResult::OtherInstance;
            // A starting/closing instance may not yet have a tagged window. Do
            // not overwrite its files merely because window enumeration missed it.
            Handle instance(OpenMutexW(SYNCHRONIZE, FALSE, SingleInstanceMutex));
            if (instance) return ApplicationExitResult::CannotInspect;
            return GetLastError() == ERROR_ACCESS_DENIED
                ? ApplicationExitResult::CannotInspect : ApplicationExitResult::Ready;
        }
        if (WaitForSingleObject(target.process.value, 0) == WAIT_OBJECT_0)
            return ApplicationExitResult::Ready;
        // Older releases interpret WM_CLOSE as hide-to-tray. They cannot gain
        // the new exit protocol until after this installation has completed.
        if (target.legacy) return ApplicationExitResult::LegacyRunning;
        auto request = RegisterWindowMessageW(ExitRequestMessageName);
        if (!request || !PostMessageW(target.window, request, 0, 0))
            return ApplicationExitResult::CannotInspect;
        auto result = WaitForSingleObject(target.process.value, 15000);
        if (result == WAIT_OBJECT_0) return ApplicationExitResult::Ready;
        return result == WAIT_TIMEOUT ? ApplicationExitResult::TimedOut : ApplicationExitResult::CannotInspect;
    }
}
