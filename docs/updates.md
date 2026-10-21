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

## Manual installation and older releases

Before replacing application files, Setup extracts its bundled update helper
and runs an installation preflight. The helper identifies the Airloom window,
checks the owning process's executable path, and sends the registered
`Airloom.RequestExit.v1` message only when the window advertises the matching
`Airloom.GracefulExit.v1` capability. It waits up to 15 seconds for that process
to exit. It does not terminate the process forcibly or treat a hidden window
as proof of exit. A different running copy, an uncertain process state, or a
timeout stops installation with a retry instruction before files are replaced.

**A running 0.1.1 or 0.2.0 must be exited manually before upgrading.** These
releases do not understand the new exit request; closing their main window only
hides them to the tray. Right-click the Airloom icon in the taskbar notification area, choose
**退出 Airloom / Exit Airloom**, and retry Setup. The new installer detects the
older capability and explains this step. Installing new files cannot repair
the old process's exit behavior before that process has stopped.

The preflight runs in Inno Setup's
[`PrepareToInstall`](https://jrsoftware.org/ishelp/topic_scriptevents.htm), before
its Restart Manager files-in-use check. New Airloom versions also handle
Restart Manager directly: a shutdown query is acknowledged without exiting,
a cancelled session end leaves the app running, and a confirmed
`ENDSESSION_CLOSEAPP` requests orderly exit. Once exit is requested, a subsequent
`WM_CLOSE` cannot turn it into hide-to-tray. Ordinary user window-close behavior
is unchanged. This follows Microsoft's
[Restart Manager application guidance](https://learn.microsoft.com/en-us/windows/win32/rstmgr/guidelines-for-applications).

## In-app installation handoff

The app copies the small, statically linked native `Airloom.Update.exe` into
its own update job directory. The helper inherits exactly three handles: a
handle to the old process, an exclusive cache lock, and a readiness event.
The helper validates the package and acknowledges readiness before the GUI exits.
The GUI stops its update work, shuts down the audio engine, removes the tray
icon, closes the window, and exits. Core shutdown is responsible for restoring
the capture session's owned mute state; the helper does not bypass that cleanup.
It waits up to 30 seconds for the actual process handle to signal exit before
starting Inno Setup, so closing or hiding the window alone is insufficient.

If the process does not exit in time, the helper does not run Setup. It attempts
to remove the downloaded package, records an error, and displays a visible explanation
with the tray-exit and retry steps. It never force-kills the old application.
The still-running GUI also monitors the accepted handoff until orderly exit
begins, so an unconsumed exit request can become an in-app error.

Setup runs for the current user, updates the current application directory,
does not reboot Windows automatically, and reports its exit code. The helper
removes the package after successful installation, cancellation or failure.
When the old process has exited, it then attempts to reopen Airloom; a failure
or reboot-required result is shown in the settings update status on the next
launch. A parent-exit timeout leaves the existing app in place instead of
starting another copy. Updating a portable copy uses the installer in that
copy's current directory.

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

## Verification scope

Portable policy fixtures verify numeric versions, malformed tags, exact asset
names, checksum parsing and Windows argument quoting. The Windows lifecycle
suite in `scripts/test-updater.ps1` builds a local test harness and exercises:

- The real helper with controlled installation/relaunch executables: bad-hash
  rejection, unrelated-file and active-job preservation, waiting for parent
  exit, installation success/cancellation, restart, and package/helper cleanup.
- The actual built WinUI application: ordinary close-to-tray when a tray is
  available, the installer's registered exit request, process termination, and
  tray removal. Installation and relaunch still use disposable fixture files.
- Restart Manager query, cancelled session end, and confirmed application exit
  against the actual WinUI process.
- A legacy window and an unresponsive capable window: installation is blocked
  without hiding or killing the process. Fixture parents stop cooperatively.
- The helper's 30-second parent-exit timeout: a visible error appears, the
  package is removed, and no installer runs against the live parent.

The suite backs up and restores the update-error registry value it uses.
It does not perform a real application upgrade or connect to a physical HomePod.
These checks do not establish mute restoration during actual hardware playback,
every Windows shutdown scenario, or a real old-version-to-new-version upgrade.
The separate native smoke check exercises the shipped portable archive's UI
and live release-metadata check; metadata success does not prove installation.

This section describes the implemented checks, not a declaration that a build
has passed them. Consult the Windows CI result for the exact release commit
before treating its lifecycle, packaging, or smoke checks as verified.
