# Native updates and download cleanup

The settings page exposes **Check for updates** and, when a newer release is
available, **Download and install**. Checking alone does not download or run an
installer. Installation begins only after the user selects that button.

## Release selection and verification

`UpdateService` performs network work off the XAML thread. It queries the fixed
public GitHub endpoint for `chummumm/airloom/releases/latest`, accepts published
non-prerelease numeric versions, and compares major/minor/patch numerically.
The expected asset name and URL are derived from that version, not arbitrary
remote executable instructions. Files use HTTPS with normal Windows certificate
validation and the current user's automatic system proxy configuration.

The installer size is bounded. The downloaded SHA-256 must match the release's
`SHA256SUMS.txt`; when GitHub supplies an asset digest, it must agree as well.
Only a complete, matching download is renamed from `download.part` into an
executable. The helper verifies the same digest again and keeps a read-only,
non-write-sharing file handle through executable launch. These are integrity
checks against the authenticated release source, not a publisher code-signing
certificate.

## Installation handoff

The app copies the small, statically linked native `Airloom.Update.exe` into
its own update job directory. The helper inherits exactly three handles: a
handle to the old process, an exclusive cache lock, and a readiness event.
The helper validates the package and acknowledges readiness before the GUI exits.
It then waits for the old process to close before starting Inno Setup, avoiding
locked application DLLs and allowing capture/mute state to be restored.

Setup runs for the current user, updates the current application directory,
does not reboot Windows automatically, and reports its exit code. The helper
removes the package after successful installation, cancellation or failure,
then reopens Airloom. A failure or reboot-required result is shown in the
settings update status on the next launch. Updating a portable copy uses the
installer in that copy's current directory.

## Cleanup scope

Only `%LOCALAPPDATA%\Airloom\Updates\run-<random identifier>` is managed.
An exclusive `active.lock` protects an active download or installation. Cleanup
recognizes only expected package, partial-download, helper and lock filenames;
it refuses linked directories and preserves unrecognized files. It never
cleans the user's Downloads folder or application settings.

The download owner removes incomplete files on cancellation or error. The
running helper cannot remove its own loaded image, so the restarted app retries
cleanup during its first seconds. A later launch also retries after a power
loss, forced shutdown or still-open file. An active update retains its lock
through the entire install; a locked file is left for a later retry.

## Verification

Portable policy fixtures verify numeric versions, malformed tags, exact asset
names, checksum parsing and Windows argument quoting. Windows lifecycle tests
launch the real helper with controlled executable fixtures: they reject a bad
hash, preserve unrelated files and active jobs, prove the installer cannot run
until its parent exits, exercise success and cancellation exit codes, and check
application restart plus package/helper cleanup. No real application upgrade
or user settings change is performed by those lifecycle tests.
