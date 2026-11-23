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

# Sourced by check-device.sh. The check app pins example.net with a key no
# server has, and refuses the daemon's certificate for it late enough after
# starting for the host to be taken for pinned. A new process of the app is
# then given one try of the host as it starts: the app refuses that too,
# and the host stays pinned, from then on.
check_pin_retry() {
    local host=example.net first i renewed=
    adb shell "$ABD pinned forget $PACKAGE $host" >/dev/null
    expect "the app refuses the certificate for the host it pins" \
        "error *" "$(look_up "https://$host/" "$PACKAGE" "--ei delayMs 11000")"
    for ((i = 0; i < 10; i++)); do
        first=$(pinned_at "$host")
        [[ -n $first ]] && break
        sleep 1
    done
    expect "the host is taken for pinned" "[0-9]*" "$first"
    # Seconds apart, so that a renewed time is a later one.
    sleep 1
    expect "a new process of the app refuses the one try of the host" \
        "error *" "$(fetch_when_mounted "https://$host/")"
    for ((i = 0; i < 10; i++)); do
        [[ $(pinned_at "$host") -gt ${first:-0} ]] && renewed=yes && break
        sleep 1
    done
    expect "the refused try keeps the host pinned from then on" "yes" "$renewed"
    adb shell "$ABD pinned forget $PACKAGE $host" >/dev/null
}
