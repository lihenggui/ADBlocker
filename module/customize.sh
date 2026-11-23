#!/system/bin/sh
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

# The installer must not unpack the whole archive: it carries a daemon for
# each architecture, and only this device's is wanted. The archive is also
# the app's APK (scripts/build.sh), whose files are of no use to the module.
SKIPUNZIP=1

APP_PACKAGE=li.mercury.adblocker

# Magisk is tested last: KernelSU and APatch also set MAGISK_VER_CODE, for
# modules that only know Magisk.
if [ "$APATCH" = true ]; then
    ui_print "- Installing on APatch"
elif [ "$KSU" = true ]; then
    ui_print "- Installing on KernelSU"
elif [ -n "$MAGISK_VER_CODE" ]; then
    ui_print "- Installing on Magisk $MAGISK_VER"
else
    abort "! This module requires KernelSU, Magisk or APatch"
fi
# The archive carries a daemon for each supported architecture, as
# bin/$ARCH/abd (see scripts/build.sh): arm64 for devices, x64 for the
# Android emulator.
unzip -l "$ZIPFILE" "bin/$ARCH/abd" >/dev/null 2>&1 ||
    abort "! Unsupported architecture: $ARCH"
ui_print "- Architecture: $ARCH"

# The shared Android tables must accept the same owner and routing rules
# as the daemon. If the device hides its config, runtime staging checks this
# before enabling capture.
if zcat /proc/config.gz >/dev/null 2>&1; then
    for option in NETFILTER_XT_MATCH_OWNER NETFILTER_XT_TARGET_TPROXY \
        NETFILTER_XT_MARK NETFILTER_XT_MATCH_MULTIPORT NETFILTER_XT_MATCH_QUOTA2 \
        IP_MULTIPLE_TABLES IP_NF_MANGLE; do
        zcat /proc/config.gz | grep -E "^CONFIG_$option=(y|m)$" >/dev/null ||
            abort "! The kernel lacks CONFIG_$option"
    done
fi
for tool in timeout ip iptables iptables-restore ip6tables ip6tables-restore; do
    [ -x "/system/bin/$tool" ] || abort "! The system has no $tool"
done

ui_print "- Extracting module files"
unzip -o "$ZIPFILE" module.prop service.sh supervise.sh stop.sh uninstall.sh \
    action.sh -d "$MODPATH" >&2 ||
    abort "! Failed to extract module files"
# Only the daemon for this device, as bin/abd (-j drops the bin/$ARCH/ part).
mkdir -p "$MODPATH/bin"
unzip -o -j "$ZIPFILE" "bin/$ARCH/abd" -d "$MODPATH/bin" >&2 ||
    abort "! Failed to extract the daemon for $ARCH"
# unzip does not reliably fail for a name that is missing from the archive.
for file in module.prop service.sh supervise.sh stop.sh uninstall.sh \
    action.sh bin/abd; do
    [ -f "$MODPATH/$file" ] || abort "! $file is missing from the archive"
done
set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm "$MODPATH/bin/abd" 0 0 0755

# Flashed from a root manager, the archive installs the app as well; flashed
# from the app, the app is this archive already.
if unzip -l "$ZIPFILE" AndroidManifest.xml >/dev/null 2>&1; then
    installed="$(pm path "$APP_PACKAGE" 2>/dev/null | head -n 1)"
    installed="${installed#package:}"
    if [ -n "$installed" ] && cmp -s "$ZIPFILE" "$installed"; then
        ui_print "- App is already installed"
    else
        ui_print "- Installing the app"
        # Binder can pass a pipe to PackageManager even when SELinux
        # prevents it receiving the installer's archive descriptor.
        size=$(wc -c <"$ZIPFILE")
        if cat "$ZIPFILE" | pm install -r -S "$size" >&2; then
            ui_print "- App installed"
        else
            ui_print "! Could not install the app. Remove .zip from the end of the file name and install the .apk file."
        fi
    fi
fi

# Apply without a reboot where the manager supports it (KernelSU; the others
# ignore the request and apply the module at the next boot). Updates don't run
# uninstall.sh, so service.sh stops the previous daemon itself.
MODULE_HOT_INSTALL_REQUEST=true
MODULE_HOT_RUN_SCRIPT=service.sh

ui_print "- Choose the filtering scope and apps in ADBlocker, or run:"
ui_print "    su -c $MODPATH/bin/abd app add <package>"
ui_print "    su -c $MODPATH/bin/abd update"
ui_print "- Data is stored in /data/adb/adblocker"
