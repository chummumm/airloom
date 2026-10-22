#include "pch.h"
#include "Updater.h"
#include "../Updater/UpdateCommon.h"
#include <winhttp.h>
#include <atomic>
#include <cwctype>
#include <mutex>
#include <thread>

namespace airloom::native
{
    struct ReleasePackage
    {
        std::string version;
        std::string filename;
        std::wstring downloadUrl;
        std::wstring checksumUrl;
        std::string apiDigest;
        uint64_t size{};
    };

    struct UpdateShared
    {
        mutable std::mutex mutex;
        UpdateSnapshot snapshot;
        ReleasePackage release;
        std::atomic<bool> stopped{};
        std::atomic<bool> cancelled{};
        bool busy{};
    };
}

namespace
{
    using namespace airloom::updates;
    using airloom::native::UpdateShared;
    using airloom::native::ReleasePackage;
    using winrt::Windows::Data::Json::JsonObject;
    using winrt::Windows::Data::Json::JsonValueType;

    constexpr uint64_t MaximumPackageBytes = 256ull * 1024 * 1024;
    constexpr wchar_t LatestReleaseUrl[] = L"https://api.github.com/repos/chummumm/airloom/releases/latest";

    struct Cancelled {};

    struct InternetHandle
    {
        HINTERNET value{};
        explicit InternetHandle(HINTERNET handle) : value(handle)
        {
            if (!value) throw winrt::hresult_error(HRESULT_FROM_WIN32(GetLastError()), L"无法连接更新服务器。");
        }
        ~InternetHandle() { WinHttpCloseHandle(value); }
        InternetHandle(InternetHandle const&) = delete;
        InternetHandle& operator=(InternetHandle const&) = delete;
    };

    void CheckCancelled(std::shared_ptr<UpdateShared> const& state)
    {
        if (state->stopped.load() || state->cancelled.load()) throw Cancelled{};
    }

    void NetworkCheck(BOOL success)
    {
        if (!success) throw winrt::hresult_error(HRESULT_FROM_WIN32(GetLastError()), L"更新连接失败。请检查网络或系统代理后重试。");
    }

    void SetState(std::shared_ptr<UpdateShared> const& state, std::wstring phase, std::wstring message)
    {
        std::lock_guard lock(state->mutex);
        state->snapshot.state = std::move(phase);
        state->snapshot.message = std::move(message);
    }

    bool TrustedHost(std::wstring host)
    {
        std::transform(host.begin(), host.end(), host.begin(), [](wchar_t value) { return static_cast<wchar_t>(towlower(value)); });
        return host == L"github.com" || host == L"api.github.com" || host.ends_with(L".githubusercontent.com");
    }

