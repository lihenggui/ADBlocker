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

# The module where no root manager installs it: the app does, with root
# alone, into a directory of its own, and the manager's list of modules does
# not show it.
#
#   direct.sh install <archive>   from a copy taken out of the archive
#   direct.sh boot                at boot, from /data/adb/service.d
#   direct.sh remove              by the app, or at boot once the app is gone
#
# KernelSU, Magisk and APatch run what is in /data/adb/service.d at
# late_start, as they run a module's service.sh. Where nothing does, the app
# starts the daemon once the device has booted.
#
# A module flashed in a root manager is the one in use once it has its
# daemon: a manager keeps a directory with module.prop alone for a module it
# applies at the next boot. `install` brings that one up to date where it
# is, and it stays the manager's to list and to remove.
DIRECT=/data/adb/adblocker_direct
MODULE=/data/adb/modules/adblocker
UPDATE=/data/adb/modules_update/adblocker
LAUNCHER=/data/adb/service.d/adblocker.sh
DATA=/data/adb/adblocker
PACKAGES=/data/system/packages.list

# The app's root shell, an adb shell and a root manager each come with a
# umask of their own.
umask 077

# What a root manager's installer gives customize.sh.
ui_print() { echo "$1"; }
abort() {
    echo "$1"
    exit 1
}
set_perm() {
    chown "$2:$3" "$1" && chmod "$4" "$1"
}
set_perm_recursive() {
    find "$1" -type d | while read -r dir; do set_perm "$dir" "$2" "$3" "$4"; done
    find "$1" -type f | while read -r file; do set_perm "$file" "$2" "$3" "$5"; done
}

# Stops the daemon of the installation about to be replaced, with the new
# one's program: wherever the module is, its pid files are the same. The
# rules are removed whatever became of the stopping, as in stop.sh.
stop_installed() {
    stopped=0
    "$MODPATH/bin/abd" module-stop || stopped=1
    "$MODPATH/bin/abd" unhook || stopped=1
    return "$stopped"
}

# Puts what it reads into service.d, as the one script of ours there.
# Written elsewhere first: a manager runs whatever it finds in service.d, a
# half-written file included.
into_service_d() {
    # As the managers make it, where none has.
    [ -d "${LAUNCHER%/*}" ] || mkdir -m 755 "${LAUNCHER%/*}" || return 1
    cat >/data/adb/.adblocker-launcher || return 1
    chmod 700 /data/adb/.adblocker-launcher && mv -f /data/adb/.adblocker-launcher "$LAUNCHER"
}

write_launcher() {
    into_service_d <<EOF
#!/system/bin/sh
# Starts ADBlocker at boot where the app installed its module. Written by direct.sh.
[ -f $DIRECT/direct.sh ] && exec sh $DIRECT/direct.sh boot
rm -f "\$0"
EOF
}

# The copies of the certificate stores stay while apps that have them
# mounted run, and with the module gone nothing of it is left to remove them
# later. At the next boot no app has them. A module installed by then has
# the data for its own, and this script's place if the app installed it.
leave_cleanup() {
    into_service_d <<EOF
#!/system/bin/sh
# Removes what was left of ADBlocker's data while apps used it. Written by direct.sh.
[ -e $DIRECT ] || [ -e $MODULE/bin/abd ] || rm -rf $DATA
rm -f "\$0"
EOF
}

# Takes the app's module out of its place in one step, then deletes it: a
# deletion cut short leaves the whole module there or none of it, not a
# part that can neither run nor be uninstalled.
drop_direct() {
    rm -rf "$DIRECT.gone" && mv "$DIRECT" "$DIRECT.gone" && rm -rf "$DIRECT.gone"
}

# Replaces the module the app installed before, or installs the first.
replace_direct() {
    if [ -e "$DIRECT" ]; then
        stop_installed && drop_direct || abort "! Could not replace the installed module"
    fi
    mv "$MODPATH" "$DIRECT" || abort "! Could not install the module"
    write_launcher || ui_print "! Could not register for boot; the app starts the daemon after a restart"
}

# Brings the module in the root manager up to date where it is. What the
# manager keeps in the directory stays. The version goes last: an update cut
# short is still seen as out of date, and can be installed again.
update_module() {
    [ ! -e "$MODULE/remove" ] || abort "! The module is marked for removal in the root manager"
    [ ! -e "$UPDATE" ] || abort "! A module update is waiting for a restart"
    stop_installed || abort "! Could not stop the installed module"
    for file in bin/abd $MODULE_FILES; do
        mv -f "$MODPATH/$file" "$MODULE/$file" || abort "! Could not replace the installed module"
    done
    # Installing is asking for it to run, whatever turned it off.
    rm -f "$MODULE/disable" || abort "! Could not turn the module on"
}

