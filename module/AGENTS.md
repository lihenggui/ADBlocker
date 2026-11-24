# AGENTS.md: module scripts

These scripts run as root on the device. They are packaged into the app's
APK, which KernelSU, Magisk and APatch can flash as a module. The module is
in one of two places: `/data/adb/modules/adblocker` when flashed in a root
manager, or `/data/adb/adblocker_direct` when the app installed it with root
alone. The flashed one is in use once it has its daemon (`bin/abd`): a
manager keeps a directory without it for a module it applies at the next
boot. Test for the daemon, never for the directory. Both keep their data
and pid files in `/data/adb/adblocker`.

## Shell

- Write POSIX `sh`: the scripts run under Android's `/system/bin/sh` or the
  root manager's BusyBox `ash`, not bash. No arrays, `[[ ]]` or process
  substitution. CI checks syntax (`bash -n`) and runs the scripts against
  a model of the device with the host's shell and tools
  (`scripts/check-module-*.sh`), so test on a device too.
- Tools come from toybox or BusyBox, whose options differ. Check that a
  command works before relying on it.
- Dispatch restart and recovery steps to named helpers. Report an incomplete
  cleanup and still attempt independent teardown steps.

## Lifecycle

- `customize.sh` is sourced by the root manager's installer: use `ui_print`
  and `abort`, and the installer's `$ZIPFILE`, `$MODPATH` and `$ARCH`.
  `direct.sh` sources it too and defines the same; it is the one installer.
- `direct.sh` installs the module without a root manager: `install` has
  `customize.sh` stage the files, replaces the module the app installed
  before or brings the one in the root manager up to date where it is, and
  starts it. For its own directory it leaves a launcher in
  `/data/adb/service.d`, which the three managers run at late_start, to
  start the daemon at boot (`direct.sh boot`); where nothing runs it, the
  app starts the daemon once the device has booted (`AutoStart`), with the
  same `service.sh`. `boot` does nothing while a root manager
  has the module or the module is disabled. `remove` uninstalls, from the
  app or at boot once the app is gone from `/data/system/packages.list`;
  a list that lacks what every device has says nothing. Where
  `uninstall.sh` had to leave the copies of the stores to the apps that use
  them, `remove` leaves a script in the launcher's place that deletes them
  at the next boot unless a module is installed by then. The daemon of such
  a module ends by itself when the app is uninstalled
  (`abd/src/daemon/owner.rs`). `scripts/install-on-emulator.sh` installs
  with `direct.sh` too.
- `service.sh` runs at boot (late_start) and must return quickly. It starts
  the daemon under `supervise.sh` and does nothing else to the device,
  except that in a root manager it removes a module the app had installed:
  the flashed module takes over. The boot guard's `disable` is honored by
  the manager, and for the app's module by `direct.sh boot`.
  The app starts list downloads after saving subscription choices. From a
  root shell, `abd update` initializes the default subscriptions as needed.
- `supervise.sh` restarts a daemon that died, at once the first time and
  then with increasing delays, and runs `abd unhook` every time one is
  gone, however it went. This is what keeps a crashed daemon from cutting
  the filtered apps off. Unhook disables TCP marking and
  withdraws the capture routes and the lookup routing rules before trying
  the tables' locks. The supervisor
  reports failure if it cannot finish removing the old rules after its retries.
- `stop.sh` is sourced by `service.sh`. The stopping is done by
  `abd module-stop`, which holds each process by a descriptor; the rules
  are removed whether or not it succeeded.
- `uninstall.sh` runs either at once, with the daemon running, or at the
  next boot before anything has started, from a root manager or from
  `direct.sh remove`: it has to work both ways. No step
  of it depends on the one before, and it never sources a script that can
  exit: the device must stop trusting the authority whatever else fails.
  From a root manager it also removes a module the app had installed and
  the flashed one never came to replace.
- The module has no `system/` overlay, no SELinux rules and no
  `post-fs-data` script. Keep it that way: a module that can break booting
  must not. The daemon mounts the certificate stores, and only in the
  processes of filtered apps.

## Version

`version` must match the release tag, and `versionCode` increases with every
release.
