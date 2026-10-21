#pragma once

namespace airloom::updates
{
    inline constexpr wchar_t WindowIdentity[] = L"Airloom.MainWindow.v1";
    inline constexpr wchar_t GracefulExitCapability[] = L"Airloom.GracefulExit.v1";
    inline constexpr wchar_t ExitRequestMessageName[] = L"Airloom.RequestExit.v1";
    inline constexpr wchar_t SingleInstanceMutex[] = L"Local\\Airloom.SingleInstance.v1";

    // Stable process exit codes consumed by the installer before file copying.
    enum class ApplicationExitResult : int
    {
        Ready = 0,
        LegacyRunning = 10,
        OtherInstance = 11,
        TimedOut = 12,
        CannotInspect = 13,
    };
}
