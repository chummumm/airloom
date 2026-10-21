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
#include <sstream>
#include <string>
#include <string_view>

namespace
{
    struct Fixture
    {
        std::mutex mutex;
        std::array<bool, 3> selected{true, true, false};
        double volume{0.56};
        bool muteLocal{};
        bool testTone{};
        bool authentication{};
        bool sourceHeadphones{};
        unsigned frame{};
    };

    constexpr std::array<char const*, 3> DeviceIds{
        "fixture-living-room", "fixture-study", "fixture-bedroom"};
    constexpr std::array<char const*, 3> DeviceNames{
        "客厅 HomePod", "书房 HomePod mini", "卧室 · 窗边的 HomePod"};

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
        auto streaming = static_cast<unsigned>(std::count(fixture.selected.begin(), fixture.selected.end(), true));
        if (fixture.authentication && fixture.selected[2]) --streaming;
        constexpr unsigned elapsedSeconds = 187;
        constexpr unsigned packetsPerReceiver = elapsedSeconds * 44100 / 352;
        std::ostringstream json;
        json.imbue(std::locale::classic());
        json << std::fixed << std::setprecision(4)
            << "{\"version\":1,\"state\":\"" << (streaming ? "streaming" : "idle")
            << "\",\"status_message\":\"界面测试数据 · 未连接真实扬声器\",\"volume\":" << fixture.volume
            << ",\"mute_local\":" << (fixture.muteLocal ? "true" : "false")
            << ",\"test_tone_active\":" << (fixture.testTone ? "true" : "false")
            << ",\"selected_source_id\":\"" << (fixture.sourceHeadphones ? "fixture-headphones" : "fixture-default")
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
            bool const playing = fixture.selected[index] && !auth;
            json << "{\"id\":\"" << DeviceIds[index] << "\",\"name\":\"" << DeviceNames[index]
                << "\",\"model\":\"" << (index == 1 ? "HomePod mini" : "HomePod")
                << "\",\"online\":true,\"password_required\":" << (index == 2 ? "true" : "false")
                << ",\"selected\":" << (fixture.selected[index] ? "true" : "false")
                << ",\"state\":\"" << (auth ? "auth_required" : playing ? "streaming" : "idle")
                << "\",\"packets_sent\":" << (playing ? packetsPerReceiver : 0) << '}';
        }
        json << "],\"sources\":[{\"id\":\"fixture-default\",\"name\":\"扬声器 · Realtek USB Audio\",\"kind\":\"system\"},"
            << "{\"id\":\"fixture-headphones\",\"name\":\"耳机 · USB Audio 输出设备\",\"kind\":\"system\"}],"
            << "\"elapsed_seconds\":" << (streaming ? elapsedSeconds : 0)
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
            << "\",\"message\":\"界面测试信号 · 非真实音频\",\"source_name\":\"扬声器 · Realtek USB Audio\"},"
            << "\"diagnostics\":{\"schema_version\":1,\"app_version\":\"ui-fixture\",\"fixture\":true,"
            << "\"capture\":{\"state\":\"signal\"},\"receivers\":[]}";
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
        else if (type == "set_mute_local") fixture.muteLocal = Field(json, "enabled") == "true";
        else if (type == "test_tone") fixture.testTone = Field(json, "enabled") != "false";
        else if (type == "select_source") fixture.sourceHeadphones = Field(json, "source_id") == "fixture-headphones";
        else if (type == "toggle_device")
        {
            auto const id = Field(json, "device_id");
            for (size_t index = 0; index < DeviceIds.size(); ++index)
            {
                if (id != DeviceIds[index]) continue;
                fixture.selected[index] = Field(json, "selected") == "true";
                if (index == 2) fixture.authentication = fixture.selected[index];
            }
        }
        else if (type == "disconnect")
        {
            if (!Field(json, "request_id").empty()) fixture.selected[2] = false;
            else fixture.selected.fill(false);
            fixture.authentication = false;
        }
        else if (type == "authenticate" || type == "pair") fixture.authentication = false;
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
