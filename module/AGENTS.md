# AGENTS.md: module scripts

These scripts run as root on the device, under KernelSU, Magisk and APatch.

## Shell

- Write POSIX `sh`: the scripts run under Android's `/system/bin/sh` or the
  root manager's BusyBox `ash`, not bash. No arrays, `[[ ]]` or process
  substitution. CI only checks syntax (`bash -n`), so test on a device.
- Tools come from toybox or BusyBox, whose options differ. Check that a
  command works before relying on it.
- Dispatch restart and recovery steps to named helpers. Report an incomplete
  cleanup and still attempt independent teardown steps.

## Lifecycle

- `customize.sh` is sourced by the root manager's installer: use `ui_print`
  and `abort`, and the installer's `$ZIPFILE`, `$MODPATH` and `$ARCH`.
- `service.sh` runs at boot (late_start) and must return quickly. It starts
  the daemon under `supervise.sh` and does nothing else to the device.
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
  next boot before anything has started: it has to work both ways. No step
  of it depends on the one before, and it never sources a script that can
  exit: the device must stop trusting the authority whatever else fails.
- The module has no `system/` overlay, no SELinux rules and no
  `post-fs-data` script. Keep it that way: a module that can break booting
  must not. The daemon mounts the certificate stores, and only in the
  processes of filtered apps.

## Version

`version` must match the release tag, and `versionCode` increases with every
release.
