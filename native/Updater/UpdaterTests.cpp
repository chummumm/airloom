#include "UpdateCommon.h"
#include "ApplicationExit.h"
#include <shellapi.h>
#include <cwchar>
#include <fstream>
#include <iostream>

using namespace airloom::updates;

namespace
{
    void Write(fs::path const& path, std::string const& text)
    {
        std::ofstream stream(path, std::ios::binary);
        stream << text;
        Require(stream.good(), "Cannot write test fixture");
    }

    HWND WindowForProcess(DWORD processId, bool tagged = true)
    {
        struct Search { DWORD processId; bool tagged; HWND window{}; } search{processId, tagged};
        EnumWindows([](HWND window, LPARAM parameter) -> BOOL {
            auto& search = *reinterpret_cast<Search*>(parameter);
            DWORD owner{};
            GetWindowThreadProcessId(window, &owner);
            if (owner == search.processId && (!search.tagged || GetPropW(window, WindowIdentity)))
            {
                search.window = window;
                return FALSE;
            }
            return TRUE;
        }, reinterpret_cast<LPARAM>(&search));
        return search.window;
    }

    HWND WaitForWindow(HANDLE process, bool tagged = true, DWORD timeout = 15000)
    {
        auto deadline = GetTickCount64() + timeout;
        while (GetTickCount64() < deadline)
        {
            if (auto window = WindowForProcess(GetProcessId(process), tagged)) return window;
            Require(WaitForSingleObject(process, 0) == WAIT_TIMEOUT, "Process exited before creating its window");
            Sleep(50);
        }
        throw std::runtime_error("Expected process window did not appear");
    }

    struct Running
    {
        Handle process;
        Handle thread;
        Handle stop;
        ~Running()
        {
            if (!process || WaitForSingleObject(process.value, 0) != WAIT_TIMEOUT) return;
            // Fixture parents cooperate through an event; real GUI children use
            // the same scoped exit request as Setup. Test cleanup never masks a
            // failed exit assertion by force-terminating an Airloom process.
            if (stop) SetEvent(stop.value);
            else if (auto window = WindowForProcess(GetProcessId(process.value)))
                PostMessageW(window, RegisterWindowMessageW(ExitRequestMessageName), 0, 0);
            WaitForSingleObject(process.value, 5000);
        }
    };

    void Launch(Running& running, fs::path const& executable, std::wstring arguments = {}, bool inherit = false)
    {
        auto command = Quote(executable.wstring()) + (arguments.empty() ? L"" : L" " + arguments);
        STARTUPINFOW startup{sizeof(startup)};
        PROCESS_INFORMATION process{};
        auto directory = executable.parent_path();
        Require(CreateProcessW(executable.c_str(), command.data(), nullptr, nullptr, inherit, 0,
            nullptr, directory.c_str(), &startup, &process) != FALSE, "Cannot start test child");
        running.process = Handle(process.hProcess);
        running.thread = Handle(process.hThread);
    }

    void SpawnFixture(Running& running, fs::path const& executable, wchar_t const* mode = L"--parent-fixture")
    {
        SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
        running.stop = Handle(CreateEventW(&security, TRUE, FALSE, nullptr));
        Require(static_cast<bool>(running.stop), "Cannot create fixture stop event");
        Launch(running, executable, std::wstring(mode) + L" "
            + std::to_wstring(reinterpret_cast<uintptr_t>(running.stop.value)), true);
    }

    void StopFixture(Running& running)
    {
        Require(SetEvent(running.stop.value) != FALSE, "Cannot stop fixture");
        Require(WaitForSingleObject(running.process.value, 5000) == WAIT_OBJECT_0, "Fixture did not stop cooperatively");
    }

    DWORD RunExitHelper(fs::path const& helper, fs::path const& application)
    {
        Running helperProcess;
        Launch(helperProcess, helper, L"--request-exit " + Quote(application.wstring()));
        Require(WaitForSingleObject(helperProcess.process.value, 20000) == WAIT_OBJECT_0,
            "Graceful exit helper exceeded its bounded wait");
        DWORD result{};
        Require(GetExitCodeProcess(helperProcess.process.value, &result) != FALSE, "Cannot read exit-helper result");
        return result;
    }

    bool TrayIconPresent(HWND window)
    {
        NOTIFYICONIDENTIFIER identifier{sizeof(identifier)};
        identifier.hWnd = window;
        identifier.uID = 1;
        RECT rectangle{};
        return SUCCEEDED(Shell_NotifyIconGetRect(&identifier, &rectangle));
    }

    void ClearTestUpdateError()
    {
        HKEY key{};
        if (RegOpenKeyExW(HKEY_CURRENT_USER, L"Software\\Airloom", 0, KEY_SET_VALUE, &key) == ERROR_SUCCESS)
        {
            RegDeleteValueW(key, L"LastUpdateError");
            RegCloseKey(key);
        }
    }

