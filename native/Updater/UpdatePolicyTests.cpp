#include "UpdatePolicy.h"
#include <iostream>
#include <stdexcept>

using namespace airloom::updates;

void Check(bool value, char const* message)
{
    if (!value) throw std::runtime_error(message);
}

int main()
{
    Check(Version::Parse("v0.2.0") > Version::Parse("0.1.99"), "minor comparison");
    Check(Version::Parse("1.10.0") > Version::Parse("1.9.99"), "numeric comparison");
    Check(Version::Parse("0.4.10") > Version::Parse("0.4.9"), "two-digit patch comparison");
    Check(Version::Parse("0.5.0") > Version::Parse("0.4.10"), "patch-ten rollover comparison");
    for (auto invalid : {"", "v", "1.2", "1.2.3.4", "-1.2.3", "1.2.3-preview.1", "1.2.3/evil", "1000000.2.3"})
        Check(!Version::Parse(invalid), "unstable or malformed version accepted");
    Check(InstallerName(*Version::Parse("v0.2.0")) == "Airloom-0.2.0-setup-x64.exe", "installer name");
    Check(!IsInstallerName("Airloom-../payload-setup-x64.exe"), "path traversal accepted");
    Check(!IsInstallerName("Airloom-v0.2.0-setup-x64.exe"), "noncanonical filename accepted");
    auto name = InstallerName(*Version::Parse("0.2.0"));
    std::string digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    Check(ChecksumFor(digest + "  " + name + "\r\n", name) == digest, "checksum fixture");
    Check(ChecksumFor(digest + " *" + name + "\n", name) == digest, "binary checksum fixture");
    Check(!ChecksumFor(digest + "  wrong.exe\n", name), "wrong asset accepted");
    Check(!ChecksumFor(digest + "  " + name + "\n" + digest + "  " + name, name), "ambiguous checksum accepted");
    Check(!ChecksumFor(std::string(64, 'z') + "  " + name, name), "nonhex checksum accepted");
    Check(IsJobName(L"run-0123456789abcdef0123456789abcdef"), "owned cache name");
    Check(!IsJobName(L"run-../../Documents"), "unsafe cache name accepted");
    Check(Quote(L"C:\\program files\\Airloom\\") == L"\"C:\\program files\\Airloom\\\\\"", "trailing slash quoting");
    Check(Quote(L"a\"b") == L"\"a\\\"b\"", "embedded quote escaping");
    std::cout << "Updater policy fixtures passed\n";
}
