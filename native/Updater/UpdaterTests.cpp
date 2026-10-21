#include "UpdateCommon.h"
#include <fstream>
#include <iostream>
#include <thread>

using namespace airloom::updates;

namespace
{
    void Write(fs::path const& path, std::string const& text)
    {
        std::ofstream stream(path, std::ios::binary);
        stream << text;
        Require(stream.good(), "Cannot write test fixture");
    }

    struct Running
    {
        Handle process;
        Handle thread;
        ~Running()
        {
            if (process && WaitForSingleObject(process.value, 0) == WAIT_TIMEOUT)
            {
                TerminateProcess(process.value, 0);
                WaitForSingleObject(process.value, 5000);
            }
        }
    };

    void SpawnFixture(Running& running)
    {
        auto executable = ExecutablePath();
        auto command = Quote(executable.wstring()) + L" --parent-fixture";
        STARTUPINFOW startup{sizeof(startup)};
        PROCESS_INFORMATION process{};
        Require(CreateProcessW(executable.c_str(), command.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr, nullptr, &startup, &process) != FALSE, "Cannot start parent fixture");
        running.process = Handle(process.hProcess);
        running.thread = Handle(process.hThread);
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

    void InstallLifecycle(fs::path const& helperSource, int exitCode)
    {
        auto job = NewJob();
        auto appDirectory = fs::temp_directory_path() / (L"Airloom-Updater-Test-" + job.filename().wstring());
        fs::create_directory(appDirectory);
        auto application = appDirectory / L"Airloom.exe";
        auto installer = job / L"Airloom-9.9.9-setup-x64.exe";
        auto helper = job / L"update-helper.exe";
        fs::copy_file(ExecutablePath(), application);
        fs::copy_file(ExecutablePath(), installer);
        fs::copy_file(helperSource, helper);
        Write(appDirectory / L"exit-code.txt", std::to_string(exitCode));
        Handle lock(CreateFileW((job / L"active.lock").c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
        Require(static_cast<bool>(lock), "Cannot create lock fixture");
        Running parent;
        SpawnFixture(parent);
        auto digest = Sha256File(installer);
        Require(StartHelper(helper, installer, application, digest, lock.value, parent.process.value), "Helper handoff failed");
        Require(!CleanupJob(job) && fs::exists(installer), "Active update was deleted");
        Sleep(200);
        Require(!fs::exists(appDirectory / L"installed.txt"), "Installer ran before parent exit");
        lock.Reset();
        TerminateProcess(parent.process.value, 0);
        WaitForSingleObject(parent.process.value, 5000);
        for (int attempt = 0; attempt < 150 && !fs::exists(appDirectory / L"restarted.txt"); ++attempt) Sleep(100);
        Require(fs::exists(appDirectory / L"installed.txt"), "Installer did not run");
        Require(fs::exists(appDirectory / L"restarted.txt"), "Application was not restarted");
        Require(!fs::exists(installer), "Installer package was not removed");
        for (int attempt = 0; attempt < 30 && fs::exists(job); ++attempt)
        {
            CleanupJob(job);
            Sleep(100);
        }
        Require(!fs::exists(job), "Helper cache was not removed");
        Sleep(200);
        fs::remove_all(appDirectory);
        ClearTestUpdateError();
    }

    void RejectTamperedPackage(fs::path const& helperSource)
    {
        auto job = NewJob();
        auto installer = job / L"Airloom-9.9.9-setup-x64.exe";
        auto helper = job / L"update-helper.exe";
        fs::copy_file(ExecutablePath(), installer);
        fs::copy_file(helperSource, helper);
        Handle lock(CreateFileW((job / L"active.lock").c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
        Handle parent(OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, TRUE, GetCurrentProcessId()));
        auto application = ExecutablePath().parent_path() / L"Airloom.exe";
        Require(!StartHelper(helper, installer, application, std::string(64, '0'), lock.value, parent.value), "Tampered package was accepted");
        lock.Reset();
        Require(CleanupJob(job), "Failed download was not cleaned");
    }
}

int wmain(int count, wchar_t** arguments)
{
    try
    {
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
        if (count == 2 && std::wstring_view(arguments[1]) == L"--parent-fixture") { Sleep(60000); return 0; }
        Require(count == 2, "Expected helper path");
        fs::path helper(arguments[1]);
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
        std::cout << "Native updater passed: checksum rejection, scoped cleanup, active lock, parent exit, install/cancel, relaunch, package cleanup\n";
        return 0;
    }
    catch (std::exception const& error)
    {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
