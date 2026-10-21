#pragma once

#include <array>
#include <cctype>
#include <compare>
#include <cstdint>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>

namespace airloom::updates
{
    struct Version
    {
        std::array<uint32_t, 3> parts{};
        auto operator<=>(Version const&) const = default;

        std::string String() const
        {
            return std::to_string(parts[0]) + "." + std::to_string(parts[1]) + "." + std::to_string(parts[2]);
        }

        static std::optional<Version> Parse(std::string_view text)
        {
            if (!text.empty() && text.front() == 'v') text.remove_prefix(1);
            Version result;
            for (size_t part = 0; part < result.parts.size(); ++part)
            {
                size_t digits{};
                while (!text.empty() && text.front() >= '0' && text.front() <= '9')
                {
                    if (++digits > 6) return std::nullopt;
                    result.parts[part] = result.parts[part] * 10 + static_cast<uint32_t>(text.front() - '0');
                    text.remove_prefix(1);
                }
                if (digits == 0) return std::nullopt;
                if (part + 1 == result.parts.size()) return text.empty() ? std::optional(result) : std::nullopt;
                if (text.empty() || text.front() != '.') return std::nullopt;
                text.remove_prefix(1);
            }
            return std::nullopt;
        }
    };

    inline std::string InstallerName(Version const& version)
    {
        return "Airloom-" + version.String() + "-setup-x64.exe";
    }

    inline bool IsInstallerName(std::string_view name)
    {
        constexpr std::string_view prefix = "Airloom-";
        constexpr std::string_view suffix = "-setup-x64.exe";
        if (!name.starts_with(prefix) || !name.ends_with(suffix)) return false;
        auto version = Version::Parse(name.substr(prefix.size(), name.size() - prefix.size() - suffix.size()));
        return version && InstallerName(*version) == name;
    }

    inline std::optional<std::string> ChecksumFor(std::string const& sums, std::string const& filename)
    {
        std::optional<std::string> found;
        std::istringstream stream(sums);
        std::string line;
        while (std::getline(stream, line))
        {
            if (!line.empty() && line.back() == '\r') line.pop_back();
            if (line.size() < 67 || line[64] != ' ' || (line[65] != ' ' && line[65] != '*')) continue;
            if (line.substr(66) != filename) continue;
            if (found) return std::nullopt;
            auto digest = line.substr(0, 64);
            for (char& character : digest)
            {
                if (!std::isxdigit(static_cast<unsigned char>(character))) return std::nullopt;
                character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
            }
            found = std::move(digest);
        }
        return found;
    }

    inline bool IsJobName(std::wstring_view name)
    {
        if (name.size() != 36 || !name.starts_with(L"run-")) return false;
        for (auto character : name.substr(4))
            if (!((character >= L'0' && character <= L'9') || (character >= L'a' && character <= L'f'))) return false;
        return true;
    }

    // Quote one argument according to the Windows C runtime command-line rules.
    inline std::wstring Quote(std::wstring_view argument)
    {
        std::wstring result = L"\"";
        size_t slashes{};
        for (auto character : argument)
        {
            if (character == L'\\') { ++slashes; continue; }
            result.append(slashes * (character == L'\"' ? 2 : 1), L'\\');
            slashes = 0;
            if (character == L'\"') result += L'\\';
            result += character;
        }
        result.append(slashes * 2, L'\\');
        result += L'\"';
        return result;
    }
}
