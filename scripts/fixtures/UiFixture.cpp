// CI-only visual fixture for the real WinUI executable. This DLL is compiled
// into an isolated temporary archive copy and never enters dist/ or an installer.
// All displayed receivers and peak-envelope bins below are explicitly synthetic.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iomanip>
#include <locale>
#include <memory>
#include <mutex>
#include <span>
#include <sstream>
#include <string>
#include <string_view>

namespace
{
    struct Fixture
    {
        std::mutex mutex;
        std::array<bool, 3> selected{true, true, false};
        std::array<bool, 3> playing{true, true, false};
        double volume{0.56};
        bool running{true};
        bool muteLocal{};
        bool testTone{};
        bool authentication{};
        bool applicationMode{};
        std::string sourceId{"fixture-default"};
        unsigned sourceRefreshes{};
        unsigned frame{};
    };

    constexpr std::array<char const*, 3> DeviceIds{
        "fixture-living-room", "fixture-study", "fixture-bedroom"};
    constexpr std::array<char const*, 3> DeviceNames{
        "客厅 HomePod", "书房 HomePod mini", "卧室 · 窗边的 HomePod"};
    struct Source { char const* id; char const* name; };
    constexpr std::array<Source, 2> EndpointSources{{
        {"fixture-default", "扬声器 · Realtek USB Audio"},
        {"fixture-headphones", "耳机 · USB Audio 输出设备"}}};
    // These identities deliberately have the production PID + creation-time
    // shape, but identify no real process on the runner.
    constexpr std::array<Source, 3> ApplicationSources{{
        {"process:4200:133700000000000000", "Microsoft Edge · 测试应用"},
        {"process:4201:133700000000000001", "Spotify · 测试应用"},
        {"process:4202:133700000000000002", "VLC media player · 测试应用"}}};

    std::span<Source const> Sources(Fixture const& fixture)
    {
        if (fixture.applicationMode) return ApplicationSources;
        return EndpointSources;
    }

    Source const* SelectedSource(Fixture const& fixture)
    {
        for (auto const& source : Sources(fixture))
            if (fixture.sourceId == source.id) return &source;
        return nullptr;
    }

    void StopPlayback(Fixture& fixture)
    {
        fixture.running = false;
        fixture.playing.fill(false);
        fixture.authentication = false;
        fixture.testTone = false;
    }

    void ConnectReceiver(Fixture& fixture, size_t index)
    {
        if (!fixture.selected[index] || !SelectedSource(fixture)) return;
        if (index == 2) fixture.authentication = true;
        else fixture.playing[index] = true;
    }

    void StartPlayback(Fixture& fixture)
    {
        if (!SelectedSource(fixture)) return;
        fixture.running = std::any_of(fixture.selected.begin(), fixture.selected.end(), [](bool value) { return value; });
        for (size_t index = 0; index < DeviceIds.size(); ++index) ConnectReceiver(fixture, index);
    }

    std::string Environment(char const* name)
    {
        std::array<char, 64> value{};
        auto length = GetEnvironmentVariableA(name, value.data(), static_cast<DWORD>(value.size()));
        return length > 0 && length < value.size() ? std::string(value.data(), length) : std::string{};
    }

    // Commands come only from the application under test. The fixture needs a
    // small allowlist of ASCII keys/IDs, never a general JSON parser or credentials.
    std::string_view Field(std::string_view json, std::string_view name)
    {
        auto key = std::string("\"") + std::string(name) + "\"";
        auto position = json.find(key);
        if (position == std::string_view::npos) return {};
        position = json.find(':', position + key.size());
        if (position == std::string_view::npos) return {};
        position = json.find_first_not_of(" \t\r\n", position + 1);
        if (position == std::string_view::npos) return {};
        if (json[position] == '"')
        {
            auto end = json.find('"', position + 1);
            return end == std::string_view::npos ? std::string_view{} : json.substr(position + 1, end - position - 1);
        }
        auto end = json.find_first_of(",} \t\r\n", position);
        return json.substr(position, end == std::string_view::npos ? json.size() - position : end - position);
    }