    template<typename Consumer>
    uint64_t Download(std::shared_ptr<UpdateShared> const& state, std::wstring const& url, uint64_t maximum, Consumer consume)
    {
        CheckCancelled(state);
        URL_COMPONENTS parts{sizeof(parts)};
        parts.dwSchemeLength = parts.dwHostNameLength = parts.dwUrlPathLength = parts.dwExtraInfoLength = static_cast<DWORD>(-1);
        NetworkCheck(WinHttpCrackUrl(url.c_str(), static_cast<DWORD>(url.size()), 0, &parts));
        Require(parts.nScheme == INTERNET_SCHEME_HTTPS && parts.nPort == INTERNET_DEFAULT_HTTPS_PORT, "Update URLs must use HTTPS");
        std::wstring host(parts.lpszHostName, parts.dwHostNameLength);
        Require(TrustedHost(host), "Unexpected update host");
        std::wstring path(parts.lpszUrlPath, parts.dwUrlPathLength);
        if (parts.dwExtraInfoLength) path.append(parts.lpszExtraInfo, parts.dwExtraInfoLength);
        InternetHandle session(WinHttpOpen(L"Airloom/0.4.3", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0));
        NetworkCheck(WinHttpSetTimeouts(session.value, 10000, 10000, 10000, 10000));
        InternetHandle connection(WinHttpConnect(session.value, host.c_str(), parts.nPort, 0));
        InternetHandle request(WinHttpOpenRequest(connection.value, L"GET", path.c_str(), nullptr,
            WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, WINHTTP_FLAG_SECURE));
        DWORD redirectPolicy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        NetworkCheck(WinHttpSetOption(request.value, WINHTTP_OPTION_REDIRECT_POLICY, &redirectPolicy, sizeof(redirectPolicy)));
        wchar_t const headers[] = L"Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n";
        NetworkCheck(WinHttpSendRequest(request.value, headers, static_cast<DWORD>(-1), WINHTTP_NO_REQUEST_DATA, 0, 0, 0));
        NetworkCheck(WinHttpReceiveResponse(request.value, nullptr));
        DWORD status{}, length = sizeof(status);
        NetworkCheck(WinHttpQueryHeaders(request.value, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            WINHTTP_HEADER_NAME_BY_INDEX, &status, &length, WINHTTP_NO_HEADER_INDEX));
        if (status == 403 || status == 429) throw winrt::hresult_error(E_FAIL, L"GitHub 暂时限制了请求频率，请稍后再检查更新。");
        if (status != 200) throw winrt::hresult_error(E_FAIL,
            winrt::hstring(L"更新服务器返回 HTTP " + std::to_wstring(status) + L"，请稍后重试。"));

        // GitHub redirects release files to its asset CDN. Keep HTTPS and host
        // validation after that redirect, without weakening certificate checks.
        DWORD finalBytes{};
        WinHttpQueryOption(request.value, WINHTTP_OPTION_URL, nullptr, &finalBytes);
        Require(finalBytes > sizeof(wchar_t) && finalBytes < 65536, "Invalid redirected URL");
        std::vector<wchar_t> finalUrl(finalBytes / sizeof(wchar_t) + 1);
        NetworkCheck(WinHttpQueryOption(request.value, WINHTTP_OPTION_URL, finalUrl.data(), &finalBytes));
        URL_COMPONENTS finalParts{sizeof(finalParts)};
        finalParts.dwHostNameLength = static_cast<DWORD>(-1);
        NetworkCheck(WinHttpCrackUrl(finalUrl.data(), 0, 0, &finalParts));
        Require(finalParts.nScheme == INTERNET_SCHEME_HTTPS && TrustedHost(std::wstring(finalParts.lpszHostName, finalParts.dwHostNameLength)), "Unexpected redirected update host");

        uint64_t total{};
        std::array<unsigned char, 65536> buffer;
        for (;;)
        {
            CheckCancelled(state);
            DWORD count{};
            NetworkCheck(WinHttpReadData(request.value, buffer.data(), static_cast<DWORD>(buffer.size()), &count));
            if (count == 0) break;
            Require(total + count <= maximum, "Update response exceeds its size limit");
            total += count;
            consume(buffer.data(), count, total);
        }
        CheckCancelled(state);
        return total;
    }

    std::string DownloadText(std::shared_ptr<UpdateShared> const& state, std::wstring const& url, uint64_t maximum)
    {
        std::string result;
        Download(state, url, maximum, [&](unsigned char const* bytes, DWORD count, uint64_t) {
            result.append(reinterpret_cast<char const*>(bytes), count);
        });
        return result;
    }

    ReleasePackage ParseRelease(std::string const& content)
    {
        auto json = JsonObject::Parse(winrt::to_hstring(content));
        Require(!json.GetNamedBoolean(L"draft") && !json.GetNamedBoolean(L"prerelease"), "Latest release must be published and stable");
        auto tag = winrt::to_string(json.GetNamedString(L"tag_name"));
        auto version = Version::Parse(tag);
        Require(version && tag == "v" + version->String(), "Unsupported release version");
        ReleasePackage package;
        package.version = version->String();
        package.filename = InstallerName(*version);
        auto base = std::string("https://github.com/chummumm/airloom/releases/download/") + tag + "/";
        auto const expectedInstaller = base + package.filename;
        auto const expectedChecksums = base + "SHA256SUMS.txt";
        bool foundInstaller{}, foundChecksums{};
        for (auto const& item : json.GetNamedArray(L"assets"))
        {
            if (item.ValueType() != JsonValueType::Object) continue;
            auto asset = item.GetObject();
            auto name = winrt::to_string(asset.GetNamedString(L"name", L""));
            auto url = winrt::to_string(asset.GetNamedString(L"browser_download_url", L""));
            if (name == package.filename)
            {
                Require(!foundInstaller && url == expectedInstaller && asset.GetNamedString(L"state", L"") == L"uploaded", "Invalid installer asset");
                auto size = asset.GetNamedNumber(L"size");
                Require(std::isfinite(size) && size > 0 && size <= static_cast<double>(MaximumPackageBytes) && std::floor(size) == size, "Invalid installer size");
                package.size = static_cast<uint64_t>(size);
                package.downloadUrl = winrt::to_hstring(url).c_str();
                auto digest = asset.GetNamedValue(L"digest", nullptr);
                if (digest && digest.ValueType() == JsonValueType::String)
                {
                    auto value = winrt::to_string(digest.GetString());
                    Require(value.starts_with("sha256:") && value.size() == 71, "Invalid release digest");
                    package.apiDigest = value.substr(7);
                }
                foundInstaller = true;
            }
            else if (name == "SHA256SUMS.txt")
            {
                Require(!foundChecksums && url == expectedChecksums && asset.GetNamedString(L"state", L"") == L"uploaded", "Invalid checksum asset");
                package.checksumUrl = winrt::to_hstring(url).c_str();
                foundChecksums = true;
            }
        }
        Require(foundInstaller && foundChecksums, "Release assets are incomplete");
        return package;
    }