    struct SavedUpdateError
    {
        std::optional<std::wstring> original;
        SavedUpdateError()
        {
            std::array<wchar_t, 2048> value{};
            DWORD bytes = static_cast<DWORD>(value.size() * sizeof(wchar_t));
            if (RegGetValueW(HKEY_CURRENT_USER, L"Software\\Airloom", L"LastUpdateError",
                RRF_RT_REG_SZ, nullptr, value.data(), &bytes) == ERROR_SUCCESS) original = value.data();
            ClearTestUpdateError();
        }
        ~SavedUpdateError()
        {
            ClearTestUpdateError();
            if (!original) return;
            HKEY key{};
            if (RegCreateKeyExW(HKEY_CURRENT_USER, L"Software\\Airloom", 0, nullptr, 0, KEY_SET_VALUE,
                nullptr, &key, nullptr) != ERROR_SUCCESS) return;
            RegSetValueExW(key, L"LastUpdateError", 0, REG_SZ, reinterpret_cast<BYTE const*>(original->c_str()),
                static_cast<DWORD>((original->size() + 1) * sizeof(wchar_t)));
            RegCloseKey(key);
        }
    };

    struct InstallFixture
    {
        fs::path job{NewJob()};
        fs::path appDirectory{fs::temp_directory_path() / (L"Airloom-Updater-Test-" + job.filename().wstring())};
        fs::path application{appDirectory / L"Airloom.exe"};
        fs::path installer{job / L"Airloom-9.9.9-setup-x64.exe"};
        fs::path helper{job / L"update-helper.exe"};
        Handle lock;
        InstallFixture(fs::path const& helperSource, int exitCode)
        {
            fs::create_directory(appDirectory);
            fs::copy_file(ExecutablePath(), application);
            fs::copy_file(ExecutablePath(), installer);
            fs::copy_file(helperSource, helper);
            Write(appDirectory / L"exit-code.txt", std::to_string(exitCode));
            lock = Handle(CreateFileW((job / L"active.lock").c_str(), GENERIC_READ | GENERIC_WRITE,
                0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
            Require(static_cast<bool>(lock), "Cannot create lock fixture");
        }
        ~InstallFixture()
        {
            lock.Reset();
            try { CleanupJob(job); fs::remove_all(appDirectory); } catch (...) { }
        }
        void CheckCompleted()
        {
            for (int attempt = 0; attempt < 150 && !fs::exists(appDirectory / L"restarted.txt"); ++attempt) Sleep(100);
            Require(fs::exists(appDirectory / L"installed.txt"), "Fixture installer did not run");
            Require(fs::exists(appDirectory / L"restarted.txt"), "Fixture application was not restarted");
            Require(!fs::exists(installer), "Installer package was not removed");
            for (int attempt = 0; attempt < 30 && fs::exists(job); ++attempt)
            {
                CleanupJob(job);
                Sleep(100);
            }
            Require(!fs::exists(job), "Helper cache was not removed");
            ClearTestUpdateError();
        }
    };

    void InstallLifecycle(fs::path const& helperSource, int exitCode, fs::path const& realGui = {})
    {
        InstallFixture fixture(helperSource, exitCode);
        Running parent;
        HWND window{};
        bool hadTray{};
        if (realGui.empty()) SpawnFixture(parent, ExecutablePath());
        else
        {
            Launch(parent, realGui);
            window = WaitForWindow(parent.process.value);
            Require(GetPropW(window, GracefulExitCapability) == reinterpret_cast<HANDLE>(1), "GUI has no graceful-exit capability");
            Sleep(300);
            hadTray = TrayIconPresent(window);
            if (hadTray)
            {
                PostMessageW(window, WM_CLOSE, 0, 0);
                Sleep(200);
                Require(!IsWindowVisible(window) && WaitForSingleObject(parent.process.value, 0) == WAIT_TIMEOUT,
                    "Ordinary window close no longer hides to the tray");
            }
        }
        auto digest = Sha256File(fixture.installer);
        Require(StartHelper(fixture.helper, fixture.installer, fixture.application, digest,
            fixture.lock.value, parent.process.value), "Helper handoff failed");
        Require(!CleanupJob(fixture.job) && fs::exists(fixture.installer), "Active update was deleted");
        Sleep(200);
        Require(!fs::exists(fixture.appDirectory / L"installed.txt"), "Installer ran before parent exit");
        fixture.lock.Reset();
        if (realGui.empty()) StopFixture(parent);
        else
        {
            Require(RunExitHelper(helperSource, realGui) == static_cast<DWORD>(ApplicationExitResult::Ready),
                "Installer request did not gracefully exit the actual WinUI process");
            Require(WaitForSingleObject(parent.process.value, 0) == WAIT_OBJECT_0, "GUI window closed but process survived");
            Require(!WindowForProcess(GetProcessId(parent.process.value)), "A tagged GUI window survived shutdown");
            if (hadTray) Require(!TrayIconPresent(window), "Tray icon survived orderly shutdown");
            std::cout << "Real WinUI update handoff: process exited, tray registered=" << hadTray << '\n';
        }
        fixture.CheckCompleted();
    }

    void RestartManagerLifecycle(fs::path const& application)
    {
        Running gui;
        Launch(gui, application);
        auto window = WaitForWindow(gui.process.value);
        DWORD_PTR response{};
        Require(SendMessageTimeoutW(window, WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP,
            SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, &response) != 0 && response == TRUE,
            "Restart Manager shutdown query was not acknowledged");
        Require(WaitForSingleObject(gui.process.value, 200) == WAIT_TIMEOUT,
            "GUI exited before Restart Manager confirmed shutdown");
        Require(SendMessageTimeoutW(window, WM_ENDSESSION, FALSE, ENDSESSION_CLOSEAPP,
            SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, &response) != 0, "Cancelled session message failed");
        Require(WaitForSingleObject(gui.process.value, 200) == WAIT_TIMEOUT,
            "Cancelled Restart Manager shutdown exited the application");
        auto hadTray = TrayIconPresent(window);
        Require(PostMessageW(window, WM_ENDSESSION, TRUE, ENDSESSION_CLOSEAPP) != FALSE,
            "Cannot confirm Restart Manager shutdown");
        Require(WaitForSingleObject(gui.process.value, 15000) == WAIT_OBJECT_0,
            "Confirmed Restart Manager shutdown left the real GUI process running");
        if (hadTray) Require(!TrayIconPresent(window), "Restart Manager shutdown left the tray icon behind");
        std::cout << "Real WinUI Restart Manager query/cancel/confirmed-exit passed\n";
    }

    void PreflightFailure(fs::path const& helperSource, bool unresponsive)
    {
        InstallFixture fixture(helperSource, 0);
        Running parent;
        SpawnFixture(parent, fixture.application, unresponsive ? L"--unresponsive-window-fixture" : L"--legacy-window-fixture");
        auto window = WaitForWindow(parent.process.value);
        auto result = RunExitHelper(helperSource, fixture.application);
        Require(result == static_cast<DWORD>(unresponsive ? ApplicationExitResult::TimedOut : ApplicationExitResult::LegacyRunning),
            "Installer preflight did not identify the blocked application");
        Require(WaitForSingleObject(parent.process.value, 0) == WAIT_TIMEOUT && IsWindowVisible(window),
            "Installer preflight killed or hid a blocked application");
        Require(!fs::exists(fixture.appDirectory / L"installed.txt"), "Blocked preflight changed installed files");
        StopFixture(parent);
        std::cout << (unresponsive ? "Bounded exit timeout" : "Legacy tray-exit migration") << " passed\n";
    }

    void ParentExitTimeout(fs::path const& helperSource)
    {
        InstallFixture fixture(helperSource, 0);
        Running parent;
        SpawnFixture(parent, ExecutablePath());
        Handle helperProcess;
        Require(StartHelper(fixture.helper, fixture.installer, fixture.application, Sha256File(fixture.installer),
            fixture.lock.value, parent.process.value, &helperProcess), "Timeout fixture handoff failed");
        fixture.lock.Reset();
        auto dialog = WaitForWindow(helperProcess.value, false, 35000);
        std::array<wchar_t, 128> title{};
        GetWindowTextW(dialog, title.data(), static_cast<int>(title.size()));
        Require(std::wstring_view(title.data()) == L"Airloom 更新未完成", "Timeout did not show visible update failure");
        Require(!fs::exists(fixture.appDirectory / L"installed.txt"), "Timeout ran an installer against a live parent");
        Require(!fs::exists(fixture.installer), "Timeout left the downloaded installer behind");
        Require(WaitForSingleObject(parent.process.value, 0) == WAIT_TIMEOUT, "Timeout force-terminated the parent");
        PostMessageW(dialog, WM_COMMAND, IDOK, 0);
        Require(WaitForSingleObject(helperProcess.value, 5000) == WAIT_OBJECT_0, "Failure dialog did not close its helper");
        StopFixture(parent);
        ClearTestUpdateError();
        std::cout << "Helper timeout: visible feedback, no installation, parent preserved\n";
    }

    void RejectTamperedPackage(fs::path const& helperSource)
    {
        InstallFixture fixture(helperSource, 0);
        Handle parent(OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, TRUE, GetCurrentProcessId()));
        Require(!StartHelper(fixture.helper, fixture.installer, fixture.application, std::string(64, '0'),
            fixture.lock.value, parent.value), "Tampered package was accepted");
        fixture.lock.Reset();
        Require(CleanupJob(fixture.job), "Failed download was not cleaned");
    }