    std::string Snapshot(Fixture& fixture)
    {
        auto const streaming = static_cast<unsigned>(std::count(fixture.playing.begin(), fixture.playing.end(), true));
        auto const mode = fixture.applicationMode ? "application" : "endpoint";
        auto const source = SelectedSource(fixture);
        auto const sourceName = source ? source->name : "";
        constexpr unsigned elapsedSeconds = 187;
        constexpr unsigned packetsPerReceiver = elapsedSeconds * 44100 / 352;
        std::ostringstream json;
        json.imbue(std::locale::classic());
        json << std::fixed << std::setprecision(4)
            << "{\"version\":1,\"state\":\"" << (streaming ? "streaming" : "idle")
            << "\",\"status_message\":\"界面测试数据 · 未连接真实扬声器\",\"volume\":" << fixture.volume
            << ",\"mute_local\":" << (fixture.muteLocal ? "true" : "false")
            << ",\"test_tone_active\":" << (fixture.testTone ? "true" : "false")
            << ",\"source_mode\":\"" << mode << "\",\"sources_refreshing\":false,\"source_message\":\""
            << (fixture.applicationMode ? "只采集所选应用及其子进程；同一浏览器的多个标签页会一起采集。"
                : "采集所选 Windows 输出设备上的全部应用声音。")
            << "\",\"selected_source_id\":\"" << fixture.sourceId
            << "\",\"selected_device_ids\":[";
        bool first = true;
        std::string firstId;
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (!fixture.selected[index]) continue;
            if (!first) json << ',';
            json << '"' << DeviceIds[index] << '"';
            if (first) firstId = DeviceIds[index];
            first = false;
        }
        json << "],\"selected_device_id\":\"" << firstId << "\",\"devices\":[";
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (index != 0) json << ',';
            bool const auth = index == 2 && fixture.authentication && fixture.selected[index];
            bool const playing = fixture.playing[index];
            json << "{\"id\":\"" << DeviceIds[index] << "\",\"name\":\"" << DeviceNames[index]
                << "\",\"model\":\"" << (index == 1 ? "HomePod mini" : "HomePod")
                << "\",\"online\":true,\"password_required\":" << (index == 2 ? "true" : "false")
                << ",\"selected\":" << (fixture.selected[index] ? "true" : "false")
                << ",\"state\":\"" << (auth ? "auth_required" : playing ? "streaming" : "idle")
                << "\",\"packets_sent\":" << (playing ? packetsPerReceiver : 0) << '}';
        }
        json << "],\"sources\":[";
        first = true;
        for (auto const& entry : Sources(fixture))
        {
            if (!first) json << ',';
            json << "{\"id\":\"" << entry.id << "\",\"name\":\"" << entry.name
                << "\",\"kind\":\"" << mode << "\",\"is_default\":"
                << (!fixture.applicationMode && first ? "true" : "false") << '}';
            first = false;
        }
        json << "],\"elapsed_seconds\":" << (streaming ? elapsedSeconds : 0)
            << ",\"level\":{\"left\":" << (streaming ? 0.36 : 0.0) << ",\"right\":" << (streaming ? 0.28 : 0.0)
            << "},\"waveform\":[";
        // Match the real core's 64 nonnegative peak-envelope bins. The native
        // renderer applies its own decibel scale; these are not signed PCM.
        for (unsigned index = 0; index < 64; ++index)
        {
            if (index != 0) json << ',';
            double const phase = static_cast<double>(index) * 0.28 + fixture.frame * 0.035;
            double const envelope = 0.18 + 0.5 * std::pow(std::sin(static_cast<double>(index) * 0.047), 2);
            json << (streaming ? std::abs(std::sin(phase)) * envelope : 0.0);
        }
        ++fixture.frame;
        json << "],\"metrics\":{\"sample_rate\":44100,\"packets_sent\":" << packetsPerReceiver * streaming
            << "},\"capture\":{\"state\":\"" << (streaming ? "signal" : "idle")
            << "\",\"message\":\"界面测试信号 · 非真实音频\",\"source_name\":\"" << sourceName << "\"},"
            << "\"diagnostics\":{\"schema_version\":1,\"app_version\":\"ui-fixture\",\"fixture\":true,"
            << "\"session_state\":\"" << (streaming ? "streaming" : "idle")
            << "\",\"mute_local\":" << (fixture.muteLocal ? "true" : "false")
            << ",\"test_tone_active\":" << (fixture.testTone ? "true" : "false")
            << ",\"capture\":{\"source_mode\":\"" << mode << "\",\"state\":\"" << (streaming ? "signal" : "idle")
            << "\"},\"receivers\":[";
        first = true;
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (!fixture.selected[index]) continue;
            if (!first) json << ',';
            json << "{\"slot\":" << index << ",\"state\":\""
                << (fixture.playing[index] ? "streaming" : index == 2 && fixture.authentication ? "auth_required" : "idle") << "\"}";
            first = false;
        }
        // Test-only command evidence, available through the real diagnostics UI.
        // Never copied into the production diagnostics schema or release DLL.
        json << "],\"fixture_state\":{\"source_id\":\"" << fixture.sourceId
            << "\",\"source_count\":" << Sources(fixture).size()
            << ",\"refresh_count\":" << fixture.sourceRefreshes
            << ",\"streaming_receivers\":" << streaming
            << ",\"auth_pending\":" << (fixture.authentication ? "true" : "false") << "}}";
        if (fixture.authentication)
        {
            json << ",\"auth\":{\"request_id\":1,\"kind\":\"password\",\"device_id\":\"fixture-bedroom\","
                << "\"device_name\":\"卧室 · 窗边的 HomePod\",\"message\":\"界面测试：此设备需要 AirPlay 访问密码。其他扬声器继续播放。\"}";
        }
        json << '}';
        return json.str();
    }
}

