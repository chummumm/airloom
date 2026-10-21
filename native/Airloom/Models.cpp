#include "pch.h"
#include "Models.h"

using namespace winrt;
using namespace Windows::Data::Json;

namespace
{
    IJsonValue Value(JsonObject const& object, wchar_t const* key)
    {
        return object && object.HasKey(key) ? object.GetNamedValue(key) : nullptr;
    }

    hstring Text(JsonObject const& object, wchar_t const* key, hstring const& fallback = {})
    {
        auto value = Value(object, key);
        return value && value.ValueType() == JsonValueType::String ? value.GetString() : fallback;
    }

    double Number(JsonObject const& object, wchar_t const* key, double fallback = 0)
    {
        auto value = Value(object, key);
        if (!value || value.ValueType() != JsonValueType::Number)
        {
            return fallback;
        }
        auto number = value.GetNumber();
        return std::isfinite(number) ? number : fallback;
    }

    bool Boolean(JsonObject const& object, wchar_t const* key, bool fallback = false)
    {
        auto value = Value(object, key);
        return value && value.ValueType() == JsonValueType::Boolean ? value.GetBoolean() : fallback;
    }

    JsonObject Object(JsonObject const& object, wchar_t const* key)
    {
        auto value = Value(object, key);
        return value && value.ValueType() == JsonValueType::Object ? value.GetObject() : JsonObject{};
    }

    JsonArray Array(JsonObject const& object, wchar_t const* key)
    {
        auto value = Value(object, key);
        return value && value.ValueType() == JsonValueType::Array ? value.GetArray() : JsonArray{};
    }

    uint64_t Counter(JsonObject const& object, wchar_t const* key)
    {
        // JSON numbers are exact only through 2^53. Counts larger than that are not useful here.
        return static_cast<uint64_t>(std::clamp(Number(object, key), 0.0, 9007199254740991.0));
    }
}

namespace airloom::native
{
    Snapshot Snapshot::Parse(JsonObject const& json)
    {
        if (Number(json, L"version") != 1)
        {
            throw hresult_invalid_argument(L"音频引擎的状态格式与界面版本不兼容。");
        }

        Snapshot result;
        result.state = Text(json, L"state", L"idle");
        result.statusMessage = Text(json, L"status_message");
        result.selectedDeviceId = Text(json, L"selected_device_id");
        for (auto const& item : Array(json, L"selected_device_ids"))
        {
            if (item.ValueType() == JsonValueType::String && result.selectedDeviceIds.size() < 256)
                result.selectedDeviceIds.push_back(item.GetString());
        }
        result.selectedSourceId = Text(json, L"selected_source_id");
        result.sourceMode = Text(json, L"source_mode", L"endpoint");
        result.sourcesRefreshing = Boolean(json, L"sources_refreshing");
        result.sourceMessage = Text(json, L"source_message");
        result.volume = std::clamp(Number(json, L"volume", 0.75), 0.0, 1.0);
        result.muteLocal = Boolean(json, L"mute_local");
        result.elapsedSeconds = Counter(json, L"elapsed_seconds");

        for (auto const& item : Array(json, L"devices"))
        {
            if (item.ValueType() != JsonValueType::Object || result.devices.size() >= 256) continue;
            auto const object = item.GetObject();
            Device device;
            device.id = Text(object, L"id");
            device.name = Text(object, L"name", L"AirPlay 扬声器");
            device.model = Text(object, L"model");
            device.address = Text(object, L"address");
            device.passwordRequired = Boolean(object, L"password_required");
            device.online = Boolean(object, L"online", true);
            device.selected = Boolean(object, L"selected", result.IsSelected(device.id));
            device.state = Text(object, L"state", L"idle");
            device.statusMessage = Text(object, L"status_message");
            device.errorMessage = Text(Object(object, L"error"), L"message");
            device.packetsSent = Counter(object, L"packets_sent");
            device.timingExchanges = Counter(object, L"timing_exchanges");
            if (!device.id.empty()) result.devices.push_back(std::move(device));
        }
        for (auto const& item : Array(json, L"sources"))
        {
            if (item.ValueType() != JsonValueType::Object || result.sources.size() >= 256) continue;
            auto const object = item.GetObject();
            Source source{Text(object, L"id"), Text(object, L"name", L"Windows 音频输出"), Text(object, L"kind")};
            if (!source.id.empty()) result.sources.push_back(std::move(source));
        }
        auto const level = Object(json, L"level");
        result.leftLevel = std::clamp(Number(level, L"left"), 0.0, 1.0);
        result.rightLevel = std::clamp(Number(level, L"right"), 0.0, 1.0);
        for (auto const& item : Array(json, L"waveform"))
        {
            if (result.waveform.size() >= 512) break;
            if (item.ValueType() != JsonValueType::Number) continue;
            auto const sample = item.GetNumber();
            result.waveform.push_back(std::isfinite(sample) ? std::clamp(sample, -1.0, 1.0) : 0.0);
        }

        auto const error = Object(json, L"error");
        result.errorCode = Text(error, L"code");
        result.errorMessage = Text(error, L"message");
        auto const auth = Object(json, L"auth");
        result.authKind = Text(auth, L"kind");
        result.authDeviceId = Text(auth, L"device_id");
        result.authDeviceName = Text(auth, L"device_name");
        result.authMessage = Text(auth, L"message");
        result.authRequestId = Counter(auth, L"request_id");
        auto const capture = Object(json, L"capture");
        result.captureState = Text(capture, L"state", L"idle");
        result.captureMessage = Text(capture, L"message");
        result.captureSourceName = Text(capture, L"source_name");
        result.testToneActive = Boolean(json, L"test_tone_active");
        result.diagnosticsJson = Object(json, L"diagnostics").Stringify();
        auto const metrics = Object(json, L"metrics");
        result.packetsSent = Counter(metrics, L"packets_sent");
        result.sampleRate = static_cast<uint32_t>(std::clamp(Number(metrics, L"sample_rate"), 0.0, 768000.0));
        auto const buffer = Value(metrics, L"buffer_ms");
        if (buffer && buffer.ValueType() == JsonValueType::Number && std::isfinite(buffer.GetNumber()) && buffer.GetNumber() >= 0)
        {
            result.bufferMs = buffer.GetNumber();
        }
        return result;
    }

    Device const* Snapshot::SelectedDevice() const noexcept
    {
        return FindDevice(selectedDeviceId);
    }

    Device const* Snapshot::FindDevice(hstring const& id) const noexcept
    {
        auto const match = std::find_if(devices.begin(), devices.end(), [&id](Device const& device)
            { return device.id == id; });
        return match == devices.end() ? nullptr : &*match;
    }

    bool Snapshot::IsSelected(hstring const& id) const noexcept
    {
        return std::find(selectedDeviceIds.begin(), selectedDeviceIds.end(), id) != selectedDeviceIds.end();
    }

    size_t Snapshot::StreamingCount() const noexcept
    {
        return static_cast<size_t>(std::count_if(devices.begin(), devices.end(), [](Device const& device)
            { return device.state == L"streaming"; }));
    }
}