# Starts the module in $1 as at boot. In a session of its own where there is
# a command for it: started from a terminal, the daemon outlives it.
start() {
    session=
    setsid true 2>/dev/null && session=setsid
    $session sh "$1/service.sh" </dev/null >/dev/null 2>&1
}

install() {
    ZIPFILE=$1
    [ -f "$ZIPFILE" ] || abort "! No archive to install from"
    case "$(getprop ro.product.cpu.abi)" in
    arm64-v8a) ARCH=arm64 ;;
    x86_64) ARCH=x64 ;;
    *) ARCH=$(getprop ro.product.cpu.abi) ;;
    esac
    stage=$(mktemp -d /data/adb/.adblocker-direct.XXXXXX) || abort "! No place to install from"
    trap 'rm -rf "$stage"' 0
    MODPATH=$stage/module
    mkdir "$MODPATH" || abort "! No place to install from"
    unzip -p "$ZIPFILE" customize.sh >"$stage/customize.sh" && [ -s "$stage/customize.sh" ] ||
        abort "! customize.sh is missing from the archive"
    # The module's own installer does the installing, as in a root manager.
    DIRECT_INSTALL=true
    . "$stage/customize.sh"
    # Complete and checked: only now is the one in use stopped and replaced.
    if [ -e "$MODULE/bin/abd" ]; then
        update_module
        target=$MODULE
    else
        replace_direct
        target=$DIRECT
    fi
    start "$target" || ui_print "! Installed, but the daemon did not start"
}

# The package manager's list of installed packages, replaced as a whole when
# it changes. One that cannot be read, or lacks what every device has, says
# nothing about the app.
app_removed() {
    # Read once: what is found of both is of the same list.
    found=$(sed -n -e 's/^com\.android\.shell .*/device/p' \
        -e 's/^li\.mercury\.adblocker .*/app/p' "$PACKAGES" 2>/dev/null)
    case "$found" in
    *app*) return 1 ;;
    *device*) return 0 ;;
    *) return 1 ;;
    esac
}

# By the app, with the daemon running.
remove() {
    # That one's data is the same data, and it is removed in the manager.
    if [ -e "$MODULE/bin/abd" ]; then
        echo "! The module is in the root manager"
        return 1
    fi
    # Part of a module can only be replaced: setting it up again stops what
    # may run of it with the new one's program, and can then be removed.
    if [ ! -f "$DIRECT/uninstall.sh" ] || [ ! -x "$DIRECT/bin/abd" ]; then
        echo "! The module is incomplete; set it up again to remove it"
        return 1
    fi
    # Stopped first, and nothing removed otherwise: uninstall.sh goes on
    # when the daemon cannot be stopped, and what follows would delete the
    # one program that can stop it.
    if ! "$DIRECT/bin/abd" module-stop; then
        echo "! The daemon could not be stopped; nothing was removed"
        return 1
    fi
    uninstall
}

# With nothing of the module running: stopped by remove, or not started at
# this boot. The module stays while its data could not be removed, for the
# removal to be tried again.
uninstall() {
    if [ -f "$DIRECT/uninstall.sh" ]; then
        sh "$DIRECT/uninstall.sh" || return 1
    else
        # Only at boot, with the app gone, is a module without its
        # uninstaller removed. Nothing runs and no app has the stores then:
        # the key goes first, and all of the data with it.
        rm -rf "$DATA/ca"
        rm -rf "$DATA" || return 1
    fi
    drop_direct && rm -f "$LAUNCHER" || return 1
    if [ -e "$DATA" ]; then
        leave_cleanup || echo "! What apps still use of the data stays in $DATA"
    fi
}

boot() {
    # A module flashed since takes over: its service.sh removes this one.
    [ ! -e "$MODULE/bin/abd" ] || exit 0
    # Nothing is left to turn the daemon off, or to say what it does.
    if app_removed; then
        uninstall
        exit
    fi
    # Turned off by its boot guard, until the app turns it on again.
    [ ! -e "$DIRECT/disable" ] || exit 0
    exec sh "$DIRECT/service.sh"
}

case "${1:-}" in
install) install "${2:-}" ;;
boot) boot ;;
remove) remove ;;
*)
    echo "usage: direct.sh install <archive> | boot | remove" >&2
    exit 2
    ;;
esac