    int RunParentFixture(std::wstring_view mode, wchar_t const* eventValue)
    {
        wchar_t* end{};
        auto number = wcstoull(eventValue, &end, 10);
        Require(number && end && *end == L'\0', "Invalid fixture event");
        Handle stop(reinterpret_cast<HANDLE>(static_cast<uintptr_t>(number)));
        if (mode == L"--parent-fixture")
            return WaitForSingleObject(stop.value, 90000) == WAIT_OBJECT_0 ? 0 : 4;
        auto instance = GetModuleHandleW(nullptr);
        WNDCLASSW windowClass{};
        windowClass.hInstance = instance;
        windowClass.lpszClassName = L"Airloom.Updater.Fixture";
        windowClass.lpfnWndProc = [](HWND window, UINT message, WPARAM wParam, LPARAM lParam) -> LRESULT {
            if (message == WM_CLOSE) { ShowWindow(window, SW_HIDE); return 0; }
            if (message == RegisterWindowMessageW(ExitRequestMessageName)) return 0;
            return DefWindowProcW(window, message, wParam, lParam);
        };
        Require(RegisterClassW(&windowClass) != 0, "Cannot register fixture window");
        auto window = CreateWindowW(windowClass.lpszClassName, L"Airloom updater fixture", WS_OVERLAPPEDWINDOW,
            0, 0, 320, 180, nullptr, nullptr, instance, nullptr);
        Require(window != nullptr, "Cannot create fixture window");
        SetPropW(window, WindowIdentity, reinterpret_cast<HANDLE>(1));
        if (mode == L"--unresponsive-window-fixture") SetPropW(window, GracefulExitCapability, reinterpret_cast<HANDLE>(1));
        ShowWindow(window, SW_SHOW);
        while (MsgWaitForMultipleObjects(1, &stop.value, FALSE, 90000, QS_ALLINPUT) == WAIT_OBJECT_0 + 1)
        {
            MSG message{};
            while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE))
            {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        RemovePropW(window, WindowIdentity);
        RemovePropW(window, GracefulExitCapability);
        DestroyWindow(window);
        return 0;
    }
}

int wmain(int count, wchar_t** arguments)
{
    try
    {
        if (count == 3 && (std::wstring_view(arguments[1]) == L"--parent-fixture"
            || std::wstring_view(arguments[1]) == L"--legacy-window-fixture"
            || std::wstring_view(arguments[1]) == L"--unresponsive-window-fixture"))
            return RunParentFixture(arguments[1], arguments[2]);
        auto executable = ExecutablePath();
        if (executable.filename() == L"Airloom.exe")
        {
            Write(executable.parent_path() / L"restarted.txt", "yes");
            return 0;
        }
        if (IsInstallerName(executable.filename().string()))
        {
            for (int index = 1; index < count; ++index)
            {
                std::wstring_view argument(arguments[index]);
                if (!argument.starts_with(L"/DIR=")) continue;
                fs::path directory(argument.substr(5));
                Write(directory / L"installed.txt", "yes");
                int result{};
                std::ifstream(directory / L"exit-code.txt") >> result;
                return result;
            }
            return 3;
        }
        Require(count == 3, "Expected helper and real WinUI application paths");
        fs::path helper(arguments[1]), application(arguments[2]);
        Require(fs::exists(application) && application.filename() == L"Airloom.exe", "Build the real GUI before lifecycle tests");
        SavedUpdateError savedError;
        auto job = NewJob();
        Write(job / L"download.part", "abc");
        Require(Sha256File(job / L"download.part") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "SHA-256 known vector failed");
        Write(job / L"keep.txt", "unrelated user file");
        Require(!CleanupJob(job) && fs::exists(job / L"keep.txt"), "Cleanup removed an unrelated file");
        fs::remove(job / L"keep.txt");
        Require(CleanupJob(job), "Interrupted download cleanup failed");
        Require(!CleanupJob(fs::temp_directory_path()), "Cleanup accepted an external directory");
        RejectTamperedPackage(helper);
        InstallLifecycle(helper, 0);
        InstallLifecycle(helper, 2);
        PreflightFailure(helper, false);
        PreflightFailure(helper, true);
        ParentExitTimeout(helper);
        InstallLifecycle(helper, 0, application);
        RestartManagerLifecycle(application);
        std::cout << "Native updater passed: checksum, scoped cleanup, cooperative parent, legacy/timeout preflight, real WinUI/tray/RM exit, local install fixture and relaunch\n";
        return 0;
    }
    catch (std::exception const& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
