#pragma once

#include "pch.h"

namespace airloom::native
{
    struct Device
    {
        winrt::hstring id;
        winrt::hstring name;
        winrt::hstring model;
        winrt::hstring address;
        bool passwordRequired{};
        bool online{true};
        bool selected{};
        winrt::hstring state{L"idle"};
        winrt::hstring statusMessage;
        winrt::hstring errorMessage;
        uint64_t packetsSent{};
        uint64_t timingExchanges{};
    };

    struct Source
    {
        winrt::hstring id;
        winrt::hstring name;
        winrt::hstring kind;
    };

    struct Snapshot
    {
        winrt::hstring state{L"idle"};
        winrt::hstring statusMessage;
        std::vector<Device> devices;
        std::vector<Source> sources;
        winrt::hstring selectedDeviceId;
        std::vector<winrt::hstring> selectedDeviceIds;
        winrt::hstring selectedSourceId;
        winrt::hstring sourceMode{L"endpoint"};
        bool sourcesRefreshing{};
        winrt::hstring sourceMessage;
        double volume{0.75};
        bool muteLocal{};
        double leftLevel{};
        double rightLevel{};
        std::vector<double> waveform;
        winrt::hstring errorCode;
        winrt::hstring errorMessage;
        winrt::hstring authKind;
        winrt::hstring authDeviceId;
        winrt::hstring authDeviceName;
        winrt::hstring authMessage;
        uint64_t authRequestId{};
        uint64_t elapsedSeconds{};
        uint64_t packetsSent{};
        uint32_t sampleRate{};
        std::optional<double> bufferMs;
        winrt::hstring captureState{L"idle"};
        winrt::hstring captureMessage;
        winrt::hstring captureSourceName;
        bool testToneActive{};
        winrt::hstring diagnosticsJson;

        static Snapshot Parse(winrt::Windows::Data::Json::JsonObject const& json);
        Device const* SelectedDevice() const noexcept;
        Device const* FindDevice(winrt::hstring const& id) const noexcept;
        bool IsSelected(winrt::hstring const& id) const noexcept;
        size_t StreamingCount() const noexcept;
    };
}
