#!/usr/bin/env bash
#
# Copyright (C) 2026 Mercury Li
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 3 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program. If not, see <https://www.gnu.org/licenses/>.

# Installs the module on an emulator that has no root manager, then starts
# the daemon, so that check-device.sh can run there.
#
#   scripts/install-on-emulator.sh [out/adblocker-<version>.apk.zip]
#
# Needs an image on which `adb root` works (google_apis, not
# google_apis_playstore). The module's own installer, module/customize.sh,
# does the installing: it is sourced on the device with what a root
# manager's installer would give it, and service.sh is then run as after a
# hot install. Nothing is changed in the system image, and the module is
# gone again with the emulator's data.
set -euo pipefail

cd "$(dirname "$0")/.."

MODPATH=/data/adb/modules/adblocker
ABD=$MODPATH/bin/abd
version="$(sed -n 's/^version=//p' module/module.prop)"
zip="${1:-out/adblocker-$version.apk.zip}"
[[ -f "$zip" ]] || {
    echo "error: $zip not found; build it with ./scripts/build.sh" >&2
    exit 2
}

# With more than one device attached, adb would refuse every command, or the
# script would act on a device it was not meant for: ANDROID_SERIAL,
# which adb reads, names the one.
if [[ -z "${ANDROID_SERIAL:-}" ]]; then
    attached="$(adb devices | awk 'NR > 1 && NF >= 2 { print $1 }')"
    count="$(printf '%s\n' "$attached" | awk 'NF { n++ } END { print n + 0 }')"
    if ((count > 1)); then
        echo "error: $count devices are attached; set ANDROID_SERIAL to one of them:" >&2
        printf '  %s\n' $attached >&2
        exit 2
    fi
fi
adb wait-for-device
# Said before anything is changed, so that a wrong target can be stopped.
serial="$(adb get-serialno | tr -d '\r')"
avd="$(adb emu avd name 2>/dev/null | head -n 1 | tr -d '\r')" || avd=""
echo "Installing on $serial${avd:+ (AVD $avd)}"

# adbd restarts to become root, and a request made while it does so is
# refused: asked again a few times.
for ((i = 0; i < 5; i++)); do
    adb wait-for-device
    [[ "$(adb shell id -u | tr -d '\r')" == 0 ]] && break
    adb root >/dev/null 2>&1 || true
    sleep 2
done
adb wait-for-device
[[ "$(adb shell id -u | tr -d '\r')" == 0 ]] || {
    echo "error: adb cannot run as root here; use a google_apis emulator image" >&2
    exit 2
}
# adbd restarts as root; the package manager is needed before installing.
for ((i = 0; i < 120; i++)); do
    [[ "$(adb shell getprop sys.boot_completed | tr -d '\r')" == 1 ]] && break
    sleep 1
done
((i < 120)) || {
    echo "error: the emulator did not finish booting" >&2
    exit 2
}

stage=$(adb shell 'umask 077; mkdir -p /data/adb && mktemp -d /data/adb/.adb-emulator.XXXXXX' | tr -d '\r')
[[ $stage =~ ^/data/adb/\.adb-emulator\.[[:alnum:]]+$ ]] || exit 2
trap 'adb shell "rm -rf $stage" >/dev/null' EXIT
DEVICE_ZIP=$stage/module.zip
DEVICE_SCRIPT=$stage/install.sh
DEVICE_CUSTOMIZE=$stage/customize.sh
adb push "$zip" "$DEVICE_ZIP" >/dev/null
# What the installers of Magisk and KernelSU define before they source
# customize.sh (their util_functions.sh). KSU because, like KernelSU, the
# module is started here without a reboot.
adb shell "cat > $DEVICE_SCRIPT" <<EOF
ZIPFILE=$DEVICE_ZIP
MODPATH=$MODPATH
customize=$DEVICE_CUSTOMIZE
KSU=true
case "\$(getprop ro.product.cpu.abi)" in
arm64-v8a) ARCH=arm64 ;;
x86_64) ARCH=x64 ;;
*) ARCH="\$(getprop ro.product.cpu.abi)" ;;
esac
ui_print() { echo "\$1"; }
abort() {
    echo "\$1"
    rm -rf "\$MODPATH" "\$customize"
    exit 1
}
set_perm() {
    chown "\$2:\$3" "\$1" && chmod "\$4" "\$1" && chcon "\${5:-u:object_r:system_file:s0}" "\$1"
}
set_perm_recursive() {
    find "\$1" -type d | while read -r dir; do set_perm "\$dir" "\$2" "\$3" "\$4" "\$6"; done
    find "\$1" -type f -o -type l | while read -r file; do set_perm "\$file" "\$2" "\$3" "\$5" "\$6"; done
}
# A previous installation is replaced, as a root manager does on update.
# Its daemon is stopped before its files are taken away from under it.
[ -f "\$MODPATH/stop.sh" ] && sh "\$MODPATH/stop.sh"
rm -rf "\$MODPATH"
mkdir -p "\$MODPATH"
unzip -p "\$ZIPFILE" customize.sh >"\$customize" || abort "! customize.sh is missing from the archive"
. "\$customize"
rm -f "\$customize"
# What this script relies on the module asking for: that the installer
# leaves the archive to customize.sh, and runs service.sh afterwards.
[ "\$SKIPUNZIP" = 1 ] || abort "! The module no longer extracts its own files"
[ "\$MODULE_HOT_RUN_SCRIPT" = service.sh ] || abort "! The module no longer asks for service.sh after installing"
# In a session of its own, so that the daemon outlives this adb shell.
setsid sh "\$MODPATH/service.sh" </dev/null >/dev/null 2>&1
EOF
status=0
adb shell "sh $DEVICE_SCRIPT" || status=$?
adb shell "rm -f $DEVICE_SCRIPT $DEVICE_ZIP $DEVICE_CUSTOMIZE"
((status == 0)) || {
    echo "error: the module installer failed" >&2
    exit 1
}

# The daemon counts as running once it says what it filters, which it does
# after its rules are in place.
for ((i = 0; i < 60; i++)); do
    adb shell "grep -q '^filtering ' /data/adb/adblocker/abd.log" 2>/dev/null && break
    sleep 1
done
((i < 60)) || {
    echo "error: the daemon is not running; its log:" >&2
    adb shell "tail -n 50 /data/adb/adblocker/abd.log" >&2 || true
    exit 1
}
echo "daemon running"