    template<typename Operation>
    void StartWorker(std::shared_ptr<UpdateShared> const& state, Operation operation)
    {
        try
        {
            std::thread([state, operation = std::move(operation)] {
                bool apartmentReady{};
                try
                {
                    winrt::init_apartment(winrt::apartment_type::multi_threaded);
                    apartmentReady = true;
                    operation();
                }
                catch (Cancelled const&)
                {
                    std::lock_guard lock(state->mutex);
                    state->snapshot.state = state->release.version.empty() ? L"idle" : L"available";
                    state->snapshot.message = L"已取消，未完成的下载已清理。";
                    state->snapshot.progress = 0;
                }
                catch (winrt::hresult_error const& error) { SetState(state, L"error", error.message().c_str()); }
                catch (std::exception const& error) { SetState(state, L"error", L"更新未完成：" + std::wstring(winrt::to_hstring(error.what())) + L"。可以重新检查更新。"); }
                catch (...) { SetState(state, L"error", L"更新未完成，请稍后重试。"); }
                {
                    std::lock_guard lock(state->mutex);
                    if (!state->snapshot.exitRequested) state->busy = false;
                }
                if (apartmentReady) winrt::uninit_apartment();
            }).detach();
        }
        catch (...)
        {
            std::lock_guard lock(state->mutex);
            state->busy = false;
            state->snapshot.state = L"error";
            state->snapshot.message = L"无法启动更新任务，请稍后重试。";
        }
    }

    struct PendingJob
    {
        fs::path path;
        Handle lock;
        bool handedOff{};
        ~PendingJob()
        {
            lock.Reset();
            if (!handedOff && !path.empty())
            {
                try { CleanupJob(path); } catch (...) { /* Startup retries cleanup after interruption. */ }
            }
        }
    };
}

namespace airloom::native
{
    UpdateService::UpdateService() : m_state(std::make_shared<UpdateShared>())
    {
        wchar_t message[2048]{};
        DWORD bytes = sizeof(message);
        if (RegGetValueW(HKEY_CURRENT_USER, L"Software\\Airloom", L"LastUpdateError", RRF_RT_REG_SZ, nullptr, message, &bytes) == ERROR_SUCCESS)
        {
            m_state->snapshot.state = L"error";
            m_state->snapshot.message = message;
            HKEY key{};
            if (RegOpenKeyExW(HKEY_CURRENT_USER, L"Software\\Airloom", 0, KEY_SET_VALUE, &key) == ERROR_SUCCESS)
            {
                RegDeleteValueW(key, L"LastUpdateError");
                RegCloseKey(key);
            }
        }
        auto state = m_state;
        try
        {
            std::thread([state] {
                // A previous helper may still be exiting when the new UI starts.
                for (int attempt = 0; attempt < 12 && !state->stopped.load(); ++attempt)
                {
                    try { CleanupStaleJobs(); } catch (...) { }
                    std::this_thread::sleep_for(std::chrono::seconds(1));
                }
            }).detach();
        }
        catch (...) { }
    }

    UpdateService::~UpdateService()
    {
        PrepareForExit();
    }

    void UpdateService::PrepareForExit() noexcept
    {
        // Stop this process's network/cache work. An accepted helper handoff is
        // independent and must remain alive until this process has fully exited.
        m_state->stopped.store(true);
        m_state->cancelled.store(true);
    }

    UpdateSnapshot UpdateService::Snapshot() const
    {
        std::lock_guard lock(m_state->mutex);
        return m_state->snapshot;
    }

