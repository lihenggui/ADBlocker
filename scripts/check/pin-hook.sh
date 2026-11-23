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

# Sourced by check-device.sh after the DNS and pin checks, which leave
# example.net unpinned and without rules. The check app pins example.net with a key no
# server has. Where Vector is installed and has ADBlocker's app as a module,
# the app first refuses the daemon's certificate for that host without the
# module, and the host is taken for pinned. With the module enabled for
# the app, the app's next start tries the host again, the app accepts it,
# and the host is pinned no longer, with nothing forgotten by hand.
# Without Vector or the app, nothing is checked.
check_pin_hook() {
    local cli="sh /data/adb/modules/zygisk_vector/cli" module=li.mercury.adblocker host=example.net
    local status scoped result i
    if ! adb shell "test -f /data/adb/modules/zygisk_vector/cli"; then
        echo "skip  the LSPosed module: Vector is not installed"
        return
    fi
    status=$(adb shell "$cli modules ls" | tr -d '\r' | awk -v m="$module" '$1 == m { print $3 }')
    if [[ -z $status ]]; then
        echo "skip  the LSPosed module: ADBlocker's app is not installed"
        return
    fi
    scoped=$(adb shell "$cli scope ls $module" | tr -d '\r' | awk -v p="$PACKAGE" '$1 == p')
    [[ -z $scoped ]] || adb shell "$cli scope rm $module $PACKAGE/0" >/dev/null
    adb shell "$ABD pinned forget $PACKAGE $host" >/dev/null

    expect "without the LSPosed module, the app refuses the certificate for the host it pins" \
        "error *" "$(look_up "https://$host/" "$PACKAGE" "--ei delayMs 11000")"
    for ((i = 0; i < 10; i++)); do
        [[ -n $(pinned_at "$host") ]] && break
        sleep 1
    done
    expect "without the LSPosed module, the host is taken for pinned" "[0-9]*" "$(pinned_at "$host")"

    adb shell "$cli scope add $module $PACKAGE/0 && $cli modules enable $module" >/dev/null
    result=$(fetch_when_mounted "https://$host/")
    expect "with the LSPosed module, the app's next start has the host it pins decrypted" \
        "[0-9]* *ADBlocker*" "$result"
    for ((i = 0; i < 10; i++)); do
        [[ -z $(pinned_at "$host") ]] && break
        sleep 1
    done
    expect "with the LSPosed module, the host is pinned no longer" "" "$(pinned_at "$host")"

    # Leave the framework as it was found.
    [[ -n $scoped ]] || adb shell "$cli scope rm $module $PACKAGE/0" >/dev/null
    # Scoped before: added above, and so left as it was.
    [[ $status == enabled ]] || adb shell "$cli modules disable $module" >/dev/null
    adb shell "$ABD pinned forget $PACKAGE $host" >/dev/null
}
