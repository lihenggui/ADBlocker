#!/bin/sh
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

# Runs module/direct.sh, and what the other module scripts do about a module
# the app installed, against a directory that stands for the device: the
# scripts' own paths moved under it, a daemon that only records how it was
# called, and the Android commands the scripts use. Needs zip and unzip.
set -eu
if [ "$#" -eq 0 ]; then
    result=0
    for scenario in install install-unsupported reinstall boot boot-implausible-list boot-unreadable-list \
        boot-app-gone boot-disabled boot-yields boot-pending-module remove remove-fails remove-refused \
        remove-leftover remove-leftover-reinstall remove-leftover-module started-by-another-name \
        remove-not-stopped boot-app-gone-not-stopped \
        takeover update-module update-module-removal update-module-pending install-pending-module \
        module-uninstall; do
        sh "$0" "$scenario" || result=1
    done
    exit "$result"
fi

cd "$(dirname "$0")/.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export FIXTURE="$fixture" SCENARIO="$1"
adb=$fixture/data/adb
data=$adb/adblocker
direct=$adb/adblocker_direct
module=$adb/modules/adblocker
update=$adb/modules_update/adblocker
launcher=$adb/service.d/adblocker.sh
packages=$fixture/data/system/packages.list
archive=$fixture/archive
mkdir -p "$adb" "${packages%/*}" "$fixture/system/bin" "$fixture/tools" "$archive/bin/x64"
: >"$fixture/calls"
# A package whose name only begins with the app's is not the app.
printf '%s\n' 'com.android.shell 2000 0 /data/user/0/com.android.shell' \
    'li.mercury.adblocker.check 10201 0 /data/user/0/li.mercury.adblocker.check' >"$packages"
printf '%s\n' 'li.mercury.adblocker 10200 0 /data/user/0/li.mercury.adblocker' >>"$packages"

# The archive the app carries: the scripts, with the device's paths under
# the fixture, and a daemon for one architecture.
for script in customize direct service supervise stop uninstall action; do
    sed -e "s|/data/adb|$adb|g" -e "s|/data/system|$fixture/data/system|g" \
        -e "s|/system/bin/|$fixture/system/bin/|g" "module/$script.sh" >"$archive/$script.sh"
done
sed 's/^versionCode=.*/versionCode=999/' module/module.prop >"$archive/module.prop"
cat >"$archive/bin/x64/abd" <<'EOF'
#!/bin/sh
case "${0%/bin/abd}" in
    */.adblocker-direct.*/module) where=staged ;;
    */modules/adblocker) where=module ;;
    */adblocker_direct) where=direct ;;
    *) where=unknown ;;
esac
printf '%s %s\n' "$where" "$*" >>"$FIXTURE/calls"
case "$SCENARIO:$*" in
    # The copies of the stores are kept while apps have them mounted.
    remove-leftover*:'trust remove') exit 3 ;;
    # A daemon that would not end.
    *-not-stopped:module-stop) [ ! -e "$FIXTURE/installed" ] || exit 1 ;;
esac
EOF
(cd "$archive" && zip -q -r "$fixture/module.zip" .)

for tool in timeout ip iptables iptables-restore ip6tables ip6tables-restore; do
    : >"$fixture/system/bin/$tool"
    chmod +x "$fixture/system/bin/$tool"
done
cat >"$fixture/tools/getprop" <<'EOF'
#!/bin/sh
case "$1" in
    sys.boot_completed) echo 1 ;;
    ro.product.cpu.abi) if [ "$SCENARIO" = install-unsupported ]; then echo riscv64; else echo x86_64; fi ;;
esac
EOF
# Root owns the module on a device; here nothing can be given to it.
printf '%s\n' '#!/bin/sh' >"$fixture/tools/chown"
# No kernel configuration to read: the installer then leaves it to the daemon.
printf '%s\n' '#!/bin/sh' 'exit 1' >"$fixture/tools/zcat"
cat >"$fixture/tools/rm" <<'EOF'
#!/bin/sh
if [ "$SCENARIO:$*" = "remove-fails:-rf $FIXTURE/data/adb/adblocker" ]; then exit 1; fi
exec /bin/rm "$@"
EOF
chmod +x "$fixture/tools/"*
PATH=$fixture/tools:$PATH
export PATH