    void UpdateService::Check()
    {
        auto state = m_state;
        {
            std::lock_guard lock(state->mutex);
            if (state->busy || state->stopped.load()) return;
            state->busy = true;
            state->cancelled.store(false);
            state->snapshot.state = L"checking";
            state->snapshot.message = L"正在检查最新版本…";
            state->snapshot.progress = 0;
        }
        StartWorker(state, [state] {
            auto release = ParseRelease(DownloadText(state, LatestReleaseUrl, 2 * 1024 * 1024));
            auto latest = Version::Parse(release.version);
            std::lock_guard lock(state->mutex);
            auto current = Version::Parse(winrt::to_string(state->snapshot.currentVersion));
            Require(latest && current, "Invalid version");
            if (*latest <= *current)
            {
                state->release = {};
                state->snapshot.availableVersion.clear();
                state->snapshot.state = L"current";
                state->snapshot.message = L"当前已是最新版本。";
            }
            else
            {
                state->release = release;
                state->snapshot.availableVersion = winrt::to_hstring(release.version).c_str();
                state->snapshot.state = L"available";
                state->snapshot.message = L"新版本已准备好。安装时会短暂退出 Airloom，完成后自动重新打开并清理下载包。";
            }
        });
    }

    void UpdateService::Install()
    {
        auto state = m_state;
        ReleasePackage release;
        {
            std::lock_guard lock(state->mutex);
            if (state->busy || state->stopped.load() || state->snapshot.state != L"available") return;
            state->busy = true;
            state->cancelled.store(false);
            state->snapshot.state = L"downloading";
            state->snapshot.message = L"正在下载更新…";
            state->snapshot.progress = 0;
            release = state->release;
        }
        StartWorker(state, [state, release] {
            auto checksum = ChecksumFor(DownloadText(state, release.checksumUrl, 65536), release.filename);
            Require(checksum && (release.apiDigest.empty() || release.apiDigest == *checksum), "Release checksum is missing or inconsistent");
            PendingJob job;
            job.path = NewJob();
            SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
            job.lock = Handle(CreateFileW((job.path / L"active.lock").c_str(), GENERIC_READ | GENERIC_WRITE, 0, &security, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
            Require(static_cast<bool>(job.lock), "Cannot lock update cache");
            auto partial = job.path / L"download.part";
            Handle file(CreateFileW(partial.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
            Require(static_cast<bool>(file), "Cannot create download file");
            auto count = Download(state, release.downloadUrl, release.size, [&](unsigned char const* bytes, DWORD size, uint64_t total) {
                DWORD written{};
                Require(WriteFile(file.value, bytes, size, &written, nullptr) && written == size, "Cannot write update package");
                std::lock_guard lock(state->mutex);
                state->snapshot.progress = static_cast<double>(total) / static_cast<double>(release.size);
            });
            Require(count == release.size && FlushFileBuffers(file.value), "Update download is incomplete");
            file.Reset();
            CheckCancelled(state);
            SetState(state, L"downloading", L"下载完成，正在校验 SHA-256…");
            Require(Sha256File(partial) == *checksum, "Update package checksum does not match");
            auto installer = job.path / winrt::to_hstring(release.filename).c_str();
            Require(MoveFileW(partial.c_str(), installer.c_str()) != FALSE, "Cannot prepare installer");
            auto application = ExecutablePath();
            auto helper = job.path / L"update-helper.exe";
            Require(CopyFileW((application.parent_path() / L"Airloom.Update.exe").c_str(), helper.c_str(), TRUE) != FALSE, "Update helper is missing");
            CheckCancelled(state);
            SetState(state, L"launching", L"校验通过，正在准备安装…");
            Handle parent(OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, TRUE, GetCurrentProcessId()));
            Require(static_cast<bool>(parent), "Cannot prepare update handoff");
            Handle helperProcess;
            Require(StartHelper(helper, installer, application, *checksum, job.lock.value, parent.value, &helperProcess), "Update helper did not accept the verified package");
            job.handedOff = true;
            {
                std::lock_guard lock(state->mutex);
                state->snapshot.state = L"launching";
                state->snapshot.message = L"校验通过，正在停止音频并退出 Airloom，随后安装并重新打开…";
                state->snapshot.progress = 1;
                state->snapshot.exitRequested = true;
            }
            auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(35);
            while (!state->stopped.load())
            {
                if (WaitForSingleObject(helperProcess.value, 100) != WAIT_TIMEOUT
                    || std::chrono::steady_clock::now() >= deadline)
                {
                    std::lock_guard lock(state->mutex);
                    state->snapshot.exitRequested = false;
                    state->snapshot.state = L"error";
                    state->snapshot.message = L"应用未能完成退出，更新没有完成。请从托盘选择“退出 Airloom”后重新运行安装包。";
                    break;
                }
            }
        });
    }

    void UpdateService::Cancel()
    {
        std::lock_guard lock(m_state->mutex);
        if (m_state->snapshot.state == L"checking" || m_state->snapshot.state == L"downloading")
        {
            m_state->cancelled.store(true);
            m_state->snapshot.message = L"正在取消并清理下载…";
        }
    }
}
