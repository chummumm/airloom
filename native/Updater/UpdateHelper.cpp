#include "UpdateCommon.h"
#include "ApplicationExit.h"
#include <shellapi.h>
#include <cwchar>

namespace
{
    using namespace airloom::updates;

    HANDLE ParseHandle(wchar_t const* value)
    {
        wchar_t* end{};
        auto number = wcstoull(value, &end, 10);
        Require(number != 0 && end && *end == L'\0', "Invalid inherited handle");
        auto handle = reinterpret_cast<HANDLE>(static_cast<uintptr_t>(number));
        DWORD flags{};
        Require(GetHandleInformation(handle, &flags) != FALSE, "Missing inherited handle");
        return handle;
    }

    void SaveError(std::wstring const& message)
    {
        HKEY key{};
        if (RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Airloom", 0, nullptr, 0, KEY_SET_VALUE, nullptr, &key, nullptr) != ERROR_SUCCESS) return;
        RegSetValueExW(key, L"LastUpdateError", 0, REG_SZ, reinterpret_cast<BYTE const*>(message.c_str()), static_cast<DWORD>((message.size() + 1) * sizeof(wchar_t)));
        RegCloseKey(key);
    }

    bool RestartApplication(fs::path const& application)
    {
        auto command = Quote(application.wstring());
        auto folder = application.parent_path();
        STARTUPINFOW startup{sizeof(startup)};
        PROCESS_INFORMATION process{};
        if (!CreateProcessW(application.c_str(), command.data(), nullptr, nullptr, FALSE, 0, nullptr, folder.c_str(), &startup, &process)) return false;
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        return true;
    }

    int Run(int count, wchar_t** arguments)
    {
        if (count == 3 && std::wstring_view(arguments[1]) == L"--request-exit")
            return static_cast<int>(RequestApplicationExit(fs::path(arguments[2])));
        if (count != 13 || std::wstring_view(arguments[1]) != L"--parent" || std::wstring_view(arguments[3]) != L"--lock"
            || std::wstring_view(arguments[5]) != L"--ready" || std::wstring_view(arguments[7]) != L"--installer"
            || std::wstring_view(arguments[9]) != L"--application" || std::wstring_view(arguments[11]) != L"--sha256") return 2;

        Handle parent(ParseHandle(arguments[2])), lock(ParseHandle(arguments[4])), ready(ParseHandle(arguments[6]));
        fs::path installer(arguments[8]), application(arguments[10]);
        auto job = ExecutablePath().parent_path();
        Require(OwnedJob(job) && SamePath(installer.parent_path(), job) && IsInstallerName(installer.filename().string()), "Invalid update location");
        Require(application.is_absolute() && application.filename() == L"Airloom.exe" && OrdinaryDirectory(application.parent_path()), "Invalid application location");
        Require(GetProcessId(parent.value) != 0 && GetFileType(lock.value) == FILE_TYPE_DISK, "Invalid update handoff");
        Require(WaitForSingleObject(parent.value, 0) == WAIT_TIMEOUT, "Parent is no longer running");
        std::wstring expectedWide(arguments[12]);
        Require(expectedWide.size() == 64, "Invalid package digest");
        std::string expected(expectedWide.begin(), expectedWide.end());
        Handle package(CreateFileW(installer.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
        Require(static_cast<bool>(package) && Sha256(package.value) == expected, "Package verification failed");
        Require(SetEvent(ready.value) != FALSE, "Cannot acknowledge update handoff");

        // The old GUI must release its files and WASAPI session before Setup runs.
        auto const parentExit = WaitForSingleObject(parent.value, 30000);
        DWORD installerResult = ERROR_TIMEOUT;
        if (parentExit == WAIT_OBJECT_0)
        {
            auto installDirectory = application.parent_path();
            auto command = Quote(installer.wstring())
                + L" /SILENT /NORESTART /RESTARTEXITCODE=3010 /SUPPRESSMSGBOXES /SP- /CLOSEAPPLICATIONS /NORESTARTAPPLICATIONS "
                + Quote(L"/DIR=" + installDirectory.wstring());
            STARTUPINFOW startup{sizeof(startup)};
            PROCESS_INFORMATION process{};
            if (CreateProcessW(installer.c_str(), command.data(), nullptr, nullptr, FALSE, 0, nullptr, job.c_str(), &startup, &process))
            {
                Handle running(process.hProcess), thread(process.hThread);
                // Keep a read-only, non-write-sharing handle until the process
                // opens the verified image, closing the validation/launch gap.
                package.Reset();
                if (WaitForSingleObject(running.value, INFINITE) == WAIT_OBJECT_0)
                    GetExitCodeProcess(running.value, &installerResult);
            }
            else installerResult = GetLastError();
        }
        package.Reset();

        // Delete only the package belonging to this update. The new app removes
        // this helper after it exits; a later launch retries after interruption.
        bool packageRemoved = false;
        for (int attempt = 0; attempt < 30; ++attempt)
        {
            if (DeleteFileW(installer.c_str()) || GetLastError() == ERROR_FILE_NOT_FOUND)
            {
                packageRemoved = true;
                break;
            }
            Sleep(100);
        }
        DeleteFileW((job / L"download.part").c_str());
        lock.Reset();

        std::wstring cleanupMessage = packageRemoved ? L"下载包已清理。" : L"下载包仍被占用，Airloom 启动后会重试清理。";
        if (parentExit != WAIT_OBJECT_0)
        {
            auto message = L"Airloom 仍在运行，更新已取消，尚未替换程序文件。\n\n"
                L"请在任务栏通知区域右键 Airloom，选择“退出 Airloom”，然后重新运行安装包或再次检查更新。"
                L"关闭主窗口只会收起到托盘。\n\n" + cleanupMessage;
            SaveError(message);
            // The parent may be hidden or blocked inside shutdown. Report the
            // bounded failure here instead of leaving an invisible registry-only
            // error which could only be seen after a later successful restart.
            MessageBoxW(nullptr, message.c_str(), L"Airloom 更新未完成", MB_OK | MB_ICONWARNING | MB_SETFOREGROUND);
        }
        else if (installerResult == 3010)
            SaveError(L"更新已安装，部分文件需要重启 Windows 后生效。" + cleanupMessage);
        else if (installerResult != 0)
            SaveError(L"更新未完成或已取消（代码 " + std::to_wstring(installerResult) + L"）。" + cleanupMessage + L"可以重新检查更新。");
        if (parentExit == WAIT_OBJECT_0 && !RestartApplication(application))
            SaveError(L"更新流程已结束。" + cleanupMessage + L"请从开始菜单重新打开 Airloom。");
        return installerResult == 0 || installerResult == 3010 ? 0 : 1;
    }
}

int WINAPI wWinMain(HINSTANCE, HINSTANCE, PWSTR, int)
{
    int count{};
    auto arguments = CommandLineToArgvW(GetCommandLineW(), &count);
    if (!arguments) return 2;
    int result = 2;
    try { result = Run(count, arguments); }
    catch (...) { /* Before acknowledgement, the parent reports the failure. */ }
    LocalFree(arguments);
    return result;
}
