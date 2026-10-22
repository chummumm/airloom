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
#include <fstream>
#include <iomanip>
#include <locale>
#include <memory>
#include <mutex>
#include <optional>
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
        std::array<unsigned, 3> connectionIds{1, 2, 0};
        unsigned connectionsCreated{2};
        unsigned disconnections{};
        double volume{0.56};
        std::array<std::optional<double>, 3> receiverVolumes{0.56, 0.56, 0.56};
        unsigned setVolumeCommands{};
        bool running{true};
        bool muteLocal{};
        bool testTone{};
        bool authentication{};
        bool applicationMode{};
        bool sourcesRefreshing{};
        bool applicationListChanged{};
        bool stereoGroup{};
        bool stereoSecondOnline{true};
        std::string sourceId{"fixture-default"};
        std::string eventDirectory;
        unsigned sourceRefreshes{};
        unsigned discoveryRefreshes{};
        unsigned sourceSelections{};
        unsigned playbackCommands{};
        unsigned eventIndex{};
        unsigned frame{};
    };

    constexpr std::array<char const*, 3> DeviceIds{
        "fixture-living-room", "fixture-study", "fixture-bedroom"};
    constexpr std::array<char const*, 3> DeviceNames{
        "客厅 HomePod", "书房 HomePod mini", "卧室 · 窗边的 HomePod"};
    constexpr char StereoId[] = "stereo:12345678-1234-5678-9abc-123456789abc";
    constexpr char StereoName[] = "客厅立体声 · 测试组合";
    constexpr std::array<char const*, 2> StereoMemberNames{
        "客厅 HomePod A · 测试成员", "客厅 HomePod B · 测试成员"};
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
    constexpr std::array<Source, 3> ChangedApplicationSources{{
        ApplicationSources[1], ApplicationSources[0],
        {"process:4203:133700000000000003", "Audacity · 新发现测试应用"}}};

    std::span<Source const> Sources(Fixture const& fixture)
    {
        if (fixture.applicationMode)
            return fixture.applicationListChanged ? std::span<Source const>(ChangedApplicationSources) : std::span<Source const>(ApplicationSources);
        return EndpointSources;
    }

    Source const* SelectedSource(Fixture const& fixture)
    {
        for (auto const& source : Sources(fixture))
            if (fixture.sourceId == source.id) return &source;
        return nullptr;
    }

    struct VolumeState { double value; bool known; bool mixed; };

    VolumeState SelectedVolume(Fixture const& fixture)
    {
        VolumeState result{fixture.volume, true, false};
        bool hasSelected{};
        bool hasReading{};
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (!fixture.selected[index]) continue;
            hasSelected = true;
            if (!fixture.receiverVolumes[index])
            {
                result.known = false;
                continue;
            }
            auto const value = *fixture.receiverVolumes[index];
            if (!hasReading) result.value = value;
            else if (std::abs(value - result.value) >= 0.005) result.mixed = true;
            hasReading = true;
        }
        result.known = result.known && hasSelected;
        result.mixed = result.mixed && result.known;
        return result;
    }

    void DisconnectReceiver(Fixture& fixture, size_t index)
    {
        if (fixture.playing[index]) ++fixture.disconnections;
        fixture.playing[index] = false;
        if (index == 2) fixture.authentication = false;
    }

    void StopPlayback(Fixture& fixture)
    {
        fixture.running = false;
        for (size_t index = 0; index < DeviceIds.size(); ++index) DisconnectReceiver(fixture, index);
        fixture.authentication = false;
        fixture.testTone = false;
    }

    void ConnectReceiver(Fixture& fixture, size_t index)
    {
        if (!fixture.selected[index] || !SelectedSource(fixture)) return;
        if (fixture.stereoGroup && index == 1 && !fixture.stereoSecondOnline) return;
        if (index == 2) fixture.authentication = true;
        else if (!fixture.playing[index])
        {
            fixture.connectionIds[index] = ++fixture.connectionsCreated;
            fixture.playing[index] = true;
        }
    }

    void StartPlayback(Fixture& fixture)
    {
        if (!SelectedSource(fixture)) return;
        fixture.running = std::any_of(fixture.selected.begin(), fixture.selected.end(), [](bool value) { return value; });
        for (size_t index = 0; index < DeviceIds.size(); ++index) ConnectReceiver(fixture, index);
    }

    std::string Environment(char const* name)
    {
        std::array<char, 4096> value{};
        auto length = GetEnvironmentVariableA(name, value.data(), static_cast<DWORD>(value.size()));
        return length > 0 && length < value.size() ? std::string(value.data(), length) : std::string{};
    }

    // The smoke driver publishes one immutable numbered event file at a time.
    // These controls exist only in this replacement DLL, never in the engine or
    // release command schema. Acknowledgements let UIA observe changes without
    // moving focus away from the open source popup.
    void ApplyFixtureEvent(Fixture& fixture)
    {
        if (fixture.eventDirectory.empty()) return;
        auto const index = fixture.eventIndex + 1;
        std::ifstream input(fixture.eventDirectory + "/event-" + std::to_string(index) + ".txt");
        std::string action;
        if (!std::getline(input, action)) return;
        if (action == "source_refreshing") fixture.sourcesRefreshing = true;
        else if (action == "applications_changed")
        {
            fixture.applicationListChanged = true;
            fixture.sourcesRefreshing = false;
        }
        else if (action == "native_stereo") fixture.stereoGroup = true;
        else if (action == "stereo_partial")
        {
            fixture.stereoSecondOnline = false;
            fixture.playing[1] = false;
        }
        else if (action == "device_volumes_mixed")
        {
            fixture.receiverVolumes[0] = 0.30;
            fixture.receiverVolumes[1] = 0.70;
        }
        else if (action == "device_volume_read_failed") fixture.receiverVolumes[1].reset();
        else return;
        fixture.eventIndex = index;
        std::ofstream acknowledgement(fixture.eventDirectory + "/ack-" + std::to_string(index) + ".txt");
        acknowledgement << action;
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
        ApplyFixtureEvent(fixture);
        auto const physicalStreaming = static_cast<unsigned>(std::count(fixture.playing.begin(), fixture.playing.end(), true));
        bool const groupSelected = fixture.selected[0] || fixture.selected[1];
        bool const groupStreaming = fixture.playing[0] && fixture.playing[1] && fixture.stereoSecondOnline;
        auto const streaming = fixture.stereoGroup ? static_cast<unsigned>(groupStreaming) + static_cast<unsigned>(fixture.playing[2]) : physicalStreaming;
        auto const groupState = !fixture.stereoSecondOnline && groupSelected ? "error" : groupStreaming ? "streaming" : "idle";
        auto const sessionState = streaming ? "streaming" : fixture.stereoGroup && groupSelected && !fixture.stereoSecondOnline ? "error" : "idle";
        auto const mode = fixture.applicationMode ? "application" : "endpoint";
        auto const source = SelectedSource(fixture);
        auto const sourceName = source ? source->name : "";
        bool const captureActive = streaming && source && fixture.running;
        auto const captureState = captureActive ? "signal" : streaming ? "waiting" : "idle";
        auto const volume = SelectedVolume(fixture);
        constexpr unsigned elapsedSeconds = 187;
        constexpr unsigned packetsPerReceiver = elapsedSeconds * 44100 / 352;
        std::ostringstream json;
        json.imbue(std::locale::classic());
        json << std::fixed << std::setprecision(4)
            << "{\"version\":1,\"state\":\"" << sessionState
            << "\",\"status_message\":\"界面测试数据 · 未连接真实扬声器\",\"volume\":" << volume.value
            << ",\"volume_known\":" << (volume.known ? "true" : "false")
            << ",\"volume_mixed\":" << (volume.mixed ? "true" : "false")
            << ",\"mute_local\":" << (fixture.muteLocal ? "true" : "false")
            << ",\"test_tone_active\":" << (fixture.testTone ? "true" : "false")
            << ",\"source_mode\":\"" << mode << "\",\"sources_refreshing\":" << (fixture.sourcesRefreshing ? "true" : "false") << ",\"source_message\":\""
            << (fixture.applicationMode ? "只采集所选应用及其子进程；同一浏览器的多个标签页会一起采集。"
                : "采集所选 Windows 输出设备上的全部应用声音。")
            << "\",\"selected_source_id\":\"" << fixture.sourceId
            << "\",\"selected_device_ids\":[";
        bool first = true;
        std::string firstId;
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (fixture.stereoGroup && index == 1) continue;
            auto const logicalId = fixture.stereoGroup && index == 0 ? StereoId : DeviceIds[index];
            if (!(fixture.stereoGroup && index == 0 ? groupSelected : fixture.selected[index])) continue;
            if (!first) json << ',';
            json << '"' << logicalId << '"';
            if (first) firstId = logicalId;
            first = false;
        }
        json << "],\"selected_device_id\":\"" << firstId << "\",\"devices\":[";
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (fixture.stereoGroup && index == 1) continue;
            if (index != 0) json << ',';
            if (fixture.stereoGroup && index == 0)
            {
                json << "{\"id\":\"" << StereoId << "\",\"name\":\"" << StereoName
                    << "\",\"kind\":\"native_stereo\",\"model\":\"HomePod\",\"expected_members\":2,\"group_complete\":"
                    << (fixture.stereoSecondOnline ? "true" : "false") << ",\"online\":" << (fixture.stereoSecondOnline ? "true" : "false")
                    << ",\"selected\":" << (groupSelected ? "true" : "false") << ",\"state\":\"" << groupState
                    << "\",\"status_message\":\"界面测试组合 · 实机同步未验证\",\"members\":[";
                for (size_t memberIndex = 0; memberIndex < 2; ++memberIndex)
                {
                    if (memberIndex != 0) json << ',';
                    bool const online = memberIndex == 0 || fixture.stereoSecondOnline;
                    json << "{\"id\":\"" << DeviceIds[memberIndex] << "\",\"name\":\"" << StereoMemberNames[memberIndex]
                        << "\",\"online\":" << (online ? "true" : "false") << ",\"state\":\""
                        << (fixture.playing[memberIndex] ? "streaming" : "idle")
                        << "\",\"status_message\":\"界面测试成员\"}";
                }
                json << "]}";
                continue;
            }
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
            << ",\"level\":{\"left\":" << (captureActive ? 0.36 : 0.0) << ",\"right\":" << (captureActive ? 0.28 : 0.0)
            << "},\"waveform\":[";
        // Match the real core's 64 nonnegative peak-envelope bins. The native
        // renderer applies its own decibel scale; these are not signed PCM.
        for (unsigned index = 0; index < 64; ++index)
        {
            if (index != 0) json << ',';
            double const phase = static_cast<double>(index) * 0.28 + fixture.frame * 0.035;
            double const envelope = 0.18 + 0.5 * std::pow(std::sin(static_cast<double>(index) * 0.047), 2);
            json << (captureActive ? std::abs(std::sin(phase)) * envelope : 0.0);
        }
        ++fixture.frame;
        json << "],\"metrics\":{\"sample_rate\":44100,\"packets_sent\":" << packetsPerReceiver * physicalStreaming
            << "},\"capture\":{\"state\":\"" << captureState
            << "\",\"message\":\"" << (captureActive ? "界面测试信号 · 非真实音频" : streaming ? "请选择声音来源；扬声器保持连接。" : "等待声音来源。")
            << "\",\"source_name\":\"" << sourceName << "\"},"
            << "\"diagnostics\":{\"schema_version\":1,\"app_version\":\"ui-fixture\",\"fixture\":true,"
            << "\"session_state\":\"" << sessionState
            << "\",\"mute_local\":" << (fixture.muteLocal ? "true" : "false")
            << ",\"test_tone_active\":" << (fixture.testTone ? "true" : "false")
            << ",\"capture\":{\"source_mode\":\"" << mode << "\",\"state\":\"" << captureState
            << "\"},\"receivers\":[";
        first = true;
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (!fixture.selected[index]) continue;
            if (!first) json << ',';
            json << "{\"slot\":" << index + 1 << ",\"state\":\""
                << (fixture.playing[index] ? "streaming" : index == 2 && fixture.authentication ? "auth_required" : "idle") << "\"}";
            first = false;
        }
        // Test-only command evidence, available through the real diagnostics UI.
        // Never copied into the production diagnostics schema or release DLL.
        json << "],\"fixture_state\":{\"source_id\":\"" << fixture.sourceId
            << "\",\"source_count\":" << Sources(fixture).size()
            << ",\"refresh_count\":" << fixture.sourceRefreshes
            << ",\"discovery_refresh_count\":" << fixture.discoveryRefreshes
            << ",\"connections_created\":" << fixture.connectionsCreated
            << ",\"disconnections\":" << fixture.disconnections
            << ",\"connection_ids\":[" << fixture.connectionIds[0] << ',' << fixture.connectionIds[1] << ',' << fixture.connectionIds[2] << ']'
            << ",\"capture_active\":" << (captureActive ? "true" : "false")
            << ",\"source_selection_commands\":" << fixture.sourceSelections
            << ",\"playback_commands\":" << fixture.playbackCommands
            << ",\"automatic_events\":" << fixture.eventIndex
            << ",\"streaming_receivers\":" << physicalStreaming
            << ",\"streaming_targets\":" << streaming
            << ",\"volume\":" << volume.value
            << ",\"volume_known\":" << (volume.known ? "true" : "false")
            << ",\"volume_mixed\":" << (volume.mixed ? "true" : "false")
            << ",\"set_volume_commands\":" << fixture.setVolumeCommands
            << ",\"receiver_volumes\":[";
        for (size_t index = 0; index < DeviceIds.size(); ++index)
        {
            if (index != 0) json << ',';
            if (fixture.receiverVolumes[index]) json << *fixture.receiverVolumes[index];
            else json << "null";
        }
        json << "]"
            << ",\"native_stereo\":" << (fixture.stereoGroup ? "true" : "false")
            << ",\"group_complete\":" << (fixture.stereoSecondOnline ? "true" : "false")
            << ",\"group_state\":\"" << groupState << "\""
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
        auto fixture = std::make_unique<Fixture>();
        fixture->eventDirectory = Environment("AIRLOOM_UI_FIXTURE_EVENTS");
        return fixture.release();
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
            ++fixture.setVolumeCommands;
            for (size_t index = 0; index < DeviceIds.size(); ++index)
                if (fixture.selected[index]) fixture.receiverVolumes[index] = fixture.volume;
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
            fixture.testTone = false;
            fixture.applicationMode = applicationMode;
            // Synthetic enumeration completes immediately. Like the real core,
            // endpoints regain their default; applications require explicit choice.
            fixture.sourceId = applicationMode ? "" : EndpointSources[0].id;
            if (applicationMode) fixture.muteLocal = false;
        }
        else if (type == "refresh") ++fixture.discoveryRefreshes;
        else if (type == "refresh_sources") ++fixture.sourceRefreshes;
        else if (type == "select_source")
        {
            ++fixture.sourceSelections;
            auto const id = Field(json, "source_id");
            for (auto const& entry : Sources(fixture))
            {
                if (id != entry.id) continue;
                fixture.testTone = false;
                fixture.sourceId = id;
                if (fixture.applicationMode) fixture.muteLocal = false;
                break;
            }
        }
        else if (type == "connect")
        {
            ++fixture.playbackCommands;
            StartPlayback(fixture);
        }
        else if (type == "toggle_device")
        {
            ++fixture.playbackCommands;
            auto const id = Field(json, "device_id");
            for (size_t index = 0; index < DeviceIds.size(); ++index)
            {
                if (id != DeviceIds[index] && !(fixture.stereoGroup && id == StereoId && index < 2)) continue;
                auto const selected = Field(json, "selected");
                bool const enabled = selected.empty() ? !fixture.selected[index] : selected == "true";
                if (enabled == fixture.selected[index]) continue;
                fixture.selected[index] = enabled;
                if (enabled && fixture.running) ConnectReceiver(fixture, index);
                else if (!enabled)
                {
                    DisconnectReceiver(fixture, index);
                    if (std::none_of(fixture.selected.begin(), fixture.selected.end(), [](bool value) { return value; }))
                        StopPlayback(fixture);
                }
            }
        }
        else if (type == "disconnect")
        {
            ++fixture.playbackCommands;
            if (!Field(json, "request_id").empty())
            {
                DisconnectReceiver(fixture, 2);
            }
            else StopPlayback(fixture);
        }
        else if (type == "authenticate" || type == "pair")
        {
            if (fixture.authentication)
            {
                fixture.connectionIds[2] = ++fixture.connectionsCreated;
                fixture.playing[2] = true;
            }
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