extern "C" __declspec(dllexport) void* __cdecl airloom_create()
{
    try
    {
        if (Environment("AIRLOOM_UI_FIXTURE") != "1") return nullptr;
        return std::make_unique<Fixture>().release();
    }
    catch (...) { return nullptr; }
}

extern "C" __declspec(dllexport) void __cdecl airloom_destroy(void* instance)
{
    delete static_cast<Fixture*>(instance);
}

extern "C" __declspec(dllexport) int __cdecl airloom_command(void* instance, char const* command)
{
    if (!instance || !command) return 1;
    try
    {
        auto& fixture = *static_cast<Fixture*>(instance);
        std::lock_guard lock(fixture.mutex);
        std::string_view json(command);
        auto const type = Field(json, "type");
        if (type == "set_volume")
        {
            auto value = std::string(Field(json, "volume"));
            char* end{};
            auto volume = std::strtod(value.c_str(), &end);
            if (value.empty() || end != value.c_str() + value.size() || !std::isfinite(volume)) return 1;
            fixture.volume = std::clamp(volume, 0.0, 1.0);
        }
        else if (type == "set_mute_local") fixture.muteLocal = !fixture.applicationMode && Field(json, "enabled") == "true";
        else if (type == "test_tone") fixture.testTone = Field(json, "enabled") != "false"
            && std::any_of(fixture.playing.begin(), fixture.playing.end(), [](bool value) { return value; });
        else if (type == "select_source_mode")
        {
            auto const mode = Field(json, "mode");
            if (mode != "endpoint" && mode != "application") return 1;
            bool const applicationMode = mode == "application";
            if (applicationMode == fixture.applicationMode) return 0;
            StopPlayback(fixture);
            fixture.applicationMode = applicationMode;
            // Synthetic enumeration completes immediately. Like the real core,
            // endpoints regain their default; applications require explicit choice.
            fixture.sourceId = applicationMode ? "" : EndpointSources[0].id;
            if (applicationMode) fixture.muteLocal = false;
        }
        else if (type == "refresh_sources") ++fixture.sourceRefreshes;
        else if (type == "select_source")
        {
            auto const id = Field(json, "source_id");
            for (auto const& entry : Sources(fixture))
            {
                if (id != entry.id) continue;
                bool const resume = fixture.running;
                StopPlayback(fixture);
                fixture.sourceId = id;
                if (fixture.applicationMode) fixture.muteLocal = false;
                if (resume) StartPlayback(fixture);
                break;
            }
        }
        else if (type == "connect") StartPlayback(fixture);
        else if (type == "toggle_device")
        {
            auto const id = Field(json, "device_id");
            for (size_t index = 0; index < DeviceIds.size(); ++index)
            {
                if (id != DeviceIds[index]) continue;
                auto const selected = Field(json, "selected");
                bool const enabled = selected.empty() ? !fixture.selected[index] : selected == "true";
                if (enabled == fixture.selected[index]) continue;
                fixture.selected[index] = enabled;
                if (enabled && fixture.running) ConnectReceiver(fixture, index);
                else if (!enabled)
                {
                    fixture.playing[index] = false;
                    if (index == 2) fixture.authentication = false;
                    if (std::none_of(fixture.selected.begin(), fixture.selected.end(), [](bool value) { return value; }))
                        StopPlayback(fixture);
                }
            }
        }
        else if (type == "disconnect")
        {
            if (!Field(json, "request_id").empty())
            {
                fixture.playing[2] = false;
                fixture.authentication = false;
            }
            else StopPlayback(fixture);
        }
        else if (type == "authenticate" || type == "pair")
        {
            if (fixture.authentication) fixture.playing[2] = true;
            fixture.authentication = false;
        }
        return 0;
    }
    catch (...) { return 1; }
}

extern "C" __declspec(dllexport) char* __cdecl airloom_snapshot(void* instance)
{
    if (!instance) return nullptr;
    try
    {
        auto& fixture = *static_cast<Fixture*>(instance);
        std::lock_guard lock(fixture.mutex);
        auto const json = Snapshot(fixture);
        auto result = new char[json.size() + 1];
        std::memcpy(result, json.c_str(), json.size() + 1);
        return result;
    }
    catch (...) { return nullptr; }
}

extern "C" __declspec(dllexport) void __cdecl airloom_free_string(char* value)
{
    delete[] value;
}