fail() {
    echo "$SCENARIO: $*" >&2
    if [ -f "$fixture/output" ]; then cat "$fixture/output" >&2; fi
    cat "$fixture/calls" >&2
    if [ -f "$data/abd.log" ]; then cat "$data/abd.log" >&2; fi
    exit 1
}

called() {
    grep -qx "$1" "$fixture/calls"
}

# The supervisor is done once the daemon it started has ended: nothing of a
# start is then left running, and the next step sees all of its calls.
await_start() {
    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        if grep -q 'daemon exited normally' "$data/abd.log" 2>/dev/null &&
            [ ! -e "$data/supervisor.pid" ]; then
            return 0
        fi
        sleep 1
    done
    fail "the daemon was not started from $1"
}

# A step that has to start the daemon of the module in $1, and no other.
starts() {
    await_start "$1"
    called "$1 run" || fail "the daemon did not run from $1"
    if grep -v "^$1 " "$fixture/calls" | grep -q ' run$'; then fail "another daemon ran"; fi
}

no_start() {
    sleep 1
    if grep -q ' run$' "$fixture/calls"; then fail "the daemon started"; fi
}

forget_calls() {
    : >"$fixture/calls"
    rm -f "$data/abd.log"
}

direct_sh() {
    sh "$archive/direct.sh" "$@" >"$fixture/output" 2>&1
}

# As the app installs it, with the calls of that forgotten.
installed_by_app() {
    direct_sh install "$fixture/module.zip" || fail "the installation failed"
    await_start direct
    forget_calls
    touch "$fixture/installed"
}

# As a root manager leaves it: the files of an earlier version, and its own.
flashed() {
    mkdir -p "$module/bin"
    for file in service.sh supervise.sh stop.sh uninstall.sh action.sh direct.sh; do
        cp "$archive/$file" "$module/$file"
    done
    printf '%s\n' 'versionCode=1' >"$module/module.prop"
    cp "$archive/bin/x64/abd" "$module/bin/abd"
    chmod +x "$module/bin/abd"
}

mode() {
    test "$(ls -ld "$1" | cut -c 1-10)" = "$2" || fail "$1 is not $2"
}

# With the new daemon, before the files of the one in use are replaced.
stopped_first() {
    printf '%s\n' 'staged module-stop' 'staged unhook' >"$fixture/first"
    head -n 2 "$fixture/calls" | cmp -s - "$fixture/first" || fail "the installed daemon was not stopped first"
}

# A removal while apps have the copies of the stores mounted.
removed_with_leftover() {
    installed_by_app
    mkdir -p "$data/store" "$data/user-store" "$data/ca"
    touch "$data/store/root.0" "$data/apps"
    direct_sh remove || fail "the removal failed"
    test ! -e "$direct" && test ! -e "$data/ca" && test ! -e "$data/apps" || fail "the module or its key is left"
    test -f "$data/store/root.0" || fail "a copy apps may have mounted was removed"
    forget_calls
}

staged_nothing() {
    if ls -d "$adb"/.adblocker-direct.* >/dev/null 2>&1; then fail "the staging directory was left behind"; fi
}

