#pragma once

#include "pch.h"

namespace airloom::native
{
    // The DLL owns its handle and returned strings. No C++ objects cross this ABI.
    // Commands enqueue work in the engine; snapshot is a nonblocking copy.
    class EngineBridge final
    {
    public:
        EngineBridge();
        ~EngineBridge();
        EngineBridge(EngineBridge const&) = delete;
        EngineBridge& operator=(EngineBridge const&) = delete;

        bool Available() const noexcept { return m_instance != nullptr; }
        winrt::hstring const& LoadError() const noexcept { return m_loadError; }
        winrt::Windows::Data::Json::JsonObject Snapshot() const;
        bool Command(winrt::Windows::Data::Json::JsonObject const& command) const;
        void Shutdown() noexcept;

    private:
        using CreateFn = void* (__cdecl*)();
        using DestroyFn = void (__cdecl*)(void*);
        using CommandFn = int (__cdecl*)(void*, char const*);
        using SnapshotFn = char* (__cdecl*)(void*);
        using FreeStringFn = void (__cdecl*)(char*);

        HMODULE m_library{};
        void* m_instance{};
        DestroyFn m_destroy{};
        CommandFn m_command{};
        SnapshotFn m_snapshot{};
        FreeStringFn m_freeString{};
        winrt::hstring m_loadError;
    };
}
