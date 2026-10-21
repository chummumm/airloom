#pragma once

#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <bcrypt.h>
#include <shlobj.h>
#include <filesystem>
#include <stdexcept>
#include <utility>
#include <vector>
#include "UpdatePolicy.h"

namespace airloom::updates
{
    namespace fs = std::filesystem;

    struct Handle
    {
        HANDLE value{};
        Handle() = default;
        explicit Handle(HANDLE handle) : value(handle) {}
        ~Handle() { Reset(); }
        Handle(Handle const&) = delete;
        Handle& operator=(Handle const&) = delete;
        Handle(Handle&& other) noexcept : value(std::exchange(other.value, nullptr)) {}
        Handle& operator=(Handle&& other) noexcept
        {
            if (this != &other) { Reset(); value = std::exchange(other.value, nullptr); }
            return *this;
        }
        explicit operator bool() const { return value && value != INVALID_HANDLE_VALUE; }
        void Reset() { if (*this) CloseHandle(value); value = nullptr; }
    };

    inline void Require(bool condition, char const* message)
    {
        if (!condition) throw std::runtime_error(message);
    }

    inline fs::path ExecutablePath()
    {
        std::wstring buffer(32768, L'\0');
        auto length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
        Require(length != 0 && length < buffer.size(), "Cannot locate executable");
        buffer.resize(length);
        return fs::path(buffer);
    }

    inline bool OrdinaryDirectory(fs::path const& path)
    {
        auto flags = GetFileAttributesW(path.c_str());
        return flags != INVALID_FILE_ATTRIBUTES && (flags & FILE_ATTRIBUTE_DIRECTORY) && !(flags & FILE_ATTRIBUTE_REPARSE_POINT);
    }