case "$SCENARIO" in
    install)
        # An adb shell's umask leaves everything open to everyone.
        umask 000
        direct_sh install "$fixture/module.zip" || fail "the installation failed"
        starts direct
        mode "$direct" drwxr-xr-x
        mode "$direct/service.sh" -rw-r--r--
        mode "$direct/bin/abd" -rwxr-xr-x
        mode "${launcher%/*}" drwxr-xr-x
        mode "$launcher" -rwx------
        for file in service.sh supervise.sh stop.sh uninstall.sh action.sh direct.sh module.prop; do
            test -f "$direct/$file" || fail "$file was not installed"
        done
        test -x "$direct/bin/abd" || fail "the daemon cannot be run"
        test -x "$launcher" || fail "nothing starts the module at boot"
        test ! -e "$adb/.adblocker-launcher" || fail "the launcher was left where it was written"
        test ! -e "$module" || fail "a module appeared in the root manager"
        staged_nothing
        ;;
    install-unsupported)
        if direct_sh install "$fixture/module.zip"; then fail "an archive without a daemon for the device was installed"; fi
        grep -q 'Unsupported architecture: riscv64' "$fixture/output" || fail "the installer did not say why"
        test ! -e "$direct" && test ! -e "$launcher" || fail "something was installed"
        test ! -s "$fixture/calls" || fail "something was stopped or started"
        staged_nothing
        ;;
    reinstall)
        installed_by_app
        touch "$direct/disable"
        direct_sh install "$fixture/module.zip" || fail "the installation failed"
        starts direct
        stopped_first
        test ! -e "$direct/disable" || fail "installing again left the module turned off"
        test -x "$launcher" || fail "nothing starts the module at boot"
        staged_nothing
        ;;
    boot)
        installed_by_app
        sh "$launcher" || fail "the launcher failed"
        starts direct
        ;;
    boot-implausible-list)
        installed_by_app
        printf '%s\n' 'com.example 10100 0 /data/user/0/com.example' >"$packages"
        sh "$launcher" || fail "the launcher failed"
        starts direct
        test -d "$data" || fail "an implausible package list removed the data"
        ;;
    boot-unreadable-list)
        installed_by_app
        rm "$packages"
        sh "$launcher" || fail "the launcher failed"
        starts direct
        ;;
    boot-app-gone)
        installed_by_app
        printf '%s\n' 'com.android.shell 2000 0 /data/user/0/com.android.shell' \
            'li.mercury.adblocker.check 10201 0 /data/user/0/li.mercury.adblocker.check' >"$packages"
        sh "$launcher" || fail "the launcher failed"
        printf '%s\n' 'direct module-stop' 'direct unhook' 'direct trust remove' | cmp -s - "$fixture/calls" ||
            fail "the module was not uninstalled"
        test ! -e "$direct" && test ! -e "$launcher" && test ! -e "$data" || fail "something of the module is left"
        ;;
    boot-disabled)
        installed_by_app
        touch "$direct/disable"
        sh "$launcher" || fail "the launcher failed"
        test ! -s "$fixture/calls" || fail "a module turned off did something at boot"
        test -d "$direct" || fail "a module turned off was removed"
        ;;
    boot-yields)
        installed_by_app
        flashed
        sh "$launcher" || fail "the launcher failed"
        test ! -s "$fixture/calls" || fail "the app's module did not leave the boot to the one in the manager"
        test -d "$direct" || fail "the app's module removed itself"
        ;;
    boot-pending-module)
        # What a manager keeps for a module it applies at the next boot.
        installed_by_app
        mkdir -p "$module"
        touch "$module/module.prop" "$module/update"
        sh "$launcher" || fail "the launcher failed"
        starts direct
        ;;
    remove)
        installed_by_app
        direct_sh remove || fail "the removal failed"
        # Stopped before anything is removed, then uninstalled, which stops again.
        printf '%s\n' 'direct module-stop' 'direct module-stop' 'direct unhook' 'direct trust remove' |
            cmp -s - "$fixture/calls" || fail "the module was not stopped and uninstalled"
        test ! -e "$direct" && test ! -e "$launcher" && test ! -e "$data" || fail "something of the module is left"
        ;;
    remove-fails)
        installed_by_app
        if direct_sh remove; then fail "a removal that left data succeeded"; fi
        called 'direct trust remove' || fail "the authority stayed trusted"
        test -f "$direct/direct.sh" && test -x "$launcher" || fail "nothing is left to try the removal again"
        ;;
    remove-refused)
        installed_by_app
        flashed
        if direct_sh remove; then fail "the data of the module in the manager was removed"; fi
        test ! -s "$fixture/calls" && test -d "$data" || fail "the module in the manager was touched"
        ;;
    remove-not-stopped)
        installed_by_app
        mkdir -p "$data/ca"
        if direct_sh remove; then fail "a removal that left the daemon running succeeded"; fi
        grep -q '^! The daemon could not be stopped' "$fixture/output" || fail "the removal did not say why"
        printf '%s\n' 'direct module-stop' | cmp -s - "$fixture/calls" || fail "something was removed all the same"
        test -x "$direct/bin/abd" && test -x "$launcher" && test -d "$data/ca" || fail "nothing is left to stop the daemon with"
        ;;
    boot-app-gone-not-stopped)
        # At boot nothing runs: what the stopping says does not hold the removal up.
        installed_by_app
        mkdir -p "$data/ca"
        printf '%s\n' 'com.android.shell 2000 0 /data/user/0/com.android.shell' >"$packages"
        sh "$launcher" || fail "the launcher failed"
        called 'direct trust remove' || fail "the authority stayed trusted"
        test ! -e "$direct" && test ! -e "$data/ca" || fail "the module or its key is left"
        ;;
    remove-leftover)
        removed_with_leftover
        mode "$launcher" -rwx------
        # At the next boot, as a root manager runs it.
        sh "$launcher" || fail "the cleanup failed"
        test ! -e "$data" && test ! -e "$launcher" || fail "something is left after the boot"
        test ! -s "$fixture/calls" || fail "something was started"
        ;;
    remove-leftover-reinstall)
        removed_with_leftover
        direct_sh install "$fixture/module.zip" || fail "the installation failed"
        starts direct
        forget_calls
        touch "$data/apps"
        sh "$launcher" || fail "the launcher failed"
        starts direct
        test -f "$data/apps" || fail "the data of the module installed since was removed"
        ;;
    remove-leftover-module)
        removed_with_leftover
        flashed
        touch "$data/apps"
        sh "$launcher" || fail "the cleanup failed"
        test -f "$data/apps" || fail "the data of the module in the manager was removed"
        test ! -e "$launcher" || fail "the cleanup stays for another boot"
        ;;
    started-by-another-name)
        installed_by_app
        (cd "$direct" && sh ./service.sh) >"$fixture/output" 2>&1 || fail "the service script failed from its own directory"
        await_start "its own directory"
        test -f "$direct/direct.sh" && test -x "$launcher" || fail "the app's module removed itself"
        forget_calls
        ln -s "$direct" "$adb/elsewhere"
        sh "$adb/elsewhere/service.sh" >"$fixture/output" 2>&1 || fail "the service script failed by a link"
        await_start "a link"
        test -f "$direct/direct.sh" && test -x "$launcher" || fail "the app's module removed itself"
        # Its uninstaller removes what the module kept, and no module.
        (cd "$direct" && sh ./uninstall.sh) >"$fixture/output" 2>&1 || fail "the uninstall script failed"
        test ! -e "$data" || fail "the data was not removed"
        test -f "$direct/direct.sh" && test -x "$launcher" || fail "the uninstaller removed the module it ran from"
        ;;
    takeover)
        installed_by_app
        flashed
        sh "$module/service.sh" >"$fixture/output" 2>&1 || fail "the module's service script failed"
        starts module
        head -n 1 "$fixture/calls" | grep -qx 'module module-stop' || fail "the app's daemon was not stopped first"
        test ! -e "$direct" && test ! -e "$launcher" || fail "the app's module is still installed"
        ;;
    update-module)
        installed_by_app
        flashed
        touch "$module/disable" "$module/skip_mount"
        direct_sh install "$fixture/module.zip" || fail "the installation failed"
        starts module
        stopped_first
        grep -qx 'versionCode=999' "$module/module.prop" || fail "the module in the manager was not updated"
        test -f "$module/skip_mount" || fail "a file of the manager's was removed"
        test ! -e "$module/disable" || fail "installing left the module turned off"
        test ! -e "$direct" && test ! -e "$launcher" || fail "a second module was installed beside the manager's"
        staged_nothing
        ;;
    update-module-removal)
        flashed
        touch "$module/remove"
        if direct_sh install "$fixture/module.zip"; then fail "a module marked for removal was updated"; fi
        grep -qx 'versionCode=1' "$module/module.prop" && test ! -s "$fixture/calls" || fail "the module was touched"
        test ! -e "$direct" || fail "a second module was installed beside the manager's"
        staged_nothing
        ;;
    update-module-pending)
        flashed
        mkdir -p "$update"
        if direct_sh install "$fixture/module.zip"; then fail "a module with an update waiting was updated"; fi
        grep -qx 'versionCode=1' "$module/module.prop" && test ! -s "$fixture/calls" || fail "the module was touched"
        staged_nothing
        ;;
    install-pending-module)
        mkdir -p "$module" "$update"
        touch "$module/module.prop" "$module/update"
        direct_sh install "$fixture/module.zip" || fail "the installation failed"
        starts direct
        test ! -s "$module/module.prop" && test ! -e "$module/bin" || fail "the module waiting in the manager was touched"
        test -x "$launcher" || fail "nothing starts the module at boot"
        ;;
    module-uninstall)
        installed_by_app
        flashed
        sh "$module/uninstall.sh" >"$fixture/output" 2>&1 || fail "the module's uninstall script failed"
        called 'module trust remove' || fail "the authority stayed trusted"
        test ! -e "$direct" && test ! -e "$launcher" || fail "the app's module would start at the next boot"
        ;;
    *)
        echo "unknown scenario: $SCENARIO" >&2
        exit 2
        ;;
esac
