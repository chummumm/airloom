#pragma once
#include <windows.h>
#include <cstdio>
#include <iterator>

namespace airloom::native
{
    // Explicit troubleshooting only. Record static stages, never source names,
    // device identifiers, credentials or other user content.
    inline void TraceStartup(char const* stage) noexcept
    {
        wchar_t path[32768]{};
        auto const length = GetEnvironmentVariableW(L"AIRLOOM_STARTUP_TRACE", path,
            static_cast<DWORD>(std::size(path)));
        if (length == 0 || length >= std::size(path)) return;
        auto file = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
            nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (file == INVALID_HANDLE_VALUE) return;
        char line[256]{};
        auto const size = _snprintf_s(line, sizeof(line), _TRUNCATE, "%lu %llu %s\r\n",
            GetCurrentProcessId(), GetTickCount64(), stage);
        if (size > 0)
        {
            DWORD written{};
            WriteFile(file, line, static_cast<DWORD>(size), &written, nullptr);
        }
        CloseHandle(file);
    }
}
