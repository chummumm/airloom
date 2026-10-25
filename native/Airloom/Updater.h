#pragma once

#include <memory>
#include <string>

namespace airloom::native
{
    struct UpdateSnapshot
    {
        std::wstring state{L"idle"};
        std::wstring currentVersion{L"0.4.4"};
        std::wstring availableVersion;
        std::wstring message{L"检查 GitHub 上的最新版本。安装结束后会自动清理下载包。"};
        double progress{};
        bool exitRequested{};
    };

    struct UpdateShared;

    // Network work stays off the XAML thread. A separate native helper waits
    // for this application to exit before installing and removing its package.
    class UpdateService final
    {
    public:
        UpdateService();
        ~UpdateService();
        UpdateService(UpdateService const&) = delete;
        UpdateService& operator=(UpdateService const&) = delete;

        UpdateSnapshot Snapshot() const;
        void Check();
        void Install();
        void Cancel();
        void PrepareForExit() noexcept;

    private:
        std::shared_ptr<UpdateShared> m_state;
    };
}