    inline fs::path CacheRoot()
    {
        PWSTR value{};
        auto result = SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &value);
        Require(SUCCEEDED(result), "Cannot locate local application data");
        fs::path base(value);
        CoTaskMemFree(value);
        auto app = base / L"Airloom";
        auto root = app / L"Updates";
        fs::create_directory(app);
        Require(OrdinaryDirectory(app), "Update cache cannot be a link");
        fs::create_directory(root);
        Require(OrdinaryDirectory(root), "Update cache cannot be a link");
        return root;
    }

    inline bool SamePath(fs::path const& left, fs::path const& right)
    {
        auto a = left.lexically_normal().wstring();
        auto b = right.lexically_normal().wstring();
        return CompareStringOrdinal(a.c_str(), -1, b.c_str(), -1, TRUE) == CSTR_EQUAL;
    }

    inline bool OwnedJob(fs::path const& job)
    {
        return IsJobName(job.filename().wstring()) && OrdinaryDirectory(job) && SamePath(job.parent_path(), CacheRoot());
    }

    inline std::string Sha256(HANDLE file)
    {
        struct HashContext
        {
            std::vector<unsigned char> object;
            BCRYPT_ALG_HANDLE algorithm{};
            BCRYPT_HASH_HANDLE hash{};
            ~HashContext() { if (hash) BCryptDestroyHash(hash); if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0); }
        } context;
        Require(BCryptOpenAlgorithmProvider(&context.algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0, "SHA-256 unavailable");
        DWORD objectLength{}, returned{};
        Require(BCryptGetProperty(context.algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLength), sizeof(objectLength), &returned, 0) >= 0, "SHA-256 unavailable");
        context.object.resize(objectLength);
        Require(BCryptCreateHash(context.algorithm, &context.hash, context.object.data(), objectLength, nullptr, 0, 0) >= 0, "SHA-256 unavailable");
        LARGE_INTEGER start{};
        Require(SetFilePointerEx(file, start, nullptr, FILE_BEGIN) != FALSE, "Cannot read package");
        std::array<unsigned char, 65536> buffer;
        DWORD count{};
        for (;;)
        {
            Require(ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &count, nullptr) != FALSE, "Cannot read package");
            if (count == 0) break;
            Require(BCryptHashData(context.hash, buffer.data(), count, 0) >= 0, "Cannot hash package");
        }
        std::array<unsigned char, 32> hash;
        Require(BCryptFinishHash(context.hash, hash.data(), static_cast<ULONG>(hash.size()), 0) >= 0, "Cannot hash package");
        constexpr char hex[] = "0123456789abcdef";
        std::string digest;
        for (auto byte : hash) { digest += hex[byte >> 4]; digest += hex[byte & 15]; }
        return digest;
    }

    inline std::string Sha256File(fs::path const& path)
    {
        Handle file(CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
        Require(static_cast<bool>(file), "Cannot open package");
        return Sha256(file.value);
    }

    inline bool CleanupJob(fs::path const& job)
    {
        if (!OwnedJob(job)) return false;
        Handle lock(CreateFileW((job / L"active.lock").c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
        if (!lock) return false; // Another process is downloading or installing.
        bool complete = true;
        for (auto const& entry : fs::directory_iterator(job))
        {
            auto const name = entry.path().filename().wstring();
            if (name == L"active.lock") continue;
            auto const flags = GetFileAttributesW(entry.path().c_str());
            if (flags == INVALID_FILE_ATTRIBUTES || (flags & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT))) { complete = false; continue; }
            bool known = name == L"download.part" || name == L"update-helper.exe";
            if (!known)
            {
                auto ascii = entry.path().filename().string();
                known = IsInstallerName(ascii);
            }
            if (!known || !DeleteFileW(entry.path().c_str())) complete = false;
        }
        lock.Reset();
        // Leave the lock file as a retry marker if the helper image is still in use.
        if (complete && !DeleteFileW((job / L"active.lock").c_str())) complete = false;
        if (complete) complete = RemoveDirectoryW(job.c_str()) != FALSE;
        return complete;
    }

    inline void CleanupStaleJobs()
    {
        auto root = CacheRoot();
        size_t visited{};
        for (auto const& entry : fs::directory_iterator(root))
        {
            if (++visited > 128) break;
            if (!IsJobName(entry.path().filename().wstring())) continue;
            if (GetFileAttributesW((entry.path() / L"active.lock").c_str()) == INVALID_FILE_ATTRIBUTES)
            {
                // Do not race the downloader between creating its directory and
                // opening the lock. Old empty jobs are safe to recover later.
                WIN32_FILE_ATTRIBUTE_DATA data{};
                if (!GetFileAttributesExW(entry.path().c_str(), GetFileExInfoStandard, &data)) continue;
                FILETIME current{};
                GetSystemTimeAsFileTime(&current);
                ULARGE_INTEGER created{}, now{};
                created.LowPart = data.ftCreationTime.dwLowDateTime;
                created.HighPart = data.ftCreationTime.dwHighDateTime;
                now.LowPart = current.dwLowDateTime;
                now.HighPart = current.dwHighDateTime;
                if (now.QuadPart < created.QuadPart + 60ull * 10000000) continue;
            }
            CleanupJob(entry.path());
        }
    }

    inline fs::path NewJob()
    {
        GUID id{};
        Require(SUCCEEDED(CoCreateGuid(&id)), "Cannot create update job");
        wchar_t text[40]{};
        Require(StringFromGUID2(id, text, 40) != 0, "Cannot create update job");
        std::wstring name = L"run-";
        for (auto character : std::wstring_view(text))
        {
            if (character >= L'A' && character <= L'F') character += L'a' - L'A';
            if ((character >= L'0' && character <= L'9') || (character >= L'a' && character <= L'f')) name += character;
        }
        auto job = CacheRoot() / name;
        Require(fs::create_directory(job), "Cannot create update cache");
        return job;
    }

    // Only the parent process handle, cache lock and ready event are inherited.
    // The helper acknowledges validated inputs before the UI is allowed to exit.
    inline bool StartHelper(fs::path const& helper, fs::path const& installer, fs::path const& application,
        std::string const& digest, HANDLE lock, HANDLE parent)
    {
        SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
        Handle ready(CreateEventW(&security, TRUE, FALSE, nullptr));
        Require(static_cast<bool>(ready), "Cannot create update handoff");
        std::array<HANDLE, 3> inherited{parent, lock, ready.value};
        for (auto handle : inherited) Require(SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) != FALSE, "Cannot prepare update handoff");
        SIZE_T bytes{};
        InitializeProcThreadAttributeList(nullptr, 1, 0, &bytes);
        std::vector<unsigned char> storage(bytes);
        auto attributes = reinterpret_cast<PPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
        Require(InitializeProcThreadAttributeList(attributes, 1, 0, &bytes) != FALSE, "Cannot prepare update handoff");
        struct AttributeCleanup { PPROC_THREAD_ATTRIBUTE_LIST value; ~AttributeCleanup() { DeleteProcThreadAttributeList(value); } } cleanup{attributes};
        Require(UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, inherited.data(), sizeof(inherited), nullptr, nullptr) != FALSE, "Cannot prepare update handoff");
        auto number = [](HANDLE handle) { return std::to_wstring(reinterpret_cast<uintptr_t>(handle)); };
        std::wstring command = Quote(helper.wstring()) + L" --parent " + number(parent) + L" --lock " + number(lock) + L" --ready " + number(ready.value)
            + L" --installer " + Quote(installer.wstring()) + L" --application " + Quote(application.wstring())
            + L" --sha256 " + std::wstring(digest.begin(), digest.end());
        STARTUPINFOEXW startup{};
        startup.StartupInfo.cb = sizeof(startup);
        startup.lpAttributeList = attributes;
        PROCESS_INFORMATION process{};
        auto folder = helper.parent_path();
        Require(CreateProcessW(helper.c_str(), command.data(), nullptr, nullptr, TRUE, EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            nullptr, folder.c_str(), &startup.StartupInfo, &process) != FALSE, "Cannot launch update helper");
        Handle processHandle(process.hProcess), threadHandle(process.hThread);
        std::array<HANDLE, 2> wait{ready.value, processHandle.value};
        auto result = WaitForMultipleObjects(static_cast<DWORD>(wait.size()), wait.data(), FALSE, 10000);
        if (result == WAIT_OBJECT_0) return true;
        if (WaitForSingleObject(processHandle.value, 0) == WAIT_TIMEOUT)
        {
            TerminateProcess(processHandle.value, 1);
            WaitForSingleObject(processHandle.value, 5000);
        }
        return false;
    }
}
